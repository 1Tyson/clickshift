//! Parses macro text into a timeline, such as `e{WAITMS:330}{LMB}{RMB}`.
//!
//! Every step has a fixed time from the start of the macro, so a late wakeup
//! delays one step without pushing back all the steps after it.

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use evdev::KeyCode;

use crate::config::{WheelDir, parse_chord, parse_key};

/// How long a click or key tap stays down. Some programs, games especially, only
/// look at input once per frame and would miss a shorter press. It is taken out
/// of the wait that follows, so `{LMB}{WAITMS:160}{LMB}` clicks 160 ms apart.
const TAP_MS: u64 = 20;
const MAX_WAIT_MS: u64 = 60_000;

const MODIFIERS: [KeyCode; 8] = [
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_RIGHTSHIFT,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    Press(KeyCode),
    Release(KeyCode),
    Scroll(WheelDir),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub at_ms: u64,
    pub emit: Emit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub steps: Vec<Step>,
    /// Includes a trailing wait, during which the macro still counts as playing.
    pub total_ms: u64,
}

impl Macro {
    pub fn parse(text: &str) -> Result<Self> {
        let mut builder = Builder::default();
        let mut rest = text;
        while let Some(character) = rest.chars().next() {
            if character == '{' {
                let end = rest
                    .find('}')
                    .ok_or_else(|| anyhow!("\"{{\" has no closing \"}}\""))?;
                let token = &rest[1..end];
                builder
                    .token(token)
                    .with_context(|| format!("in {{{token}}}"))?;
                rest = &rest[end + 1..];
            } else {
                builder.character(character)?;
                rest = &rest[character.len_utf8()..];
            }
        }
        builder.finish()
    }
}

#[derive(Default)]
struct Builder {
    steps: Vec<Step>,
    now: u64,
    /// Keys of the latest tap, waiting to go up at this time.
    tap_up: Option<(Vec<KeyCode>, u64)>,
}

impl Builder {
    fn finish(mut self) -> Result<Macro> {
        self.flush_tap();
        if self.steps.is_empty() {
            bail!("a macro needs at least one key or click");
        }
        Ok(Macro {
            steps: self.steps,
            total_ms: self.now,
        })
    }

    /// Ends the latest tap, and delays what comes next if its wait was too short
    /// for the tap to finish.
    fn flush_tap(&mut self) {
        if let Some((codes, at_ms)) = self.tap_up.take() {
            for &code in codes.iter().rev() {
                self.steps.push(Step {
                    at_ms,
                    emit: Emit::Release(code),
                });
            }
            self.now = self.now.max(at_ms);
        }
    }

    fn push(&mut self, emit: Emit) {
        self.flush_tap();
        self.steps.push(Step {
            at_ms: self.now,
            emit,
        });
    }

    fn tap(&mut self, codes: Vec<KeyCode>) {
        self.flush_tap();
        for &code in &codes {
            self.steps.push(Step {
                at_ms: self.now,
                emit: Emit::Press(code),
            });
        }
        self.tap_up = Some((codes, self.now + TAP_MS));
    }

    fn wait(&mut self, argument: &str) -> Result<()> {
        let wait: u64 = argument
            .trim()
            .parse()
            .map_err(|_| anyhow!("the wait must be a whole number of milliseconds"))?;
        if wait > MAX_WAIT_MS {
            bail!("the wait cannot be longer than {MAX_WAIT_MS} ms");
        }
        self.now += wait;
        Ok(())
    }

    fn token(&mut self, token: &str) -> Result<()> {
        let upper = token.trim().to_uppercase();
        if let Some(argument) = upper.strip_prefix("WAITMS:") {
            return self.wait(argument);
        }
        if let Some(key) = upper.strip_prefix("PRESS:") {
            self.push(Emit::Press(parse_key(key)?));
            return Ok(());
        }
        if let Some(key) = upper.strip_prefix("RELEASE:") {
            self.push(Emit::Release(parse_key(key)?));
            return Ok(());
        }
        if let Some(direction) = wheel_token(&upper) {
            self.push(Emit::Scroll(direction));
            return Ok(());
        }
        if let Some((button, suffix)) = mouse_token(&upper) {
            match suffix {
                "" => self.tap(vec![button]),
                "D" => self.push(Emit::Press(button)),
                _ => self.push(Emit::Release(button)),
            }
            return Ok(());
        }

        let chord = parse_chord(token)?;
        if let [only] = chord.as_slice()
            && MODIFIERS.contains(only)
        {
            bail!(
                "write the whole chord, like {{Ctrl+C}}, or use {{PRESS:Ctrl}} and {{RELEASE:Ctrl}}"
            );
        }
        self.tap(chord);
        Ok(())
    }

