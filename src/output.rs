//! Human-readable output for status / report / top / history / rules.

use crate::config::Config;
use crate::i18n::{t, tf};
use crate::model::{Action, Report, Safety};
use crate::sys::{self, Sty, fmt_bytes, fmt_delta, fmt_time, pad, pad_left, tilde};
use crate::walk::Snapshot;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Our output types always serialize; a failure would be a bug worth a clear message.
pub fn print_json<T: Serialize>(v: &T) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("roomy: cannot serialize output: {e}"),
    }
}

fn bar(used: u64, total: u64, width: usize) -> String {
    if total == 0 {
        return String::new();
    }
    let filled = ((used as f64 / total as f64) * width as f64).round() as usize;
    format!(
        "[{}{}]",
        "■".repeat(filled.min(width)),
        "·".repeat(width.saturating_sub(filled))
    )
}

fn safety_label(s: &Sty, safety: Safety) -> String {
    match safety {
        Safety::Safe => s.green(&pad(&t("safety.safe"), 8)),
        Safety::Review => s.yellow(&pad(&t("safety.review"), 8)),
    }
}

#[derive(Serialize)]
pub struct AppJson {
    name: String,
    rss: u64,
    processes: usize,
}

#[derive(Serialize)]
pub struct StatusJson {
    schema: u32,
    disk_total: u64,
    disk_avail: u64,
    memory_total: u64,
    memory_free_pct: Option<i64>,
    memory_pressure: i64,
    swap_used: u64,
    top_apps: Vec<AppJson>,
    last_scan: Option<i64>,
    safe_bytes: Option<u64>,
    review_bytes: Option<u64>,
}

/// Returns true when something is over a threshold (for `status --exit-code`).
pub fn status(cfg: &Config, json: bool) -> bool {
    let disk = sys::disk();
    let mem = sys::mem();
    let apps = sys::top_apps(5);
    let report = crate::state::load::<Report>(&crate::state::report_path());
    let alarming = disk.avail < crate::config::gb(cfg.thresholds.disk_warn_gb) || mem.pressure >= 2;
    if json {
        let j = StatusJson {
            schema: crate::app::SCHEMA,
            disk_total: disk.total,
            disk_avail: disk.avail,
            memory_total: mem.total,
            memory_free_pct: mem.free_pct,
            memory_pressure: mem.pressure,
            swap_used: mem.swap_used,
            top_apps: apps
                .iter()
                .map(|a| AppJson {
                    name: a.name.clone(),
                    rss: a.rss,
                    processes: a.procs,
                })
                .collect(),
            last_scan: report.as_ref().map(|r| r.time),
            safe_bytes: report.as_ref().map(|r| r.total(Safety::Safe)),
            review_bytes: report.as_ref().map(|r| r.total(Safety::Review)),
        };
        print_json(&j);
        return alarming;
    }
    let s = Sty::new();
    let used = disk.total.saturating_sub(disk.avail);
    let pct = (used * 100).checked_div(disk.total).unwrap_or(0);
    let free = fmt_bytes(disk.avail);
    let free = if disk.avail < crate::config::gb(cfg.thresholds.disk_critical_gb) {
        s.red(&free)
    } else if disk.avail < crate::config::gb(cfg.thresholds.disk_warn_gb) {
        s.yellow(&free)
    } else {
        s.green(&free)
    };
    let w = 10;
    println!(
        "{} {}",
        s.bold(crate::app::NAME),
        s.dim(crate::app::VERSION)
    );
    println!(
        "{} {} {}",
        pad(&t("status.disk"), w),
        bar(used, disk.total, 20),
        tf(
            "status.disk_line",
            &[
                ("free", &free),
                ("total", &fmt_bytes(disk.total)),
                ("pct", &pct)
            ]
        )
    );
    let pressure = sys::pressure_label_i18n(mem.pressure);
    let pressure = match mem.pressure {
        4 => s.red(&pressure),
        2 => s.yellow(&pressure),
        _ => pressure,
    };
    println!(
        "{} {}",
        pad(&t("status.memory"), w),
        tf(
            "status.memory_line",
            &[
                ("total", &sys::fmt_mem(mem.total)),
                (
                    "free",
                    &mem.free_pct.map_or("?".into(), |p| format!("{p}%"))
                ),
                ("pressure", &pressure),
                ("swap", &sys::fmt_mem(mem.swap_used)),
            ]
        )
    );
    let top: Vec<String> = apps
        .iter()
        .map(|a| format!("{} {}", a.name, sys::fmt_mem(a.rss)))
        .collect();
    println!("{} {}", pad(&t("status.top"), w), top.join(", "));
    match &report {
        Some(r) => println!(
            "{} {}",
            pad(&t("status.last_scan"), w),
            tf(
                "status.last_scan_line",
                &[
                    ("time", &fmt_time(r.time)),
                    ("safe", &fmt_bytes(r.total(Safety::Safe))),
                    ("review", &fmt_bytes(r.total(Safety::Review)))
                ]
            )
        ),
        None => println!(
            "{} {}",
            pad(&t("status.last_scan"), w),
            t("status.never_scanned")
        ),
    }
    println!(
        "{} {}",
        pad(&t("status.agent"), w),
        crate::service::status_line()
    );
    alarming
}

