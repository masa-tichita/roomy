//! Parallel disk walk. One pass records the size of every large folder.
//!
//! Sizes are allocated blocks (like `du`). Hard links count once. Symbolic links are not followed
//! and other volumes are not entered. APFS clones cannot be told apart, so folders full of clones
//! look bigger than the space their removal would free.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Snapshot {
    pub time: i64,
    pub roots: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    /// path -> bytes, only folders of at least `snapshot_min_mb` within `snapshot_max_depth`
    pub entries: BTreeMap<String, u64>,
}

impl Snapshot {
    /// Two snapshots can be compared only if they covered the same places.
    pub fn comparable(&self, other: &Snapshot) -> bool {
        self.roots == other.roots && self.exclude == other.exclude
    }
}

pub struct WalkStats {
    pub secs: f64,
    pub errors: u64,
}

/// A worker panicked while holding the lock; the scan result would be incomplete anyway.
const POISON: &str = "walker thread panicked";

struct Walker<'a> {
    exclude: &'a [PathBuf],
    min: u64,
    max_depth: usize,
    out: Mutex<Vec<(PathBuf, u64)>>,
    seen_links: Mutex<HashSet<(u64, u64)>>,
    errors: AtomicU64,
}

impl Walker<'_> {
    fn dir_size(&self, path: &Path, dev: u64, depth: usize) -> u64 {
        let entries: Vec<fs::DirEntry> = if let Ok(rd) = fs::read_dir(path) {
            rd.filter_map(Result::ok).collect()
        } else {
            self.errors.fetch_add(1, Ordering::Relaxed);
            return 0;
        };
        let total: u64 = entries
            .par_iter()
            .map(|e| {
                // DirEntry::metadata does not follow symbolic links
                let Ok(md) = e.metadata() else { return 0 };
                let ft = md.file_type();
                if ft.is_symlink() {
                    return 0;
                }
                if ft.is_dir() {
                    let p = e.path();
                    // stay on this volume (no NFS mounts etc.) and out of excluded folders
                    if md.dev() != dev || self.exclude.contains(&p) {
                        return 0;
                    }
                    return md.blocks() * 512 + self.dir_size(&p, dev, depth + 1);
                }
                if md.nlink() > 1
                    && !self
                        .seen_links
                        .lock()
                        .expect(POISON)
                        .insert((md.dev(), md.ino()))
                {
                    return 0;
                }
                md.blocks() * 512
            })
            .sum();
        // Roots are always recorded so their total is known even when small.
        if (total >= self.min || depth == 0) && depth <= self.max_depth {
            self.out
                .lock()
                .expect(POISON)
                .push((path.to_path_buf(), total));
        }
        total
    }
}

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(12)
            .stack_size(32 << 20)
            .build()
            .expect("cannot create thread pool")
    })
}

pub fn snapshot(
    roots: &[PathBuf],
    exclude: &[PathBuf],
    min: u64,
    max_depth: usize,
) -> (Snapshot, WalkStats) {
    let start = Instant::now();
    let w = Walker {
        exclude,
        min,
        max_depth,
        out: Mutex::new(Vec::new()),
        seen_links: Mutex::new(HashSet::new()),
        errors: AtomicU64::new(0),
    };
    pool().install(|| {
        for r in roots {
            if let Ok(md) = fs::metadata(r) {
                w.dir_size(r, md.dev(), 0);
            }
        }
    });
    let entries = w
        .out
        .into_inner()
        .expect(POISON)
        .into_iter()
        .map(|(p, b)| (p.to_string_lossy().into_owned(), b))
        .collect();
    let snap = Snapshot {
        time: crate::sys::now(),
        roots: roots
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect(),
        exclude: exclude
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect(),
        entries,
    };
    (
        snap,
        WalkStats {
            secs: start.elapsed().as_secs_f64(),
            errors: w.errors.into_inner(),
        },
    )
}

/// Size of one path.
pub fn measure(p: &Path) -> u64 {
    let Ok(md) = fs::symlink_metadata(p) else {
        return 0;
    };
    if !md.is_dir() {
        return md.blocks() * 512;
    }
    let w = Walker {
        exclude: &[],
        min: u64::MAX,
        max_depth: 0,
        out: Mutex::new(Vec::new()),
        seen_links: Mutex::new(HashSet::new()),
        errors: AtomicU64::new(0),
    };
    md.blocks() * 512 + pool().install(|| w.dir_size(p, md.dev(), 0))
}