    fn character(&mut self, character: char) -> Result<()> {
        let (code, shifted) = character_key(character).ok_or_else(|| {
            anyhow!("\"{character}\" cannot be typed as a key; use a {{KEY}} token")
        })?;
        self.tap(if shifted {
            vec![KeyCode::KEY_LEFTSHIFT, code]
        } else {
            vec![code]
        });
        Ok(())
    }
}

fn wheel_token(token: &str) -> Option<WheelDir> {
    match token {
        "MWUP" => Some(WheelDir::Up),
        "MWDN" => Some(WheelDir::Down),
        "MWLT" => Some(WheelDir::Left),
        "MWRT" => Some(WheelDir::Right),
        _ => None,
    }
}

/// A mouse button token and its suffix: nothing for a click, D for down, U for up.
fn mouse_token(token: &str) -> Option<(KeyCode, &str)> {
    const BUTTONS: [(&str, KeyCode); 5] = [
        ("LMB", KeyCode::BTN_LEFT),
        ("RMB", KeyCode::BTN_RIGHT),
        ("MMB", KeyCode::BTN_MIDDLE),
        ("MB4", KeyCode::BTN_SIDE),
        ("MB5", KeyCode::BTN_EXTRA),
    ];
    let (name, button) = BUTTONS.iter().find(|(name, _)| token.starts_with(name))?;
    let suffix = &token[name.len()..];
    matches!(suffix, "" | "D" | "U").then_some((*button, suffix))
}

