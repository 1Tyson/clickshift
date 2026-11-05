//! Runs the real binary against a virtual mouse and reads its real output.
//!
//! Nothing reaches the desktop: the harness grabs clickshift's output devices
//! before sending any input, so only the harness sees the results. The real
//! mouse is never touched.
//!
//! Needs packaging/71-clickshift-test.rules, so it is opt-in:
//!     cargo test --test e2e -- --ignored

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use clickshift::macros::{Emit, Macro};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, Device, EventSummary, InputEvent, InputId, KeyCode, KeyEvent,
    RelativeAxisCode, RelativeAxisEvent,
};
use zbus::blocking::Connection;

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(150);

const REL_X: RelativeAxisCode = RelativeAxisCode::REL_X;
const REL_WHEEL: RelativeAxisCode = RelativeAxisCode::REL_WHEEL;
const REL_WHEEL_HI_RES: RelativeAxisCode = RelativeAxisCode::REL_WHEEL_HI_RES;

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Seen {
    Key(KeyCode, i32),
    Rel(RelativeAxisCode, i32),
}

/// Kills the daemon even if the harness panics while starting up, so a failed
/// test cannot leave a process and virtual devices behind.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Harness {
    source: Option<VirtualDevice>,
    source_name: String,
    out_mouse: Device,
    out_keyboard: Device,
    daemon: Daemon,
    lines: Receiver<String>,
    config_path: PathBuf,
    bus_name: String,
}

fn build_source(name: &str) -> VirtualDevice {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in [
        KeyCode::BTN_LEFT,
        KeyCode::BTN_RIGHT,
        KeyCode::BTN_MIDDLE,
        KeyCode::BTN_SIDE,
        KeyCode::BTN_EXTRA,
    ] {
        keys.insert(code);
    }
    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    for axis in [REL_X, RelativeAxisCode::REL_Y, REL_WHEEL, REL_WHEEL_HI_RES] {
        axes.insert(axis);
    }
    VirtualDevice::builder()
        .expect("opening /dev/uinput")
        .name(name)
        .input_id(InputId::new(BusType::BUS_VIRTUAL, 0x4353, 0x00ff, 1))
        .with_keys(&keys)
        .unwrap()
        .with_relative_axes(&axes)
        .unwrap()
        .build()
        .expect("creating the virtual test mouse")
}

