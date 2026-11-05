//! Parses the TOML configuration and resolves what an input does.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use evdev::KeyCode;
use serde::Deserialize;

use crate::macros::Macro;

/// The buttons the virtual mouse declares: BTN_LEFT through BTN_TASK.
pub const VIRTUAL_MOUSE_BUTTONS: RangeInclusive<u16> = 0x110..=0x117;

const MAX_LAYER: u8 = 9;
const DEFAULT_COMBO_MS: u64 = 50;
const DEFAULT_HOLD_MS: u64 = 300;
const MAX_TIMING_MS: u64 = 10_000;

/// Where the daemon and the settings app look for the config by default.
pub fn default_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(base.join("clickshift/config.toml"))
}

/// What the focused window is, as reported by the GNOME Shell extension.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focus {
    pub app_id: String,
    pub wm_class: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelDir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Input {
    Button(KeyCode),
    Wheel(WheelDir),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send the physical input unchanged.
    Passthrough,
    /// Swallow the input.
    Disabled,
    /// Hold these codes for as long as the physical button is held.
    Keys(Vec<KeyCode>),
    /// First click presses these codes, the next click releases them.
    Sticky(Vec<KeyCode>),
    /// Run a shell command on press.
    Run(String),
    /// Emit one wheel notch on press.
    Scroll(WheelDir),
    /// Activate a layer while the button is held.
    Layer(u8),
    /// Switch to a layer, or back to the base layer if already on it.
    LayerToggle(u8),
    /// One action for a quick click and another for a long press.
    Timed(Box<Timed>),
    /// Play a timed sequence.
    Macro(Arc<Macro>, MacroMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroMode {
    /// Press to start, press again to stop.
    Toggle,
    /// Plays once while the button is held; letting go stops it.
    WhileHeld,
    /// Plays over and over while the button is held; letting go stops it.
    RepeatWhileHeld,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timed {
    pub tap: Action,
    pub hold: Action,
    pub hold_ms: u64,
}

#[derive(Debug, Default)]
struct Profile {
    actions: HashMap<Input, Action>,
    combos: HashMap<(KeyCode, KeyCode), Action>,
}

#[derive(Debug, Default)]
struct Layer {
    default: Profile,
    apps: HashMap<String, Profile>,
}

#[derive(Debug)]
pub struct Config {
    pub device_name: String,
    pub combo_ms: u64,
    /// Index 0 is the base layer, always present.
    layers: HashMap<u8, Layer>,
}

type RawProfile = HashMap<String, RawAction>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    device: RawDevice,
    #[serde(default)]
    default: RawProfile,
    #[serde(default)]
    apps: HashMap<String, RawProfile>,
    #[serde(default)]
    layers: HashMap<String, RawLayer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDevice {
    name: String,
    #[serde(default)]
    allow_left_remap: bool,
    #[serde(default = "default_combo_ms")]
    combo_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayer {
    #[serde(default)]
    default: RawProfile,
    #[serde(default)]
    apps: HashMap<String, RawProfile>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAction {
    Simple(String),
    Timed(RawTimed),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTimed {
    tap: String,
    hold: String,
    #[serde(default = "default_hold_ms")]
    hold_ms: u64,
}

fn default_combo_ms() -> u64 {
    DEFAULT_COMBO_MS
}

fn default_hold_ms() -> u64 {
    DEFAULT_HOLD_MS
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(text)?;
        let allow_left = raw.device.allow_left_remap;
        if raw.device.combo_ms == 0 || raw.device.combo_ms > MAX_TIMING_MS {
            bail!("combo_ms must be between 1 and {MAX_TIMING_MS}");
        }

        let mut layers = HashMap::new();
        layers.insert(
            0,
            parse_layer(&raw.default, &raw.apps, allow_left).context("in the base layer")?,
        );
        for (name, layer) in &raw.layers {
            let number = parse_layer_number(name).with_context(|| format!("[layers.{name}]"))?;
            layers.insert(
                number,
                parse_layer(&layer.default, &layer.apps, allow_left)
                    .with_context(|| format!("in [layers.{name}]"))?,
            );
        }

        let config = Self {
            device_name: raw.device.name.trim().to_owned(),
            combo_ms: raw.device.combo_ms,
            layers,
        };
        config.check_layer_references()?;
        Ok(config)
    }

    /// Looks through the layer first, then the base layer. In each, the app
    /// profile wins over the default profile.
    pub fn resolve(&self, input: Input, focus: &Focus, layer: u8) -> &Action {
        static PASSTHROUGH: Action = Action::Passthrough;
        self.profiles(focus, layer)
            .find_map(|profile| profile.actions.get(&input))
            .unwrap_or(&PASSTHROUGH)
    }

    pub fn combo(&self, a: KeyCode, b: KeyCode, focus: &Focus, layer: u8) -> Option<&Action> {
        let pair = ordered(a, b);
        self.profiles(focus, layer)
            .find_map(|profile| profile.combos.get(&pair))
    }

    pub fn in_combo(&self, button: KeyCode, focus: &Focus, layer: u8) -> bool {
        self.profiles(focus, layer).any(|profile| {
            profile
                .combos
                .keys()
                .any(|(a, b)| *a == button || *b == button)
        })
    }

    fn profiles<'a>(&'a self, focus: &Focus, layer: u8) -> impl Iterator<Item = &'a Profile> {
        let upper = (layer != 0)
            .then(|| self.layers.get(&layer))
            .flatten()
            .into_iter();
        let base = self.layers.get(&0).into_iter();
        upper.chain(base).flat_map(move |layer| {
            let app = [&focus.app_id, &focus.wm_class]
                .into_iter()
                .filter(|name| !name.is_empty())
                .find_map(|name| layer.apps.get(&normalize_app(name)));
            app.into_iter().chain(std::iter::once(&layer.default))
        })
    }

    pub fn summary(&self) -> String {
        let (mut mappings, mut apps) = (0, 0);
        for layer in self.layers.values() {
            mappings += layer.default.actions.len() + layer.default.combos.len();
            apps += layer.apps.len();
        }
        format!(
            "device \"{}\", {} layer(s), {} default mapping(s), {} app profile(s)",
            self.device_name,
            self.layers.len(),
            mappings,
            apps
        )
    }

    fn check_layer_references(&self) -> Result<()> {
        for layer in self.layers.values() {
            let profiles = layer.apps.values().chain(std::iter::once(&layer.default));
            for profile in profiles {
                let actions = profile.actions.values().chain(profile.combos.values());
                for action in actions {
                    self.check_action_layers(action)?;
                }
            }
        }
        Ok(())
    }

    fn check_action_layers(&self, action: &Action) -> Result<()> {
        match action {
            Action::Layer(n) | Action::LayerToggle(n) if !self.layers.contains_key(n) => {
                bail!("layer {n} is used but [layers.{n}] is not defined")
            }
            Action::Timed(timed) => {
                self.check_action_layers(&timed.tap)?;
                self.check_action_layers(&timed.hold)
            }
            _ => Ok(()),
        }
    }
}

/// Any mouse button code, including ones the virtual mouse does not declare.
pub fn is_mouse_button(code: KeyCode) -> bool {
    (KeyCode::BTN_0.code()..KeyCode::KEY_OK.code()).contains(&code.code())
}

/// Every code the virtual keyboard declares. Declaring them all up front means
/// a reloaded config can use new keys without recreating the device.
pub fn is_keyboard_key(code: KeyCode) -> bool {
    let code = code.code();
    let dpad = KeyCode::BTN_DPAD_UP.code()..=KeyCode::BTN_DPAD_RIGHT.code();
    let trigger_happy = KeyCode::BTN_TRIGGER_HAPPY1.code();
    (code >= 1 && code < KeyCode::BTN_0.code())
        || (code >= KeyCode::KEY_OK.code() && code < trigger_happy && !dpad.contains(&code))
}

fn ordered(a: KeyCode, b: KeyCode) -> (KeyCode, KeyCode) {
    if a.code() <= b.code() { (a, b) } else { (b, a) }
}

fn normalize_app(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".desktop").unwrap_or(&lower).to_owned()
}

fn parse_layer_number(name: &str) -> Result<u8> {
    match name.parse::<u8>() {
        Ok(number) if (1..=MAX_LAYER).contains(&number) => Ok(number),
        _ => bail!("layer must be a number from 1 to {MAX_LAYER}"),
    }
}

fn parse_layer(
    default: &RawProfile,
    apps: &HashMap<String, RawProfile>,
    allow_left: bool,
) -> Result<Layer> {
    let default = parse_profile(default, allow_left).context("in the default profile")?;
    let mut parsed = HashMap::new();
    for (app, raw) in apps {
        let profile =
            parse_profile(raw, allow_left).with_context(|| format!("in app \"{app}\""))?;
        parsed.insert(normalize_app(app), profile);
    }
    Ok(Layer {
        default,
        apps: parsed,
    })
}

fn parse_profile(raw: &RawProfile, allow_left: bool) -> Result<Profile> {
    let mut profile = Profile::default();
    for (name, raw_action) in raw {
        let action =
            parse_raw_action(raw_action).with_context(|| format!("action for \"{name}\""))?;
        if let Some((first, second)) = name.split_once('+') {
            let pair = parse_combo(first, second, allow_left)?;
            if matches!(action, Action::Timed(_)) {
                bail!("\"{name}\": a button combo cannot use tap and hold");
            }
            if profile.combos.insert(pair, action).is_some() {
                bail!("combo \"{name}\" is mapped twice");
            }
            continue;
        }

        let input = parse_input(name)?;
        check_allowed(input, &action, name, allow_left)?;
        if profile.actions.insert(input, action).is_some() {
            bail!("\"{name}\" is mapped twice");
        }
    }
    Ok(profile)
}

fn parse_combo(first: &str, second: &str, allow_left: bool) -> Result<(KeyCode, KeyCode)> {
    let a = parse_button(first)?;
    let b = parse_button(second)?;
    if a == b {
        bail!("a combo needs two different buttons");
    }
    if !allow_left && (a == KeyCode::BTN_LEFT || b == KeyCode::BTN_LEFT) {
        bail!("a combo with the left button needs allow_left_remap = true in [device]");
    }
    Ok(ordered(a, b))
}

fn check_allowed(input: Input, action: &Action, name: &str, allow_left: bool) -> Result<()> {
    // Losing left click makes it hard to stop the program with the mouse.
    if input == Input::Button(KeyCode::BTN_LEFT) && *action != Action::Passthrough && !allow_left {
        bail!("remapping the left button needs allow_left_remap = true in [device]");
    }
    // A wheel notch has no release, so anything held would never end.
    if matches!(input, Input::Wheel(_))
        && matches!(
            action,
            Action::Sticky(_) | Action::Layer(_) | Action::Timed(_)
        )
    {
        bail!("\"{name}\": wheel directions cannot use sticky, layer or tap and hold");
    }
    // One flick of the wheel is many notches, which would start and stop it at random.
    if matches!(input, Input::Wheel(_)) && matches!(action, Action::Macro(..)) {
        bail!("\"{name}\": wheel directions cannot use a macro");
    }
    Ok(())
}

fn parse_input(name: &str) -> Result<Input> {
    let lower = name.trim().to_lowercase();
    let input = match lower.as_str() {
        "wheel_up" => Input::Wheel(WheelDir::Up),
        "wheel_down" => Input::Wheel(WheelDir::Down),
        "wheel_left" => Input::Wheel(WheelDir::Left),
        "wheel_right" => Input::Wheel(WheelDir::Right),
        _ => Input::Button(parse_button(name)?),
    };
    Ok(input)
}

fn parse_button(name: &str) -> Result<KeyCode> {
    let lower = name.trim().to_lowercase();
    let code = match lower.as_str() {
        "left" => KeyCode::BTN_LEFT,
        "right" => KeyCode::BTN_RIGHT,
        "middle" => KeyCode::BTN_MIDDLE,
        "side" => KeyCode::BTN_SIDE,
        "extra" => KeyCode::BTN_EXTRA,
        "forward" => KeyCode::BTN_FORWARD,
        "back" => KeyCode::BTN_BACK,
        "task" => KeyCode::BTN_TASK,
        _ => {
            let code = KeyCode::from_str(&lower.to_uppercase())
                .map_err(|_| anyhow!("unknown mouse button \"{name}\""))?;
            if !is_mouse_button(code) {
                bail!("\"{name}\" is not a mouse button");
            }
            code
        }
    };
    Ok(code)
}

fn parse_raw_action(raw: &RawAction) -> Result<Action> {
    match raw {
        RawAction::Simple(text) => parse_action(text),
        RawAction::Timed(timed) => {
            if timed.hold_ms == 0 || timed.hold_ms > MAX_TIMING_MS {
                bail!("hold_ms must be between 1 and {MAX_TIMING_MS}");
            }
            let tap = parse_action(&timed.tap).context("in tap")?;
            // A tap has already been released when it fires, so it would stop at once.
            if matches!(tap, Action::Macro(_, mode) if mode != MacroMode::Toggle) {
                bail!("tap cannot use macro-hold or macro-repeat; use macro:");
            }
            Ok(Action::Timed(Box::new(Timed {
                tap,
                hold: parse_action(&timed.hold).context("in hold")?,
                hold_ms: timed.hold_ms,
            })))
        }
    }
}

fn parse_action(text: &str) -> Result<Action> {
    let trimmed = text.trim();
    match trimmed.to_lowercase().as_str() {
        "passthrough" => return Ok(Action::Passthrough),
        "disabled" | "none" => return Ok(Action::Disabled),
        _ => {}
    }

    if let Some((kind, argument)) = trimmed.split_once(':') {
        let argument = argument.trim();
        return match kind.trim().to_lowercase().as_str() {
            "run" if argument.is_empty() => bail!("run: needs a command"),
            "run" => Ok(Action::Run(argument.to_owned())),
            "layer" => Ok(Action::Layer(parse_layer_number(argument)?)),
            "layer-toggle" => Ok(Action::LayerToggle(parse_layer_number(argument)?)),
            "sticky" => Ok(Action::Sticky(parse_chord(argument)?)),
            "scroll" => Ok(Action::Scroll(parse_scroll(argument)?)),
            "macro" => parse_macro(argument, MacroMode::Toggle),
            "macro-hold" => parse_macro(argument, MacroMode::WhileHeld),
            "macro-repeat" => parse_macro(argument, MacroMode::RepeatWhileHeld),
            other => bail!("unknown action type \"{other}\""),
        };
    }
    Ok(Action::Keys(parse_chord(trimmed)?))
}

fn parse_macro(text: &str, mode: MacroMode) -> Result<Action> {
    let recipe = Macro::parse(text)?;
    // With no length, repeating would never give the event loop a turn.
    if mode == MacroMode::RepeatWhileHeld && recipe.total_ms == 0 {
        bail!("a repeating macro needs a {{WAITMS:...}} or a key tap");
    }
    Ok(Action::Macro(Arc::new(recipe), mode))
}

fn parse_scroll(direction: &str) -> Result<WheelDir> {
    match direction.to_lowercase().as_str() {
        "up" => Ok(WheelDir::Up),
        "down" => Ok(WheelDir::Down),
        "left" => Ok(WheelDir::Left),
        "right" => Ok(WheelDir::Right),
        _ => bail!("scroll direction must be up, down, left or right"),
    }
}

pub(crate) fn parse_chord(text: &str) -> Result<Vec<KeyCode>> {
    let mut codes = Vec::new();
    for token in text.split('+') {
        let code = parse_key(token)?;
        if codes.contains(&code) {
            bail!("\"{}\" appears twice in \"{text}\"", token.trim());
        }
        codes.push(code);
    }
    Ok(codes)
}

pub(crate) fn parse_key(token: &str) -> Result<KeyCode> {
    let name = token.trim();
    let upper = name.to_uppercase();
    if upper.is_empty() {
        bail!("empty key name");
    }
    let code = match upper.as_str() {
        "CTRL" | "CONTROL" => KeyCode::KEY_LEFTCTRL,
        "SHIFT" => KeyCode::KEY_LEFTSHIFT,
        "ALT" => KeyCode::KEY_LEFTALT,
        "ALTGR" => KeyCode::KEY_RIGHTALT,
        "SUPER" | "META" | "WIN" => KeyCode::KEY_LEFTMETA,
        "ESCAPE" => KeyCode::KEY_ESC,
        "RETURN" => KeyCode::KEY_ENTER,
        "DEL" => KeyCode::KEY_DELETE,
        "PGUP" => KeyCode::KEY_PAGEUP,
        "PGDN" => KeyCode::KEY_PAGEDOWN,
        "MOUSE_LEFT" => KeyCode::BTN_LEFT,
        "MOUSE_RIGHT" => KeyCode::BTN_RIGHT,
        "MOUSE_MIDDLE" => KeyCode::BTN_MIDDLE,
        "MOUSE_BACK" => KeyCode::BTN_SIDE,
        "MOUSE_FORWARD" => KeyCode::BTN_EXTRA,
        _ if upper.starts_with("KEY_") || upper.starts_with("BTN_") => {
            KeyCode::from_str(&upper).map_err(|_| anyhow!("unknown key \"{name}\""))?
        }
        _ => KeyCode::from_str(&format!("KEY_{upper}"))
            .map_err(|_| anyhow!("unknown key \"{name}\""))?,
    };
    if !VIRTUAL_MOUSE_BUTTONS.contains(&code.code()) && !is_keyboard_key(code) {
        bail!("\"{name}\" cannot be sent by the virtual devices");
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(app_id: &str, wm_class: &str) -> Focus {
        Focus {
            app_id: app_id.into(),
            wm_class: wm_class.into(),
        }
    }

    fn side() -> Input {
        Input::Button(KeyCode::BTN_SIDE)
    }

    fn extra() -> Input {
        Input::Button(KeyCode::BTN_EXTRA)
    }

    fn body(text: &str) -> String {
        format!("[device]\nname='m'\n{text}\n")
    }

    const SAMPLE: &str = r#"
        [device]
        name = "  Test Mouse "

        [default]
        side = "Alt+Left"
        extra = "disabled"

        [apps.firefox]
        side = "Ctrl+W"

        [apps."org.gnome.Nautilus"]
        extra = "passthrough"

        [layers.1.default]
        extra = "F13"

        [layers.1.apps.firefox]
        side = "F14"
    "#;

    #[test]
    fn device_name_is_trimmed() {
        assert_eq!(Config::parse(SAMPLE).unwrap().device_name, "Test Mouse");
    }

    #[test]
    fn app_profile_overrides_default() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(
            cfg.resolve(side(), &focus("firefox.desktop", ""), 0),
            &Action::Keys(vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_W])
        );
    }

    #[test]
    fn app_matching_uses_wm_class_and_ignores_case() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(
            cfg.resolve(extra(), &focus("", "ORG.GNOME.NAUTILUS"), 0),
            &Action::Passthrough
        );
    }

    #[test]
    fn app_profile_inherits_unlisted_buttons_from_default() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(
            cfg.resolve(extra(), &focus("firefox", ""), 0),
            &Action::Disabled
        );
    }

    #[test]
    fn unknown_app_uses_default_and_unmapped_button_passes_through() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let other = focus("gedit", "gedit");
        assert_eq!(
            cfg.resolve(side(), &other, 0),
            &Action::Keys(vec![KeyCode::KEY_LEFTALT, KeyCode::KEY_LEFT])
        );
        assert_eq!(
            cfg.resolve(Input::Button(KeyCode::BTN_MIDDLE), &other, 0),
            &Action::Passthrough
        );
    }

    #[test]
    fn layer_wins_then_falls_back_to_base_layer() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let firefox = focus("firefox", "");
        assert_eq!(
            cfg.resolve(side(), &firefox, 1),
            &Action::Keys(vec![KeyCode::KEY_F14])
        );
        assert_eq!(
            cfg.resolve(extra(), &firefox, 1),
            &Action::Keys(vec![KeyCode::KEY_F13])
        );
        assert_eq!(
            cfg.resolve(side(), &focus("gedit", ""), 1),
            &Action::Keys(vec![KeyCode::KEY_LEFTALT, KeyCode::KEY_LEFT])
        );
    }

    #[test]
    fn every_action_type_parses() {
        let cfg = Config::parse(&body(
            r#"
            [default]
            side = "run:notify-send 'Hi There'"
            extra = "sticky:mouse_left"
            middle = "scroll:down"
            right = { tap = "F13", hold = "layer:2", hold_ms = 400 }
            wheel_up = "VolumeUp"
            "side+extra" = "PlayPause"

            [layers.2.default]
            side = "layer-toggle:2"
            "#,
        ))
        .unwrap();
        let none = Focus::default();
        assert_eq!(
            cfg.resolve(side(), &none, 0),
            &Action::Run("notify-send 'Hi There'".into())
        );
        assert_eq!(
            cfg.resolve(extra(), &none, 0),
            &Action::Sticky(vec![KeyCode::BTN_LEFT])
        );
        assert_eq!(
            cfg.resolve(Input::Button(KeyCode::BTN_MIDDLE), &none, 0),
            &Action::Scroll(WheelDir::Down)
        );
        assert_eq!(
            cfg.resolve(Input::Button(KeyCode::BTN_RIGHT), &none, 0),
            &Action::Timed(Box::new(Timed {
                tap: Action::Keys(vec![KeyCode::KEY_F13]),
                hold: Action::Layer(2),
                hold_ms: 400,
            }))
        );
        assert_eq!(
            cfg.resolve(Input::Wheel(WheelDir::Up), &none, 0),
            &Action::Keys(vec![KeyCode::KEY_VOLUMEUP])
        );
        assert_eq!(
            cfg.combo(KeyCode::BTN_EXTRA, KeyCode::BTN_SIDE, &none, 0),
            Some(&Action::Keys(vec![KeyCode::KEY_PLAYPAUSE]))
        );
        assert!(cfg.in_combo(KeyCode::BTN_SIDE, &none, 0));
        assert!(!cfg.in_combo(KeyCode::BTN_RIGHT, &none, 0));
    }

    #[test]
    fn left_remap_needs_explicit_opt_in() {
        assert!(Config::parse(&body("[default]\nleft='Ctrl+A'")).is_err());
        assert!(Config::parse(&body("[default]\n'left+side'='F13'")).is_err());
        assert!(Config::parse(&body("[default]\nleft='passthrough'")).is_ok());
        let allowed = "[device]\nname='m'\nallow_left_remap=true\n[default]\nleft='Ctrl+A'\n";
        assert!(Config::parse(allowed).is_ok());
    }

    #[test]
    fn invalid_values_are_rejected() {
        for text in [
            "[default]\nside='Ctrl+Nonsense'",
            "[default]\nside='Ctrl+'",
            "[default]\nside='Ctrl+Ctrl'",
            "[default]\nside=''",
            "[default]\nkeyboard='Ctrl+A'",
            "[default]\nKEY_A='Ctrl+A'",
            "[default]\nside='BTN_SOUTH'",
            "[default]\nside='run:'",
            "[default]\nside='teleport:home'",
            "[default]\nside='scroll:sideways'",
            "[default]\nside='layer:1'",
            "[default]\nside='layer:0'",
            "[default]\nside='layer:10'",
            "[default]\n'side+side'='F13'",
            "[default]\n'side+extra'={tap='F13',hold='F14'}",
            "[default]\nwheel_up='sticky:F13'",
            "[default]\nwheel_up='macro:q'",
            "[default]\nside='macro:{NOSUCHKEY}'",
            "[default]\nside='macro:'",
            "[default]\nwheel_up='macro-hold:q'",
            "[default]\nside='macro-repeat:{MWUP}'",
            "[default]\nside={tap='macro-hold:q',hold='F14'}",
            "[default]\nside={tap='macro-repeat:q',hold='F14'}",
            "[default]\nside={tap='F13',hold='F14',hold_ms=0}",
            "[layers.0.default]\nside='F13'",
            "[layers.10.default]\nside='F13'",
        ] {
            assert!(Config::parse(&body(text)).is_err(), "should reject: {text}");
        }
    }

    #[test]
    fn key_ranges_match_what_the_virtual_devices_declare() {
        assert!(is_mouse_button(KeyCode::BTN_LEFT));
        assert!(is_mouse_button(KeyCode::BTN_TASK));
        assert!(!is_mouse_button(KeyCode::KEY_A));
        assert!(!is_mouse_button(KeyCode::KEY_OK));
        assert!(is_keyboard_key(KeyCode::KEY_A));
        assert!(is_keyboard_key(KeyCode::KEY_OK));
        assert!(is_keyboard_key(KeyCode::KEY_F24));
        assert!(!is_keyboard_key(KeyCode::BTN_LEFT));
        assert!(!is_keyboard_key(KeyCode::BTN_DPAD_UP));
        assert!(!is_keyboard_key(KeyCode::KEY_RESERVED));
    }
}
