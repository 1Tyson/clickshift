//! Edits the config file for the settings app, keeping the user's comments and
//! layout. Every save is validated by the same parser the daemon uses.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use evdev::KeyCode;
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value, value};

use crate::config::{Config, MacroMode, is_keyboard_key};

const DEFAULT_HOLD_MS: i64 = 300;

/// Which profile is being edited.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slot {
    /// 0 is the base layer.
    pub layer: u8,
    /// `None` is the profile used for every app.
    pub app: Option<String>,
}

impl Slot {
    fn path(&self) -> Vec<String> {
        let mut path = Vec::new();
        if self.layer != 0 {
            path.push("layers".to_owned());
            path.push(self.layer.to_string());
        }
        match &self.app {
            Some(app) => {
                path.push("apps".to_owned());
                path.push(app.clone());
            }
            None => path.push("default".to_owned()),
        }
        path
    }
}

/// An action as the settings app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Unset,
    Passthrough,
    Disabled,
    Keys(String),
    MouseButton(String),
    Run(String),
    Sticky(String),
    Scroll(String),
    Macro(String, MacroMode),
    LayerHold(u8),
    LayerToggle(u8),
    TapHold {
        tap: String,
        hold: String,
        hold_ms: i64,
    },
}

impl Form {
    fn from_text(text: &str) -> Self {
        let trimmed = text.trim();
        let lower = trimmed.to_lowercase();
        match lower.as_str() {
            "passthrough" => return Self::Passthrough,
            "disabled" | "none" => return Self::Disabled,
            _ => {}
        }
        if let Some((kind, argument)) = trimmed.split_once(':') {
            let argument = argument.trim().to_owned();
            match kind.trim().to_lowercase().as_str() {
                "run" => return Self::Run(argument),
                "sticky" => return Self::Sticky(argument),
                "scroll" => return Self::Scroll(argument),
                "macro" => return Self::Macro(argument, MacroMode::Toggle),
                "macro-hold" => return Self::Macro(argument, MacroMode::WhileHeld),
                "macro-repeat" => return Self::Macro(argument, MacroMode::RepeatWhileHeld),
                "layer" => {
                    if let Ok(layer) = argument.parse() {
                        return Self::LayerHold(layer);
                    }
                }
                "layer-toggle" => {
                    if let Ok(layer) = argument.parse() {
                        return Self::LayerToggle(layer);
                    }
                }
                _ => {}
            }
        }
        if lower.starts_with("mouse_") && !lower.contains('+') {
            return Self::MouseButton(lower);
        }
        Self::Keys(trimmed.to_owned())
    }

