//! `~/.config/roomy/config.toml`. Every key is optional; missing keys take the defaults below.
//! Defaults are meant to be safe on anyone's Mac. Personal needs go in the config file.

use crate::app;
use crate::rules::{Rule, RuleOverride};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub config_version: u32,
    /// "auto", "en" or "ja"
    pub language: String,
    pub thresholds: Thresholds,
    pub scan: Scan,
    pub candidates: Candidates,
    pub worktrees: Worktrees,
    pub apps: Apps,
    pub downloads: Downloads,
    pub clean: CleanCfg,
    pub versioned: Vec<VersionedDir>,
    pub service: Service,
    /// Rule ids to turn off.
    pub disabled_rules: Vec<String>,
    /// Built-in detectors to turn off, e.g. `["docker", "apps"]` (see `detect::DETECTORS`).
    pub disabled_detectors: Vec<String>,
    /// Per-rule changes, e.g. `[rule_overrides."claude.desktop-vm"] safety = "safe"`.
    pub rule_overrides: BTreeMap<String, RuleOverride>,
    /// Your own rules (same format as the built-in ones).
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    pub disk_warn_gb: f64,
    pub disk_critical_gb: f64,
    /// Notify when free space drops this much between two background checks.
    pub disk_drop_gb: f64,
    pub swap_warn_gb: f64,
    /// Report folders that grew this much since the previous scan.
    pub growth_gb: f64,
    /// Notify after the daily scan when this much can be freed safely.
    pub safe_notify_gb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Scan {
    pub roots: Vec<String>,
    pub exclude: Vec<String>,
    pub snapshot_min_mb: u64,
    pub snapshot_max_depth: usize,
    pub keep_snapshots: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Candidates {
    /// Smaller candidates are not shown.
    pub min_mb: u64,
    /// Size from which an app cache folder is listed.
    pub cache_min_mb: u64,
    /// `clean` asks for a fresh scan when the report is older than this.
    pub stale_report_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Worktrees {
    /// Where to look for git worktrees. Folders that do not exist are skipped.
    pub roots: Vec<String>,
    /// Worktrees touched more recently than this are left alone.
    pub idle_days: i64,
    /// Folders managed by another tool (e.g. Orca), optionally with its removal command.
    pub managed: Vec<Managed>,
    /// Ask `gh` whether a branch's pull request was merged (catches squash merges).
    pub check_pull_requests: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Managed {
    pub path: String,
    pub name: String,
    /// Command that removes a worktree through the tool, `{path}` replaced by its path,
    /// e.g. `["orca", "worktree", "rm", "--worktree", "path:{path}"]`. Without it, worktrees
    /// here are marked "review" and removed with `git worktree remove`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Apps {
    pub min_gb: f64,
    pub unused_days: i64,
    /// Apps never suggested (substring of the name).
    pub keep: Vec<String>,
}

/// Limits for unattended removal (`--yes`, `--json`, MCP).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanCfg {
    /// One unattended `clean` may remove at most this much; larger plans need a person.
    pub max_unattended_gb: f64,
}

impl Default for CleanCfg {
    fn default() -> Self {
        CleanCfg {
            max_unattended_gb: 50.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Downloads {
    pub age_days: i64,
}

/// A folder that gets one subfolder per installed version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedDir {
    pub path: String,
    #[serde(default = "default_keep")]
    pub keep: usize,
}

fn default_keep() -> usize {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Service {
    pub interval_min: u32,
    pub daily_scan: bool,
    /// On battery, the daily scan waits until the charge is at least this.
    pub min_battery_pct: u32,
    /// Hours (0-23) during which no notifications are sent, e.g. [23, 7].
    pub quiet_hours: Option<[u32; 2]>,
    /// `false`: keep checking and recording history, but never notify.
    pub notifications: bool,
    /// `false`: no memory-pressure or swap notifications (disk alerts still come).
    pub memory_alerts: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            config_version: CONFIG_VERSION,
            language: "auto".into(),
            thresholds: Thresholds::default(),
            scan: Scan::default(),
            candidates: Candidates::default(),
            worktrees: Worktrees::default(),
            apps: Apps::default(),
            downloads: Downloads::default(),
            clean: CleanCfg::default(),
            versioned: vec![VersionedDir {
                path: "~/.codex/packages/app-server-daemon/releases".into(),
                keep: 2,
            }],
            service: Service::default(),
            disabled_rules: vec![],
            disabled_detectors: vec![],
            rule_overrides: BTreeMap::new(),
            rules: vec![],
        }
    }
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            disk_warn_gb: 50.0,
            disk_critical_gb: 25.0,
            disk_drop_gb: 10.0,
            swap_warn_gb: 4.0,
            growth_gb: 3.0,
            safe_notify_gb: 10.0,
        }
    }
}

impl Default for Scan {
    fn default() -> Self {
        Scan {
            roots: strings(&["~", "/Applications"]),
            exclude: strings(&[
                "~/Library/CloudStorage",
                "~/Library/Mobile Documents",
                "~/OrbStack",
            ]),
            snapshot_min_mb: 50,
            snapshot_max_depth: 8,
            keep_snapshots: 30,
        }
    }
}

impl Default for Candidates {
    fn default() -> Self {
        Candidates {
            min_mb: 100,
            cache_min_mb: 300,
            stale_report_days: 3,
        }
    }
}

impl Default for Worktrees {
    fn default() -> Self {
        Worktrees {
            roots: strings(&[
                "~/dev",
                "~/src",
                "~/code",
                "~/projects",
                "~/Projects",
                "~/Developer",
                "~/work",
                "~/repos",
                "~/git",
                "~/.codex/worktrees",
            ]),
            idle_days: 7,
            managed: vec![],
            check_pull_requests: true,
        }
    }
}

impl Default for Apps {
    fn default() -> Self {
        Apps {
            min_gb: 1.0,
            unused_days: 90,
            keep: vec![],
        }
    }
}

impl Default for Downloads {
    fn default() -> Self {
        Downloads { age_days: 60 }
    }
}

impl Default for Service {
    fn default() -> Self {
        Service {
            interval_min: 15,
            daily_scan: true,
            min_battery_pct: 50,
            quiet_hours: None,
            notifications: true,
            memory_alerts: true,
        }
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(ToString::to_string).collect()
}

pub fn gb(x: f64) -> u64 {
    (x.max(0.0) * 1e9) as u64
}

pub fn path() -> PathBuf {
    app::config_dir().join("config.toml")
}

pub struct Loaded {
    pub config: Config,
    pub exists: bool,
    pub error: Option<String>,
    pub unknown_keys: Vec<String>,
}

/// Never fails: a broken file falls back to defaults and reports the error.
pub fn load() -> Loaded {
    let p = path();
    let Ok(text) = fs::read_to_string(&p) else {
        return Loaded {
            config: Config::default(),
            exists: false,
            error: None,
            unknown_keys: vec![],
        };
    };
    match parse(&text) {
        Ok((config, unknown_keys)) => Loaded {
            config,
            exists: true,
            error: None,
            unknown_keys,
        },
        Err(e) => Loaded {
            config: Config::default(),
            exists: true,
            error: Some(e),
            unknown_keys: vec![],
        },
    }
}

pub fn parse(text: &str) -> Result<(Config, Vec<String>), String> {
    let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    let given: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let known = toml::Value::try_from(Config::default()).map_err(|e| e.to_string())?;
    let mut unknown = vec![];
    unknown_keys(&given, &known, "", &mut unknown);
    for d in &config.disabled_detectors {
        if !crate::detect::DETECTORS.contains(&d.as_str()) {
            unknown.push(format!("disabled_detectors.{d}"));
        }
    }
    Ok((config, unknown))
}

/// Keys present in the file but not in the schema (typos). Free-form tables are not checked.
fn unknown_keys(given: &toml::Value, known: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    let (Some(g), Some(k)) = (given.as_table(), known.as_table()) else {
        return;
    };
    for (key, value) in g {
        let full = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if matches!(full.as_str(), "rule_overrides" | "rules" | "versioned") {
            continue;
        }
        match k.get(key) {
            None if !is_optional_key(&full) => out.push(full),
            Some(kv) => unknown_keys(value, kv, &full, out),
            None => {}
        }
    }
}

/// Keys whose default is `None`, so they do not appear in the serialized defaults.
fn is_optional_key(key: &str) -> bool {
    matches!(key, "service.quiet_hours")
}

/// Starter file: commented-out defaults so the user can see every knob.
pub fn template() -> String {
    let body = toml::to_string_pretty(&Config::default()).unwrap_or_default();
    let commented: String = body
        .lines()
        .map(|l| {
            if l.is_empty() || (l.starts_with('[') && !l.starts_with("[[")) {
                format!("{l}\n")
            } else {
                format!("# {l}\n")
            }
        })
        .collect();
    format!(
        "# roomy configuration. Uncomment a line to change it. See README for details.\n\
         config_version = {CONFIG_VERSION}\n\n{commented}\n\
         # Example: keep an app out of suggestions\n\
         # [apps]\n# keep = [\"MATLAB\"]\n\n\
         # Example: your own rule\n\
         # [[rules]]\n# id = \"my.renders\"\n# paths = [\"~/Movies/Renders\"]\n# target = \"children\"\n\
         # safety = \"review\"\n# action = \"trash\"\n# note = \"Old renders\"\n"
    )
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact values parsed from literals
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let (c, unknown) = parse("").unwrap();
        assert_eq!(c.thresholds.disk_warn_gb, 50.0);
        assert_eq!(unknown, Vec::<String>::new());
    }

    #[test]
    fn partial_section_keeps_other_defaults() {
        let (c, _) = parse("[thresholds]\ndisk_warn_gb = 80\n").unwrap();
        assert_eq!(c.thresholds.disk_warn_gb, 80.0);
        assert_eq!(c.thresholds.disk_critical_gb, 25.0);
    }

    #[test]
    fn typos_are_reported() {
        let (_, unknown) = parse("languge = \"ja\"\n[thresholds]\ndisk_warm_gb = 1\n").unwrap();
        assert_eq!(unknown, vec!["languge", "thresholds.disk_warm_gb"]);
    }

    #[test]
    fn optional_and_free_form_keys_are_not_typos() {
        let text =
            "[service]\nquiet_hours = [23, 7]\n[rule_overrides.\"x.y\"]\nsafety = \"safe\"\n";
        let (c, unknown) = parse(text).unwrap();
        assert!(unknown.is_empty(), "{unknown:?}");
        assert_eq!(c.service.quiet_hours, Some([23, 7]));
    }

    #[test]
    fn unknown_detector_names_are_reported() {
        let (c, unknown) = parse("disabled_detectors = [\"docker\", \"dokcer\"]\n").unwrap();
        assert_eq!(c.disabled_detectors, vec!["docker", "dokcer"]);
        assert_eq!(unknown, vec!["disabled_detectors.dokcer"]);
    }

    #[test]
    fn template_parses() {
        let (c, unknown) = parse(&template()).unwrap();
        assert_eq!(unknown, Vec::<String>::new());
        assert_eq!(c.config_version, CONFIG_VERSION);
    }

    #[test]
    fn wrong_type_is_an_error() {
        assert!(parse("[thresholds]\ndisk_warn_gb = \"lots\"\n").is_err());
    }
}
