//! The machine-facing layer. Every result as a JSON value with `"schema": 1` and English text.
//! Used by the CLI's `--json` and by the MCP server, so both always agree.
//!
//! Outputs are sized for AI agents: summaries first, details on request (`explain`).

use crate::clean::{Outcome, Plan};
use crate::config::Config;
use crate::i18n::{Lang, Msg};
use crate::model::{Action, Candidate, Method, Report, Safety};
use crate::sys::{self, fmt_bytes, tilde};
use crate::walk::Snapshot;
use serde_json::{Value, json};
use std::path::Path;

const EN: Lang = Lang::En;

fn en(m: &Msg) -> String {
    m.render_in(EN)
}

fn en_all(v: &[Msg]) -> Vec<String> {
    v.iter().map(en).collect()
}

pub fn error(message: &str) -> Value {
    json!({ "schema": crate::app::SCHEMA, "error": message })
}

// ---- status ----

/// Returns the JSON and whether something is over a threshold.
pub fn status(cfg: &Config) -> (Value, bool) {
    let disk = sys::disk();
    let mem = sys::mem();
    let apps = sys::top_apps(5);
    let report = crate::state::load::<Report>(&crate::state::report_path());
    let disk_low = disk.avail < crate::config::gb(cfg.thresholds.disk_warn_gb);
    let alarming = disk_low || mem.pressure >= 2;
    let v = json!({
        "schema": crate::app::SCHEMA,
        "disk": {
            "total": disk.total,
            "free": disk.avail,
            "free_text": fmt_bytes(disk.avail),
            "available_incl_purgeable": disk.avail_incl_purgeable,
            "available_incl_purgeable_text": disk.avail_incl_purgeable.map(fmt_bytes),
            "low": disk_low,
        },
        "memory": {
            "total": mem.total,
            "free_pct": mem.free_pct,
            "pressure": match mem.pressure { 1 => "normal", 2 => "warning", 4 => "critical", _ => "unknown" },
            "swap_used": mem.swap_used,
        },
        "top_apps": apps.iter().map(|a| json!({ "name": a.name, "rss": a.rss, "processes": a.procs })).collect::<Vec<_>>(),
        "last_scan": report.as_ref().map(|r| json!({
            "time": r.time,
            "safe_bytes": r.total(Safety::Safe),
            "review_bytes": r.total(Safety::Review),
            "candidates": r.candidates.len(),
        })),
        "outside": outside_json(&crate::memory::outside_space()),
        "alarming": alarming,
    });
    (v, alarming)
}

// ---- candidates ----

pub fn action_brief(a: &Action) -> Value {
    match a {
        Action::Paths { paths, method } => json!({
            "kind": if *method == Method::Trash { "trash" } else { "delete" },
            "items": paths.len(),
        }),
        Action::Command { argv, .. } => json!({ "kind": "command", "command": argv.join(" ") }),
        Action::Manual { hint } => json!({ "kind": "manual", "hint": en(hint) }),
    }
}

/// One candidate in a few hundred bytes: enough to decide, not the file list.
pub fn candidate_brief(n: usize, c: &Candidate) -> Value {
    json!({
        "n": n,
        "id": c.id,
        "title": en(&c.title),
        "category": en(&c.category),
        "bytes": c.bytes,
        "size": fmt_bytes(c.bytes),
        "reclaimable": c.reclaimable,
        "reclaimable_text": c.reclaimable.map(fmt_bytes),
        "safety": c.safety,
        "path": c.path,
        "why": en_all(&c.evidence),
        "notes": en_all(&c.notes),
        "action": action_brief(&c.action),
    })
}

/// The last scan, summarized. `explain` gives the details of one candidate.
pub fn summary(r: &Report) -> Value {
    json!({
        "schema": crate::app::SCHEMA,
        "time": r.time,
        "disk": {
            "total": r.disk_total,
            "free": r.disk_avail,
            "free_text": fmt_bytes(r.disk_avail),
            "available_incl_purgeable": r.disk_avail_incl_purgeable,
        },
        "reclaimable": {
            "safe": r.total(Safety::Safe),
            "safe_text": fmt_bytes(r.total(Safety::Safe)),
            "review": r.total(Safety::Review),
            "review_text": fmt_bytes(r.total(Safety::Review)),
        },
        "candidates": r.candidates.iter().enumerate().map(|(i, c)| candidate_brief(i + 1, c)).collect::<Vec<_>>(),
        "grew": r.growth.iter().map(|g| json!({
            "path": tilde(Path::new(&g.path)),
            "delta": g.delta,
            "delta_text": sys::fmt_delta(g.delta),
            "now": g.now,
        })).collect::<Vec<_>>(),
        "grew_since": r.growth_since,
        "left_alone": r.excluded.len(),
        "unreadable_folders": r.walk.unreadable,
        "skipped_protected": r.walk.skipped_protected,
    })
}