fn open_named(name: &str) -> Device {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some((_, device)) =
            evdev::enumerate().find(|(_, device)| device.name().is_some_and(|n| n.trim() == name))
        {
            return device;
        }
        assert!(
            Instant::now() < deadline,
            "cannot open \"{name}\". Install packaging/71-clickshift-test.rules (see README)."
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn config_text(source_name: &str, body: &str) -> String {
    format!("[device]\nname = \"{source_name}\"\n\n{body}\n")
}

impl Harness {
    fn start(body: &str) -> Self {
        let id = format!(
            "{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let source_name = format!("clickshift-test {id} source");
        let prefix = format!("clickshift-test {id} out");
        let bus_name = format!("io.github.clickshift.Test.T{}", id.replace('-', "_"));

        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let config_path = dir.join(format!("e2e-{id}.toml"));
        std::fs::write(&config_path, config_text(&source_name, body)).unwrap();

        let source = build_source(&source_name);
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_clickshift"))
            .args(["run", "--verbose", "--config"])
            .arg(&config_path)
            .args(["--virtual-prefix", &prefix, "--bus-name", &bus_name])
            .stdout(Stdio::piped())
            .spawn()
            .expect("starting clickshift");
        let stdout = daemon.stdout.take().unwrap();
        let daemon = Daemon(daemon);

        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        // Taken before any input is sent, so nothing can reach the desktop.
        let mut out_mouse = open_named(&format!("{prefix} mouse"));
        let mut out_keyboard = open_named(&format!("{prefix} keyboard"));
        out_mouse.grab().expect("grabbing the output mouse");
        out_keyboard.grab().expect("grabbing the output keyboard");
        out_mouse.set_nonblocking(true).unwrap();
        out_keyboard.set_nonblocking(true).unwrap();

        let harness = Self {
            source: Some(source),
            source_name,
            out_mouse,
            out_keyboard,
            daemon,
            lines,
            config_path,
            bus_name,
        };
        harness.wait_for_line("Remapping");
        harness
    }

    fn wait_for_line(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return,
                Ok(_) => {}
                Err(_) => panic!("clickshift never printed \"{needle}\""),
            }
        }
    }

    fn send(&mut self, events: &[InputEvent]) {
        self.source.as_mut().unwrap().emit(events).unwrap();
    }

    fn button(&mut self, code: KeyCode, value: i32) {
        self.send(&[*KeyEvent::new(code, value)]);
    }

    fn click(&mut self, code: KeyCode) {
        self.button(code, 1);
        self.button(code, 0);
    }

    fn notch_up(&mut self) {
        self.send(&[
            *RelativeAxisEvent::new(REL_WHEEL_HI_RES, 120),
            *RelativeAxisEvent::new(REL_WHEEL, 1),
        ]);
    }

    fn write_config(&self, body: &str) {
        std::fs::write(&self.config_path, config_text(&self.source_name, body)).unwrap();
    }

    fn set_focus(&self, app_id: &str) {
        Connection::session()
            .unwrap()
            .call_method(
                Some(self.bus_name.as_str()),
                "/io/github/clickshift/Daemon",
                Some("io.github.clickshift.Focus1"),
                "SetFocus",
                &(app_id, ""),
            )
            .expect("calling SetFocus");
    }

    fn expect_keyboard(&mut self, expected: &[Seen]) {
        assert_eq!(collect(&mut self.out_keyboard, expected.len()), expected);
    }

    fn expect_mouse(&mut self, expected: &[Seen]) {
        assert_eq!(collect(&mut self.out_mouse, expected.len()), expected);
    }

    fn expect_quiet(&mut self) {
        std::thread::sleep(QUIET);
        assert_eq!(
            drain(&mut self.out_keyboard),
            [],
            "keyboard should be quiet"
        );
        assert_eq!(drain(&mut self.out_mouse), [], "mouse should be quiet");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.config_path);
    }
}

fn drain(device: &mut Device) -> Vec<Seen> {
    let Ok(events) = device.fetch_events() else {
        return Vec::new();
    };
    events
        .filter_map(|event| match event.destructure() {
            EventSummary::Key(_, code, value) => Some(Seen::Key(code, value)),
            EventSummary::RelativeAxis(_, axis, value) => Some(Seen::Rel(axis, value)),
            _ => None,
        })
        .collect()
}

/// Reads until `count` events arrive, then a little longer to catch extras.
fn collect(device: &mut Device, count: usize) -> Vec<Seen> {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    while seen.len() < count && Instant::now() < deadline {
        seen.extend(drain(device));
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(30));
    seen.extend(drain(device));
    seen
}

fn down(code: KeyCode) -> Seen {
    Seen::Key(code, 1)
}

fn up(code: KeyCode) -> Seen {
    Seen::Key(code, 0)
}

use KeyCode as K;

