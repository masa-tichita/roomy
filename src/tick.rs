//! What launchd runs every few minutes: record a sample, notify on thresholds, and run the daily
//! scan when it is due. Must be cheap when there is nothing to do.

use crate::config::{Config, gb};
use crate::i18n::{t, tf};
use crate::model::{Report, Safety};
use crate::state;
use crate::sys::{self, fmt_bytes};
use chrono::Timelike;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Default)]
struct AgentState {
    last_avail: Option<u64>,
    last_time: Option<i64>,
    /// alert key -> last time it was sent
    notified: BTreeMap<String, i64>,
    last_scan_attempt: Option<i64>,
    #[serde(default)]
    last_pressure: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Sample {
    pub t: i64,
    pub avail: u64,
    pub total: u64,
    pub pressure: i64,
    pub free_pct: Option<i64>,
    pub swap: u64,
}

pub struct Alert {
    pub key: &'static str,
    pub title: String,
    pub body: String,
    pub cooldown: i64,
    /// Sent even during quiet hours.
    pub urgent: bool,
}

const HOUR: i64 = 3600;

/// `prev_pressure`: the memory pressure at the previous check. A "warning" level must last two
/// checks in a row before it is reported; macOS reaches it briefly under normal load.
pub fn alerts(
    cfg: &Config,
    s: &Sample,
    prev: Option<(u64, i64)>,
    prev_pressure: Option<i64>,
    top: &[sys::AppMem],
) -> Vec<Alert> {
    let th = &cfg.thresholds;
    let mut v = vec![];
    let free = fmt_bytes(s.avail);
    if s.avail < gb(th.disk_critical_gb) {
        v.push(Alert {
            key: "disk_critical",
            title: t("tick.disk_critical"),
            body: tf("tick.disk_body", &[("free", &free)]),
            cooldown: HOUR,
            urgent: true,
        });
    } else if s.avail < gb(th.disk_warn_gb) {
        v.push(Alert {
            key: "disk_warn",
            title: t("tick.disk_warn"),
            body: tf("tick.disk_body", &[("free", &free)]),
            cooldown: 12 * HOUR,
            urgent: false,
        });
    }
    if let Some((last_avail, last_t)) = prev {
        let window = (i64::from(cfg.service.interval_min) * 60) * 2 + 60;
        let dropped = last_avail.saturating_sub(s.avail);
        if s.t - last_t <= window && dropped >= gb(th.disk_drop_gb) {
            let minutes = ((s.t - last_t) / 60).max(1);
            v.push(Alert {
                key: "disk_drop",
                title: t("tick.disk_drop"),
                body: tf(
                    "tick.drop_body",
                    &[
                        ("amount", &fmt_bytes(dropped)),
                        ("minutes", &minutes),
                        ("free", &free),
                    ],
                ),
                cooldown: HOUR,
                urgent: true,
            });
        }
    }
    let apps: String = top
        .iter()
        .take(3)
        .map(|a| format!("{} {}", a.name, sys::fmt_mem(a.rss)))
        .collect::<Vec<_>>()
        .join(", ");
    match s.pressure {
        4 => v.push(Alert {
            key: "mem_critical",
            title: t("tick.mem_critical"),
            body: tf("tick.mem_body", &[("apps", &apps)]),
            cooldown: HOUR / 2,
            urgent: true,
        }),
        2 if prev_pressure.is_some_and(|p| p >= 2) => v.push(Alert {
            key: "mem_warn",
            title: t("tick.mem_warn"),
            body: tf("tick.mem_body", &[("apps", &apps)]),
            cooldown: 2 * HOUR,
            urgent: false,
        }),
        _ => {}
    }
    if s.swap >= gb(th.swap_warn_gb) {
        v.push(Alert {
            key: "swap",
            title: t("tick.swap"),
            body: tf(
                "tick.swap_body",
                &[("swap", &sys::fmt_mem(s.swap)), ("apps", &apps)],
            ),
            cooldown: 6 * HOUR,
            urgent: false,
        });
    }
    if !cfg.service.memory_alerts {
        v.retain(|a| !matches!(a.key, "mem_warn" | "mem_critical" | "swap"));
    }
    v
}

pub fn in_quiet_hours(quiet: Option<[u32; 2]>, hour: u32) -> bool {
    match quiet {
        Some([start, end]) if start == end => false,
        Some([start, end]) if start < end => hour >= start && hour < end,
        Some([start, end]) => hour >= start || hour < end,
        None => false,
    }
}

fn scan_due(cfg: &Config, st: &AgentState, now: i64) -> bool {
    if !cfg.service.daily_scan {
        return false;
    }
    let last_report = state::load::<Report>(&state::report_path()).map_or(0, |r| r.time);
    let last_attempt = st.last_scan_attempt.unwrap_or(0);
    if now - last_report < 24 * HOUR || now - last_attempt < HOUR {
        return false;
    }
    let p = sys::power();
    p.on_ac
        || p.battery_pct
            .is_some_and(|b| b >= cfg.service.min_battery_pct)
}

pub fn daily_summary(cfg: &Config, r: &Report) -> Option<String> {
    let safe = r.total(Safety::Safe);
    let notable = !r.growth.is_empty()
        || safe >= gb(cfg.thresholds.safe_notify_gb)
        || r.disk_avail < gb(cfg.thresholds.disk_warn_gb);
    if !notable {
        return None;
    }
    let mut body = tf(
        "tick.daily_body",
        &[
            ("free", &fmt_bytes(r.disk_avail)),
            ("safe", &fmt_bytes(safe)),
        ],
    );
    if let Some(g) = r.growth.first() {
        body.push('\n');
        body.push_str(&tf(
            "tick.daily_growth",
            &[
                ("path", &sys::tilde(std::path::Path::new(&g.path))),
                ("delta", &sys::fmt_delta(g.delta)),
            ],
        ));
    }
    Some(body)
}

pub fn run(cfg: &Config) -> anyhow::Result<()> {
    state::ensure_log_dir();
    state::rotate_log(&state::log_path(), 1_000_000);
    let Some(_lock) = sys::try_lock(&state::lock_path()) else {
        println!("{} busy", sys::fmt_time(sys::now()));
        return Ok(());
    };
    let now = sys::now();
    let disk = sys::disk();
    let mem = sys::mem();
    let sample = Sample {
        t: now,
        avail: disk.avail,
        total: disk.total,
        pressure: mem.pressure,
        free_pct: mem.free_pct,
        swap: mem.swap_used,
    };
    state::append_line(&state::history_path(), &sample)?;
    state::trim_lines(&state::history_path(), 2_000_000);

    let mut st: AgentState = state::load(&state::agent_state_path()).unwrap_or_default();
    let prev = st.last_avail.zip(st.last_time);
    let top = if mem.pressure >= 2 || mem.swap_used >= gb(cfg.thresholds.swap_warn_gb) {
        sys::top_apps(3)
    } else {
        vec![]
    };
    let quiet = in_quiet_hours(cfg.service.quiet_hours, chrono::Local::now().hour());
    let muted = !cfg.service.notifications || sys::notifications_disabled();
    let mut sent = vec![];
    for a in alerts(cfg, &sample, prev, st.last_pressure, &top) {
        if muted || (quiet && !a.urgent) {
            continue;
        }
        if now - st.notified.get(a.key).copied().unwrap_or(0) >= a.cooldown {
            sys::notify(&a.title, &a.body);
            st.notified.insert(a.key.to_string(), now);
            sent.push(a.key);
        }
    }
    st.last_avail = Some(disk.avail);
    st.last_time = Some(now);
    st.last_pressure = Some(mem.pressure);

    let mut scanned = String::new();
    if scan_due(cfg, &st, now) {
        st.last_scan_attempt = Some(now);
        state::save(&state::agent_state_path(), &st)?;
        match crate::scan::run(cfg, true) {
            Ok(r) => {
                scanned = format!(
                    " scan={:.0}s candidates={}",
                    r.walk.seconds,
                    r.candidates.len()
                );
                if let Some(body) = daily_summary(cfg, &r)
                    && !quiet
                    && !muted
                    && now - st.notified.get("daily").copied().unwrap_or(0) >= 20 * HOUR
                {
                    sys::notify(&t("tick.daily_title"), &body);
                    st.notified.insert("daily".into(), now);
                    sent.push("daily");
                }
            }
            Err(e) => scanned = format!(" scan_error={e}"),
        }
    }
    state::save(&state::agent_state_path(), &st)?;
    println!(
        "{} free={} pressure={} swap={} notified={:?}{scanned}",
        sys::fmt_time(now),
        fmt_bytes(disk.avail),
        mem.pressure,
        fmt_bytes(mem.swap_used),
        sent
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(avail_gb: u64) -> Sample {
        Sample {
            t: 10_000,
            avail: avail_gb * 1_000_000_000,
            total: 0,
            pressure: 1,
            free_pct: None,
            swap: 0,
        }
    }

    fn keys(v: &[Alert]) -> Vec<&str> {
        v.iter().map(|a| a.key).collect()
    }

    #[test]
    fn disk_levels() {
        let cfg = Config::default();
        assert!(alerts(&cfg, &sample(100), None, None, &[]).is_empty());
        assert_eq!(
            keys(&alerts(&cfg, &sample(40), None, None, &[])),
            ["disk_warn"]
        );
        assert_eq!(
            keys(&alerts(&cfg, &sample(10), None, None, &[])),
            ["disk_critical"]
        );
    }

    #[test]
    fn sudden_drop_only_within_window() {
        let cfg = Config::default();
        let now = sample(100);
        let recent = Some((115 * 1_000_000_000, now.t - 900));
        assert_eq!(keys(&alerts(&cfg, &now, recent, None, &[])), ["disk_drop"]);
        let long_ago = Some((115 * 1_000_000_000, now.t - 86_400));
        assert!(alerts(&cfg, &now, long_ago, None, &[]).is_empty());
    }

    #[test]
    fn memory_and_swap() {
        let cfg = Config::default();
        let mut s = sample(100);
        s.pressure = 4;
        s.swap = 5_000_000_000;
        assert_eq!(
            keys(&alerts(&cfg, &s, None, None, &[])),
            ["mem_critical", "swap"]
        );
    }

    #[test]
    fn memory_warning_must_last_two_checks() {
        let cfg = Config::default();
        let mut s = sample(100);
        s.pressure = 2;
        assert!(
            alerts(&cfg, &s, None, None, &[]).is_empty(),
            "first warning sample: wait"
        );
        assert!(alerts(&cfg, &s, None, Some(1), &[]).is_empty());
        assert_eq!(keys(&alerts(&cfg, &s, None, Some(2), &[])), ["mem_warn"]);
    }

    #[test]
    fn memory_alerts_can_be_turned_off() {
        let mut cfg = Config::default();
        cfg.service.memory_alerts = false;
        let mut s = sample(10);
        s.pressure = 4;
        s.swap = 9_000_000_000;
        assert_eq!(keys(&alerts(&cfg, &s, None, None, &[])), ["disk_critical"]);
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        assert!(in_quiet_hours(Some([23, 7]), 2));
        assert!(in_quiet_hours(Some([23, 7]), 23));
        assert!(!in_quiet_hours(Some([23, 7]), 12));
        assert!(in_quiet_hours(Some([1, 5]), 3));
        assert!(!in_quiet_hours(Some([1, 5]), 5));
        assert!(!in_quiet_hours(None, 3));
    }
}