pub fn report(r: &Report, verbose: bool) {
    let s = Sty::new();
    println!(
        "{}  {}",
        s.bold(&tf("report.header", &[("time", &fmt_time(r.time))])),
        s.dim(&tf(
            "report.walk",
            &[("secs", &format!("{:.0}", r.walk.seconds))]
        ))
    );
    println!(
        "{}",
        tf(
            "report.free",
            &[
                ("free", &fmt_bytes(r.disk_avail)),
                ("total", &fmt_bytes(r.disk_total))
            ]
        )
    );
    println!();
    println!(
        "{}  {}",
        s.bold(&t("report.candidates")),
        tf(
            "report.totals",
            &[
                ("safe", &s.green(&fmt_bytes(r.total(Safety::Safe)))),
                ("review", &s.yellow(&fmt_bytes(r.total(Safety::Review))))
            ]
        )
    );
    if r.candidates.is_empty() {
        println!("  {}", t("report.none"));
    }
    let catw = r
        .candidates
        .iter()
        .map(|c| unicode_width::UnicodeWidthStr::width(c.category.render().as_str()))
        .max()
        .unwrap_or(0)
        .min(14);
    for (i, c) in r.candidates.iter().enumerate() {
        println!(
            "  {}  {}  {}  {}  {}",
            pad_left(&(i + 1).to_string(), 2),
            pad_left(&fmt_bytes(c.bytes), 8),
            safety_label(&s, c.safety),
            s.dim(&pad(&c.category.render(), catw)),
            c.title
        );
        let indent = " ".repeat(26 + catw);
        if !c.evidence.is_empty() {
            println!(
                "{indent}{}",
                s.dim(
                    &c.evidence
                        .iter()
                        .map(super::i18n::Msg::render)
                        .collect::<Vec<_>>()
                        .join(" · ")
                )
            );
        }
        for n in &c.notes {
            println!("{indent}{}", s.dim(&n.render()));
        }
        if verbose {
            println!("{indent}{}", s.dim(&format!("{}  [{}]", c.path, c.id)));
        }
        if let Action::Manual { hint } = &c.action {
            println!("{indent}{}", s.dim(&format!("→ {hint}")));
        }
    }
    if !r.growth.is_empty() {
        println!();
        let since = r.growth_since.map(fmt_time).unwrap_or_default();
        println!("{}", s.bold(&tf("report.growth", &[("since", &since)])));
        for g in &r.growth {
            println!(
                "  {}  {}  {}",
                s.yellow(&pad_left(&fmt_delta(g.delta), 10)),
                tilde(Path::new(&g.path)),
                s.dim(&tf("report.now", &[("size", &fmt_bytes(g.now))]))
            );
        }
    }
    if !r.excluded.is_empty() {
        println!();
        if verbose {
            println!("{}", s.bold(&t("report.excluded")));
            for e in &r.excluded {
                println!("  {}  {}", e.path, s.dim(&e.reason.render()));
            }
        } else {
            println!(
                "{}",
                s.dim(&tf("report.excluded_count", &[("n", &r.excluded.len())]))
            );
        }
    }
    if r.walk.skipped_protected {
        println!("{}", s.dim(&t("report.skipped_protected")));
    }
    if r.walk.unreadable > 0 {
        println!(
            "{}",
            s.dim(&tf("report.unreadable", &[("n", &r.walk.unreadable)]))
        );
    }
    if !r.candidates.is_empty() {
        println!();
        println!("{}", s.dim(&t("report.next")));
    }
}

pub fn top(snap: &Snapshot, prev: Option<&Snapshot>, dir: &Path, n: usize, json: bool) {
    let key = dir.to_string_lossy();
    let mut kids: Vec<(&String, u64)> = snap
        .entries
        .iter()
        .filter(|(p, _)| {
            Path::new(p.as_str())
                .parent()
                .is_some_and(|par| par.to_string_lossy() == key)
        })
        .map(|(p, b)| (p, *b))
        .collect();
    kids.sort_by_key(|x| std::cmp::Reverse(x.1));
    kids.truncate(n);
    let delta =
        |p: &str, now: u64| prev.and_then(|pv| pv.entries.get(p).map(|b| now as i64 - *b as i64));
    if json {
        #[derive(Serialize)]
        struct Row<'a> {
            path: &'a str,
            bytes: u64,
            delta: Option<i64>,
        }
        let rows: Vec<Row> = kids
            .iter()
            .map(|(p, b)| Row {
                path: p,
                bytes: *b,
                delta: delta(p, *b),
            })
            .collect();
        print_json(&serde_json::json!({
            "schema": crate::app::SCHEMA,
            "path": key,
            "time": snap.time,
            "bytes": snap.entries.get(&*key),
            "folders": rows,
        }));
        return;
    }
    let s = Sty::new();
    let total = snap.entries.get(&*key).copied();
    println!(
        "{}  {}",
        s.bold(&tilde(dir)),
        s.dim(&format!(
            "{} · {}",
            total.map(fmt_bytes).unwrap_or_default(),
            tf("top.as_of", &[("time", &fmt_time(snap.time))])
        ))
    );
    if kids.is_empty() {
        println!("  {}", t("top.empty"));
    }
    for (p, b) in kids {
        let d = match delta(p, b) {
            Some(d) if d.unsigned_abs() >= 100_000_000 => s.yellow(&fmt_delta(d)),
            None if prev.is_some() => s.dim(&t("top.new")),
            _ => String::new(),
        };
        let name = Path::new(p.as_str())
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "  {}  {}  {}",
            pad_left(&fmt_bytes(b), 9),
            pad(&name, 40),
            d
        );
    }
}