/// The whole report (every path of every candidate). Large; prefer `summary` + `explain`.
pub fn full_report(r: &Report) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    if let Some(list) = v["candidates"].as_array_mut() {
        for (j, c) in list.iter_mut().zip(&r.candidates) {
            j["title"] = en(&c.title).into();
            j["category"] = en(&c.category).into();
            j["evidence"] = en_all(&c.evidence).into();
            j["notes"] = en_all(&c.notes).into();
            if let Action::Manual { hint } = &c.action {
                j["action"]["hint"] = en(hint).into();
            }
        }
    }
    if let Some(list) = v["excluded"].as_array_mut() {
        for (j, e) in list.iter_mut().zip(&r.excluded) {
            j["reason"] = en(&e.reason).into();
        }
    }
    v
}

/// Index of a candidate given by number (1-based) or id.
pub fn find(r: &Report, target: &str) -> Option<usize> {
    match target.parse::<usize>() {
        Ok(n) if n >= 1 && n <= r.candidates.len() => Some(n - 1),
        Ok(_) => None,
        Err(_) => r.candidates.iter().position(|c| c.id == target),
    }
}

/// One candidate in full, with up to `max_paths` of the paths it would remove.
pub fn explain(r: &Report, target: &str, max_paths: usize) -> Result<Value, String> {
    let i = find(r, target).ok_or_else(|| format!("no candidate {target}; see `roomy report`"))?;
    let c = &r.candidates[i];
    let mut v = candidate_brief(i + 1, c);
    v["schema"] = crate::app::SCHEMA.into();
    v["scanned"] = r.time.into();
    match &c.action {
        Action::Paths { paths, .. } => {
            v["paths"] = paths
                .iter()
                .take(max_paths)
                .map(|p| tilde(Path::new(p)))
                .collect::<Vec<_>>()
                .into();
            v["paths_total"] = paths.len().into();
        }
        Action::Command { argv, cwd } => {
            v["argv"] = argv.clone().into();
            v["cwd"] = cwd.clone().into();
        }
        Action::Manual { .. } => {}
    }
    v["describe"] = crate::clean::describe(&c.action, false).into();
    Ok(v)
}

// ---- cleaning ----

pub fn plan(plan: &Plan) -> Value {
    let total: u64 = plan.items.iter().map(|(_, c)| c.bytes).sum();
    let frees: u64 = plan.items.iter().map(|(_, c)| c.frees()).sum();
    json!({
        "schema": crate::app::SCHEMA,
        "dry_run": true,
        "total_bytes": total,
        "total_text": fmt_bytes(total),
        "frees_bytes": frees,
        "frees_text": fmt_bytes(frees),
        "plan": plan.items.iter().map(|(i, c)| candidate_brief(i + 1, c)).collect::<Vec<_>>(),
        "refused": refused(plan),
    })
}

fn refused(plan: &Plan) -> Vec<Value> {
    plan.refused
        .iter()
        .map(|r| json!({ "id": r.id, "title": r.title, "reason": r.reason }))
        .collect()
}

pub fn outcome(plan: &Plan, out: &Outcome) -> Value {
    json!({
        "schema": crate::app::SCHEMA,
        "dry_run": false,
        "freed_bytes": out.freed,
        "freed_text": fmt_bytes(out.freed),
        "moved_to_trash": out.moved_to_trash,
        "results": plan.items.iter().zip(&out.results).map(|((_, c), (_, r))| json!({
            "id": c.id,
            "title": en(&c.title),
            "ok": r.is_ok(),
            "error": r.as_ref().err(),
        })).collect::<Vec<_>>(),
        "refused": refused(plan),
    })
}

// ---- folders, history, rules ----

pub fn top(snap: &Snapshot, prev: Option<&Snapshot>, dir: &Path, n: usize) -> Value {
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
    let folders: Vec<Value> = kids
        .iter()
        .map(|(p, b)| {
            let delta = prev.and_then(|pv| pv.entries.get(*p)).map(|old| *b as i64 - *old as i64);
            json!({ "path": tilde(Path::new(p.as_str())), "bytes": b, "size": fmt_bytes(*b), "delta": delta })
        })
        .collect();
    json!({
        "schema": crate::app::SCHEMA,
        "path": tilde(dir),
        "time": snap.time,
        "bytes": snap.entries.get(&*key),
        "folders": folders,
    })
}

