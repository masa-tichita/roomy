//! Removing candidates. Every candidate is re-checked and every path passes the guard first.

use crate::config::Config;
use crate::detect::{self, Ctx};
use crate::guard;
use crate::i18n::{t, tf};
use crate::model::{Action, Candidate, Method, Report, Safety};
use crate::state;
use crate::sys::{self, Sty, days_since, fmt_bytes};
use crate::walk::Sizer;
use serde::Serialize;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

pub struct Opts {
    pub targets: Vec<String>,
    pub all_safe: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub trash: bool,
    /// Also remove "review" candidates without a person confirming.
    pub allow_review: bool,
    pub json: bool,
}

/// Who is deciding. Unattended runs (`--yes`, `--json`, MCP, no terminal) get stricter rules:
/// "review" candidates need an explicit opt-in and the total size is capped.
#[derive(Clone, Copy)]
pub struct Policy {
    pub unattended: bool,
    pub allow_review: bool,
}

impl Policy {
    /// A person confirms in a terminal or the dashboard.
    pub const INTERACTIVE: Policy = Policy {
        unattended: false,
        allow_review: true,
    };
}

#[derive(Serialize)]
struct AuditEntry<'a> {
    time: i64,
    id: &'a str,
    title: String,
    action: &'a Action,
    estimated_bytes: u64,
    /// Clone-aware estimate, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_reclaimable: Option<u64>,
    ok: bool,
    error: Option<String>,
    unattended: bool,
}

/// Indices (into the report) of the selected candidates.
pub fn select(report: &Report, targets: &[String], all_safe: bool) -> Result<Vec<usize>, String> {
    let mut picked = vec![];
    for tg in targets {
        let idx = match tg.parse::<usize>() {
            Ok(n) if n >= 1 && n <= report.candidates.len() => n - 1,
            Ok(_) => return Err(tf("clean.bad_number", &[("n", tg)])),
            Err(_) => report
                .candidates
                .iter()
                .position(|c| c.id == *tg)
                .ok_or_else(|| tf("clean.unknown_id", &[("id", tg)]))?,
        };
        if !picked.contains(&idx) {
            picked.push(idx);
        }
    }
    if all_safe {
        for (i, c) in report.candidates.iter().enumerate() {
            if c.safety == Safety::Safe
                && !matches!(c.action, Action::Manual { .. })
                && !picked.contains(&i)
            {
                picked.push(i);
            }
        }
    }
    Ok(picked)
}

pub fn run(cfg: &Config, opts: &Opts) -> anyhow::Result<i32> {
    let s = Sty::new();
    let Some(mut report) = state::load::<Report>(&state::report_path()) else {
        anyhow::bail!(t("clean.no_report"));
    };
    if !opts.json && days_since(report.time) >= cfg.candidates.stale_report_days {
        eprintln!(
            "{}",
            s.yellow(&tf("clean.stale", &[("days", &days_since(report.time))]))
        );
    }
    let picked = select(&report, &opts.targets, opts.all_safe).map_err(anyhow::Error::msg)?;
    if picked.is_empty() {
        anyhow::bail!(t("clean.nothing_selected"));
    }
    let unattended = opts.yes || opts.json || !std::io::stdin().is_terminal();
    let policy = Policy {
        unattended,
        allow_review: opts.allow_review,
    };
    let plan = prepare(cfg, &report, &picked, policy);
    // With --json, nothing is removed unless --yes is given as well.
    let dry_run = opts.dry_run || (opts.json && !opts.yes);

    if opts.json {
        if dry_run || plan.items.is_empty() {
            crate::output::print_json(&crate::api::plan(&plan));
            return Ok(i32::from(plan.items.is_empty()));
        }
        let out = apply(&mut report, &plan, opts.trash, true, true)?;
        crate::output::print_json(&crate::api::outcome(&plan, &out));
        return Ok(i32::from(!out.all_ok() || !plan.refused.is_empty()));
    }

    println!("{}", s.bold(&t("clean.plan")));
    for (i, c) in &plan.items {
        println!(
            "  {:>2}  {}  {}",
            i + 1,
            sys::pad_left(&fmt_bytes(c.frees()), 9),
            c.title
        );
        if let Some(b) = c.allocated_if_different() {
            println!(
                "      {}",
                s.dim(&tf("size.allocated", &[("bytes", &fmt_bytes(b))]))
            );
        }
        println!("      {}", s.dim(&describe(&c.action, opts.trash)));
    }
    for r in &plan.refused {
        println!("  {}  {}: {}", s.yellow("--"), r.title, r.reason);
    }
    if plan.items.is_empty() {
        return Ok(1);
    }
    if dry_run {
        println!("{}", s.dim(&t("clean.dry_run")));
        return Ok(0);
    }
    if !opts.yes {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(t("clean.need_yes"));
        }
        print!("{} ", t("clean.confirm"));
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("{}", t("clean.cancelled"));
            return Ok(1);
        }
    }

    let out = apply(&mut report, &plan, opts.trash, false, unattended)?;
    for (title, result) in &out.results {
        match result {
            Ok(()) => println!("  {} {title}", s.green("✓")),
            Err(e) => println!("  {} {title}: {e}", s.red("✗")),
        }
    }
    println!(
        "{}",
        s.bold(&tf("clean.freed", &[("bytes", &fmt_bytes(out.freed))]))
    );
    if out.moved_to_trash {
        println!("{}", s.dim(&t("clean.trash_note")));
    }
    Ok(i32::from(!out.all_ok()))
}