pub fn history(samples: &[crate::agent::Sample], days: usize, json: bool) {
    let mut by_day: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for x in samples {
        let day = fmt_time(x.t).chars().take(10).collect::<String>();
        let e = by_day.entry(day).or_insert((u64::MAX, x.total));
        e.0 = e.0.min(x.avail);
    }
    let rows: Vec<_> = by_day.into_iter().rev().take(days).collect();
    if json {
        let days: Vec<serde_json::Value> = rows
            .iter()
            .rev()
            .map(|(d, (min_free, total))| serde_json::json!({ "date": d, "min_free": min_free, "total": total }))
            .collect();
        print_json(&serde_json::json!({ "schema": crate::app::SCHEMA, "days": days }));
        return;
    }
    let s = Sty::new();
    if samples.is_empty() {
        println!("{}", t("history.empty"));
        return;
    }
    println!("{}", s.bold(&t("history.title")));
    for (day, (min_free, total)) in rows.into_iter().rev() {
        println!(
            "  {day}  {}  {}",
            pad_left(&fmt_bytes(min_free), 9),
            bar(total.saturating_sub(min_free), total, 30)
        );
    }
}

pub fn rules(cfg: &Config, json: bool) {
    let rules = crate::rules::all(cfg);
    if json {
        use crate::i18n::Lang::En;
        let list: Vec<serde_json::Value> = rules
            .iter()
            .map(|r| {
                let ov = cfg.rule_overrides.get(&r.id);
                serde_json::json!({
                    "id": r.id,
                    "enabled": !cfg.disabled_rules.contains(&r.id),
                    "title": r.title.get_in(En),
                    "paths": r.paths,
                    "target": r.target,
                    "safety": ov.and_then(|o| o.safety).unwrap_or(r.safety),
                    "action": r.action,
                    "command": r.command,
                    "builtin": crate::rules::builtin().iter().any(|b| b.id == r.id),
                })
            })
            .collect();
        print_json(&serde_json::json!({ "schema": crate::app::SCHEMA, "rules": list }));
        return;
    }
    let s = Sty::new();
    for r in &rules {
        let off = cfg.disabled_rules.contains(&r.id);
        let safety = cfg
            .rule_overrides
            .get(&r.id)
            .and_then(|o| o.safety)
            .unwrap_or(r.safety);
        let line = format!(
            "{}  {}  {}",
            pad(&r.id, 26),
            safety_label(&s, safety),
            r.title.get()
        );
        println!(
            "{}",
            if off {
                s.dim(&format!("{line}  ({})", t("rules.disabled")))
            } else {
                line
            }
        );
        println!(
            "{}",
            s.dim(&format!("{}{}", " ".repeat(38), r.paths.join(", ")))
        );
    }
    let errs = crate::rules::validate(&rules);
    for e in errs {
        println!("{}", s.red(&e));
    }
}

/// Report for `--json`: same shape as report.json, with every message rendered to text.
pub fn report_json(r: &Report) -> serde_json::Value {
    use crate::i18n::Lang::En;
    let render_all = |v: &[crate::i18n::Msg]| -> serde_json::Value {
        v.iter().map(|m| m.render_in(En)).collect::<Vec<_>>().into()
    };
    let mut v = serde_json::to_value(r).unwrap_or_default();
    if let Some(list) = v["candidates"].as_array_mut() {
        for (j, c) in list.iter_mut().zip(&r.candidates) {
            j["title"] = c.title.render_in(En).into();
            j["category"] = c.category.render_in(En).into();
            j["evidence"] = render_all(&c.evidence);
            j["notes"] = render_all(&c.notes);
            if let Action::Manual { hint } = &c.action {
                j["action"]["hint"] = hint.render_in(En).into();
            }
        }
    }
    if let Some(list) = v["excluded"].as_array_mut() {
        for (j, e) in list.iter_mut().zip(&r.excluded) {
            j["reason"] = e.reason.render_in(En).into();
        }
    }
    v
}
