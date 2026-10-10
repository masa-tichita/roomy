//! Data-driven rules: "this location can be cleaned like this".
//!
//! Built-in rules live in `rules/builtin.toml` (compiled in). Users add rules in the config file,
//! turn rules off with `disabled_rules`, and adjust them with `rule_overrides`.

use crate::detect::Ctx;
use crate::i18n::{Msg, Text, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::sys::{days_since, expand, mtime, tilde, which};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const BUILTIN: &str = include_str!("../rules/builtin.toml");

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The matched path itself.
    #[serde(rename = "self")]
    #[default]
    Itself,
    /// Everything inside the matched folder (the folder stays).
    Children,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Delete,
    Trash,
    Command,
    /// Only report; roomy never removes it.
    #[default]
    None,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub category: Text,
    pub title: Text,
    /// `~` and `*` / `?` wildcards inside a path component are allowed.
    pub paths: Vec<String>,
    #[serde(default)]
    pub target: Target,
    #[serde(default = "review")]
    pub safety: Safety,
    #[serde(default)]
    pub action: RuleAction,
    /// For `action = "command"`.
    #[serde(default)]
    pub command: Vec<String>,
    /// Commands that must exist for the rule to apply.
    #[serde(default)]
    pub requires: Vec<String>,
    /// If any process command line contains one of these, the candidate becomes "review".
    #[serde(default)]
    pub skip_if_running: Vec<String>,
    /// File (relative to the matched path) whose modification time tells when it was last used.
    #[serde(default)]
    pub idle_marker: Option<String>,
    /// With `idle_marker`: used more recently than this means "review".
    #[serde(default)]
    pub idle_days: i64,
    #[serde(default)]
    pub min_mb: Option<u64>,
    #[serde(default)]
    pub note: Text,
    /// For `action = "none"`: how to remove it by hand.
    #[serde(default)]
    pub hint: Text,
}

fn review() -> Safety {
    Safety::Review
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct RuleOverride {
    pub safety: Option<Safety>,
    pub min_mb: Option<u64>,
    pub idle_days: Option<i64>,
}

#[derive(Deserialize)]
struct RuleFile {
    rules: Vec<Rule>,
}

pub fn builtin() -> Vec<Rule> {
    toml::from_str::<RuleFile>(BUILTIN)
        .expect("rules/builtin.toml is invalid")
        .rules
}

/// Built-in rules followed by the user's. A user rule with a built-in id replaces it.
pub fn all(cfg: &crate::config::Config) -> Vec<Rule> {
    let mut rules = builtin();
    for r in &cfg.rules {
        match rules.iter_mut().find(|b| b.id == r.id) {
            Some(b) => *b = r.clone(),
            None => rules.push(r.clone()),
        }
    }
    rules
}

pub fn validate(rules: &[Rule]) -> Vec<String> {
    let mut errs = vec![];
    let mut seen = HashSet::new();
    for r in rules {
        if !seen.insert(&r.id) {
            errs.push(crate::i18n::tf("rules.err_duplicate", &[("id", &r.id)]));
        }
        if r.paths.is_empty() {
            errs.push(crate::i18n::tf("rules.err_no_paths", &[("id", &r.id)]));
        }
        if r.paths
            .iter()
            .any(|p| !(p.starts_with("~/") || p.starts_with('/')))
        {
            errs.push(crate::i18n::tf("rules.err_relative", &[("id", &r.id)]));
        }
        if r.action == RuleAction::Command && r.command.is_empty() {
            errs.push(crate::i18n::tf("rules.err_no_command", &[("id", &r.id)]));
        }
    }
    errs
}

/// Every path any rule talks about, so the generic cache sweep does not report them twice.
pub fn claimed_paths(rules: &[Rule]) -> HashSet<PathBuf> {
    rules
        .iter()
        .flat_map(|r| r.paths.iter())
        .flat_map(|p| {
            if p.contains(['*', '?']) {
                expand_glob(p)
            } else {
                vec![expand(p)]
            }
        })
        .collect()
}

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    for rule in all(ctx.cfg) {
        if ctx.cfg.disabled_rules.contains(&rule.id) {
            continue;
        }
        if let Some(c) = evaluate(ctx, &rule) {
            out.push(c);
        }
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    let id = c.source.strip_prefix("rule:").unwrap_or(&c.source);
    let rule = all(ctx.cfg)
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| m("recheck.rule_gone"))?;
    if ctx.cfg.disabled_rules.contains(&rule.id) {
        return Err(m("recheck.rule_disabled"));
    }
    evaluate(ctx, &rule).ok_or_else(|| m("recheck.gone"))
}

