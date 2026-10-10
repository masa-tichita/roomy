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

    /// The binary with an English environment that points only into the sandbox.
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_roomy"));
        cmd.args(args)
            .env("HOME", &self.home)
            .env("LANG", "en_US.UTF-8")
            .env_remove("LC_ALL") // set on CI runners, and it outranks LANG
            .env_remove("LC_MESSAGES")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("ROOMY_CONFIG_DIR")
            .env_remove("ROOMY_STATE_DIR")
            .env("NO_COLOR", "1");
        cmd
    }

    fn roomy(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
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

/// Scans stay inside the sandbox: only `~` is walked (so the apps detector leaves /Applications
/// alone), and Docker, which would answer from the machine running the tests, is off.
const BASE: &str = r#"
disabled_detectors = ["docker"]
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
fn disabled_detectors_find_nothing_and_clean_nothing() {
    let sb = Sandbox::new(BASE);
    let blob = sb.file("Library/Caches/BigApp/blob", 100_000);
    assert!(sb.roomy(&["scan"]).status.success());
    assert!(ids(&report(&sb)).contains(&"cache.app:BigApp".to_string()));

    // Turned off after the scan: the stale candidate is refused at re-check time.
    fs::write(
        sb.home.join(".config/roomy/config.toml"),
        BASE.replace("[\"docker\"]", "[\"docker\", \"cache\", \"dokcer\"]"),
    )
    .unwrap();
    let o = sb.roomy(&["clean", "cache.app:BigApp", "--yes"]);
    assert!(!o.status.success());
    assert!(
        stdout(&o).contains("detector is disabled"),
        "{}",
        stdout(&o)
    );
    assert!(blob.exists());
    let o = sb.roomy(&["config", "validate"]);
    assert!(
        stdout(&o).contains("disabled_detectors.dokcer"),
        "{}",
        stdout(&o)
    );

    assert!(sb.roomy(&["scan"]).status.success());
    assert!(!ids(&report(&sb)).contains(&"cache.app:BigApp".to_string()));
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
fn tick_records_history_and_runs_the_daily_scan() {
    let sb = Sandbox::new(BASE);
    let o = sb.roomy(&["tick"]);
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
        r["skipped_protected"], true,
        "no Full Disk Access in the sandbox"
    );

    let o = sb.roomy(&["tick"]);
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
    let ja = sb
        .command(&["report"])
        .env("LANG", "ja_JP.UTF-8")
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
    let sb = Sandbox::new(&BASE.replace("[scan]\n", "[scan]\nsnapshot_min_mb = 0\n"));
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
    assert!(sb.roomy(&["tick"]).status.success());
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

const TWO_RULES: &str = "\n[[rules]]\nid = \"test.safe\"\ntitle = \"Safe cache\"\npaths = [\"~/Library/Caches/SafeApp\"]\n\
target = \"children\"\nsafety = \"safe\"\naction = \"delete\"\n\
[[rules]]\nid = \"test.review\"\ntitle = \"Review cache\"\npaths = [\"~/Library/Caches/ReviewApp\"]\n\
target = \"children\"\nsafety = \"review\"\naction = \"delete\"\n";

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}{}", stdout(o), String::from_utf8_lossy(&o.stderr)))
}

