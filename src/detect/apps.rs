//! Large apps not opened for a long time. Report only: apps should be removed with their own
//! uninstaller (or `brew uninstall --cask`), which also cleans up helpers and license data.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Safety};
use crate::rules::children;
use crate::sys::{days_since, expand, run, tilde};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Only folders the scan walked: sizing apps outside `scan.roots` (or in `scan.exclude`) would
/// walk every bundle again on each scan, tens of GB with a few Xcode versions installed.
pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    for base in [PathBuf::from("/Applications"), expand("~/Applications")] {
        if !ctx.sizer.covers(&base) {
            continue;
        }
        for p in children(&base) {
            if let Some(c) = evaluate(ctx, &p) {
                out.push(c);
            }
        }
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    evaluate(ctx, &expand(&c.path)).ok_or_else(|| m("recheck.gone"))
}

/// The bundle to ask Spotlight about: the path itself, or the first `.app` inside a vendor folder.
fn bundle(p: &Path) -> Option<PathBuf> {
    if p.extension().is_some_and(|e| e == "app") {
        return Some(p.to_path_buf());
    }
    children(p)
        .into_iter()
        .find(|c| c.extension().is_some_and(|e| e == "app"))
}

/// `mdls -raw` prints "2026-04-28 02:00:00 +0000" or "(null)".
pub fn parse_mdls_date(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .map(|d| d.timestamp())
}

fn mdls(p: &Path, attr: &str) -> Option<i64> {
    let out = run(
        &["mdls", "-raw", "-name", attr, &p.to_string_lossy()],
        None,
        Duration::from_secs(5),
    )?;
    parse_mdls_date(&out.stdout)
}

fn evaluate(ctx: &Ctx, p: &Path) -> Option<Candidate> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    let display = name.trim_end_matches(".app").to_string();
    let lower = display.to_lowercase();
    if ctx
        .cfg
        .apps
        .keep
        .iter()
        .any(|k| lower.contains(&k.to_lowercase()))
    {
        return None;
    }
    if !p.is_dir() || p.is_symlink() {
        return None;
    }
    let bytes = ctx
        .sizer
        .size_at_least(p, crate::config::gb(ctx.cfg.apps.min_gb))?;
    let app = bundle(p)?;
    if ctx.running(&format!("{}/Contents/MacOS/", app.to_string_lossy())) {
        return None;
    }
    let evidence = if let Some(last) = mdls(&app, "kMDItemLastUsedDate") {
        let days = days_since(last);
        if days < ctx.cfg.apps.unused_days {
            return None;
        }
        mf("apps.last_used", &[("days", &days)])
    } else {
        // Never opened: judge by when it was installed.
        let added = mdls(&app, "kMDItemDateAdded").or_else(|| crate::sys::mtime(&app))?;
        if days_since(added) < ctx.cfg.apps.unused_days {
            return None;
        }
        mf("apps.never_used", &[("days", &days_since(added))])
    };
    Some(Candidate {
        id: format!("app:{display}"),
        source: "apps".into(),
        path: tilde(p),
        category: m("category.apps"),
        title: Msg::plain(display),
        bytes,
        reclaimable: None,
        safety: Safety::Review,
        evidence: vec![evidence],
        notes: vec![],
        action: Action::Manual {
            hint: m("apps.hint"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdls_dates() {
        assert_eq!(
            parse_mdls_date("2026-04-28 02:00:00 +0000"),
            Some(1_777_341_600)
        );
        assert_eq!(parse_mdls_date("(null)"), None);
    }
}
