//! Walk the disk, compare with the last comparable snapshot, run the detectors, save the report.
//! The caller holds the state lock.

use crate::config::{Config, gb};
use crate::detect::{self, Ctx};
use crate::model::{Report, WalkInfo};
use crate::state;
use crate::sys::{self, expand};
use crate::walk::{self, Sizer, Snapshot};
use std::path::{Path, PathBuf};

/// `background`: started by the background service, not by the user. Throttles I/O and, without Full Disk
/// Access, stays out of TCC-protected folders so no permission dialogs appear.
pub fn run(cfg: &Config, background: bool) -> anyhow::Result<Report> {
    if background {
        sys::throttle_io();
    }
    let protect = background && !sys::has_full_disk_access();
    let off_limits = if protect {
        sys::tcc_protected()
    } else {
        vec![]
    };
    let mut exclude: Vec<PathBuf> = cfg.scan.exclude.iter().map(|p| expand(p)).collect();
    exclude.extend(off_limits.iter().cloned());
    exclude.sort();
    exclude.dedup();
    let roots: Vec<PathBuf> = cfg
        .scan
        .roots
        .iter()
        .map(|p| expand(p))
        .filter(|p| p.is_dir())
        .collect();

    let (snap, stats) = walk::snapshot(
        &roots,
        &exclude,
        cfg.scan.snapshot_min_mb * 1_000_000,
        cfg.scan.snapshot_max_depth,
    );

    let prev = previous_comparable(&snap);
    let growth = prev
        .as_ref()
        .map(|p| walk::growth(p, &snap, gb(cfg.thresholds.growth_gb)))
        .unwrap_or_default();
    let growth_since = prev.map(|p| p.time);

    state::save(&snapshot_path(&state::snapshots_dir()), &snap)?;
    state::prune_snapshots(cfg.scan.keep_snapshots);

    let sizer = Sizer { snap };
    let ctx = Ctx {
        cfg,
        sizer: &sizer,
        cmdlines: sys::cmdlines(),
        cwds: std::cell::OnceCell::new(),
        off_limits,
    };
    let (candidates, excluded) = detect::detect_all(&ctx);

    let disk = sys::disk();
    let report = Report {
        schema: crate::app::SCHEMA,
        version: crate::app::VERSION.into(),
        time: sys::now(),
        disk_total: disk.total,
        disk_avail: disk.avail,
        candidates,
        growth,
        growth_since,
        excluded,
        walk: WalkInfo {
            seconds: stats.secs,
            unreadable: stats.errors,
            skipped_protected: protect,
        },
    };
    state::save(&state::report_path(), &report)?;
    Ok(report)
}

/// A new, unused file name that sorts after every earlier snapshot. Milliseconds, plus a counter,
/// because two scans can finish within the same second.
fn snapshot_path(dir: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    // A fixed-width counter keeps names sorting in creation order.
    (0..1000)
        .map(|n| dir.join(format!("{stamp}-{n:03}.json")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stamp}.json")))
}

/// Newest saved snapshot that covered the same places (so the difference is real growth).
fn previous_comparable(cur: &Snapshot) -> Option<Snapshot> {
    state::snapshot_files()
        .iter()
        .filter_map(|p| state::load::<Snapshot>(p))
        .find(|s| s.comparable(cur))
}

/// The snapshot saved by the last scan, for sizes when re-checking candidates.
pub fn latest_snapshot() -> Snapshot {
    state::snapshot_files()
        .first()
        .and_then(|p| state::load(p))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names_never_collide_and_keep_their_order() {
        let d = tempfile::tempdir().unwrap();
        let mut names = vec![];
        for _ in 0..5 {
            let p = snapshot_path(d.path());
            std::fs::write(&p, "{}").unwrap();
            names.push(p);
        }
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, names, "unique and in creation order");
    }
}
