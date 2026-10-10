//! Folders that gain one subfolder per installed version (self-updating tools).
//! Keeps the newest `keep`, whatever a `current` symlink points to, and anything running.

use super::Ctx;
use crate::config::VersionedDir;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::rules::children;
use crate::sys::{expand, mtime, tilde};
use std::fs;
use std::path::{Path, PathBuf};

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    for vd in &ctx.cfg.versioned {
        if let Some(c) = evaluate(ctx, vd) {
            out.push(c);
        }
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    let vd = ctx
        .cfg
        .versioned
        .iter()
        .find(|v| tilde(&expand(&v.path)) == c.path)
        .ok_or_else(|| m("recheck.rule_gone"))?;
    evaluate(ctx, vd).ok_or_else(|| m("recheck.gone"))
}

/// Targets of symlinks next to or inside the versions folder (e.g. `../current -> releases/1.2`).
fn pinned(dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    for d in [Some(dir), dir.parent()].into_iter().flatten() {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_symlink())
                    && let Ok(target) = fs::canonicalize(e.path())
                {
                    v.push(target);
                }
            }
        }
    }
    v
}

fn evaluate(ctx: &Ctx, vd: &VersionedDir) -> Option<Candidate> {
    let dir = expand(&vd.path);
    if ctx.is_off_limits(&dir) || !dir.is_dir() {
        return None;
    }
    let mut versions: Vec<(PathBuf, i64)> = children(&dir)
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| (p.clone(), mtime(&p).unwrap_or(0)))
        .collect();
    versions.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    let pinned = pinned(&dir);
    let old: Vec<PathBuf> = versions
        .into_iter()
        .skip(vd.keep)
        .map(|(p, _)| p)
        .filter(|p| {
            let canon = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            !pinned.contains(&canon) && !ctx.running(&p.to_string_lossy())
        })
        .collect();
    if old.is_empty() {
        return None;
    }
    let owner = dir
        .parent()
        .and_then(|p| p.file_name())
        .map_or_else(|| tilde(&dir), |n| n.to_string_lossy().into_owned());
    Some(Candidate {
        id: format!("versioned:{}", tilde(&dir)),
        source: "versioned".into(),
        path: tilde(&dir),
        category: m("category.devtools"),
        title: mf("versioned.title", &[("name", &owner), ("n", &old.len())]),
        bytes: old.iter().map(|p| ctx.sizer.size(p)).sum(),
        safety: Safety::Safe,
        evidence: vec![mf("versioned.kept", &[("n", &vd.keep)])],
        notes: vec![],
        action: Action::Paths {
            paths: old
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            method: Method::Delete,
        },
    })
}
