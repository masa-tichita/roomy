//! `roomy doctor`: everything that commonly goes wrong, checked in one place.

use crate::app;
use crate::config;
use crate::i18n::{self, Lang, Msg, m, mf, t, t_in};
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

    let own = crate::service::is_loaded(app::SERVICE_LABEL);
    let brew = crate::service::brew_loaded();
    let (level, detail) = match (own, brew) {
        (true, true) => (Level::Fail, m("service.both")),
        (false, false) => (Level::Warn, m("doctor.agent_off")),
        (true, false) => (Level::Ok, m("service.running_own")),
        (false, true) => (Level::Ok, m("service.running_brew")),
    };
    v.push(check("service", "doctor.agent", level, detail));

    let samples = state::read_lines::<crate::tick::Sample>(&state::history_path());
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

/// Checks as JSON (English), and the number of failures.
pub fn json() -> (serde_json::Value, usize) {
    let checks = checks();
    let failures = checks.iter().filter(|c| c.level == Level::Fail).count();
    let list: Vec<serde_json::Value> = checks
        .iter()
        .map(|c| serde_json::json!({ "id": c.id, "level": c.level, "detail": c.detail.render_in(Lang::En) }))
        .collect();
    let v = serde_json::json!({
        "schema": app::SCHEMA,
        "version": app::VERSION,
        "macos": sys::os_version(),
        "arch": std::env::consts::ARCH,
        "checks": list,
    });
    (v, failures)
}

/// Returns the number of failures.
pub fn run(json: bool) -> usize {
    if json {
        let (v, failures) = self::json();
        print_json(&v);
        return failures;
    }
    let checks = checks();
    let failures = checks.iter().filter(|c| c.level == Level::Fail).count();
    let s = Sty::new();
    println!(
        "{} {}  macOS {} ({})",
        s.bold(app::NAME),
        app::VERSION,
        sys::os_version(),
        std::env::consts::ARCH
    );
    let width = label_width(&checks, i18n::lang());
    for c in &checks {
        let mark = match c.level {
            Level::Ok => s.green("✓"),
            Level::Warn => s.yellow("!"),
            Level::Fail => s.red("✗"),
        };
        println!(
            "  {mark} {}  {}",
            sys::pad(&t(c.label), width),
            c.detail.render()
        );
    }
    failures
}

/// The label column fits the widest label in the reader's language ("フルディスクアクセス" is
/// 20 columns, "Background service" 18).
fn label_width(checks: &[Check], lang: Lang) -> usize {
    sys::column_width(checks.iter().map(|c| t_in(lang, c.label)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_labels() -> Vec<Check> {
        [
            "doctor.config",
            "doctor.rules",
            "doctor.state",
            "doctor.fda",
            "doctor.tools",
            "doctor.notify",
            "doctor.agent",
            "doctor.last_agent",
            "doctor.last_scan",
        ]
        .into_iter()
        .map(|label| check("x", label, Level::Ok, Msg::plain("")))
        .collect()
    }

    #[test]
    fn label_column_fits_the_widest_label() {
        let checks = all_labels();
        assert_eq!(label_width(&checks, Lang::Ja), 20, "フルディスクアクセス");
        assert_eq!(label_width(&checks, Lang::En), 18, "Background service");
        for lang in [Lang::En, Lang::Ja] {
            let w = label_width(&checks, lang);
            for c in &checks {
                assert_eq!(sys::width(&sys::pad(&t_in(lang, c.label), w)), w);
            }
        }
    }
}
