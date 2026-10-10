//! Messages in English (default) and Japanese.
//!
//! Language comes from config (`language = "en" | "ja"`), else `LC_ALL` / `LC_MESSAGES` / `LANG`,
//! else the macOS preferred languages (launchd jobs have no `LANG`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Ja,
}

static LANG: OnceLock<Lang> = OnceLock::new();
static EN_CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
static JA_CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();

const EN: &str = include_str!("../locales/en.toml");
const JA: &str = include_str!("../locales/ja.toml");

pub fn init(setting: &str) {
    let lang = match setting {
        "en" => Lang::En,
        "ja" => Lang::Ja,
        _ => detect(),
    };
    let _ = LANG.set(lang);
}

pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

fn from_tag(tag: &str) -> Option<Lang> {
    let t = tag.trim().trim_matches('"').to_ascii_lowercase();
    if t.is_empty() || t == "c" || t == "posix" {
        None
    } else if t.starts_with("ja") {
        Some(Lang::Ja)
    } else {
        Some(Lang::En)
    }
}

fn detect() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(l) = std::env::var(var).ok().as_deref().and_then(from_tag) {
            return l;
        }
    }
    // e.g. "(\n    \"ja-JP\",\n    \"en-JP\"\n)"
    crate::sys::run(
        &["defaults", "read", "-g", "AppleLanguages"],
        None,
        std::time::Duration::from_secs(3),
    )
    .and_then(|o| {
        o.stdout
            .lines()
            .map(|l| l.trim().trim_end_matches(','))
            .find_map(|l| l.starts_with('"').then(|| from_tag(l)).flatten())
    })
    .unwrap_or(Lang::En)
}

pub fn parse_catalog(src: &str) -> HashMap<String, String> {
    toml::from_str(src).expect("invalid locale file")
}

fn catalog(lang: Lang) -> &'static HashMap<String, String> {
    match lang {
        Lang::En => EN_CATALOG.get_or_init(|| parse_catalog(EN)),
        Lang::Ja => JA_CATALOG.get_or_init(|| {
            let mut c = parse_catalog(EN);
            c.extend(parse_catalog(JA));
            c
        }),
    }
}

/// Message for `key` in a given language. Unknown keys return the key itself so a missing
/// translation is visible.
pub fn t_in(lang: Lang, key: &str) -> String {
    catalog(lang)
        .get(key)
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// Message for `key` in the user's language.
pub fn t(key: &str) -> String {
    t_in(lang(), key)
}

/// Message with `{name}` placeholders filled in.
pub fn tf(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = t(key);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), &value.to_string());
    }
    s
}

/// A message stored in reports. Rendered when shown, so the language always matches the reader
/// (the background service may scan in Japanese while the terminal is English, or the other way round).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Msg {
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        args: std::collections::BTreeMap<String, String>,
    },
    Text(Text),
}

impl Msg {
    pub fn render(&self) -> String {
        self.render_in(lang())
    }

    /// Rendered in a fixed language (JSON output is always English).
    pub fn render_in(&self, lang: Lang) -> String {
        match self {
            Msg::Key { key, args } => {
                let mut s = t_in(lang, key);
                for (name, value) in args {
                    s = s.replace(&format!("{{{name}}}"), value);
                }
                s
            }
            Msg::Text(text) => text.get_in(lang),
        }
    }

    pub fn plain(s: impl Into<String>) -> Msg {
        Msg::Text(Text::Plain(s.into()))
    }
}

impl std::fmt::Display for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// Stored message without arguments.
pub fn m(key: &str) -> Msg {
    Msg::Key {
        key: key.into(),
        args: std::collections::BTreeMap::default(),
    }
}

/// Stored message with `{name}` arguments (formatted now, inserted when rendered).
pub fn mf(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> Msg {
    Msg::Key {
        key: key.into(),
        args: args
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

/// Text in rule files and user config: a plain string, or `{ en = "...", ja = "..." }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Text {
    Plain(String),
    Localized(HashMap<String, String>),
}

impl Default for Text {
    fn default() -> Self {
        Text::Plain(String::new())
    }
}

impl Text {
    pub fn get(&self) -> String {
        self.get_in(lang())
    }

    pub fn get_in(&self, lang: Lang) -> String {
        match self {
            Text::Plain(s) => s.clone(),
            Text::Localized(m) => {
                let code = if lang == Lang::Ja { "ja" } else { "en" };
                m.get(code)
                    .or_else(|| m.get("en"))
                    .or_else(|| m.values().next())
                    .cloned()
                    .unwrap_or_default()
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.get().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_translated_both_ways() {
        let en = parse_catalog(EN);
        let ja = parse_catalog(JA);
        let missing_ja: Vec<_> = en.keys().filter(|k| !ja.contains_key(*k)).collect();
        let missing_en: Vec<_> = ja.keys().filter(|k| !en.contains_key(*k)).collect();
        assert!(missing_ja.is_empty(), "missing in ja.toml: {missing_ja:?}");
        assert!(missing_en.is_empty(), "missing in en.toml: {missing_en:?}");
    }

    #[test]
    fn placeholders_match() {
        let en = parse_catalog(EN);
        let ja = parse_catalog(JA);
        let names = |s: &str| {
            let mut v: Vec<String> = s
                .split('{')
                .skip(1)
                .filter_map(|p| p.split_once('}').map(|x| x.0.to_string()))
                .collect();
            // A translation may use a placeholder more than once.
            v.sort();
            v.dedup();
            v
        };
        for (k, v) in &en {
            if let Some(j) = ja.get(k) {
                assert_eq!(names(v), names(j), "placeholders differ for {k}");
            }
        }
    }

    /// Every `t("…")` / `tf("…", …)` in the source has an English message.
    #[test]
    fn every_used_key_exists() {
        fn sources(dir: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    sources(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(std::fs::read_to_string(p).unwrap());
                }
            }
        }
        let mut files = vec![];
        sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let en = parse_catalog(EN);
        let mut missing = vec![];
        for src in &files {
            for opener in ["t(", "tf(", "m(", "mf("] {
                for (i, _) in src.match_indices(opener) {
                    let before = src[..i].chars().last().unwrap_or(' ');
                    if before.is_alphanumeric() || before == '_' {
                        continue; // part of another identifier, e.g. `get(` or `fmt(`
                    }
                    let rest = src[i + opener.len()..].trim_start();
                    let Some(rest) = rest.strip_prefix('"') else {
                        continue;
                    };
                    let key: String = rest.chars().take_while(|c| *c != '"').collect();
                    if key.contains('.') && !key.contains(' ') && !en.contains_key(&key) {
                        missing.push(key);
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "keys used in code but missing in en.toml: {missing:?}"
        );
    }

    #[test]
    fn stored_messages_round_trip() {
        let msgs = vec![
            mf("wt.dirty", &[("n", &3)]),
            m("safety.safe"),
            Msg::plain("Xcode"),
            Msg::Text(Text::Localized(
                [("en".to_string(), "Hi".to_string())].into(),
            )),
        ];
        let json = serde_json::to_string(&msgs).unwrap();
        let back: Vec<Msg> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msgs);
        assert_eq!(back[2].render(), "Xcode");
    }

    #[test]
    fn language_tags() {
        assert_eq!(from_tag("ja_JP.UTF-8"), Some(Lang::Ja));
        assert_eq!(from_tag("\"ja-JP\""), Some(Lang::Ja));
        assert_eq!(from_tag("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(from_tag("C"), None);
        assert_eq!(from_tag(""), None);
    }
}