/// Size from the snapshot when recorded there, measured otherwise.
pub struct Sizer {
    pub snap: Snapshot,
}

impl Sizer {
    pub fn size(&self, p: &Path) -> u64 {
        match self.snap.entries.get(&*p.to_string_lossy()) {
            Some(&b) => b,
            None => measure(p),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Growth {
    pub path: String,
    pub delta: i64,
    pub now: u64,
}

/// Folders that grew by at least `threshold`. A parent is left out when one child explains most of its growth.
pub fn growth(prev: &Snapshot, cur: &Snapshot, threshold: u64) -> Vec<Growth> {
    if !prev.comparable(cur) {
        return vec![];
    }
    let grown: Vec<Growth> = cur
        .entries
        .iter()
        .filter_map(|(p, &now)| {
            let before = prev.entries.get(p).copied().unwrap_or(0);
            let delta = now as i64 - before as i64;
            (delta >= threshold as i64).then(|| Growth {
                path: p.clone(),
                delta,
                now,
            })
        })
        .collect();
    let explained_by_child = |a: &Growth| {
        let prefix = format!("{}/", a.path);
        grown
            .iter()
            .any(|b| b.path.starts_with(&prefix) && b.delta as f64 >= 0.7 * a.delta as f64)
    };
    let mut out: Vec<Growth> = grown
        .iter()
        .filter(|a| !explained_by_child(a))
        .cloned()
        .collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.delta));
    out.truncate(10);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_hard_links_once_and_skips_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/big"), vec![1u8; 1 << 20]).unwrap();
        fs::hard_link(root.join("a/big"), root.join("a/link")).unwrap();
        std::os::unix::fs::symlink("/usr", root.join("a/sym")).unwrap();
        let once = measure(&root.join("a"));
        assert!(
            (1 << 20..2 << 20).contains(&once),
            "hard link counted twice or symlink followed: {once}"
        );
    }

    #[test]
    fn snapshot_records_big_folders_and_respects_exclude() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        for sub in ["keep", "skip", "small"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(root.join("keep/f"), vec![1u8; 2 << 20]).unwrap();
        fs::write(root.join("skip/f"), vec![1u8; 2 << 20]).unwrap();
        fs::write(root.join("small/f"), vec![1u8; 10]).unwrap();
        let (snap, _) = snapshot(
            std::slice::from_ref(&root),
            &[root.join("skip")],
            1 << 20,
            4,
        );
        let has = |p: &Path| snap.entries.contains_key(&*p.to_string_lossy());
        assert!(has(&root.join("keep")));
        assert!(!has(&root.join("skip")));
        assert!(!has(&root.join("small")));
        let total = snap.entries[&*root.to_string_lossy()];
        assert!(total < 4 << 20, "excluded folder was counted");
    }

    fn snap(entries: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            time: 0,
            roots: vec!["/r".into()],
            exclude: vec![],
            entries: entries.iter().map(|(p, b)| (p.to_string(), *b)).collect(),
        }
    }

    #[test]
    fn growth_points_at_the_cause() {
        let g = 1_000_000_000;
        let prev = snap(&[("/r", 10 * g), ("/r/a", 5 * g), ("/r/a/b", g)]);
        let cur = snap(&[("/r", 16 * g), ("/r/a", 11 * g), ("/r/a/b", 7 * g)]);
        let out = growth(&prev, &cur, 3 * g);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/r/a/b");
        assert_eq!(out[0].delta, 6 * g as i64);
    }

    #[test]
    fn growth_keeps_parent_when_spread_over_children() {
        let g = 1_000_000_000;
        let prev = snap(&[("/r", 0), ("/r/a", 0), ("/r/b", 0)]);
        let cur = snap(&[("/r", 8 * g), ("/r/a", 4 * g), ("/r/b", 4 * g)]);
        let paths: Vec<_> = growth(&prev, &cur, 3 * g)
            .into_iter()
            .map(|x| x.path)
            .collect();
        assert!(paths.contains(&"/r".to_string()));
    }

    #[test]
    fn growth_not_compared_across_different_roots() {
        let prev = Snapshot {
            roots: vec!["/x".into()],
            ..snap(&[])
        };
        let cur = snap(&[("/r", 9_000_000_000)]);
        assert!(growth(&prev, &cur, 1).is_empty());
    }
}
