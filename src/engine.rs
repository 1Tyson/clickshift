//! Turns physical input into output events. Contains no device I/O, and takes
//! the current time as a parameter so timing can be tested without waiting.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use evdev::KeyCode;

use crate::config::{Action, Config, Focus, Input, MacroMode, Timed, WheelDir};
use crate::macros::{Emit, Macro};

const RELEASE: i32 = 0;
const PRESS: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Key(KeyCode, i32),
    Scroll(WheelDir),
    Run(String),
}

/// What must be undone when a physical button is released.
enum Hold {
    Keys(Vec<KeyCode>),
    Layer(u8),
    /// Tells this run of a macro apart from a later one started another way.
    Macro(u64),
    Nothing,
}

/// The context an action starts in.
#[derive(Clone, Copy)]
struct Moment<'a> {
    focus: &'a Focus,
    now: Instant,
}

struct Playing {
    id: u64,
    repeat: bool,
    recipe: Arc<Macro>,
    start: Instant,
    next_step: usize,
    /// The macro stops if this stops being the focused window, so its keys
    /// never land in whatever the user switched to.
    focus: Focus,
    /// What the macro has pressed and not released yet.
    held: Vec<KeyCode>,
}

impl Playing {
    fn deadline(&self) -> Instant {
        let at_ms = self
            .recipe
            .steps
            .get(self.next_step)
            .map_or(self.recipe.total_ms, |step| step.at_ms);
        self.start + Duration::from_millis(at_ms)
    }
}

#[derive(Default)]
pub struct Engine {
    /// Recorded per button so a release undoes exactly what its press did, even
    /// if the focused app or layer changed in between.
    held: HashMap<KeyCode, Hold>,
    /// How many sources hold each output, so two chords sharing Ctrl do not
    /// release it until the last one is let go.
    down: HashMap<KeyCode, u32>,
    /// Chords latched on by a sticky action.
    stuck: HashSet<Vec<KeyCode>>,
    /// Momentary layers in press order; the latest wins over the toggled one.
    momentary: Vec<u8>,
    toggled: u8,
    /// A button that may yet become a combo, waiting for its partner.
    combo_wait: Option<(KeyCode, Instant)>,
    /// Buttons still deciding between tap and hold.
    timed: HashMap<KeyCode, (Timed, Instant)>,
    /// The two buttons of an active combo, pointing at each other.
    combo_partner: HashMap<KeyCode, KeyCode>,
    /// Physical buttons whose release must be ignored.
    ignored_releases: HashSet<KeyCode>,
    playing: Option<Playing>,
    last_macro_id: u64,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layer(&self) -> u8 {
        self.momentary.last().copied().unwrap_or(self.toggled)
    }

    pub fn on_button(
        &mut self,
        config: &Config,
        focus: &Focus,
        button: KeyCode,
        value: i32,
        now: Instant,
    ) -> Vec<Output> {
        let mut output = Vec::new();
        match value {
            PRESS => self.press(config, focus, button, now, &mut output),
            RELEASE => self.release(config, focus, button, now, &mut output),
            // Autorepeat: the output is already held down.
            _ => {}
        }
        output
    }

