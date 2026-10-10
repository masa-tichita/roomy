//! App caches not covered by a rule: big folders in `~/Library/Caches` and `~/.cache`.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::rules::{children, claimed_paths};
use crate::sys::{expand, tilde};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// `~/Library/Caches` holds caches by convention; `~/.cache` is used more loosely, so review.
fn bases() -> [(PathBuf, Safety); 2] {
    [
        (expand("~/Library/Caches"), Safety::Safe),
        (expand("~/.cache"), Safety::Review),
    ]
}

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    let claimed = claimed_paths(&crate::rules::all(ctx.cfg));
    for (base, safety) in bases() {
        if ctx.is_off_limits(&base) {
            continue;
        }
        for child in children(&base) {
            if let Some(c) = evaluate(ctx, &child, safety, &claimed) {
                out.push(c);
            }
        }
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    let path = expand(&c.path);
    let claimed = claimed_paths(&crate::rules::all(ctx.cfg));
    let (_, safety) = bases()
        .into_iter()
        .find(|(b, _)| path.parent() == Some(b.as_path()))
        .ok_or_else(|| m("recheck.gone"))?;
    evaluate(ctx, &path, safety, &claimed).ok_or_else(|| m("recheck.gone"))
}

fn evaluate(
    ctx: &Ctx,
    path: &Path,
    base_safety: Safety,
    claimed: &HashSet<PathBuf>,
) -> Option<Candidate> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if name.starts_with("com.apple.") || claimed.iter().any(|c| c.starts_with(path)) {
        return None;
    }
    if !path.is_dir() {
        return None;
    }
    let bytes = ctx.sizer.size(path);
    if bytes < ctx.cfg.candidates.cache_min_mb * 1_000_000 {
        return None;
    }
    let mut safety = base_safety;
    let mut evidence = vec![];
    // "com.google.Chrome" -> "Chrome", "Arc" -> "Arc"
    let app = name.rsplit('.').next().unwrap_or(&name);
    if app.len() >= 3 && ctx.running(&format!("/{app}.app/")) {
        safety = Safety::Review;
        evidence.push(mf("cache.app_running", &[("app", &app)]));
    }
    Some(Candidate {
        id: format!("cache.app:{name}"),
        source: "cache".into(),
        path: tilde(path),
        category: m("category.cache"),
        title: mf("cache.app_title", &[("name", &name)]),
        bytes,
        reclaimable: None,
        safety,
        evidence,
        notes: vec![m("cache.app_note")],
        action: Action::Paths {
            paths: vec![path.to_string_lossy().into_owned()],
            method: Method::Delete,
        },
    })
}