fn evaluate(ctx: &Ctx, rule: &Rule) -> Option<Candidate> {
    let ov = ctx
        .cfg
        .rule_overrides
        .get(&rule.id)
        .cloned()
        .unwrap_or_default();
    if rule.requires.iter().any(|cmd| which(cmd).is_none()) {
        return None;
    }
    // Off-limits patterns are dropped before touching the disk (a stat can trigger a TCC prompt).
    let matched: Vec<PathBuf> = rule
        .paths
        .iter()
        .filter(|p| !ctx.is_off_limits(&expand(p)))
        .flat_map(|p| expand_glob(p))
        .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()))
        .collect();
    if matched.is_empty() {
        return None;
    }
    let removal: Vec<PathBuf> = match rule.target {
        Target::Itself => matched.clone(),
        Target::Children => matched.iter().flat_map(|m| children(m)).collect(),
    };
    if removal.is_empty() {
        return None;
    }
    let bytes: u64 = matched.iter().map(|p| ctx.sizer.size(p)).sum();
    let min = ov
        .min_mb
        .or(rule.min_mb)
        .unwrap_or(ctx.cfg.candidates.min_mb)
        * 1_000_000;
    if bytes < min {
        return None;
    }

    let mut safety = ov.safety.unwrap_or(rule.safety);
    let mut evidence = vec![];
    if let Some(p) = rule
        .skip_if_running
        .iter()
        .find(|p| ctx.cmdlines.iter().any(|c| c.contains(p.as_str())))
    {
        safety = Safety::Review;
        evidence.push(mf("rule.running", &[("what", p)]));
    }
    if let Some(marker) = &rule.idle_marker {
        let idle_days = ov.idle_days.unwrap_or(rule.idle_days);
        if let Some(t) = matched.iter().filter_map(|m| mtime(&m.join(marker))).max() {
            let days = days_since(t);
            evidence.push(mf("rule.last_used", &[("days", &days)]));
            if days < idle_days {
                safety = Safety::Review;
            }
        }
    }

    let action = match rule.action {
        RuleAction::Delete | RuleAction::Trash => Action::Paths {
            paths: removal
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            method: if rule.action == RuleAction::Trash {
                Method::Trash
            } else {
                Method::Delete
            },
        },
        RuleAction::Command => Action::Command {
            argv: rule.command.clone(),
            cwd: None,
        },
        RuleAction::None => Action::Manual {
            hint: Msg::Text(rule.hint.clone()),
        },
    };
    let category = if rule.category.is_empty() {
        m("category.other")
    } else {
        Msg::Text(rule.category.clone())
    };
    Some(Candidate {
        id: rule.id.clone(),
        source: format!("rule:{}", rule.id),
        path: tilde(&matched[0]),
        category,
        title: Msg::Text(rule.title.clone()),
        bytes,
        reclaimable: None,
        safety,
        evidence,
        notes: if rule.note.is_empty() {
            vec![]
        } else {
            vec![Msg::Text(rule.note.clone())]
        },
        action,
    })
}

pub fn children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Expands `~` and wildcards (`*`, `?`) inside path components.
pub fn expand_glob(pattern: &str) -> Vec<PathBuf> {
    let full = expand(pattern);
    let mut current = vec![PathBuf::from("/")];
    for comp in full.components().skip(1) {
        let name = comp.as_os_str().to_string_lossy();
        let mut next = vec![];
        for base in &current {
            if name.contains(['*', '?']) {
                for child in children(base) {
                    let n = child
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if wildcard(&name, &n) {
                        next.push(child);
                    }
                }
            } else {
                next.push(base.join(&*name));
            }
        }
        current = next;
    }
    current.retain(|p| fs::symlink_metadata(p).is_ok());
    current
}

pub fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_are_valid() {
        let rules = builtin();
        assert!(!rules.is_empty());
        let errs = validate(&rules);
        assert!(errs.is_empty(), "{errs:?}");
        for r in &rules {
            assert!(!r.title.is_empty(), "{} has no title", r.id);
            if let Text::Localized(m) = &r.title {
                assert!(
                    m.contains_key("en") && m.contains_key("ja"),
                    "{} title needs en and ja",
                    r.id
                );
            }
            if let Text::Localized(m) = &r.note {
                assert!(
                    m.contains_key("en") && m.contains_key("ja"),
                    "{} note needs en and ja",
                    r.id
                );
            }
            if r.action == RuleAction::None {
                assert!(!r.hint.is_empty(), "{} needs a hint", r.id);
            }
            for p in &r.paths {
                // Built-in rules must stay inside the home folder.
                assert!(p.starts_with("~/"), "{} path {p} is outside home", r.id);
            }
        }
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("*DeviceSupport", "iOS DeviceSupport"));
        assert!(wildcard("cache-?", "cache-1"));
        assert!(!wildcard("cache-?", "cache-12"));
        assert!(wildcard("*", ""));
        assert!(!wildcard("a*b", "acd"));
    }

    #[test]
    fn user_rule_replaces_builtin_with_same_id() {
        let mut cfg = crate::config::Config::default();
        let mut r = builtin()[0].clone();
        r.title = Text::Plain("mine".into());
        cfg.rules.push(r.clone());
        let all = all(&cfg);
        assert_eq!(all.iter().filter(|x| x.id == r.id).count(), 1);
        assert_eq!(
            all.iter().find(|x| x.id == r.id).unwrap().title.get(),
            "mine"
        );
    }

    #[test]
    fn user_rule_parses_with_plain_text() {
        let src = r#"
            [[rules]]
            id = "my.renders"
            title = "Old renders"
            paths = ["~/Movies/Renders"]
            target = "children"
            action = "trash"
        "#;
        let f: RuleFile = toml::from_str(src).unwrap();
        assert_eq!(f.rules[0].safety, Safety::Review);
        assert_eq!(f.rules[0].target, Target::Children);
    }

    #[test]
    fn unknown_rule_field_is_rejected() {
        let src = "[[rules]]\nid = \"x\"\ntitle = \"x\"\npaths = [\"~/x\"]\nsafty = \"safe\"\n";
        assert!(toml::from_str::<RuleFile>(src).is_err());
    }
}
