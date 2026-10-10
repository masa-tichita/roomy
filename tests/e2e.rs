//! End-to-end tests: run the real binary with HOME pointing at a throwaway folder, so nothing
//! outside the sandbox can be touched.

#![allow(clippy::unwrap_used)] // test helpers: a failure should abort the test

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Sandbox {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl Sandbox {
    fn new(config: &str) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        fs::create_dir_all(home.join(".config/roomy")).unwrap();
        fs::write(home.join(".config/roomy/config.toml"), config).unwrap();
        Sandbox { _dir: dir, home }
    }

    fn roomy(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_roomy"))
            .args(args)
            .env("HOME", &self.home)
            .env("LANG", "en_US.UTF-8")
            .env_remove("LC_ALL")
            .env_remove("LC_MESSAGES")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("ROOMY_CONFIG_DIR")
            .env_remove("ROOMY_STATE_DIR")
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    fn file(&self, rel: &str, bytes: usize) -> PathBuf {
        let p = self.home.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![7u8; bytes]).unwrap();
        p
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn report(sb: &Sandbox) -> serde_json::Value {
    let o = sb.roomy(&["report", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

fn ids(r: &serde_json::Value) -> Vec<String> {
    r["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

const BASE: &str = r#"
[scan]
roots = ["~"]
[candidates]
min_mb = 0
cache_min_mb = 0
[thresholds]
disk_warn_gb = 0
disk_critical_gb = 0
swap_warn_gb = 100000
[worktrees]
roots = []
[apps]
min_gb = 100000
"#;

#[test]
fn scan_then_clean_removes_only_the_selected_candidate() {
    let sb = Sandbox::new(&format!(
        "{BASE}\n[[rules]]\nid = \"test.cache\"\ntitle = \"Test cache\"\npaths = [\"~/Library/Caches/TestApp\"]\n\
         target = \"children\"\nsafety = \"safe\"\naction = \"delete\"\n"
    ));
    sb.file("Library/Caches/TestApp/a/blob", 200_000);
    sb.file("Library/Caches/TestApp/b", 100_000);
    let keep = sb.file("Library/Caches/Other/keep", 100_000);

    let o = sb.roomy(&["scan"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r = report(&sb);
    assert!(ids(&r).contains(&"test.cache".to_string()), "{:?}", ids(&r));

    // dry run changes nothing
    let o = sb.roomy(&["clean", "test.cache", "--dry-run"]);
    assert!(o.status.success());
    assert!(sb.home.join("Library/Caches/TestApp/b").exists());

    // without a terminal, --yes is required
    let o = sb.roomy(&["clean", "test.cache"]);
    assert!(!o.status.success());
    assert!(sb.home.join("Library/Caches/TestApp/b").exists());

    let o = sb.roomy(&["clean", "test.cache", "--yes"]);
    assert!(
        o.status.success(),
        "{}{}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let cache = sb.home.join("Library/Caches/TestApp");
    assert!(cache.is_dir(), "target = children keeps the folder itself");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
    assert!(keep.exists(), "other caches are untouched");

    // removed from the report, recorded in the audit log
    assert!(!ids(&report(&sb)).contains(&"test.cache".to_string()));
    let audit = fs::read_to_string(
        sb.home
            .join("Library/Application Support/roomy/audit.jsonl"),
    )
    .unwrap();
    assert!(audit.contains("\"id\":\"test.cache\"") && audit.contains("\"ok\":true"));
}

#[test]
fn guard_refuses_protected_folders_even_if_a_rule_says_safe() {
    let sb = Sandbox::new(&format!(
        "{BASE}\n[[rules]]\nid = \"test.docs\"\ntitle = \"Docs\"\npaths = [\"~/Documents/old\"]\n\
         safety = \"safe\"\naction = \"delete\"\n"
    ));
    let f = sb.file("Documents/old/thesis.pdf", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let o = sb.roomy(&["clean", "test.docs", "--yes"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("protected"), "{}", stdout(&o));
    assert!(f.exists());
}

#[test]
fn guard_refuses_symlinks_out_of_home() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("precious"), "x").unwrap();
    let sb = Sandbox::new(&format!(
        "{BASE}\n[[rules]]\nid = \"test.link\"\ntitle = \"Link\"\npaths = [\"~/Library/Caches/Linked\"]\n\
         target = \"children\"\nsafety = \"safe\"\naction = \"delete\"\n"
    ));
    fs::create_dir_all(sb.home.join("Library/Caches")).unwrap();
    std::os::unix::fs::symlink(outside.path(), sb.home.join("Library/Caches/Linked")).unwrap();
    assert!(sb.roomy(&["scan"]).status.success());
    // A symlinked rule path is never matched, so there is nothing to clean.
    assert!(!ids(&report(&sb)).contains(&"test.link".to_string()));
    assert!(outside.path().join("precious").exists());
}

#[test]
fn recheck_stops_removal_when_the_situation_changed() {
    let sb = Sandbox::new(&format!(
        "{BASE}\n[[rules]]\nid = \"test.cache\"\ntitle = \"Test cache\"\npaths = [\"~/Library/Caches/TestApp\"]\n\
         target = \"children\"\nsafety = \"safe\"\naction = \"delete\"\n"
    ));
    sb.file("Library/Caches/TestApp/a", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    // The folder disappears after the scan: clean must notice instead of acting on stale data.
    fs::remove_dir_all(sb.home.join("Library/Caches/TestApp")).unwrap();
    let o = sb.roomy(&["clean", "test.cache", "--yes"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("no longer applies"), "{}", stdout(&o));
}

#[test]
fn disabled_and_overridden_rules() {
    let sb = Sandbox::new(&format!(
        // Top-level keys must come before any [table], so they go first.
        "disabled_rules = [\"test.off\"]\n{BASE}\n[rule_overrides.\"test.on\"]\nsafety = \"review\"\n\
         [[rules]]\nid = \"test.off\"\ntitle = \"Off\"\npaths = [\"~/Library/Caches/Off\"]\nsafety = \"safe\"\naction = \"delete\"\n\
         [[rules]]\nid = \"test.on\"\ntitle = \"On\"\npaths = [\"~/Library/Caches/On\"]\nsafety = \"safe\"\naction = \"delete\"\n"
    ));
    sb.file("Library/Caches/Off/x", 100_000);
    sb.file("Library/Caches/On/x", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let r = report(&sb);
    let list = ids(&r);
    assert!(!list.contains(&"test.off".to_string()));
    // a disabled rule's path must not come back through the generic cache sweep either
    assert!(!list.iter().any(|i| i.ends_with(":Off")), "{list:?}");
    let on = r["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "test.on")
        .unwrap();
    assert_eq!(on["safety"], "review");
}

#[test]
fn broken_config_falls_back_to_defaults_and_validate_reports_it() {
    let sb = Sandbox::new("[thresholds]\ndisk_warn_gb = \"lots\"\n");
    let o = sb.roomy(&["config", "validate"]);
    assert!(!o.status.success());
    let o = sb.roomy(&["status", "--json"]);
    assert!(o.status.success(), "status still works on defaults");
    assert!(String::from_utf8_lossy(&o.stderr).contains("error"));
}

#[test]
fn agent_records_history_and_runs_the_daily_scan() {
    let sb = Sandbox::new(BASE);
    let o = sb.roomy(&["agent"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let state = sb.home.join("Library/Application Support/roomy");
    assert_eq!(
        fs::read_to_string(state.join("history.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(
        state.join("report.json").exists(),
        "first run scans because no report exists yet"
    );
    let r = report(&sb);
    assert_eq!(
        r["walk"]["skipped_protected"], true,
        "no Full Disk Access in the sandbox"
    );

    let o = sb.roomy(&["agent"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("notified=[]"));
    assert!(
        !stdout(&o).contains("scan="),
        "a fresh report means no second scan"
    );
}

#[test]
fn japanese_and_english_render_the_same_report() {
    let sb = Sandbox::new(&format!(
        "{BASE}\n[[rules]]\nid = \"test.cache\"\ntitle = {{ en = \"Test cache\", ja = \"テスト用キャッシュ\" }}\n\
         paths = [\"~/Library/Caches/TestApp\"]\nsafety = \"safe\"\naction = \"delete\"\n"
    ));
    sb.file("Library/Caches/TestApp/a", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let en = stdout(&sb.roomy(&["report"]));
    let ja = Command::new(env!("CARGO_BIN_EXE_roomy"))
        .args(["report"])
        .env("HOME", &sb.home)
        .env("LANG", "ja_JP.UTF-8")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let ja = stdout(&ja);
    assert!(
        en.contains("Test cache") && en.contains("Candidates"),
        "{en}"
    );
    assert!(
        ja.contains("テスト用キャッシュ") && ja.contains("削除候補"),
        "{ja}"
    );
}

#[test]
fn top_reads_the_snapshot() {
    let sb =
        Sandbox::new("[scan]\nroots = [\"~\"]\nsnapshot_min_mb = 0\n[worktrees]\nroots = []\n");
    sb.file("big/blob", 300_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let o = sb.roomy(&["top", "--json"]);
    let out: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(out["schema"], 1);
    let paths: Vec<&str> = out["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.iter().any(|p| Path::new(p).ends_with("big")),
        "{paths:?}"
    );
}

#[test]
fn dashboard_renders_at_any_size() {
    let sb = Sandbox::new(&format!(
        "{}\n[[rules]]\nid = \"test.cache\"\ntitle = \"Test cache\"\npaths = [\"~/Library/Caches/TestApp\"]\n\
         safety = \"safe\"\naction = \"delete\"\n",
        BASE.replace("[scan]\n", "[scan]\nsnapshot_min_mb = 0\n")
    ));
    sb.file("Library/Caches/TestApp/a", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    for size in ["120x36", "80x24", "40x10", "10x3"] {
        let o = sb.roomy(&["ui", "--print", size]);
        assert!(
            o.status.success(),
            "{size}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let big = stdout(&sb.roomy(&["ui", "--print", "120x36"]));
    assert!(
        big.contains("Mac health report") && big.contains("Test cache"),
        "{big}"
    );
    let folders = stdout(&sb.roomy(&["ui", "--print", "120x30", "--folders"]));
    assert!(folders.contains("Library"), "{folders}");
}

/// Every `--json` output is an object with `"schema": 1`, and its text is English whatever the
/// user's language, so scripts and agents can rely on it.
#[test]
fn json_outputs_are_versioned_and_language_independent() {
    let sb = Sandbox::new(&format!(
        "language = \"ja\"\n{BASE}\n[[rules]]\nid = \"test.cache\"\n\
         title = {{ en = \"Test cache\", ja = \"テスト用キャッシュ\" }}\n\
         paths = [\"~/Library/Caches/TestApp\"]\nsafety = \"safe\"\naction = \"delete\"\n"
    ));
    sb.file("Library/Caches/TestApp/a", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    assert!(sb.roomy(&["agent"]).status.success());
    for args in [
        &["status", "--json"][..],
        &["report", "--json"],
        &["top", "--json"],
        &["rules", "--json"],
        &["history", "--json"],
        &["doctor", "--json"],
    ] {
        let o = sb.roomy(args);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}\n{}", stdout(&o)));
        assert_eq!(v["schema"], 1, "{args:?}");
    }
    let r = report(&sb);
    let c = r["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "test.cache")
        .unwrap();
    assert_eq!(c["title"], "Test cache");
    // ... while the human output follows the configured language.
    assert!(stdout(&sb.roomy(&["report"])).contains("テスト用キャッシュ"));
}

#[test]
fn threshold_exit_code_is_distinct_from_usage_errors() {
    let sb = Sandbox::new("[thresholds]\ndisk_warn_gb = 1000000\n");
    assert_eq!(sb.roomy(&["status", "--exit-code"]).status.code(), Some(3));
    assert_eq!(sb.roomy(&["status"]).status.code(), Some(0));
    assert_eq!(sb.roomy(&["no-such-command"]).status.code(), Some(2));
}
