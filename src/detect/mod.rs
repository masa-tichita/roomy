//! Detectors find removal candidates. Each one can also re-check a single candidate right before
//! `clean` removes it, because the world may have changed since the scan.

mod apps;
mod caches;
mod docker;
mod downloads;
mod editor;
mod versioned;
pub mod worktree;
mod xcode;

use crate::config::Config;
use crate::model::{Candidate, Excluded};
use crate::walk::Sizer;
use std::cell::OnceCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Ctx<'a> {
    pub cfg: &'a Config,
    pub sizer: &'a Sizer,
    pub cmdlines: Vec<String>,
    /// Working directories of the user's processes. Listing them (`lsof`) is slow, so it happens
    /// only when a worktree needs the answer; read them with [`Ctx::cwds`].
    pub cwds: OnceCell<Vec<PathBuf>>,
    /// Folders that must not be touched in this run (TCC-protected ones in background runs
    /// without Full Disk Access).
    pub off_limits: Vec<PathBuf>,
}

impl Ctx<'_> {
    pub fn is_off_limits(&self, p: &Path) -> bool {
        self.off_limits.iter().any(|o| p.starts_with(o))
    }

    pub fn running(&self, needle: &str) -> bool {
        self.cmdlines.iter().any(|c| c.contains(needle))
    }

    pub fn cwds(&self) -> &[PathBuf] {
        self.cwds.get_or_init(crate::sys::process_cwds)
    }

    pub fn min_bytes(&self) -> u64 {
        self.cfg.candidates.min_mb * 1_000_000
    }

    pub fn detector_on(&self, name: &str) -> bool {
        !self.cfg.disabled_detectors.iter().any(|d| d == name)
    }
}

/// Names for `disabled_detectors`; each is also the first part of its candidates' `source`.
pub const DETECTORS: &[&str] = &[
    "cache",
    "xcode",
    "versioned",
    "editor",
    "worktree",
    "docker",
    "apps",
    "downloads",
];

type Detect = fn(&Ctx, &mut Vec<Candidate>);

pub fn detect_all(ctx: &Ctx) -> (Vec<Candidate>, Vec<Excluded>) {
    let mut found = vec![];
    let mut excluded = vec![];
    crate::rules::detect(ctx, &mut found);
    let from_rules = found.len();
    let detectors: [(&str, Detect); 7] = [
        ("cache", caches::detect),
        ("xcode", xcode::detect),
        ("versioned", versioned::detect),
        ("editor", editor::detect),
        ("docker", docker::detect),
        ("apps", apps::detect),
        ("downloads", downloads::detect),
    ];
    for (name, detect) in detectors {
        if ctx.detector_on(name) {
            detect(ctx, &mut found);
        }
    }
    if ctx.detector_on("worktree") {
        worktree::detect(ctx, &mut found, &mut excluded);
    }

    // Rules carry their own minimum; the global one applies to everything else.
    let min = ctx.min_bytes();
    let mut i = 0;
    found.retain(|c| {
        i += 1;
        i <= from_rules || c.bytes >= min
    });
    let mut ids = HashSet::new();
    found.retain(|c| ids.insert(c.id.clone()));
    found.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    (found, excluded)
}

/// Re-evaluates one candidate now. Returns the fresh candidate (its action is what will run),
/// or why it no longer qualifies.
pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, String> {
    let source = c.source.split(':').next().unwrap_or("");
    let fresh = match source {
        _ if !ctx.detector_on(source) => Err(crate::i18n::m("recheck.detector_disabled")),
        "rule" => crate::rules::recheck(ctx, c),
        "cache" => caches::recheck(ctx, c),
        "xcode" => xcode::recheck(ctx, c),
        "versioned" => versioned::recheck(ctx, c),
        "editor" => editor::recheck(ctx, c),
        "worktree" => worktree::recheck(ctx, c),
        "docker" => docker::recheck(ctx, c),
        "apps" => apps::recheck(ctx, c),
        "downloads" => downloads::recheck(ctx, c),
        _ => Err(crate::i18n::m("recheck.unknown_source")),
    }
    .map_err(|e| e.render())?;
    if c.safety == crate::model::Safety::Safe && fresh.safety != c.safety {
        let why: Vec<String> = fresh
            .evidence
            .iter()
            .map(super::i18n::Msg::render)
            .collect();
        return Err(crate::i18n::tf(
            "recheck.became_review",
            &[("why", &why.join(" / "))],
        ));
    }
    Ok(fresh)
}