#[test]
#[ignore = "needs the test udev rule"]
fn forwards_motion_and_unmapped_input_unchanged() {
    let mut h = Harness::start("");
    h.send(&[*RelativeAxisEvent::new(REL_X, 7)]);
    h.expect_mouse(&[Seen::Rel(REL_X, 7)]);
    h.click(K::BTN_RIGHT);
    h.expect_mouse(&[down(K::BTN_RIGHT), up(K::BTN_RIGHT)]);
    h.notch_up();
    h.expect_mouse(&[Seen::Rel(REL_WHEEL_HI_RES, 120), Seen::Rel(REL_WHEEL, 1)]);
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn maps_a_button_to_a_chord() {
    let mut h = Harness::start("[default]\nside = \"Ctrl+F13\"");
    h.button(K::BTN_SIDE, 1);
    h.expect_keyboard(&[down(K::KEY_LEFTCTRL), down(K::KEY_F13)]);
    h.button(K::BTN_SIDE, 0);
    h.expect_keyboard(&[up(K::KEY_F13), up(K::KEY_LEFTCTRL)]);
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn switches_profile_when_focus_changes_over_dbus() {
    let mut h = Harness::start(
        "[default]\nside = \"F13\"\n[apps.brave-browser]\nside = \"F14\"\n[apps.zalo]\nside = \"disabled\"",
    );
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    h.set_focus("brave-browser.desktop");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F14), up(K::KEY_F14)]);

    h.set_focus("zalo.desktop");
    h.click(K::BTN_SIDE);
    h.expect_quiet();

    h.set_focus("something.else");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn reloads_config_and_survives_a_broken_edit() {
    let mut h = Harness::start("[default]\nside = \"F13\"");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    h.write_config("[default]\nside = \"NotARealKey\"");
    std::thread::sleep(Duration::from_millis(1500));
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    // F20 was not in the original config, so this also proves new keys work.
    h.write_config("[default]\nside = \"F20\"");
    h.wait_for_line("Reloaded config");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F20), up(K::KEY_F20)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn reconnects_and_releases_held_keys_when_the_mouse_vanishes() {
    let mut h = Harness::start("[default]\nside = \"F13\"");
    h.button(K::BTN_SIDE, 1);
    h.expect_keyboard(&[down(K::KEY_F13)]);

    h.source = None;
    h.wait_for_line("disconnected");
    h.expect_keyboard(&[up(K::KEY_F13)]);

    h.source = Some(build_source(&h.source_name));
    h.wait_for_line("Remapping");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);
}