/// Lowest free space per day, oldest first.
pub fn daily_minimum(samples: &[crate::tick::Sample], days: usize) -> Vec<(String, u64, u64)> {
    let mut by_day: std::collections::BTreeMap<String, (u64, u64)> =
        std::collections::BTreeMap::new();
    for x in samples {
        let day: String = sys::fmt_time(x.t).chars().take(10).collect();
        let e = by_day.entry(day).or_insert((u64::MAX, x.total));
        e.0 = e.0.min(x.avail);
    }
    let mut rows: Vec<(String, u64, u64)> = by_day
        .into_iter()
        .rev()
        .take(days)
        .map(|(d, (m, t))| (d, m, t))
        .collect();
    rows.reverse();
    rows
}

pub fn history(samples: &[crate::tick::Sample], days: usize) -> Value {
    let rows = daily_minimum(samples, days);
    json!({
        "schema": crate::app::SCHEMA,
        "days": rows.iter().map(|(d, m, t)| json!({ "date": d, "min_free": m, "total": t })).collect::<Vec<_>>(),
    })
}

pub fn rules(cfg: &Config) -> Value {
    let builtin: Vec<String> = crate::rules::builtin().into_iter().map(|r| r.id).collect();
    let list: Vec<Value> = crate::rules::all(cfg)
        .iter()
        .map(|r| {
            let ov = cfg.rule_overrides.get(&r.id);
            json!({
                "id": r.id,
                "enabled": !cfg.disabled_rules.contains(&r.id),
                "title": r.title.get_in(EN),
                "paths": r.paths,
                "target": r.target,
                "safety": ov.and_then(|o| o.safety).unwrap_or(r.safety),
                "action": r.action,
                "command": r.command,
                "builtin": builtin.contains(&r.id),
            })
        })
        .collect();
    json!({ "schema": crate::app::SCHEMA, "rules": list })
}

/// `top` for a folder (default: home) from the saved snapshots.
pub fn top_from_state(path: Option<&str>, n: usize) -> Result<Value, String> {
    let files = crate::state::snapshot_files();
    let snap = files
        .first()
        .and_then(|p| crate::state::load::<Snapshot>(p))
        .ok_or("no scan yet; run `roomy scan`")?;
    let prev = files
        .iter()
        .skip(1)
        .filter_map(|p| crate::state::load::<Snapshot>(p))
        .find(|s| s.comparable(&snap));
    let dir = match path {
        Some(p) => {
            let p = sys::expand(p);
            std::fs::canonicalize(&p).unwrap_or(p)
        }
        None => sys::home(),
    };
    Ok(top(&snap, prev.as_ref(), &dir, n))
}

// ---- memory ----

pub fn memory(holders: &[crate::memory::Holder], outside: &[crate::memory::Outside]) -> Value {
    let mem = sys::mem();
    json!({
        "schema": crate::app::SCHEMA,
        "memory": {
            "total": mem.total,
            "free_pct": mem.free_pct,
            "pressure": match mem.pressure { 1 => "normal", 2 => "warning", 4 => "critical", _ => "unknown" },
            "swap_used": mem.swap_used,
            "swap_text": sys::fmt_mem(mem.swap_used),
        },
        "outside": outside_json(outside),
        "holders": holders.iter().enumerate().map(|(i, h)| json!({
            "n": i + 1,
            "id": h.id,
            "name": h.name,
            "kind": if matches!(h.kind, crate::memory::Kind::App { .. }) { "app" } else { "process" },
            "compressed": h.compressed,
            "compressed_text": sys::fmt_mem(h.compressed),
            "resident": h.resident,
            "pids": h.pids,
            "detail": h.detail,
            "protected": h.protected.as_ref().map(en),
        })).collect::<Vec<_>>(),
        "hint": "Quitting an app releases its swapped memory; restarting the Mac releases all of it. Ask the user before quitting anything (roomy memory quit <n>).",
    })
}

pub fn outside_json(outside: &[crate::memory::Outside]) -> Vec<Value> {
    outside
        .iter()
        .map(|o| json!({ "name": o.name, "kind": o.kind, "bytes": o.bytes, "size": fmt_bytes(o.bytes) }))
        .collect()
}