/// The key for a typed character, and whether it needs Shift, on a US layout.
fn character_key(character: char) -> Option<(KeyCode, bool)> {
    if character.is_ascii_alphanumeric() {
        let name = format!("KEY_{}", character.to_ascii_uppercase());
        let code = KeyCode::from_str(&name).ok()?;
        return Some((code, character.is_ascii_uppercase()));
    }
    let (code, shifted) = match character {
        ' ' => (KeyCode::KEY_SPACE, false),
        '-' => (KeyCode::KEY_MINUS, false),
        '_' => (KeyCode::KEY_MINUS, true),
        '=' => (KeyCode::KEY_EQUAL, false),
        '+' => (KeyCode::KEY_EQUAL, true),
        '[' => (KeyCode::KEY_LEFTBRACE, false),
        ']' => (KeyCode::KEY_RIGHTBRACE, false),
        ';' => (KeyCode::KEY_SEMICOLON, false),
        ':' => (KeyCode::KEY_SEMICOLON, true),
        '\'' => (KeyCode::KEY_APOSTROPHE, false),
        '"' => (KeyCode::KEY_APOSTROPHE, true),
        ',' => (KeyCode::KEY_COMMA, false),
        '<' => (KeyCode::KEY_COMMA, true),
        '.' => (KeyCode::KEY_DOT, false),
        '>' => (KeyCode::KEY_DOT, true),
        '/' => (KeyCode::KEY_SLASH, false),
        '?' => (KeyCode::KEY_SLASH, true),
        '\\' => (KeyCode::KEY_BACKSLASH, false),
        '|' => (KeyCode::KEY_BACKSLASH, true),
        '`' => (KeyCode::KEY_GRAVE, false),
        '~' => (KeyCode::KEY_GRAVE, true),
        '!' => (KeyCode::KEY_1, true),
        '@' => (KeyCode::KEY_2, true),
        '#' => (KeyCode::KEY_3, true),
        '$' => (KeyCode::KEY_4, true),
        '%' => (KeyCode::KEY_5, true),
        '^' => (KeyCode::KEY_6, true),
        '&' => (KeyCode::KEY_7, true),
        '*' => (KeyCode::KEY_8, true),
        '(' => (KeyCode::KEY_9, true),
        ')' => (KeyCode::KEY_0, true),
        _ => return None,
    };
    Some((code, shifted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Emit::{Press, Release};
    use KeyCode as K;

    /// Written in the same style as macros from X-Mouse Button Control.
    const SAMPLE_MACRO: &str = "a{WAITMS:300}{LMB}{WAITMS:150}{LMB}{WAITMS:250}{RMB}{WAITMS:200}{LMBD}{WAITMS:400}{LMBU}{WAITMS:100}b{WAITMS:600}a{WAITMS:300}{LMB}{WAITMS:150}{LMB}{WAITMS:250}{RMB}{WAITMS:200}{LMBD}{WAITMS:400}{LMBU}{WAITMS:100}";

    fn timeline(text: &str) -> Vec<(u64, Emit)> {
        Macro::parse(text)
            .unwrap()
            .steps
            .iter()
            .map(|step| (step.at_ms, step.emit))
            .collect()
    }

    #[test]
    fn long_macro_keeps_its_exact_timing() {
        let parsed = Macro::parse(SAMPLE_MACRO).unwrap();
        let waits: u64 = SAMPLE_MACRO
            .split("{WAITMS:")
            .skip(1)
            .map(|part| part.split('}').next().unwrap().parse::<u64>().unwrap())
            .sum();
        assert_eq!(parsed.total_ms, waits);

        // Every press has its release, so nothing is left held at the end.
        let count = |wanted: fn(&Emit) -> bool| {
            parsed
                .steps
                .iter()
                .filter(|step| wanted(&step.emit))
                .count()
        };
        assert_eq!(
            count(|emit| matches!(emit, Press(_))),
            count(|emit| matches!(emit, Release(_)))
        );
        assert!(
            parsed
                .steps
                .windows(2)
                .all(|pair| pair[0].at_ms <= pair[1].at_ms)
        );
    }

    #[test]
    fn taps_are_held_briefly_and_the_hold_comes_out_of_the_wait() {
        assert_eq!(
            timeline("{LMB}{WAITMS:160}{RMB}"),
            [
                (0, Press(K::BTN_LEFT)),
                (20, Release(K::BTN_LEFT)),
                (160, Press(K::BTN_RIGHT)),
                (180, Release(K::BTN_RIGHT)),
            ]
        );
    }

    #[test]
    fn a_wait_shorter_than_a_tap_never_overlaps_or_reorders() {
        assert_eq!(
            timeline("{LMB}{WAITMS:5}{LMB}"),
            [
                (0, Press(K::BTN_LEFT)),
                (20, Release(K::BTN_LEFT)),
                (20, Press(K::BTN_LEFT)),
                (40, Release(K::BTN_LEFT)),
            ]
        );
    }

    #[test]
    fn down_and_up_tokens_hold_for_exactly_the_wait() {
        assert_eq!(
            timeline("{LMBD}{WAITMS:400}{LMBU}"),
            [(0, Press(K::BTN_LEFT)), (400, Release(K::BTN_LEFT))]
        );
    }

    #[test]
    fn trailing_wait_counts_towards_the_length() {
        assert_eq!(Macro::parse("q{WAITMS:400}").unwrap().total_ms, 400);
        assert_eq!(Macro::parse("q").unwrap().total_ms, TAP_MS);
    }

    #[test]
    fn typed_characters_use_shift_when_needed() {
        assert_eq!(
            timeline("aB"),
            [
                (0, Press(K::KEY_A)),
                (20, Release(K::KEY_A)),
                (20, Press(K::KEY_LEFTSHIFT)),
                (20, Press(K::KEY_B)),
                (40, Release(K::KEY_B)),
                (40, Release(K::KEY_LEFTSHIFT)),
            ]
        );
    }

    #[test]
    fn tokens_ignore_case_and_cover_keys_chords_and_the_wheel() {
        assert_eq!(timeline("{lmbd}"), [(0, Press(K::BTN_LEFT))]);
        assert_eq!(timeline("{MB5U}"), [(0, Release(K::BTN_EXTRA))]);
        assert_eq!(timeline("{MWDN}"), [(0, Emit::Scroll(WheelDir::Down))]);
        assert_eq!(
            timeline("{Enter}"),
            [(0, Press(K::KEY_ENTER)), (20, Release(K::KEY_ENTER))]
        );
        assert_eq!(
            timeline("{Ctrl+C}"),
            [
                (0, Press(K::KEY_LEFTCTRL)),
                (0, Press(K::KEY_C)),
                (20, Release(K::KEY_C)),
                (20, Release(K::KEY_LEFTCTRL)),
            ]
        );
        assert_eq!(
            timeline("{PRESS:Ctrl}{RELEASE:Ctrl}"),
            [(0, Press(K::KEY_LEFTCTRL)), (0, Release(K::KEY_LEFTCTRL))]
        );
    }

    #[test]
    fn mistakes_are_refused_instead_of_guessed() {
        for text in [
            "",
            "{WAITMS:100}",
            "{LMB",
            "{}",
            "}",
            "{WAITMS:abc}",
            "{WAITMS:-5}",
            "{WAITMS:10-20}",
            "{WAITMS:60001}q",
            "{NOSUCHKEY}",
            "{LMBX}",
            "{CTRL}a",
            "{WAIT:1}q",
            "ă",
            "a\nb",
        ] {
            assert!(Macro::parse(text).is_err(), "should reject: {text:?}");
        }
    }
}
