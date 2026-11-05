use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, Device, EventSummary, InputEvent, InputId, KeyCode, KeyEvent,
    RelativeAxisCode, RelativeAxisEvent, SynchronizationCode,
};

use clickshift::config::{
    self, Config, VIRTUAL_MOUSE_BUTTONS, WheelDir, is_keyboard_key, is_mouse_button,
};
use clickshift::device::{OWN_DEVICE_PREFIX, device_name, is_mouse};
use clickshift::engine::{Engine, Output};
use clickshift::focus::{self, SharedFocus};

const DEFAULT_VIRTUAL_PREFIX: &str = "clickshift virtual";
const RELEASE_WAIT: Duration = Duration::from_secs(10);
const DEVICE_RETRY: Duration = Duration::from_millis(500);
const CONFIG_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Gives the compositor time to adopt the virtual mouse before the real one is
/// grabbed, so the pointer never stops responding.
const VIRTUAL_DEVICE_SETTLE: Duration = Duration::from_millis(500);
const HI_RES_PER_NOTCH: i32 = 120;

#[derive(Parser)]
#[command(
    version,
    about = "Per-application mouse button remapper for GNOME on Wayland"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List readable mice and the buttons they report
    List,
    /// Validate a config file without touching any device
    Check {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Remap the configured mouse
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    /// Only print what would happen; the mouse keeps working normally
    #[arg(long)]
    dry_run: bool,
    /// Exit after this many seconds, returning the mouse (for safe testing)
    #[arg(long, value_name = "SECONDS")]
    grab_seconds: Option<u64>,
    /// Print every focus change and every button
    #[arg(long)]
    verbose: bool,
    /// Lets the test suite run beside a real instance
    #[arg(long, hide = true, default_value = DEFAULT_VIRTUAL_PREFIX)]
    virtual_prefix: String,
    /// Lets the test suite run beside a real instance
    #[arg(long, hide = true, default_value = focus::DEFAULT_BUS_NAME)]
    bus_name: String,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::List => list(),
        Command::Check { config } => {
            let cfg = Config::load(&config_path(config)?)?;
            println!("OK: {}", cfg.summary());
            Ok(())
        }
        Command::Run(args) => run(args),
    }
}

fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    explicit.map_or_else(config::default_path, Ok)
}

fn list() -> Result<()> {
    let mut found = 0;
    for (path, device) in evdev::enumerate() {
        if !is_mouse(&device) || device_name(&device).starts_with(OWN_DEVICE_PREFIX) {
            continue;
        }
        found += 1;
        println!("{}  \"{}\"", path.display(), device_name(&device));
        if let Some(keys) = device.supported_keys() {
            let buttons: Vec<String> = keys
                .iter()
                .filter(|key| is_mouse_button(*key))
                .map(|key| format!("{key:?}"))
                .collect();
            println!("    buttons: {}", buttons.join(", "));
        }
    }
    if found == 0 {
        println!("No readable mouse found. Install the udev rule from the README first.");
    }
    Ok(())
}

/// The config file, reloaded whenever it changes. A broken edit is reported and
/// ignored so a typo never takes the mouse away.
struct LiveConfig {
    path: PathBuf,
    config: Config,
    modified: Option<SystemTime>,
}

impl LiveConfig {
    fn load(path: PathBuf) -> Result<Self> {
        let modified = modified_time(&path);
        let config = Config::load(&path)?;
        println!("{}", config.summary());
        Ok(Self {
            path,
            config,
            modified,
        })
    }