    fn from_item(item: &Item) -> Self {
        if let Some(text) = item.as_str() {
            return Self::from_text(text);
        }
        let Some(table) = item.as_table_like() else {
            return Self::Unset;
        };
        let text = |key: &str| {
            table
                .get(key)
                .and_then(Item::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self::TapHold {
            tap: text("tap"),
            hold: text("hold"),
            hold_ms: table
                .get("hold_ms")
                .and_then(Item::as_integer)
                .unwrap_or(DEFAULT_HOLD_MS),
        }
    }

    fn to_item(&self) -> Option<Item> {
        let text = match self {
            Self::Unset => return None,
            Self::Passthrough => "passthrough".to_owned(),
            Self::Disabled => "disabled".to_owned(),
            Self::Keys(keys) | Self::MouseButton(keys) => keys.clone(),
            Self::Run(command) => format!("run:{command}"),
            Self::Sticky(keys) => format!("sticky:{keys}"),
            Self::Scroll(direction) => format!("scroll:{direction}"),
            Self::Macro(text, MacroMode::Toggle) => format!("macro:{text}"),
            Self::Macro(text, MacroMode::WhileHeld) => format!("macro-hold:{text}"),
            Self::Macro(text, MacroMode::RepeatWhileHeld) => format!("macro-repeat:{text}"),
            Self::LayerHold(layer) => format!("layer:{layer}"),
            Self::LayerToggle(layer) => format!("layer-toggle:{layer}"),
            Self::TapHold { tap, hold, hold_ms } => {
                let mut table = InlineTable::new();
                table.insert("tap", tap.as_str().into());
                table.insert("hold", hold.as_str().into());
                table.insert("hold_ms", (*hold_ms).into());
                return Some(Item::Value(Value::InlineTable(table)));
            }
        };
        Some(value(text))
    }

    fn referenced_layers(&self) -> Vec<u8> {
        match self {
            Self::LayerHold(layer) | Self::LayerToggle(layer) => vec![*layer],
            Self::TapHold { tap, hold, .. } => [tap, hold]
                .into_iter()
                .filter_map(|text| match Self::from_text(text) {
                    Self::LayerHold(layer) | Self::LayerToggle(layer) => Some(layer),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// The token the config uses for a key, such as "T" or "F13". `None` for codes
/// that cannot be sent or are not keys.
pub fn key_token(evdev_code: u16) -> Option<String> {
    let code = KeyCode::new(evdev_code);
    if !is_keyboard_key(code) {
        return None;
    }
    let name = format!("{code:?}");
    name.strip_prefix("KEY_").map(str::to_owned)
}

#[derive(Debug, Clone)]
pub struct Document {
    doc: DocumentMut,
}

impl Document {
    pub fn parse(text: &str) -> Result<Self> {
        let doc = text.parse::<DocumentMut>().context("reading TOML")?;
        Ok(Self { doc })
    }

    pub fn to_text(&self) -> String {
        self.doc.to_string()
    }

    pub fn validate(&self) -> Result<()> {
        Config::parse(&self.to_text()).map(|_| ())
    }

    fn device_table(&self) -> Option<&dyn TableLike> {
        self.doc.get("device")?.as_table_like()
    }

    pub fn device_name(&self) -> String {
        self.device_table()
            .and_then(|device| device.get("name"))
            .and_then(Item::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    pub fn allow_left_remap(&self) -> bool {
        self.device_table()
            .and_then(|device| device.get("allow_left_remap"))
            .and_then(Item::as_bool)
            .unwrap_or(false)
    }

    pub fn set_device_name(&mut self, name: &str) {
        let device = table_at(self.doc.as_table_mut(), &["device".to_owned()]);
        device.insert("name", value(name));
    }

    /// Every app with a profile in any layer, sorted.
    pub fn apps(&self) -> Vec<String> {
        let mut names = BTreeSet::new();
        let mut collect = |apps: Option<&Item>| {
            if let Some(table) = apps.and_then(Item::as_table_like) {
                names.extend(table.iter().map(|(name, _)| name.to_owned()));
            }
        };
        collect(self.doc.get("apps"));
        if let Some(layers) = self.doc.get("layers").and_then(Item::as_table_like) {
            for (_, layer) in layers.iter() {
                collect(layer.as_table_like().and_then(|layer| layer.get("apps")));
            }
        }
        names.into_iter().collect()
    }

    pub fn add_app(&mut self, app: &str) {
        let slot = Slot {
            layer: 0,
            app: Some(app.to_owned()),
        };
        table_at(self.doc.as_table_mut(), &slot.path());
    }

    /// Removes the app's profile from every layer.
    pub fn remove_app(&mut self, app: &str) {
        if let Some(apps) = self.doc.get_mut("apps").and_then(Item::as_table_like_mut) {
            apps.remove(app);
        }
        if let Some(layers) = self.doc.get_mut("layers").and_then(Item::as_table_like_mut) {
            for (_, layer) in layers.iter_mut() {
                let apps = layer
                    .as_table_like_mut()
                    .and_then(|layer| layer.get_mut("apps"))
                    .and_then(Item::as_table_like_mut);
                if let Some(apps) = apps {
                    apps.remove(app);
                }
            }
        }
    }

    fn profile(&self, slot: &Slot) -> Option<&dyn TableLike> {
        let mut path = slot.path().into_iter();
        let mut table = self.doc.get(&path.next()?)?.as_table_like()?;
        for key in path {
            table = table.get(&key)?.as_table_like()?;
        }
        Some(table)
    }

    pub fn form(&self, slot: &Slot, input: &str) -> Form {
        self.profile(slot)
            .and_then(|profile| profile.get(input))
            .map_or(Form::Unset, Form::from_item)
    }

    /// The combos in this profile, as written in the file.
    pub fn combos(&self, slot: &Slot) -> Vec<String> {
        let Some(profile) = self.profile(slot) else {
            return Vec::new();
        };
        let mut combos: Vec<String> = profile
            .iter()
            .map(|(key, _)| key.to_owned())
            .filter(|key| key.contains('+'))
            .collect();
        combos.sort();
        combos
    }

    pub fn set_form(&mut self, slot: &Slot, input: &str, form: &Form) {
        for layer in form.referenced_layers() {
            self.ensure_layer(layer);
        }
        match form.to_item() {
            Some(mut item) => {
                let table = table_at(self.doc.as_table_mut(), &slot.path());
                match table.get_mut(input) {
                    // Assigning in place keeps the key, and the comments above it.
                    Some(existing) => {
                        // A comment after the value belongs to the value.
                        if let (Some(old), Some(new)) = (existing.as_value(), item.as_value_mut()) {
                            *new.decor_mut() = old.decor().clone();
                        }
                        *existing = item;
                    }
                    None => {
                        table.insert(input, item);
                    }
                }
            }
            None => {
                if self.profile(slot).is_some() {
                    table_at(self.doc.as_table_mut(), &slot.path()).remove(input);
                }
            }
        }
    }

    fn ensure_layer(&mut self, layer: u8) {
        if layer == 0 {
            return;
        }
        let slot = Slot { layer, app: None };
        table_at(self.doc.as_table_mut(), &slot.path());
    }
}

/// Walks to a table, creating what is missing. Only the last one gets its own
/// header in the file; the ones above it stay implicit.
fn table_at<'a>(root: &'a mut Table, path: &[String]) -> &'a mut dyn TableLike {
    let mut current: &mut dyn TableLike = root;
    for (index, key) in path.iter().enumerate() {
        let is_last = index + 1 == path.len();
        let usable = current.get(key).is_some_and(Item::is_table_like);
        if !usable {
            let mut table = Table::new();
            table.set_implicit(!is_last);
            current.insert(key, Item::Table(table));
        }
        let item = current.get_mut(key).expect("table was just ensured");
        if is_last && let Some(table) = item.as_table_mut() {
            table.set_implicit(false);
        }
        current = item.as_table_like_mut().expect("table was just ensured");
    }
    current
}

#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Written,
    /// The file changed on disk, so it was loaded instead of being overwritten.
    /// The requested edit was not applied.
    ReloadedFromDisk,
}

/// A document tied to its file on disk.
pub struct Store {
    path: PathBuf,
    doc: Document,
    /// What the file held when last read or written.
    disk_text: String,
}

impl Store {
    /// A missing file starts as a minimal config and is created on first save.
    pub fn open(path: &Path, default_device: &str) -> Result<Self> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let disk_text = read_or_empty(&path)?;
        let doc = if disk_text.is_empty() {
            let mut doc = Document::parse("")?;
            doc.set_device_name(default_device);
            doc
        } else {
            Document::parse(&disk_text).with_context(|| format!("in {}", path.display()))?
        };
        Ok(Self {
            path,
            doc,
            disk_text,
        })
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Applies an edit and saves it, or changes nothing if the result is
    /// invalid. Never overwrites changes made outside the app.
    pub fn update(&mut self, edit: impl FnOnce(&mut Document)) -> Result<Saved> {
        let on_disk = read_or_empty(&self.path)?;
        if on_disk != self.disk_text {
            let doc = Document::parse(&on_disk)
                .map_err(|error| anyhow!("the file on disk is broken: {error:#}"))?;
            self.doc = doc;
            self.disk_text = on_disk;
            return Ok(Saved::ReloadedFromDisk);
        }

        let mut edited = self.doc.clone();
        edit(&mut edited);
        edited.validate()?;
        let text = edited.to_text();
        write_atomically(&self.path, &text)?;
        self.doc = edited;
        self.disk_text = text;
        Ok(Saved::Written)
    }
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// A crash mid-save can never leave the daemon reading a half-written file.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let Some(parent) = path.parent() else {
        bail!("{} has no parent folder", path.display());
    };
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())?;
        }
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("saving {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Focus, Input};

    const COMMENTED: &str = r#"# My mouse setup
[device]
name = "m" # the AJAZZ

# Everywhere
[default]
side = "Alt+Left" # back

[apps."org.gnome.Ptyxis"]
side = "Ctrl+Shift+T"
"#;

    fn base() -> Slot {
        Slot::default()
    }

    fn app(name: &str) -> Slot {
        Slot {
            layer: 0,
            app: Some(name.to_owned()),
        }
    }

    fn side() -> Input {
        Input::Button(KeyCode::BTN_SIDE)
    }

    #[test]
    fn edits_keep_comments_and_unrelated_lines() {
        let mut doc = Document::parse(COMMENTED).unwrap();
        doc.set_form(&base(), "extra", &Form::Keys("F13".into()));
        let text = doc.to_text();
        for kept in ["# My mouse setup", "# the AJAZZ", "# Everywhere", "# back"] {
            assert!(text.contains(kept), "lost {kept:?} in:\n{text}");
        }
        assert!(text.contains("extra = \"F13\""));
    }

    #[test]
    fn replacing_a_value_keeps_the_comments_on_that_line() {
        let mut doc =
            Document::parse("[device]\nname='m'\n[default]\n# above\nside = 'F13' # after\n")
                .unwrap();
        doc.set_form(&base(), "side", &Form::Keys("F14".into()));
        let text = doc.to_text();
        assert!(text.contains("# above"), "{text}");
        assert!(text.contains("side = \"F14\" # after"), "{text}");

        doc.set_form(
            &base(),
            "side",
            &Form::TapHold {
                tap: "F13".into(),
                hold: "F14".into(),
                hold_ms: 300,
            },
        );
        let text = doc.to_text();
        assert!(
            text.contains("# above") && text.contains("# after"),
            "{text}"
        );
        doc.validate().unwrap();
    }

    #[test]
    fn every_form_round_trips_through_the_daemon_parser() {
        let forms = [
            Form::Passthrough,
            Form::Disabled,
            Form::Keys("Ctrl+W".into()),
            Form::MouseButton("mouse_back".into()),
            Form::Run("notify-send 'Hi: there'".into()),
            Form::Sticky("mouse_left".into()),
            Form::Scroll("down".into()),
            Form::Macro(
                "e{WAITMS:330}{LMB}{LMBD}{WAITMS:400}{LMBU}\\\"".into(),
                MacroMode::Toggle,
            ),
            Form::Macro("e{WAITMS:330}{LMB}".into(), MacroMode::WhileHeld),
            Form::Macro("e{WAITMS:330}{LMB}".into(), MacroMode::RepeatWhileHeld),
            Form::LayerHold(2),
            Form::LayerToggle(3),
            Form::TapHold {
                tap: "F13".into(),
                hold: "Ctrl+W".into(),
                hold_ms: 450,
            },
        ];
        for form in forms {
            let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
            doc.set_form(&base(), "side", &form);
            doc.validate()
                .unwrap_or_else(|e| panic!("{form:?} invalid: {e:#}\n{}", doc.to_text()));
            let reread = Document::parse(&doc.to_text()).unwrap();
            assert_eq!(reread.form(&base(), "side"), form);
        }
    }

    #[test]
    fn saved_actions_mean_what_the_daemon_expects() {
        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        doc.set_form(&app("brave-browser"), "side", &Form::Keys("Ctrl+W".into()));
        doc.set_form(&base(), "wheel_up", &Form::Keys("VolumeUp".into()));
        doc.set_form(&base(), "side+extra", &Form::Keys("F13".into()));
        let config = Config::parse(&doc.to_text()).unwrap();

        let brave = Focus {
            app_id: "brave-browser.desktop".into(),
            wm_class: String::new(),
        };
        assert_eq!(
            config.resolve(side(), &brave, 0),
            &Action::Keys(vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_W])
        );
        assert!(
            config
                .combo(KeyCode::BTN_EXTRA, KeyCode::BTN_SIDE, &brave, 0)
                .is_some()
        );
    }

    #[test]
    fn app_names_with_dots_are_quoted_and_matched() {
        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        doc.set_form(&app("org.gnome.Nautilus"), "side", &Form::Disabled);
        assert!(doc.to_text().contains("[apps.\"org.gnome.Nautilus\"]"));
        assert_eq!(doc.apps(), ["org.gnome.Nautilus"]);
        doc.validate().unwrap();
    }

    #[test]
    fn unset_removes_the_entry_and_creates_nothing() {
        let mut doc = Document::parse(COMMENTED).unwrap();
        doc.set_form(&base(), "side", &Form::Unset);
        assert_eq!(doc.form(&base(), "side"), Form::Unset);

        doc.set_form(&app("never-added"), "side", &Form::Unset);
        assert!(!doc.apps().contains(&"never-added".to_owned()));
        doc.validate().unwrap();
    }

    #[test]
    fn using_a_layer_defines_it() {
        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        doc.set_form(&base(), "side", &Form::LayerHold(4));
        doc.validate().unwrap();

        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        doc.set_form(
            &base(),
            "side",
            &Form::TapHold {
                tap: "F13".into(),
                hold: "layer:5".into(),
                hold_ms: 300,
            },
        );
        doc.validate().unwrap();
    }

    #[test]
    fn layer_slots_are_separate_from_the_base_layer() {
        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        let layer_app = Slot {
            layer: 2,
            app: Some("zalo".into()),
        };
        doc.set_form(&layer_app, "side", &Form::Keys("F14".into()));
        assert_eq!(doc.form(&app("zalo"), "side"), Form::Unset);
        assert_eq!(doc.form(&layer_app, "side"), Form::Keys("F14".into()));
        assert_eq!(doc.apps(), ["zalo"]);
        doc.validate().unwrap();
    }

    #[test]
    fn removing_an_app_clears_it_from_every_layer() {
        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        doc.add_app("zalo");
        doc.set_form(
            &Slot {
                layer: 3,
                app: Some("zalo".into()),
            },
            "side",
            &Form::Disabled,
        );
        doc.remove_app("zalo");
        assert!(doc.apps().is_empty());
        doc.validate().unwrap();
    }

    #[test]
    fn combos_are_listed_separately() {
        let mut doc = Document::parse("[device]\nname='m'\n").unwrap();
        doc.set_form(&base(), "side", &Form::Disabled);
        doc.set_form(&base(), "side+extra", &Form::Keys("F13".into()));
        assert_eq!(doc.combos(&base()), ["side+extra"]);
        doc.validate().unwrap();
    }

    #[test]
    fn reads_tables_written_in_any_toml_style() {
        let doc = Document::parse(
            "[device]\nname='m'\n[apps]\nzalo = { side = 'F13' }\n[default.extra]\ntap='F14'\nhold='F15'\n",
        )
        .unwrap();
        assert_eq!(doc.form(&app("zalo"), "side"), Form::Keys("F13".into()));
        assert_eq!(
            doc.form(&base(), "extra"),
            Form::TapHold {
                tap: "F14".into(),
                hold: "F15".into(),
                hold_ms: 300,
            }
        );
    }

    #[test]
    fn key_tokens_come_from_evdev_codes() {
        assert_eq!(key_token(KeyCode::KEY_T.code()).as_deref(), Some("T"));
        assert_eq!(key_token(KeyCode::KEY_F13.code()).as_deref(), Some("F13"));
        assert_eq!(key_token(KeyCode::BTN_LEFT.code()), None);
        assert_eq!(key_token(0), None);
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("clickshift-doc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    #[test]
    fn store_creates_a_missing_file_on_first_save() {
        let path = temp_path("create");
        let mut store = Store::open(&path, "My Mouse").unwrap();
        assert!(!path.exists());
        let saved = store
            .update(|doc| doc.set_form(&base(), "side", &Form::Disabled))
            .unwrap();
        assert_eq!(saved, Saved::Written);
        let config = Config::load(&path).unwrap();
        assert_eq!(config.device_name, "My Mouse");
    }

    #[test]
    fn store_refuses_invalid_edits_and_leaves_the_file_alone() {
        let path = temp_path("invalid");
        std::fs::write(&path, COMMENTED).unwrap();
        let mut store = Store::open(&path, "").unwrap();
        let result = store.update(|doc| doc.set_form(&base(), "side", &Form::Keys("Nope".into())));
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
        assert_eq!(
            store.doc().form(&base(), "side"),
            Form::Keys("Alt+Left".into())
        );
    }

    #[test]
    fn store_never_overwrites_outside_changes() {
        let path = temp_path("external");
        std::fs::write(&path, COMMENTED).unwrap();
        let mut store = Store::open(&path, "").unwrap();

        let outside = format!("{COMMENTED}\n[apps.zalo]\nside = 'F20'\n");
        std::fs::write(&path, &outside).unwrap();

        let saved = store
            .update(|doc| doc.set_form(&base(), "extra", &Form::Disabled))
            .unwrap();
        assert_eq!(saved, Saved::ReloadedFromDisk);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), outside);
        assert_eq!(
            store.doc().form(&app("zalo"), "side"),
            Form::Keys("F20".into())
        );
        assert_eq!(store.doc().form(&base(), "extra"), Form::Unset);

        // The next edit goes through, on top of the outside change.
        store
            .update(|doc| doc.set_form(&base(), "extra", &Form::Disabled))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("F20") && text.contains("disabled"));
    }

    #[test]
    fn store_leaves_no_temporary_files_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("atomic");
        std::fs::write(&path, COMMENTED).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut store = Store::open(&path, "").unwrap();
        store
            .update(|doc| doc.set_form(&base(), "extra", &Form::Disabled))
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
    }

    #[test]
    fn store_saves_through_a_symlink_without_replacing_it() {
        let path = temp_path("symlink");
        let real = path.with_file_name("real.toml");
        std::fs::write(&real, COMMENTED).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        let mut store = Store::open(&path, "").unwrap();
        store
            .update(|doc| doc.set_form(&base(), "extra", &Form::Disabled))
            .unwrap();
        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
        assert!(std::fs::read_to_string(&real).unwrap().contains("disabled"));
    }
}