    /// `None` means the wheel direction is not remapped and should be forwarded.
    pub fn wheel_action<'a>(
        &self,
        config: &'a Config,
        focus: &Focus,
        direction: WheelDir,
    ) -> Option<&'a Action> {
        match config.resolve(Input::Wheel(direction), focus, self.layer()) {
            Action::Passthrough => None,
            action => Some(action),
        }
    }

    /// Fires a wheel action once per notch.
    pub fn on_wheel(
        &mut self,
        action: &Action,
        notches: u32,
        focus: &Focus,
        now: Instant,
    ) -> Vec<Output> {
        let mut output = Vec::new();
        let moment = Moment { focus, now };
        for _ in 0..notches {
            let hold = self.start(action, KeyCode::KEY_RESERVED, moment, &mut output);
            self.finish(hold, &mut output);
        }
        output
    }

    /// The earliest time `tick` needs to run, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        let combo = self.combo_wait.map(|(_, deadline)| deadline);
        let timed = self.timed.values().map(|(_, deadline)| *deadline).min();
        let playing = self.playing.as_ref().map(Playing::deadline);
        combo.into_iter().chain(timed).chain(playing).min()
    }

    /// Fires everything whose wait has run out.
    pub fn tick(&mut self, config: &Config, focus: &Focus, now: Instant) -> Vec<Output> {
        let mut output = Vec::new();

        if let Some((button, deadline)) = self.combo_wait
            && now >= deadline
        {
            self.combo_wait = None;
            self.press_now(config, focus, button, now, &mut output);
        }

        let expired: Vec<KeyCode> = self
            .timed
            .iter()
            .filter(|(_, (_, deadline))| now >= *deadline)
            .map(|(button, _)| *button)
            .collect();
        for button in expired {
            if let Some((timed, _)) = self.timed.remove(&button) {
                let hold = self.start(&timed.hold, button, Moment { focus, now }, &mut output);
                self.held.insert(button, hold);
            }
        }
        self.advance_macro(Moment { focus, now }, &mut output);
        output
    }

    /// Releases every output and forgets physical state, for when the mouse is
    /// gone and its release events will never come. A toggled layer stays on.
    pub fn reset(&mut self) -> Vec<Output> {
        let output = self
            .down
            .drain()
            .map(|(code, _)| Output::Key(code, RELEASE))
            .collect();
        self.held.clear();
        self.stuck.clear();
        self.momentary.clear();
        self.combo_wait = None;
        self.timed.clear();
        self.combo_partner.clear();
        self.ignored_releases.clear();
        self.playing = None;
        output
    }

    fn is_tracked(&self, button: KeyCode) -> bool {
        self.held.contains_key(&button)
            || self.timed.contains_key(&button)
            || self.ignored_releases.contains(&button)
            || self.combo_wait.is_some_and(|(first, _)| first == button)
    }

    fn press(
        &mut self,
        config: &Config,
        focus: &Focus,
        button: KeyCode,
        now: Instant,
        output: &mut Vec<Output>,
    ) {
        if self.is_tracked(button) {
            return;
        }

        if let Some((first, _)) = self.combo_wait.take() {
            if let Some(action) = config.combo(first, button, focus, self.layer()) {
                let hold = self.start(action, first, Moment { focus, now }, output);
                self.held.insert(first, hold);
                self.combo_partner.insert(first, button);
                self.combo_partner.insert(button, first);
                self.ignored_releases.insert(button);
                return;
            }
            // Not a combo after all: the first button is an ordinary press.
            self.press_now(config, focus, first, now, output);
        }

        if config.in_combo(button, focus, self.layer()) {
            let wait = Duration::from_millis(config.combo_ms);
            self.combo_wait = Some((button, now + wait));
            return;
        }
        self.press_now(config, focus, button, now, output);
    }

    fn press_now(
        &mut self,
        config: &Config,
        focus: &Focus,
        button: KeyCode,
        now: Instant,
        output: &mut Vec<Output>,
    ) {
        match config.resolve(Input::Button(button), focus, self.layer()) {
            Action::Timed(timed) => {
                let deadline = now + Duration::from_millis(timed.hold_ms);
                self.timed.insert(button, ((**timed).clone(), deadline));
            }
            action => {
                let hold = self.start(action, button, Moment { focus, now }, output);
                self.held.insert(button, hold);
            }
        }
    }

    fn release(
        &mut self,
        config: &Config,
        focus: &Focus,
        button: KeyCode,
        now: Instant,
        output: &mut Vec<Output>,
    ) {
        // Released before a partner came: it was a plain click all along.
        if self.combo_wait.is_some_and(|(first, _)| first == button) {
            self.combo_wait = None;
            self.press_now(config, focus, button, now, output);
        }

        if let Some((timed, _)) = self.timed.remove(&button) {
            let hold = self.start(&timed.tap, button, Moment { focus, now }, output);
            self.finish(hold, output);
            return;
        }

        // Whichever combo button lifts first ends the combo; the other button's
        // release then has nothing left to do.
        if let Some(partner) = self.combo_partner.remove(&button) {
            self.combo_partner.remove(&partner);
            if self.ignored_releases.remove(&button) {
                if let Some(hold) = self.held.remove(&partner) {
                    self.finish(hold, output);
                }
            } else {
                if let Some(hold) = self.held.remove(&button) {
                    self.finish(hold, output);
                }
                self.ignored_releases.insert(partner);
            }
            return;
        }

        if self.ignored_releases.remove(&button) {
            return;
        }
        if let Some(hold) = self.held.remove(&button) {
            self.finish(hold, output);
        }
    }

    fn start(
        &mut self,
        action: &Action,
        button: KeyCode,
        moment: Moment,
        output: &mut Vec<Output>,
    ) -> Hold {
        match action {
            Action::Passthrough => {
                self.press_codes(&[button], output);
                Hold::Keys(vec![button])
            }
            Action::Keys(codes) => {
                self.press_codes(codes, output);
                Hold::Keys(codes.clone())
            }
            Action::Sticky(codes) => {
                if self.stuck.remove(codes) {
                    self.release_codes(codes, output);
                } else {
                    self.press_codes(codes, output);
                    self.stuck.insert(codes.clone());
                }
                Hold::Nothing
            }
            Action::Run(command) => {
                output.push(Output::Run(command.clone()));
                Hold::Nothing
            }
            Action::Scroll(direction) => {
                output.push(Output::Scroll(*direction));
                Hold::Nothing
            }
            Action::Layer(layer) => {
                self.momentary.push(*layer);
                Hold::Layer(*layer)
            }
            Action::LayerToggle(layer) => {
                self.toggled = if self.toggled == *layer { 0 } else { *layer };
                Hold::Nothing
            }
            Action::Macro(recipe, MacroMode::Toggle) => {
                let same = self
                    .playing
                    .as_ref()
                    .is_some_and(|playing| playing.recipe == *recipe);
                self.stop_macro(output);
                if !same {
                    self.start_macro(recipe, false, moment, output);
                }
                Hold::Nothing
            }
            Action::Macro(recipe, mode) => {
                self.stop_macro(output);
                let repeat = *mode == MacroMode::RepeatWhileHeld;
                Hold::Macro(self.start_macro(recipe, repeat, moment, output))
            }
            // Config parsing forbids nesting, and press_now handles the top level.
            Action::Disabled | Action::Timed(_) => Hold::Nothing,
        }
    }

    fn finish(&mut self, hold: Hold, output: &mut Vec<Output>) {
        match hold {
            Hold::Keys(codes) => self.release_codes(&codes, output),
            Hold::Layer(layer) => {
                if let Some(index) = self.momentary.iter().rposition(|l| *l == layer) {
                    self.momentary.remove(index);
                }
            }
            Hold::Macro(id) => {
                if self
                    .playing
                    .as_ref()
                    .is_some_and(|playing| playing.id == id)
                {
                    self.stop_macro(output);
                }
            }
            Hold::Nothing => {}
        }
    }

    fn stop_macro(&mut self, output: &mut Vec<Output>) {
        if let Some(playing) = self.playing.take() {
            self.release_codes(&playing.held, output);
        }
    }

    /// Only one macro plays at a time, so the caller stops any other first.
    fn start_macro(
        &mut self,
        recipe: &Arc<Macro>,
        repeat: bool,
        moment: Moment,
        output: &mut Vec<Output>,
    ) -> u64 {
        self.last_macro_id += 1;
        self.playing = Some(Playing {
            id: self.last_macro_id,
            repeat,
            recipe: recipe.clone(),
            start: moment.now,
            next_step: 0,
            focus: moment.focus.clone(),
            held: Vec::new(),
        });
        self.advance_macro(moment, output);
        self.last_macro_id
    }

    fn advance_macro(&mut self, moment: Moment, output: &mut Vec<Output>) {
        let Some(mut playing) = self.playing.take() else {
            return;
        };
        if playing.focus != *moment.focus {
            self.release_codes(&playing.held, output);
            return;
        }

        let recipe = playing.recipe.clone();
        loop {
            while let Some(step) = recipe.steps.get(playing.next_step) {
                if playing.start + Duration::from_millis(step.at_ms) > moment.now {
                    break;
                }
                match step.emit {
                    Emit::Press(code) => {
                        if !playing.held.contains(&code) {
                            self.press_codes(&[code], output);
                            playing.held.push(code);
                        }
                    }
                    // Only what the macro pressed, never a button the user is holding.
                    Emit::Release(code) => {
                        if let Some(index) = playing.held.iter().position(|held| *held == code) {
                            playing.held.remove(index);
                            self.release_codes(&[code], output);
                        }
                    }
                    Emit::Scroll(direction) => output.push(Output::Scroll(direction)),
                }
                playing.next_step += 1;
            }

            let end = playing.start + Duration::from_millis(recipe.total_ms);
            if playing.next_step < recipe.steps.len() || moment.now < end {
                self.playing = Some(playing);
                return;
            }
            // A macro that presses without releasing must not leave a key stuck.
            self.release_codes(&playing.held, output);
            playing.held.clear();
            if !playing.repeat {
                return;
            }
            // From the planned end, so repeats do not drift. Config parsing
            // guarantees a repeating macro has a length, so this loop ends.
            playing.start = end;
            playing.next_step = 0;
        }
    }

    fn press_codes(&mut self, codes: &[KeyCode], output: &mut Vec<Output>) {
        for &code in codes {
            let count = self.down.entry(code).or_insert(0);
            *count += 1;
            if *count == 1 {
                output.push(Output::Key(code, PRESS));
            }
        }
    }

    fn release_codes(&mut self, codes: &[KeyCode], output: &mut Vec<Output>) {
        for &code in codes.iter().rev() {
            if let Some(count) = self.down.get_mut(&code) {
                *count -= 1;
                if *count == 0 {
                    self.down.remove(&code);
                    output.push(Output::Key(code, RELEASE));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyCode as K;

    const CONFIG: &str = r#"
        [device]
        name = "m"
        combo_ms = 50

        [default]
        side = "Ctrl+W"
        extra = "Ctrl+T"
        task = "disabled"
        back = "layer:1"
        forward = "layer-toggle:1"
        right = "sticky:mouse_left"
        middle = { tap = "F13", hold = "F14", hold_ms = 300 }
        BTN_0 = "run:echo hi"
        BTN_1 = "scroll:down"
        BTN_2 = "F15"
        BTN_3 = "F16"
        BTN_4 = "macro:{F20}{WAITMS:100}{MOUSE_LEFT}{WAITMS:100}"
        BTN_5 = "macro:{PRESS:F21}{WAITMS:100}{F22}"
        BTN_6 = { tap = "macro:{F20}", hold = "F14", hold_ms = 300 }
        BTN_7 = "macro-hold:{PRESS:F21}{WAITMS:100}{F22}"
        BTN_8 = "macro-repeat:{F20}{WAITMS:100}"
        "BTN_2+BTN_3" = "F17"
        wheel_up = "F18"

        [apps.firefox]
        side = "F5"

        [layers.1.default]
        extra = "F19"
    "#;

    struct Rig {
        cfg: Config,
        engine: Engine,
        focus: Focus,
        start: Instant,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                cfg: Config::parse(CONFIG).unwrap(),
                engine: Engine::new(),
                focus: Focus::default(),
                start: Instant::now(),
            }
        }

        fn at(&self, ms: u64) -> Instant {
            self.start + Duration::from_millis(ms)
        }

        fn btn(&mut self, button: K, value: i32, ms: u64) -> Vec<Output> {
            let now = self.at(ms);
            self.engine
                .on_button(&self.cfg, &self.focus, button, value, now)
        }

        fn tick(&mut self, ms: u64) -> Vec<Output> {
            let now = self.at(ms);
            self.engine.tick(&self.cfg, &self.focus, now)
        }
    }

    fn keys(pairs: &[(K, i32)]) -> Vec<Output> {
        pairs.iter().map(|(k, v)| Output::Key(*k, *v)).collect()
    }

    #[test]
    fn unmapped_button_passes_through() {
        let cfg = Config::parse("[device]\nname='m'\n").unwrap();
        let mut engine = Engine::new();
        let focus = Focus::default();
        let now = Instant::now();
        assert_eq!(
            engine.on_button(&cfg, &focus, K::BTN_RIGHT, 1, now),
            keys(&[(K::BTN_RIGHT, 1)])
        );
        assert_eq!(
            engine.on_button(&cfg, &focus, K::BTN_RIGHT, 0, now),
            keys(&[(K::BTN_RIGHT, 0)])
        );
    }

    #[test]
    fn chord_presses_in_order_and_releases_in_reverse() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.btn(K::BTN_SIDE, 1, 0),
            keys(&[(K::KEY_LEFTCTRL, 1), (K::KEY_W, 1)])
        );
        assert_eq!(
            rig.btn(K::BTN_SIDE, 0, 1),
            keys(&[(K::KEY_W, 0), (K::KEY_LEFTCTRL, 0)])
        );
    }

    #[test]
    fn disabled_button_emits_nothing() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_TASK, 1, 0).is_empty());
        assert!(rig.btn(K::BTN_TASK, 0, 1).is_empty());
    }

    #[test]
    fn release_matches_press_when_focus_changes_while_held() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_SIDE, 1, 0);
        rig.focus.app_id = "firefox".into();
        assert_eq!(
            rig.btn(K::BTN_SIDE, 0, 1),
            keys(&[(K::KEY_W, 0), (K::KEY_LEFTCTRL, 0)])
        );
    }

    #[test]
    fn shared_modifier_stays_down_until_last_button_released() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_SIDE, 1, 0);
        assert_eq!(rig.btn(K::BTN_EXTRA, 1, 1), keys(&[(K::KEY_T, 1)]));
        assert_eq!(rig.btn(K::BTN_SIDE, 0, 2), keys(&[(K::KEY_W, 0)]));
        assert_eq!(
            rig.btn(K::BTN_EXTRA, 0, 3),
            keys(&[(K::KEY_T, 0), (K::KEY_LEFTCTRL, 0)])
        );
    }

    #[test]
    fn repeats_duplicates_and_stray_releases_emit_nothing() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_SIDE, 0, 0).is_empty());
        rig.btn(K::BTN_SIDE, 1, 1);
        assert!(rig.btn(K::BTN_SIDE, 2, 2).is_empty());
        assert!(rig.btn(K::BTN_SIDE, 1, 3).is_empty());
        assert_eq!(rig.btn(K::BTN_SIDE, 0, 4).len(), 2);
        assert!(rig.btn(K::BTN_SIDE, 0, 5).is_empty());
    }

    #[test]
    fn quick_click_fires_tap_action_only() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_MIDDLE, 1, 0).is_empty());
        assert_eq!(rig.engine.next_deadline(), Some(rig.at(300)));
        assert_eq!(
            rig.btn(K::BTN_MIDDLE, 0, 100),
            keys(&[(K::KEY_F13, 1), (K::KEY_F13, 0)])
        );
        assert_eq!(rig.engine.next_deadline(), None);
        assert!(rig.tick(400).is_empty());
    }

    #[test]
    fn long_press_fires_hold_action_until_release() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_MIDDLE, 1, 0);
        assert!(rig.tick(299).is_empty());
        assert_eq!(rig.tick(300), keys(&[(K::KEY_F14, 1)]));
        assert!(rig.tick(500).is_empty());
        assert_eq!(rig.btn(K::BTN_MIDDLE, 0, 600), keys(&[(K::KEY_F14, 0)]));
    }

    #[test]
    fn combo_fires_once_and_ends_when_either_button_lifts() {
        for (lift_first, lift_second) in [(K::BTN_2, K::BTN_3), (K::BTN_3, K::BTN_2)] {
            let mut rig = Rig::new();
            assert!(rig.btn(K::BTN_2, 1, 0).is_empty());
            assert_eq!(rig.btn(K::BTN_3, 1, 20), keys(&[(K::KEY_F17, 1)]));
            assert_eq!(rig.btn(lift_first, 0, 30), keys(&[(K::KEY_F17, 0)]));
            assert!(rig.btn(lift_second, 0, 40).is_empty());
            // Both buttons work normally afterwards.
            assert!(rig.btn(K::BTN_2, 1, 50).is_empty());
            assert_eq!(rig.tick(100), keys(&[(K::KEY_F15, 1)]));
            assert_eq!(rig.btn(K::BTN_2, 0, 110), keys(&[(K::KEY_F15, 0)]));
        }
    }

    #[test]
    fn combo_member_alone_acts_normally_after_the_wait() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_2, 1, 0).is_empty());
        assert!(rig.tick(49).is_empty());
        assert_eq!(rig.tick(50), keys(&[(K::KEY_F15, 1)]));
        // Too late to be a combo: the second button acts on its own (and waits).
        assert!(rig.btn(K::BTN_3, 1, 60).is_empty());
        assert_eq!(rig.tick(110), keys(&[(K::KEY_F16, 1)]));
    }

    #[test]
    fn combo_member_clicked_quickly_still_fires() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_2, 1, 0).is_empty());
        assert_eq!(
            rig.btn(K::BTN_2, 0, 10),
            keys(&[(K::KEY_F15, 1), (K::KEY_F15, 0)])
        );
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn unrelated_press_during_combo_wait_flushes_first_button() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_2, 1, 0);
        assert_eq!(
            rig.btn(K::BTN_SIDE, 1, 10),
            keys(&[(K::KEY_F15, 1), (K::KEY_LEFTCTRL, 1), (K::KEY_W, 1)])
        );
    }

    #[test]
    fn sticky_latches_until_second_click() {
        let mut rig = Rig::new();
        assert_eq!(rig.btn(K::BTN_RIGHT, 1, 0), keys(&[(K::BTN_LEFT, 1)]));
        assert!(rig.btn(K::BTN_RIGHT, 0, 10).is_empty());
    }

    #[test]
    fn sticky_second_click_releases() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_RIGHT, 1, 0);
        rig.btn(K::BTN_RIGHT, 0, 10);
        assert_eq!(rig.btn(K::BTN_RIGHT, 1, 500), keys(&[(K::BTN_LEFT, 0)]));
        assert!(rig.btn(K::BTN_RIGHT, 0, 510).is_empty());
    }

    #[test]
    fn momentary_layer_applies_only_while_held() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_BACK, 1, 0).is_empty());
        assert_eq!(rig.engine.layer(), 1);
        assert_eq!(rig.btn(K::BTN_EXTRA, 1, 10), keys(&[(K::KEY_F19, 1)]));
        assert_eq!(rig.btn(K::BTN_EXTRA, 0, 20), keys(&[(K::KEY_F19, 0)]));
        assert!(rig.btn(K::BTN_BACK, 0, 30).is_empty());
        assert_eq!(rig.engine.layer(), 0);
        assert_eq!(
            rig.btn(K::BTN_EXTRA, 1, 40),
            keys(&[(K::KEY_LEFTCTRL, 1), (K::KEY_T, 1)])
        );
    }

    #[test]
    fn output_held_across_layer_change_is_still_released() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_BACK, 1, 0);
        rig.btn(K::BTN_EXTRA, 1, 10);
        rig.btn(K::BTN_BACK, 0, 20);
        assert_eq!(rig.btn(K::BTN_EXTRA, 0, 30), keys(&[(K::KEY_F19, 0)]));
    }

    #[test]
    fn toggle_layer_switches_on_and_off() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_FORWARD, 1, 0);
        rig.btn(K::BTN_FORWARD, 0, 10);
        assert_eq!(rig.engine.layer(), 1);
        assert_eq!(rig.btn(K::BTN_EXTRA, 1, 20), keys(&[(K::KEY_F19, 1)]));
        rig.btn(K::BTN_EXTRA, 0, 30);
        rig.btn(K::BTN_FORWARD, 1, 40);
        rig.btn(K::BTN_FORWARD, 0, 50);
        assert_eq!(rig.engine.layer(), 0);
    }

    #[test]
    fn run_and_scroll_fire_on_press_only() {
        let mut rig = Rig::new();
        assert_eq!(rig.btn(K::BTN_0, 1, 0), vec![Output::Run("echo hi".into())]);
        assert!(rig.btn(K::BTN_0, 0, 10).is_empty());
        assert_eq!(
            rig.btn(K::BTN_1, 1, 20),
            vec![Output::Scroll(WheelDir::Down)]
        );
        assert!(rig.btn(K::BTN_1, 0, 30).is_empty());
    }

    #[test]
    fn wheel_action_taps_once_per_notch_and_unmapped_forwards() {
        let rig = Rig::new();
        let mut engine = Engine::new();
        assert!(
            engine
                .wheel_action(&rig.cfg, &rig.focus, WheelDir::Down)
                .is_none()
        );
        let action = engine
            .wheel_action(&rig.cfg, &rig.focus, WheelDir::Up)
            .unwrap()
            .clone();
        assert_eq!(
            engine.on_wheel(&action, 2, &rig.focus, rig.at(0)),
            keys(&[
                (K::KEY_F18, 1),
                (K::KEY_F18, 0),
                (K::KEY_F18, 1),
                (K::KEY_F18, 0)
            ])
        );
    }

    /// Runs every due deadline up to `end`, as the event loop would.
    fn play_until(rig: &mut Rig, end: u64) -> Vec<(u64, Output)> {
        let mut seen = Vec::new();
        while let Some(deadline) = rig.engine.next_deadline() {
            let ms = (deadline - rig.start).as_millis() as u64;
            if ms > end {
                break;
            }
            seen.extend(rig.tick(ms).into_iter().map(|output| (ms, output)));
        }
        seen
    }

    #[test]
    fn macro_plays_on_schedule_and_then_ends() {
        let mut rig = Rig::new();
        // The first step does not wait for the event loop.
        assert_eq!(rig.btn(K::BTN_4, 1, 0), keys(&[(K::KEY_F20, 1)]));
        assert!(rig.btn(K::BTN_4, 0, 5).is_empty());
        assert_eq!(rig.engine.next_deadline(), Some(rig.at(20)));
        assert_eq!(
            play_until(&mut rig, 1000),
            [
                (20, Output::Key(K::KEY_F20, 0)),
                (100, Output::Key(K::BTN_LEFT, 1)),
                (120, Output::Key(K::BTN_LEFT, 0)),
            ]
        );
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn late_wakeup_catches_up_without_shifting_later_steps() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_4, 1, 0);
        assert_eq!(rig.tick(110), keys(&[(K::KEY_F20, 0), (K::BTN_LEFT, 1)]));
        assert_eq!(rig.engine.next_deadline(), Some(rig.at(120)));
    }

    #[test]
    fn pressing_a_macro_again_stops_it_and_releases_what_it_held() {
        let mut rig = Rig::new();
        assert_eq!(rig.btn(K::BTN_5, 1, 0), keys(&[(K::KEY_F21, 1)]));
        rig.btn(K::BTN_5, 0, 5);
        assert_eq!(rig.btn(K::BTN_5, 1, 50), keys(&[(K::KEY_F21, 0)]));
        assert_eq!(rig.engine.next_deadline(), None);
        rig.btn(K::BTN_5, 0, 55);
        // A third press plays it from the start.
        assert_eq!(rig.btn(K::BTN_5, 1, 60), keys(&[(K::KEY_F21, 1)]));
    }

    #[test]
    fn a_different_macro_replaces_the_one_playing() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_5, 1, 0);
        assert_eq!(
            rig.btn(K::BTN_4, 1, 50),
            keys(&[(K::KEY_F21, 0), (K::KEY_F20, 1)])
        );
    }

    #[test]
    fn switching_windows_stops_a_macro_before_its_next_key() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_5, 1, 0);
        rig.focus.app_id = "firefox".into();
        assert_eq!(rig.tick(100), keys(&[(K::KEY_F21, 0)]));
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn a_key_left_down_by_a_macro_is_released_when_it_ends() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_5, 1, 0);
        let seen: Vec<Output> = play_until(&mut rig, 1000)
            .into_iter()
            .map(|(_, output)| output)
            .collect();
        assert_eq!(
            seen,
            keys(&[(K::KEY_F22, 1), (K::KEY_F22, 0), (K::KEY_F21, 0)])
        );
    }

    #[test]
    fn a_macro_release_leaves_a_button_the_user_holds_alone() {
        let cfg = Config::parse(
            "[device]\nname='m'\n[default]\nside='macro:{LMB}{WAITMS:100}{LMBU}{WAITMS:100}'\n",
        )
        .unwrap();
        let mut engine = Engine::new();
        let focus = Focus::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        assert_eq!(
            engine.on_button(&cfg, &focus, K::BTN_LEFT, 1, at(0)),
            keys(&[(K::BTN_LEFT, 1)])
        );
        assert!(
            engine
                .on_button(&cfg, &focus, K::BTN_SIDE, 1, at(1))
                .is_empty()
        );
        // Neither the macro's own click nor its stray {LMBU} lets go of it.
        assert!(engine.tick(&cfg, &focus, at(300)).is_empty());
        assert_eq!(
            engine.on_button(&cfg, &focus, K::BTN_LEFT, 0, at(310)),
            keys(&[(K::BTN_LEFT, 0)])
        );
    }

    #[test]
    fn macro_can_be_the_tap_of_a_tap_and_hold() {
        let mut rig = Rig::new();
        assert!(rig.btn(K::BTN_6, 1, 0).is_empty());
        assert_eq!(rig.btn(K::BTN_6, 0, 50), keys(&[(K::KEY_F20, 1)]));
        assert_eq!(rig.tick(70), keys(&[(K::KEY_F20, 0)]));
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn hold_macro_stops_on_release_and_releases_what_it_held() {
        let mut rig = Rig::new();
        assert_eq!(rig.btn(K::BTN_7, 1, 0), keys(&[(K::KEY_F21, 1)]));
        assert_eq!(rig.btn(K::BTN_7, 0, 50), keys(&[(K::KEY_F21, 0)]));
        assert_eq!(rig.engine.next_deadline(), None);
        assert!(rig.tick(200).is_empty());
    }

    #[test]
    fn hold_macro_plays_once_even_if_still_held() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_7, 1, 0);
        let seen = play_until(&mut rig, 5000);
        assert_eq!(seen.last(), Some(&(120, Output::Key(K::KEY_F21, 0))));
        assert_eq!(rig.engine.next_deadline(), None);
        assert!(rig.btn(K::BTN_7, 0, 6000).is_empty());
    }

    #[test]
    fn repeat_macro_loops_without_drift_until_release() {
        let mut rig = Rig::new();
        assert_eq!(rig.btn(K::BTN_8, 1, 0), keys(&[(K::KEY_F20, 1)]));
        assert_eq!(
            play_until(&mut rig, 220),
            [
                (20, Output::Key(K::KEY_F20, 0)),
                (100, Output::Key(K::KEY_F20, 1)),
                (120, Output::Key(K::KEY_F20, 0)),
                (200, Output::Key(K::KEY_F20, 1)),
                (220, Output::Key(K::KEY_F20, 0)),
            ]
        );
        assert_eq!(rig.btn(K::BTN_8, 0, 250), []);
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn switching_windows_stops_a_repeating_macro() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_8, 1, 0);
        rig.tick(20);
        rig.focus.app_id = "firefox".into();
        assert!(rig.tick(100).is_empty());
        assert_eq!(rig.engine.next_deadline(), None);
        assert!(rig.btn(K::BTN_8, 0, 150).is_empty());
    }

    #[test]
    fn releasing_a_button_leaves_a_newer_macro_playing() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_7, 1, 0);
        rig.btn(K::BTN_8, 1, 10);
        rig.btn(K::BTN_7, 0, 20);
        assert!(rig.engine.next_deadline().is_some());
        rig.btn(K::BTN_8, 0, 30);
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn reset_stops_a_macro_and_releases_what_it_held() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_5, 1, 0);
        assert_eq!(rig.engine.reset(), keys(&[(K::KEY_F21, 0)]));
        assert_eq!(rig.engine.next_deadline(), None);
    }

    #[test]
    fn reset_releases_everything_and_forgets_physical_state() {
        let mut rig = Rig::new();
        rig.btn(K::BTN_SIDE, 1, 0);
        rig.btn(K::BTN_RIGHT, 1, 1);
        rig.btn(K::BTN_BACK, 1, 2);
        rig.btn(K::BTN_MIDDLE, 1, 3);

        let mut released = rig.engine.reset();
        released.sort_by_key(|o| match o {
            Output::Key(code, _) => code.code(),
            _ => 0,
        });
        assert_eq!(
            released,
            keys(&[(K::KEY_W, 0), (K::KEY_LEFTCTRL, 0), (K::BTN_LEFT, 0)])
        );
        assert_eq!(rig.engine.layer(), 0);
        assert_eq!(rig.engine.next_deadline(), None);
        // The stale release from the replugged mouse does nothing.
        assert!(rig.btn(K::BTN_SIDE, 0, 10).is_empty());
        // And the button works again.
        assert_eq!(rig.btn(K::BTN_SIDE, 1, 20).len(), 2);
    }
}
