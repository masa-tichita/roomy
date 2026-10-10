//! Old items in Downloads. Always "review" and moved to the Trash, never deleted outright.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::rules::children;
use crate::sys::{days_since, expand, fmt_bytes, mtime, tilde};
use std::path::PathBuf;

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    out.extend(evaluate(ctx));
}

pub fn recheck(ctx: &Ctx, _c: &Candidate) -> Result<Candidate, Msg> {
    evaluate(ctx).ok_or_else(|| m("recheck.gone"))
}

fn evaluate(ctx: &Ctx) -> Option<Candidate> {
    let dir = expand("~/Downloads");
    if ctx.is_off_limits(&dir) || !dir.is_dir() {
        return None;
    }
    let age = ctx.cfg.downloads.age_days;
    let mut old: Vec<(PathBuf, u64)> = children(&dir)
        .into_iter()
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .filter(|p| mtime(p).is_some_and(|m| days_since(m) >= age))
        .map(|p| {
            let b = ctx.sizer.size(&p);
            (p, b)
        })
        .collect();
    if old.is_empty() {
        return None;
    }
    old.sort_by_key(|x| std::cmp::Reverse(x.1));
    let biggest: Vec<String> = old
        .iter()
        .take(5)
        .map(|(p, b)| {
            format!(
                "{} ({})",
                p.file_name().unwrap_or_default().to_string_lossy(),
                fmt_bytes(*b)
            )
        })
        .collect();
    Some(Candidate {
        id: "downloads.old".into(),
        source: "downloads".into(),
        path: tilde(&dir),
        category: m("category.files"),
        title: mf("downloads.title", &[("days", &age), ("n", &old.len())]),
        bytes: old.iter().map(|x| x.1).sum(),
        safety: Safety::Review,
        evidence: vec![mf("downloads.evidence", &[("days", &age)])],
        notes: vec![mf("downloads.biggest", &[("list", &biggest.join(", "))])],
        action: Action::Paths {
            paths: old
                .iter()
                .map(|(p, _)| p.to_string_lossy().into_owned())
                .collect(),
            method: Method::Trash,
        },
    })
}