/// The workflow from the agent guide: report -> explain -> clean (dry run) -> clean --yes.
#[test]
fn agent_workflow_over_json() {
    let sb = Sandbox::new(&format!("{BASE}{TWO_RULES}"));
    for i in 0..30 {
        sb.file(&format!("Library/Caches/SafeApp/f{i}"), 10_000);
    }
    sb.file("Library/Caches/ReviewApp/x", 10_000);
    assert!(sb.roomy(&["scan"]).status.success());

    // the summary stays small and has no path lists
    let o = sb.roomy(&["report", "--json"]);
    assert!(
        o.stdout.len() < 8_000,
        "summary is {} bytes",
        o.stdout.len()
    );
    let r = json(&o);
    let safe = r["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "test.safe")
        .unwrap();
    assert_eq!(safe["action"]["kind"], "delete");
    assert_eq!(safe["action"]["items"], 30);
    assert!(safe.get("paths").is_none());

    // explain lists paths, capped
    let e = json(&sb.roomy(&["explain", "test.safe", "--json", "--max-paths", "5"]));
    assert_eq!(e["paths"].as_array().unwrap().len(), 5);
    assert_eq!(e["paths_total"], 30);

    // --json without --yes is a dry run; review needs --allow-review
    let p = json(&sb.roomy(&["clean", "test.safe", "test.review", "--json"]));
    assert_eq!(p["dry_run"], true);
    assert_eq!(p["plan"].as_array().unwrap().len(), 1);
    assert_eq!(p["refused"][0]["id"], "test.review");
    assert!(sb.home.join("Library/Caches/SafeApp/f0").exists());

    let done = json(&sb.roomy(&["clean", "test.safe", "--json", "--yes"]));
    assert_eq!(done["dry_run"], false);
    assert_eq!(done["results"][0]["ok"], true);
    assert!(!sb.home.join("Library/Caches/SafeApp/f0").exists());
    assert!(sb.home.join("Library/Caches/ReviewApp/x").exists());

    let done = json(&sb.roomy(&["clean", "test.review", "--json", "--yes", "--allow-review"]));
    assert_eq!(done["results"][0]["ok"], true);
    assert!(!sb.home.join("Library/Caches/ReviewApp/x").exists());

    let audit = fs::read_to_string(
        sb.home
            .join("Library/Application Support/roomy/audit.jsonl"),
    )
    .unwrap();
    assert!(audit.contains("\"unattended\":true"));
}

#[test]
fn unattended_removal_is_capped() {
    let sb = Sandbox::new(&format!(
        "{BASE}\n[clean]\nmax_unattended_gb = 0.0001\n{TWO_RULES}"
    ));
    sb.file("Library/Caches/SafeApp/big", 1_000_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let o = sb.roomy(&["clean", "test.safe", "--json", "--yes"]);
    assert!(!o.status.success());
    let p = json(&o);
    assert!(
        p["refused"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("unattended limit"),
        "{p}"
    );
    assert!(sb.home.join("Library/Caches/SafeApp/big").exists());
}

#[test]
fn json_errors_are_json() {
    let sb = Sandbox::new(BASE);
    let o = sb.roomy(&["report", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let v = json(&o);
    assert!(v["error"].as_str().unwrap().contains("roomy scan"), "{v}");
}

fn mcp(sb: &Sandbox, args: &[&str], lines: &[&str]) -> Vec<serde_json::Value> {
    use std::io::Write;
    let mut child = sb
        .command(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for l in lines {
        writeln!(stdin, "{l}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {l}"))
        })
        .collect()
}

#[test]
fn mcp_server_lists_tools_and_cleans_with_the_same_rules() {
    let sb = Sandbox::new(&format!("{BASE}{TWO_RULES}"));
    sb.file("Library/Caches/SafeApp/a", 10_000);
    sb.file("Library/Caches/ReviewApp/a", 10_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let call = |id: u32, name: &str, args: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
        )
    };
    let replies = mcp(
        &sb,
        &["mcp"],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            &call(3, "roomy_report", "{}"),
            &call(
                4,
                "roomy_plan_clean",
                r#"{"ids":["test.safe","test.review"]}"#,
            ),
            &call(5, "roomy_clean", r#"{"ids":["test.review"]}"#),
            &call(6, "roomy_clean", r#"{"ids":["test.safe"]}"#),
        ],
    );
    assert_eq!(
        replies.len(),
        6,
        "one reply per request, none for the notification"
    );
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    let tools: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"roomy_clean") && tools.contains(&"roomy_scan"));
    assert_eq!(replies[2]["result"]["structuredContent"]["schema"], 1);
    let plan = &replies[3]["result"]["structuredContent"];
    assert_eq!(plan["plan"].as_array().unwrap().len(), 1);
    assert_eq!(plan["refused"][0]["id"], "test.review");
    // review without allow_review: refused, nothing removed
    assert!(sb.home.join("Library/Caches/ReviewApp/a").exists());
    assert_eq!(replies[4]["result"]["structuredContent"]["dry_run"], true);
    // safe: removed
    assert_eq!(
        replies[5]["result"]["structuredContent"]["results"][0]["ok"],
        true
    );
    assert!(!sb.home.join("Library/Caches/SafeApp/a").exists());
}

#[test]
fn mcp_read_only_cannot_clean() {
    let sb = Sandbox::new(&format!("{BASE}{TWO_RULES}"));
    sb.file("Library/Caches/SafeApp/a", 10_000);
    assert!(sb.roomy(&["scan"]).status.success());
    let replies = mcp(
        &sb,
        &["mcp", "--read-only"],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"roomy_clean","arguments":{"ids":["test.safe"]}}}"#,
        ],
    );
    let tools = replies[0]["result"]["tools"].as_array().unwrap();
    assert!(!tools.iter().any(|t| t["name"] == "roomy_clean"));
    assert_eq!(replies[1]["result"]["isError"], true);
    assert!(sb.home.join("Library/Caches/SafeApp/a").exists());
}

#[test]
fn guide_formats_and_skill_install() {
    let sb = Sandbox::new(BASE);
    assert!(stdout(&sb.roomy(&["guide"])).starts_with("# roomy — guide for AI agents"));
    assert!(stdout(&sb.roomy(&["guide", "--format", "skill"])).starts_with("---\nname: roomy"));
    assert!(stdout(&sb.roomy(&["guide", "--format", "agents-md"])).contains("roomy clean"));
    assert!(
        sb.roomy(&["guide", "--install-claude-skill"])
            .status
            .success()
    );
    assert!(sb.home.join(".claude/skills/roomy/SKILL.md").exists());
    assert!(
        !sb.roomy(&["guide", "--install-claude-skill"])
            .status
            .success(),
        "keeps an existing skill"
    );
}
