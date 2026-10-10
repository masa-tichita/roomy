//! Walk the disk, compare with the last comparable snapshot, run the detectors, save the report.
//! The caller holds the state lock.

use crate::config::{Config, gb};
use crate::detect::{self, Ctx};
use crate::model::{Report, WalkInfo};
use crate::state;
use crate::sys::{self, expand};
use crate::walk::{self, Sizer, Snapshot};
use std::path::PathBuf;

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

    let name = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    state::save(&state::snapshots_dir().join(format!("{name}.json")), &snap)?;
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
