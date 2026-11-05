//! The settings app's languages. Each text is written where it is used as
//! [English, Vietnamese, Chinese], so a missing translation does not compile.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use clickshift::config;

pub type Text = [&'static str; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Vietnamese,
    Chinese,
}

/// The dropdown's choices after "Automatic", with each language in its own name.
pub const CHOICES: [(Language, &str); 3] = [
    (Language::English, "English"),
    (Language::Vietnamese, "Tiếng Việt"),
    (Language::Chinese, "简体中文"),
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn tr(text: Text) -> &'static str {
    text[CURRENT.load(Ordering::Relaxed)]
}

pub fn set_current(language: Language) {
    CURRENT.store(language as usize, Ordering::Relaxed);
}

/// Reads locale values in priority order; a value may list several, as LANGUAGE does.
fn from_locale<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Language> {
    values
        .into_iter()
        .flat_map(|value| value.split(':'))
        .find_map(|locale| match locale.split(['_', '.', '@']).next() {
            Some("en") => Some(Language::English),
            Some("vi") => Some(Language::Vietnamese),
            Some("zh") => Some(Language::Chinese),
            _ => None,
        })
}

pub fn system_language() -> Language {
    let values: Vec<String> = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect();
    from_locale(values.iter().map(String::as_str)).unwrap_or(Language::English)
}

fn code(language: Language) -> &'static str {
    match language {
        Language::English => "en",
        Language::Vietnamese => "vi",
        Language::Chinese => "zh",
    }
}

fn from_code(text: &str) -> Option<Language> {
    match text.trim() {
        "en" => Some(Language::English),
        "vi" => Some(Language::Vietnamese),
        "zh" => Some(Language::Chinese),
        _ => None,
    }
}

/// Kept beside the config file but apart from it, so the daemon never reads it.
fn saved_path() -> Option<PathBuf> {
    Some(config::default_path().ok()?.parent()?.join("language"))
}

/// The language chosen in the app, or `None` to follow the system.
pub fn saved_choice() -> Option<Language> {
    from_code(&std::fs::read_to_string(saved_path()?).ok()?)
}

pub fn save_choice(choice: Option<Language>) -> std::io::Result<()> {
    let Some(path) = saved_path() else {
        return Ok(());
    };
    match choice {
        Some(language) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, code(language))
        }
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_names_map_to_languages() {
        assert_eq!(from_locale(["vi_VN.UTF-8"]), Some(Language::Vietnamese));
        assert_eq!(from_locale(["zh_CN.UTF-8"]), Some(Language::Chinese));
        assert_eq!(from_locale(["en_US.UTF-8"]), Some(Language::English));
        assert_eq!(from_locale(["vi@latin"]), Some(Language::Vietnamese));
    }

    #[test]
    fn earlier_values_win_and_unknown_ones_are_skipped() {
        assert_eq!(
            from_locale(["fr:zh_CN:en", "vi_VN"]),
            Some(Language::Chinese)
        );
        assert_eq!(from_locale(["C", "", "vi_VN"]), Some(Language::Vietnamese));
        assert_eq!(from_locale(["C", "POSIX", "fr_FR"]), None);
    }

    #[test]
    fn saved_codes_round_trip_and_junk_means_follow_the_system() {
        for (language, _) in CHOICES {
            assert_eq!(from_code(code(language)), Some(language));
        }
        assert_eq!(from_code(" vi\n"), Some(Language::Vietnamese));
        assert_eq!(from_code("klingon"), None);
    }
}
