//! Human-readable output for status / report / top / history / rules.

use crate::config::Config;
use crate::i18n::{t, tf};
use crate::model::{Action, Report, Safety};
use crate::sys::{self, Sty, fmt_bytes, fmt_delta, fmt_time, pad, pad_left, tilde};
use crate::walk::Snapshot;
use serde::Serialize;
use std::path::Path;

/// Our output types always serialize; a failure would be a bug worth a clear message.
/// Indented for people at a terminal, compact for programs (fewer tokens for AI agents).
pub fn print_json<T: Serialize>(v: &T) {
    use std::io::IsTerminal;
    let text = if std::io::stdout().is_terminal() {
        serde_json::to_string_pretty(v)
    } else {
        serde_json::to_string(v)
    };
    match text {
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

/// Returns true when something is over a threshold (for `status --exit-code`).
pub fn status(cfg: &Config, json: bool) -> bool {
    let disk = sys::disk();
    let mem = sys::mem();
    let apps = sys::top_apps(5);
    let report = crate::state::load::<Report>(&crate::state::report_path());
    let alarming = disk.avail < crate::config::gb(cfg.thresholds.disk_warn_gb) || mem.pressure >= 2;
    if json {
        print_json(&crate::api::status(cfg).0);
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
        print_json(&crate::api::top(snap, prev, dir, n));
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

pub fn history(samples: &[crate::tick::Sample], days: usize, json: bool) {
    if json {
        print_json(&crate::api::history(samples, days));
        return;
    }
    let s = Sty::new();
    if samples.is_empty() {
        println!("{}", t("history.empty"));
        return;
    }
    println!("{}", s.bold(&t("history.title")));
    for (day, min_free, total) in crate::api::daily_minimum(samples, days) {
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
        print_json(&crate::api::rules(cfg));
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

/// `roomy explain` for people.
pub fn explain(r: &Report, target: &str, max_paths: usize) {
    let s = Sty::new();
    let Some(i) = crate::api::find(r, target) else {
        return;
    };
    let c = &r.candidates[i];
    println!(
        "{}  {}",
        s.bold(&c.title.render()),
        s.dim(&format!("#{} · {}", i + 1, c.id))
    );
    println!(
        "{}  {}  {}",
        fmt_bytes(c.bytes),
        safety_label(&s, c.safety),
        s.dim(&c.category.render())
    );
    println!("{}", s.dim(&c.path));
    for e in &c.evidence {
        println!("  {} {}", s.green("✓"), e.render());
    }
    for n in &c.notes {
        println!("  {} {}", s.yellow("•"), n.render());
    }
    println!("  → {}", crate::clean::describe(&c.action, false));
    match &c.action {
        Action::Paths { paths, .. } => {
            for p in paths.iter().take(max_paths) {
                println!("    {}", s.dim(&tilde(Path::new(p))));
            }
            if paths.len() > max_paths {
                println!(
                    "    {}",
                    s.dim(&tf("common.and_more", &[("n", &(paths.len() - max_paths))]))
                );
            }
        }
        Action::Command { argv, .. } => println!("    {}", s.dim(&argv.join(" "))),
        Action::Manual { hint } => println!("    {}", s.dim(&hint.render())),
    }
}