    fn reload_if_changed(&mut self) {
        let Some(modified) = modified_time(&self.path) else {
            return;
        };
        if self.modified == Some(modified) {
            return;
        }
        self.modified = Some(modified);
        match Config::load(&self.path) {
            Ok(config) => {
                println!("Reloaded config: {}", config.summary());
                self.config = config;
            }
            Err(error) => eprintln!("Keeping previous config: {error:#}"),
        }
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn start_focus_service(args: &RunArgs) -> SharedFocus {
    let shared: SharedFocus = Arc::new(RwLock::new(config::Focus::default()));
    match focus::serve(shared.clone(), args.verbose, &args.bus_name) {
        // The connection serves requests on a background thread and must never
        // be dropped, so it deliberately lives until the process exits.
        Ok(connection) => std::mem::forget(connection),
        Err(error) => {
            eprintln!("warning: no focus tracking, using the default profile only: {error:#}");
        }
    }
    shared
}

/// Where outputs go: nowhere in a dry run, otherwise the virtual devices.
enum Sink {
    Print,
    Devices {
        mouse: VirtualDevice,
        keyboard: VirtualDevice,
        verbose: bool,
    },
}

impl Sink {
    fn create(prefix: &str, verbose: bool) -> Result<Self> {
        Ok(Self::Devices {
            mouse: build_virtual_mouse(prefix)?,
            keyboard: build_virtual_keyboard(prefix)?,
            verbose,
        })
    }

    fn is_dry(&self) -> bool {
        matches!(self, Self::Print)
    }

    fn forward(&mut self, events: &mut Vec<InputEvent>) -> Result<()> {
        if let Self::Devices { mouse, .. } = self
            && !events.is_empty()
        {
            mouse.emit(events)?;
        }
        events.clear();
        Ok(())
    }

    fn send(&mut self, outputs: Vec<Output>) -> Result<()> {
        for output in outputs {
            match self {
                Self::Print => println!("  -> {output:?}"),
                Self::Devices {
                    mouse,
                    keyboard,
                    verbose,
                } => {
                    if *verbose {
                        println!("  -> {output:?}");
                    }
                    match output {
                        Output::Key(code, value) => {
                            let event = *KeyEvent::new(code, value);
                            if is_mouse_button(code) {
                                mouse.emit(&[event])?;
                            } else {
                                keyboard.emit(&[event])?;
                            }
                        }
                        Output::Scroll(direction) => mouse.emit(&scroll_events(direction))?,
                        Output::Run(command) => spawn_command(command),
                    }
                }
            }
        }
        Ok(())
    }
}

fn scroll_events(direction: WheelDir) -> [InputEvent; 2] {
    let (axis, hi_res, sign) = match direction {
        WheelDir::Up => (
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            1,
        ),
        WheelDir::Down => (
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            -1,
        ),
        WheelDir::Right => (
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
            1,
        ),
        WheelDir::Left => (
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
            -1,
        ),
    };
    [
        *RelativeAxisEvent::new(axis, sign),
        *RelativeAxisEvent::new(hi_res, sign * HI_RES_PER_NOTCH),
    ]
}

fn spawn_command(command: String) {
    let child = Process::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        // Waits on its own thread so a long-running app cannot stall the mouse.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(error) => eprintln!("could not run \"{command}\": {error}"),
    }
}

fn virtual_id(product: u16) -> InputId {
    InputId::new(BusType::BUS_VIRTUAL, 0x4353, product, 1)
}

/// Declares a fixed set of capabilities, so the devices can be created before
/// the mouse appears and survive it being replugged.
fn build_virtual_mouse(prefix: &str) -> Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in VIRTUAL_MOUSE_BUTTONS {
        keys.insert(KeyCode::new(code));
    }

    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    for axis in [
        RelativeAxisCode::REL_X,
        RelativeAxisCode::REL_Y,
        RelativeAxisCode::REL_WHEEL,
        RelativeAxisCode::REL_HWHEEL,
        RelativeAxisCode::REL_WHEEL_HI_RES,
        RelativeAxisCode::REL_HWHEEL_HI_RES,
    ] {
        axes.insert(axis);
    }

    VirtualDevice::builder()?
        .name(&format!("{prefix} mouse"))
        .input_id(virtual_id(1))
        .with_keys(&keys)?
        .with_relative_axes(&axes)?
        .build()
        .context("creating virtual mouse")
}