/// The kernel sends releases for physically held buttons when a device goes
/// away, so only a latched key proves clickshift's own cleanup runs.
#[test]
#[ignore = "needs the test udev rule"]
fn releases_sticky_keys_when_the_mouse_vanishes() {
    let mut h = Harness::start("[default]\nside = \"sticky:F13\"");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13)]);

    h.source = None;
    h.wait_for_line("disconnected");
    h.expect_keyboard(&[up(K::KEY_F13)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn keeps_waiting_until_the_mouse_returns() {
    let mut h = Harness::start("[default]\nside = \"F13\"");
    h.source = None;
    h.wait_for_line("disconnected");
    h.wait_for_line("Waiting for a readable mouse");
    h.source = Some(build_source(&h.source_name));
    h.wait_for_line("Remapping");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn returns_the_mouse_when_killed() {
    let mut h = Harness::start("");
    let mut source = open_named(&h.source_name);
    assert!(source.grab().is_err(), "clickshift should hold the mouse");

    h.daemon.0.kill().unwrap();
    h.daemon.0.wait().unwrap();
    source.grab().expect("the kernel should free the mouse");
}

#[test]
#[ignore = "needs the test udev rule"]
fn run_action_starts_a_command() {
    let marker =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("e2e-run-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);

    let mut h = Harness::start(&format!(
        "[default]\nside = \"run:touch '{}'\"",
        marker.display()
    ));
    h.click(K::BTN_SIDE);

    let deadline = Instant::now() + TIMEOUT;
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "the command did not run");
    std::fs::remove_file(&marker).unwrap();
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn tap_and_hold_use_real_time() {
    let mut h =
        Harness::start("[default]\nside = { tap = \"F13\", hold = \"F14\", hold_ms = 150 }");
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    h.button(K::BTN_SIDE, 1);
    h.expect_keyboard(&[down(K::KEY_F14)]);
    h.button(K::BTN_SIDE, 0);
    h.expect_keyboard(&[up(K::KEY_F14)]);
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn button_combo_fires_and_single_buttons_still_work() {
    let mut h =
        Harness::start("[default]\nside = \"F13\"\nextra = \"F14\"\n\"side+extra\" = \"F15\"");
    h.button(K::BTN_SIDE, 1);
    h.button(K::BTN_EXTRA, 1);
    h.expect_keyboard(&[down(K::KEY_F15)]);
    h.button(K::BTN_SIDE, 0);
    h.expect_keyboard(&[up(K::KEY_F15)]);
    h.button(K::BTN_EXTRA, 0);
    h.expect_quiet();

    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    h.button(K::BTN_EXTRA, 1);
    h.expect_keyboard(&[down(K::KEY_F14)]);
    h.button(K::BTN_EXTRA, 0);
    h.expect_keyboard(&[up(K::KEY_F14)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn macro_plays_in_real_time_and_keeps_the_mouse_responsive() {
    let mut h = Harness::start("[default]\nside = \"macro:{F13}{WAITMS:300}{LMB}{WAITMS:100}\"");
    let start = Instant::now();
    h.click(K::BTN_SIDE);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    // The pointer keeps moving while the macro waits.
    h.send(&[*RelativeAxisEvent::new(REL_X, 7)]);
    h.expect_mouse(&[Seen::Rel(REL_X, 7)]);
    assert!(
        start.elapsed() < Duration::from_millis(290),
        "macro blocked the mouse"
    );

    h.expect_mouse(&[down(K::BTN_LEFT), up(K::BTN_LEFT)]);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300),
        "too early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(450),
        "too late: {elapsed:?}"
    );
    h.expect_quiet();
}

/// A long macro, measured with the kernel's own timestamps.
/// Scheduling errors must not add up over its whole length.
#[test]
#[ignore = "needs the test udev rule"]
fn long_macro_stays_on_schedule_from_start_to_end() {
    let text = "a{WAITMS:300}{LMB}{WAITMS:150}{LMB}{WAITMS:250}{RMB}{WAITMS:200}{LMBD}{WAITMS:400}{LMBU}{WAITMS:100}".repeat(8);
    let expected = Macro::parse(&text).unwrap();
    let mut h = Harness::start(&format!("[default]\nside = \"macro:{text}\""));
    h.click(K::BTN_SIDE);

    let mut seen: Vec<(SystemTime, KeyCode, i32)> = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(expected.total_ms + 3000);
    while seen.len() < expected.steps.len() && Instant::now() < deadline {
        for device in [&mut h.out_keyboard, &mut h.out_mouse] {
            if let Ok(events) = device.fetch_events() {
                seen.extend(events.filter_map(|event| match event.destructure() {
                    EventSummary::Key(_, code, value) => Some((event.timestamp(), code, value)),
                    _ => None,
                }));
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    seen.sort_by_key(|(time, _, _)| *time);
    assert_eq!(seen.len(), expected.steps.len());

    let first = seen[0].0;
    let mut worst = 0.0_f64;
    for (step, (time, code, value)) in expected.steps.iter().zip(&seen) {
        let wanted = match step.emit {
            Emit::Press(code) => (code, 1),
            Emit::Release(code) => (code, 0),
            Emit::Scroll(_) => unreachable!("this macro does not scroll"),
        };
        assert_eq!((*code, *value), wanted, "at {} ms", step.at_ms);
        let actual = time.duration_since(first).unwrap().as_secs_f64() * 1000.0;
        worst = worst.max((actual - step.at_ms as f64).abs());
    }
    println!(
        "{} steps over {} ms, worst error {worst:.2} ms",
        expected.steps.len(),
        expected.total_ms
    );
    assert!(worst < 15.0, "worst error {worst:.2} ms");
}

#[test]
#[ignore = "needs the test udev rule"]
fn hold_macro_stops_when_the_button_is_released() {
    let mut h = Harness::start("[default]\nside = \"macro-hold:{LMBD}{WAITMS:300}{F13}\"");
    h.button(K::BTN_SIDE, 1);
    h.expect_mouse(&[down(K::BTN_LEFT)]);
    h.button(K::BTN_SIDE, 0);
    h.expect_mouse(&[up(K::BTN_LEFT)]);
    std::thread::sleep(Duration::from_millis(350));
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn repeat_macro_loops_until_the_button_is_released() {
    let mut h = Harness::start("[default]\nside = \"macro-repeat:{F13}{WAITMS:100}\"");
    h.button(K::BTN_SIDE, 1);
    for _ in 0..3 {
        h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);
    }
    h.button(K::BTN_SIDE, 0);
    std::thread::sleep(Duration::from_millis(40));
    drain(&mut h.out_keyboard);
    std::thread::sleep(Duration::from_millis(150));
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn macro_stops_when_pressed_again() {
    let mut h = Harness::start("[default]\nside = \"macro:{LMBD}{WAITMS:300}{F13}\"");
    h.click(K::BTN_SIDE);
    h.expect_mouse(&[down(K::BTN_LEFT)]);
    h.click(K::BTN_SIDE);
    h.expect_mouse(&[up(K::BTN_LEFT)]);
    std::thread::sleep(Duration::from_millis(350));
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn macro_stops_when_the_focused_window_changes() {
    let mut h = Harness::start("[default]\nside = \"macro:{LMBD}{WAITMS:300}{F13}\"");
    h.set_focus("game");
    h.click(K::BTN_SIDE);
    h.expect_mouse(&[down(K::BTN_LEFT)]);
    h.set_focus("chat");
    h.expect_mouse(&[up(K::BTN_LEFT)]);
    std::thread::sleep(Duration::from_millis(350));
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn macro_stops_and_releases_when_the_mouse_vanishes() {
    let mut h = Harness::start("[default]\nside = \"macro:{LMBD}{WAITMS:2000}{F13}\"");
    h.click(K::BTN_SIDE);
    h.expect_mouse(&[down(K::BTN_LEFT)]);
    h.source = None;
    h.wait_for_line("disconnected");
    h.expect_mouse(&[up(K::BTN_LEFT)]);
    std::thread::sleep(Duration::from_millis(100));
    h.expect_quiet();
}

#[test]
#[ignore = "needs the test udev rule"]
fn sticky_button_holds_until_clicked_again() {
    let mut h = Harness::start("[default]\nside = \"sticky:mouse_middle\"");
    h.click(K::BTN_SIDE);
    h.expect_mouse(&[down(K::BTN_MIDDLE)]);
    h.expect_quiet();
    h.click(K::BTN_SIDE);
    h.expect_mouse(&[up(K::BTN_MIDDLE)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn layers_work_while_held_and_when_toggled() {
    let mut h = Harness::start(
        "[default]\nside = \"layer:1\"\nextra = \"F13\"\nmiddle = \"layer-toggle:1\"\n\
         [layers.1.default]\nextra = \"F14\"",
    );
    h.click(K::BTN_EXTRA);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    h.button(K::BTN_SIDE, 1);
    h.click(K::BTN_EXTRA);
    h.expect_keyboard(&[down(K::KEY_F14), up(K::KEY_F14)]);
    h.button(K::BTN_SIDE, 0);

    h.click(K::BTN_EXTRA);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);

    h.click(K::BTN_MIDDLE);
    h.click(K::BTN_EXTRA);
    h.expect_keyboard(&[down(K::KEY_F14), up(K::KEY_F14)]);

    h.click(K::BTN_MIDDLE);
    h.click(K::BTN_EXTRA);
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);
}

#[test]
#[ignore = "needs the test udev rule"]
fn wheel_can_become_keys_or_reverse_direction() {
    let mut h = Harness::start("[default]\nwheel_up = \"F13\"\nside = \"scroll:down\"");
    h.notch_up();
    h.expect_keyboard(&[down(K::KEY_F13), up(K::KEY_F13)]);
    h.expect_quiet();

    h.button(K::BTN_SIDE, 1);
    h.expect_mouse(&[Seen::Rel(REL_WHEEL, -1), Seen::Rel(REL_WHEEL_HI_RES, -120)]);
    h.button(K::BTN_SIDE, 0);
    h.expect_quiet();

    h.send(&[
        *RelativeAxisEvent::new(REL_WHEEL_HI_RES, -120),
        *RelativeAxisEvent::new(REL_WHEEL, -1),
    ]);
    h.expect_mouse(&[Seen::Rel(REL_WHEEL_HI_RES, -120), Seen::Rel(REL_WHEEL, -1)]);
}
