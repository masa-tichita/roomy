//! `roomy doctor`: everything that commonly goes wrong, checked in one place.

use crate::app;
use crate::config;
use crate::i18n::{Lang, Msg, m, mf, t};
use crate::model::Report;
use crate::output::print_json;
use crate::state;
use crate::sys::{self, Sty, fmt_time, which};
use serde::Serialize;

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Level {
    Ok,
    Warn,
    Fail,
}

struct Check {
    /// Stable identifier for `--json`.
    id: &'static str,
    label: &'static str,
    level: Level,
    detail: Msg,
}

fn check(id: &'static str, label: &'static str, level: Level, detail: Msg) -> Check {
    Check {
        id,
        label,
        level,
        detail,
    }
}

fn checks() -> Vec<Check> {
    let mut v = vec![];

    let loaded = config::load();
    let cfg_path = config::path().display().to_string();
    v.push(match (&loaded.error, loaded.exists) {
        (Some(e), _) => check(
            "config",
            "doctor.config",
            Level::Fail,
            mf("doctor.config_error", &[("path", &cfg_path), ("error", e)]),
        ),
        (None, false) => check(
            "config",
            "doctor.config",
            Level::Ok,
            mf("doctor.config_default", &[("path", &cfg_path)]),
        ),
        (None, true) if !loaded.unknown_keys.is_empty() => check(
            "config",
            "doctor.config",
            Level::Warn,
            mf(
                "doctor.config_unknown",
                &[("keys", &loaded.unknown_keys.join(", "))],
            ),
        ),
        (None, true) => check("config", "doctor.config", Level::Ok, Msg::plain(cfg_path)),
    });
    let rule_errors = crate::rules::validate(&crate::rules::all(&loaded.config));
    if !rule_errors.is_empty() {
        v.push(check(
            "rules",
            "doctor.rules",
            Level::Fail,
            Msg::plain(rule_errors.join("; ")),
        ));
    }

    let dir = app::state_dir();
    let probe = dir.join(".probe");
    let writable = std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    v.push(if writable {
        check(
            "state_dir",
            "doctor.state",
            Level::Ok,
            Msg::plain(dir.display().to_string()),
        )
    } else {
        check(
            "state_dir",
            "doctor.state",
            Level::Fail,
            mf("doctor.state_error", &[("path", &dir.display())]),
        )
    });

    v.push(if sys::has_full_disk_access() {
        check(
            "full_disk_access",
            "doctor.fda",
            Level::Ok,
            m("doctor.fda_yes"),
        )
    } else {
        check(
            "full_disk_access",
            "doctor.fda",
            Level::Warn,
            m("doctor.fda_no"),
        )
    });

    let tools = [
        "git", "docker", "xcrun", "npm", "pnpm", "uv", "brew", "mdls", "lsof",
    ];
    let (have, missing): (Vec<&str>, Vec<&str>) = tools.iter().partition(|c| which(c).is_some());
    let mut detail = have.join(" ");
    if !missing.is_empty() {
        let missing: Vec<String> = missing.iter().map(|m| format!("-{m}")).collect();
        detail = format!("{detail}  ({})", missing.join(" "));
    }
    v.push(check(
        "tools",
        "doctor.tools",
        Level::Ok,
        Msg::plain(detail),
    ));
    v.push(check(
        "notifications",
        "doctor.notify",
        Level::Ok,
        Msg::plain(sys::notifier()),
    ));

    let own = crate::service::is_loaded(app::AGENT_LABEL);
    let brew = crate::service::brew_loaded();
    let (level, detail) = match (own, brew) {
        (true, true) => (Level::Fail, m("service.both")),
        (false, false) => (Level::Warn, m("doctor.agent_off")),
        (true, false) => (Level::Ok, m("service.running_own")),
        (false, true) => (Level::Ok, m("service.running_brew")),
    };
    v.push(check("agent", "doctor.agent", level, detail));

    let samples = state::read_lines::<crate::agent::Sample>(&state::history_path());
    if let Some(t0) = samples.last().map(|x| x.t) {
        v.push(check(
            "last_agent_run",
            "doctor.last_agent",
            Level::Ok,
            Msg::plain(fmt_time(t0)),
        ));
    }
    v.push(match state::load::<Report>(&state::report_path()) {
        Some(r) => check(
            "last_scan",
            "doctor.last_scan",
            Level::Ok,
            Msg::plain(fmt_time(r.time)),
        ),
        None => check(
            "last_scan",
            "doctor.last_scan",
            Level::Warn,
            m("status.never_scanned"),
        ),
    });
    v
}

/// Returns the number of failures.
pub fn run(json: bool) -> usize {
    let checks = checks();
    let failures = checks.iter().filter(|c| c.level == Level::Fail).count();
    if json {
        let list: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| serde_json::json!({ "id": c.id, "level": c.level, "detail": c.detail.render_in(Lang::En) }))
            .collect();
        print_json(&serde_json::json!({
            "schema": app::SCHEMA,
            "version": app::VERSION,
            "macos": sys::os_version(),
            "arch": std::env::consts::ARCH,
            "checks": list,
        }));
        return failures;
    }
    let s = Sty::new();
    println!(
        "{} {}  macOS {} ({})",
        s.bold(app::NAME),
        app::VERSION,
        sys::os_version(),
        std::env::consts::ARCH
    );
    for c in &checks {
        let mark = match c.level {
            Level::Ok => s.green("✓"),
            Level::Warn => s.yellow("!"),
            Level::Fail => s.red("✗"),
        };
        println!(
            "  {mark} {}  {}",
            sys::pad(&t(c.label), 18),
            c.detail.render()
        );
    }
    failures
}