fn build_virtual_keyboard(prefix: &str) -> Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 0..=u16::MAX {
        let code = KeyCode::new(code);
        if is_keyboard_key(code) {
            keys.insert(code);
        }
    }
    VirtualDevice::builder()?
        .name(&format!("{prefix} keyboard"))
        .input_id(virtual_id(2))
        .with_keys(&keys)?
        .build()
        .context("creating virtual keyboard")
}

fn find_device(name: &str) -> Option<Device> {
    evdev::enumerate()
        .map(|(_, device)| device)
        .find(|device| is_mouse(device) && device_name(device) == name)
}

/// Grabbing while a button is down would leave it stuck for the compositor.
fn wait_for_buttons_released(device: &Device) -> Result<()> {
    let start = Instant::now();
    let mut announced = false;
    while device.get_key_state()?.iter().next().is_some() {
        if start.elapsed() > RELEASE_WAIT {
            bail!("a mouse button stayed pressed for {RELEASE_WAIT:?}");
        }
        if !announced {
            println!("Waiting for all mouse buttons to be released...");
            announced = true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn start_watchdog(seconds: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        println!("Test time of {seconds}s is over, returning the mouse.");
        std::process::exit(0);
    });
}

/// Safety relies on the kernel: when this process ends for any reason, closing
/// its file descriptors drops the grab and destroys the virtual devices, which
/// also releases anything they held down. No cleanup handler can be skipped.
fn run(args: RunArgs) -> Result<()> {
    let mut live = LiveConfig::load(config_path(args.config.clone())?)?;
    let focus = start_focus_service(&args);
    let mut sink = if args.dry_run {
        Sink::Print
    } else {
        let sink = Sink::create(&args.virtual_prefix, args.verbose)?;
        std::thread::sleep(VIRTUAL_DEVICE_SETTLE);
        sink
    };
    if let Some(seconds) = args.grab_seconds.filter(|_| !args.dry_run) {
        start_watchdog(seconds);
    }

    let mut engine = Engine::new();
    let mut waiting_announced = false;
    loop {
        live.reload_if_changed();
        let Some(mut device) = find_device(&live.config.device_name) else {
            if !waiting_announced {
                println!(
                    "Waiting for a readable mouse named \"{}\" (is it connected, and is the udev rule installed?)",
                    live.config.device_name
                );
                waiting_announced = true;
            }
            std::thread::sleep(DEVICE_RETRY);
            continue;
        };

        if let Err(error) = take_device(&mut device, &sink) {
            eprintln!("Cannot use the mouse yet: {error:#}");
            std::thread::sleep(DEVICE_RETRY);
            continue;
        }
        waiting_announced = false;
        let verb = if sink.is_dry() {
            "Dry run on"
        } else {
            "Remapping"
        };
        println!("{verb} \"{}\".", device_name(&device));

        let reason = pump(
            &mut device,
            &mut live,
            &focus,
            &mut engine,
            &mut sink,
            args.verbose,
        )?;
        println!("{reason}, looking for the mouse again.");
        sink.send(engine.reset())?;
    }
}

fn take_device(device: &mut Device, sink: &Sink) -> Result<()> {
    if !sink.is_dry() {
        wait_for_buttons_released(device)?;
        device
            .grab()
            .context("grabbing mouse (is another remapper using it?)")?;
    }
    device.set_nonblocking(true).context("configuring mouse")
}

/// Processes events until the mouse goes away or the config points at another.
fn pump(
    device: &mut Device,
    live: &mut LiveConfig,
    focus: &SharedFocus,
    engine: &mut Engine,
    sink: &mut Sink,
    verbose: bool,
) -> Result<&'static str> {
    let mut pending: Vec<InputEvent> = Vec::with_capacity(8);
    let mut next_config_check = Instant::now() + CONFIG_CHECK_INTERVAL;
    let dry = sink.is_dry();

    loop {
        let wake_at = engine
            .next_deadline()
            .map_or(next_config_check, |deadline| {
                deadline.min(next_config_check)
            });
        wait_readable(device, wake_at)?;

        match device.fetch_events() {
            Ok(events) => {
                let events: Vec<InputEvent> = events.collect();
                let log = verbose || dry;
                for event in events {
                    handle_event(event, &live.config, focus, engine, sink, &mut pending, log)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Ok("The mouse disconnected"),
        }
        sink.forward(&mut pending)?;

        let now = Instant::now();
        if engine
            .next_deadline()
            .is_some_and(|deadline| now >= deadline)
        {
            let current = focus::current(focus);
            sink.send(engine.tick(&live.config, &current, now))?;
        }
        if now >= next_config_check {
            next_config_check = now + CONFIG_CHECK_INTERVAL;
            live.reload_if_changed();
            if device_name(device) != live.config.device_name {
                return Ok("The configured mouse changed");
            }
        }
    }
}

fn wait_readable(device: &Device, wake_at: Instant) -> Result<()> {
    let timeout = wake_at.saturating_duration_since(Instant::now());
    // Rounds up so a deadline is never missed by waking a moment early forever.
    let millis = i32::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
    let mut descriptor = libc::pollfd {
        fd: device.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` is a valid pollfd that outlives this call, and the
    // count of 1 matches it.
    let result = unsafe { libc::poll(&raw mut descriptor, 1, millis) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("waiting for mouse events");
        }
    }
    Ok(())
}

fn handle_event(
    event: InputEvent,
    config: &Config,
    focus: &SharedFocus,
    engine: &mut Engine,
    sink: &mut Sink,
    pending: &mut Vec<InputEvent>,
    log: bool,
) -> Result<()> {
    match event.destructure() {
        EventSummary::RelativeAxis(_, axis, value) => {
            let Some((direction, is_hi_res)) = wheel_direction(axis, value) else {
                pending.push(event);
                return Ok(());
            };
            let current = focus::current(focus);
            let Some(action) = engine.wheel_action(config, &current, direction) else {
                pending.push(event);
                return Ok(());
            };
            // A remapped direction swallows both the notch and hi-res events,
            // and fires on the notch so one physical click fires once.
            if !is_hi_res {
                sink.forward(pending)?;
                let outputs =
                    engine.on_wheel(action, value.unsigned_abs(), &current, Instant::now());
                if log {
                    println!("{direction:?} notches={}", value.unsigned_abs());
                }
                sink.send(outputs)?;
            }
        }
        EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
            sink.forward(pending)?;
        }
        EventSummary::Key(_, button, value) => {
            // Keep motion ordered before the click it preceded.
            sink.forward(pending)?;
            let current = focus::current(focus);
            let outputs = engine.on_button(config, &current, button, value, Instant::now());
            if log {
                println!(
                    "{button:?} value={value} app=\"{}\" layer={}",
                    current.app_id,
                    engine.layer()
                );
            }
            sink.send(outputs)?;
        }
        _ => {}
    }
    Ok(())
}

fn wheel_direction(axis: RelativeAxisCode, value: i32) -> Option<(WheelDir, bool)> {
    let (positive, negative, is_hi_res) = match axis {
        RelativeAxisCode::REL_WHEEL => (WheelDir::Up, WheelDir::Down, false),
        RelativeAxisCode::REL_WHEEL_HI_RES => (WheelDir::Up, WheelDir::Down, true),
        RelativeAxisCode::REL_HWHEEL => (WheelDir::Right, WheelDir::Left, false),
        RelativeAxisCode::REL_HWHEEL_HI_RES => (WheelDir::Right, WheelDir::Left, true),
        _ => return None,
    };
    Some((if value > 0 { positive } else { negative }, is_hi_res))
}