pub struct Refused {
    pub id: String,
    pub title: String,
    pub reason: String,
}

/// What `clean` would do: each selected candidate re-checked now, or the reason it is refused.
pub struct Plan {
    /// (index in the report, fresh candidate)
    pub items: Vec<(usize, Candidate)>,
    pub refused: Vec<Refused>,
}

pub fn prepare(cfg: &Config, report: &Report, picked: &[usize], policy: Policy) -> Plan {
    let sizer = Sizer {
        snap: crate::scan::latest_snapshot(),
    };
    let ctx = Ctx {
        cfg,
        sizer: &sizer,
        cmdlines: sys::cmdlines(),
        cwds: std::cell::OnceCell::new(),
        off_limits: vec![],
    };
    let mut plan = Plan {
        items: vec![],
        refused: vec![],
    };
    for &i in picked {
        let old = &report.candidates[i];
        let refuse = |reason: String| Refused {
            id: old.id.clone(),
            title: old.title.render(),
            reason,
        };
        if let Action::Manual { hint } = &old.action {
            plan.refused
                .push(refuse(tf("clean.manual", &[("hint", hint)])));
            continue;
        }
        if policy.unattended && !policy.allow_review && old.safety == Safety::Review {
            plan.refused.push(refuse(t("clean.needs_allow_review")));
            continue;
        }
        match detect::recheck(&ctx, old).and_then(|fresh| guard_paths(&fresh).map(|()| fresh)) {
            Ok(fresh) => plan.items.push((i, fresh)),
            Err(why) => plan.refused.push(refuse(why)),
        }
    }
    let total: u64 = plan.items.iter().map(|(_, c)| c.bytes).sum();
    let limit = crate::config::gb(cfg.clean.max_unattended_gb);
    if policy.unattended && total > limit {
        let reason = tf(
            "clean.over_limit",
            &[("total", &fmt_bytes(total)), ("limit", &fmt_bytes(limit))],
        );
        for (_, c) in plan.items.drain(..) {
            plan.refused.push(Refused {
                id: c.id.clone(),
                title: c.title.render(),
                reason: reason.clone(),
            });
        }
    }
    plan
}

pub struct Outcome {
    /// (title, result) per planned item, in order
    pub results: Vec<(String, Result<(), String>)>,
    /// Measured change in free space.
    pub freed: u64,
    pub moved_to_trash: bool,
}

impl Outcome {
    pub fn all_ok(&self) -> bool {
        self.results.iter().all(|(_, r)| r.is_ok())
    }
}

/// Runs a prepared plan, writes the audit log, and removes finished candidates from the report.
/// `quiet`: commands do not write to the terminal (used by the TUI).
pub fn apply(
    report: &mut Report,
    plan: &Plan,
    force_trash: bool,
    quiet: bool,
    unattended: bool,
) -> anyhow::Result<Outcome> {
    let before = sys::disk().avail;
    let mut done = vec![];
    let mut results = vec![];
    let mut moved_to_trash = false;
    for (i, c) in &plan.items {
        let result = execute(&c.action, force_trash, quiet);
        let entry = AuditEntry {
            time: sys::now(),
            id: &c.id,
            title: c.title.render(),
            action: &c.action,
            estimated_bytes: c.bytes,
            estimated_reclaimable: c.reclaimable,
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            unattended,
        };
        let _ = state::append_line(&state::audit_path(), &entry);
        if result.is_ok() {
            done.push(*i);
            moved_to_trash |= force_trash
                || matches!(
                    c.action,
                    Action::Paths {
                        method: Method::Trash,
                        ..
                    }
                );
        }
        results.push((c.title.render(), result));
    }
    let disk = sys::disk();
    let after = disk.avail;
    done.sort_unstable_by(|a, b| b.cmp(a));
    for i in &done {
        report.candidates.remove(*i);
    }
    report.disk_avail = after;
    report.disk_avail_incl_purgeable = disk.avail_incl_purgeable;
    state::save(&state::report_path(), report)?;
    Ok(Outcome {
        results,
        freed: after.saturating_sub(before),
        moved_to_trash,
    })
}

fn guard_paths(c: &Candidate) -> Result<(), String> {
    if let Action::Paths { paths, .. } = &c.action {
        for p in paths {
            guard::check(Path::new(p))
                .map_err(|r| format!("{} ({})", r.message(), sys::tilde(Path::new(p))))?;
        }
    }
    Ok(())
}

