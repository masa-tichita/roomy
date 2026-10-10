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
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Ctx<'a> {
    pub cfg: &'a Config,
    pub sizer: &'a Sizer,
    pub cmdlines: Vec<String>,
    pub cwds: Vec<PathBuf>,
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

    pub fn min_bytes(&self) -> u64 {
        self.cfg.candidates.min_mb * 1_000_000
    }
}

pub fn detect_all(ctx: &Ctx) -> (Vec<Candidate>, Vec<Excluded>) {
    let mut found = vec![];
    let mut excluded = vec![];
    crate::rules::detect(ctx, &mut found);
    let from_rules = found.len();
    caches::detect(ctx, &mut found);
    xcode::detect(ctx, &mut found);
    versioned::detect(ctx, &mut found);
    editor::detect(ctx, &mut found);
    worktree::detect(ctx, &mut found, &mut excluded);
    docker::detect(ctx, &mut found);
    apps::detect(ctx, &mut found);
    downloads::detect(ctx, &mut found);

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
    let fresh = match c.source.split(':').next().unwrap_or("") {
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