pub fn describe(a: &Action, force_trash: bool) -> String {
    match a {
        Action::Paths { paths, method } => {
            let n = paths.len();
            if force_trash || *method == Method::Trash {
                tf("clean.do_trash", &[("n", &n)])
            } else {
                tf("clean.do_delete", &[("n", &n)])
            }
        }
        Action::Command { argv, .. } => tf("clean.do_command", &[("cmd", &argv.join(" "))]),
        Action::Manual { hint } => hint.render(),
    }
}

pub fn execute(a: &Action, force_trash: bool, quiet: bool) -> Result<(), String> {
    match a {
        Action::Paths { paths, method } => {
            let mut errors = vec![];
            for p in paths {
                let path = Path::new(p);
                if fs::symlink_metadata(path).is_err() {
                    continue; // already gone
                }
                if let Err(r) = guard::check(path) {
                    errors.push(format!("{}: {}", sys::tilde(path), r.message()));
                    continue;
                }
                let r = if force_trash || *method == Method::Trash {
                    sys::trash(path)
                } else {
                    remove(path)
                };
                if let Err(e) = r {
                    errors.push(format!("{}: {e}", sys::tilde(path)));
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                let more = errors.len().saturating_sub(3);
                let mut msg = errors.into_iter().take(3).collect::<Vec<_>>().join("; ");
                if more > 0 {
                    msg.push(' ');
                    msg.push_str(&tf("common.and_more", &[("n", &more)]));
                }
                Err(msg)
            }
        }
        Action::Command { argv, cwd } => {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            if let Some(d) = cwd {
                cmd.current_dir(d);
            }
            if quiet {
                cmd.stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
            }
            match cmd.status() {
                Ok(st) if st.success() => Ok(()),
                Ok(st) => Err(tf(
                    "clean.command_failed",
                    &[("code", &st.code().unwrap_or(-1))],
                )),
                Err(e) => Err(e.to_string()),
            }
        }
        Action::Manual { hint } => Err(hint.render()),
    }
}

/// Deletes a file or folder. Read-only folders (some caches make them) are made writable and retried.
pub fn remove(path: &Path) -> Result<(), String> {
    let md = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !md.is_dir() {
        return fs::remove_file(path).map_err(|e| e.to_string());
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            make_writable(path);
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn make_writable(dir: &Path) {
    if let Ok(md) = fs::symlink_metadata(dir)
        && md.is_dir()
    {
        let mut perm = md.permissions();
        perm.set_mode(perm.mode() | 0o700);
        let _ = fs::set_permissions(dir, perm);
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                make_writable(&e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, safety: Safety, action: Action) -> Candidate {
        Candidate {
            id: id.into(),
            source: "rule:x".into(),
            path: "~/x".into(),
            category: crate::i18n::Msg::plain(""),
            title: crate::i18n::Msg::plain(id),
            bytes: 1,
            reclaimable: None,
            safety,
            evidence: vec![],
            notes: vec![],
            action,
        }
    }

    fn report() -> Report {
        let paths = Action::Paths {
            paths: vec![],
            method: Method::Delete,
        };
        Report {
            schema: 1,
            version: String::new(),
            time: 0,
            disk_total: 0,
            disk_avail: 0,
            disk_avail_incl_purgeable: None,
            candidates: vec![
                cand("a", Safety::Safe, paths.clone()),
                cand("b", Safety::Review, paths.clone()),
                cand(
                    "c",
                    Safety::Safe,
                    Action::Manual {
                        hint: crate::i18n::Msg::plain(""),
                    },
                ),
            ],
            growth: vec![],
            growth_since: None,
            excluded: vec![],
            walk: crate::model::WalkInfo::default(),
        }
    }

    #[test]
    fn selection_by_number_id_and_all_safe() {
        let r = report();
        assert_eq!(
            select(&r, &["2".into(), "a".into()], false).unwrap(),
            vec![1, 0]
        );
        assert_eq!(
            select(&r, &[], true).unwrap(),
            vec![0],
            "manual candidates are never auto-selected"
        );
        assert!(select(&r, &["9".into()], false).is_err());
        assert!(select(&r, &["0".into()], false).is_err());
        assert!(select(&r, &["nope".into()], false).is_err());
    }

    #[test]
    fn removes_read_only_trees() {
        let d = tempfile::tempdir().unwrap();
        let ro = d.path().join("ro");
        fs::create_dir_all(ro.join("inner")).unwrap();
        fs::write(ro.join("inner/f"), "x").unwrap();
        fs::set_permissions(ro.join("inner"), fs::Permissions::from_mode(0o555)).unwrap();
        remove(&ro).unwrap();
        assert!(!ro.exists());
    }

    #[test]
    fn missing_paths_are_skipped_not_errors() {
        let a = Action::Paths {
            paths: vec!["/definitely/not/here".into()],
            method: Method::Delete,
        };
        assert_eq!(execute(&a, false, true), Ok(()));
    }
}
