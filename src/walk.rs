//! Parallel disk walk. One pass records the size of every large folder.
//!
//! Sizes are allocated blocks (like `du`). Hard links count once. Symbolic links are not followed
//! and other volumes are not entered. APFS clones cannot be told apart, so folders full of clones
//! look bigger than the space their removal would free; `reclaimable` measures that for removal
//! candidates only, because it costs one more system call per file.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Smallest size recorded (`snapshot_min_mb`). 0 in snapshots from older versions.
    #[serde(default)]
    pub min: u64,
    /// Deepest level below a root that was recorded. 0 in snapshots from older versions.
    #[serde(default)]
    pub max_depth: usize,
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
        min,
        max_depth,
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

/// Walker for `reclaimable`.
struct Private {
    deadline: Instant,
    seen_links: Mutex<HashSet<(u64, u64)>>,
    /// At least one file reported its private size (the volume supports it).
    reported: AtomicBool,
    late: AtomicBool,
}

impl Private {
    fn file(&self, p: &Path, md: &fs::Metadata) -> u64 {
        if md.nlink() > 1
            && !self
                .seen_links
                .lock()
                .expect(POISON)
                .insert((md.dev(), md.ino()))
        {
            return 0;
        }
        match crate::sys::private_size(p) {
            Some(b) => {
                self.reported.store(true, Ordering::Relaxed);
                b
            }
            None => md.blocks() * 512,
        }
    }

    fn dir(&self, path: &Path, dev: u64) -> u64 {
        if Instant::now() >= self.deadline {
            self.late.store(true, Ordering::Relaxed);
            return 0;
        }
        let Ok(rd) = fs::read_dir(path) else { return 0 };
        let entries: Vec<fs::DirEntry> = rd.filter_map(Result::ok).collect();
        entries
            .par_iter()
            .map(|e| {
                let Ok(md) = e.metadata() else { return 0 };
                let ft = md.file_type();
                if ft.is_symlink() {
                    0
                } else if ft.is_dir() {
                    if md.dev() == dev {
                        md.blocks() * 512 + self.dir(&e.path(), dev)
                    } else {
                        0
                    }
                } else {
                    self.file(&e.path(), &md)
                }
            })
            .sum()
    }
}

/// What removing `paths` would really free: like `measure`, but each file counts only the
/// blocks it does not share with APFS clones. Blocks shared only among `paths` themselves are
/// left out too, so this is a lower bound. None when the volume does not report private sizes
/// or the walk is still running at `deadline` (too many files).
pub fn reclaimable(paths: &[PathBuf], deadline: Instant) -> Option<u64> {
    let w = Private {
        deadline,
        seen_links: Mutex::new(HashSet::new()),
        reported: AtomicBool::new(false),
        late: AtomicBool::new(false),
    };
    let total: u64 = pool().install(|| {
        paths
            .par_iter()
            .map(|p| {
                let Ok(md) = fs::symlink_metadata(p) else {
                    return 0;
                };
                if md.is_dir() {
                    md.blocks() * 512 + w.dir(p, md.dev())
                } else if md.file_type().is_symlink() {
                    0
                } else {
                    w.file(p, &md)
                }
            })
            .sum()
    });
    (w.reported.into_inner() && !w.late.into_inner()).then_some(total)
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

    /// Whether the walk went through `p` deep enough to record it: under a root, not excluded,
    /// within `max_depth`. Such a folder missing from the snapshot is smaller than `snap.min`.
    pub fn covers(&self, p: &Path) -> bool {
        let s = &self.snap;
        !s.exclude.iter().any(|e| p.starts_with(e))
            && s.roots.iter().any(|r| {
                p.strip_prefix(r)
                    .is_ok_and(|rest| rest.components().count() <= s.max_depth)
            })
    }

    /// Size of `p` if it is at least `min`. Folders the walk already found to be smaller are not
    /// walked again.
    pub fn size_at_least(&self, p: &Path, min: u64) -> Option<u64> {
        let b = match self.snap.entries.get(&*p.to_string_lossy()) {
            Some(&b) => b,
            None if self.snap.min <= min && self.covers(p) => return None,
            None => measure(p),
        };
        (b >= min).then_some(b)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Growth {
    pub path: String,
    pub delta: i64,
    pub now: u64,
}

/// Folders that grew by at least `threshold`. A parent is left out when one child explains most of
/// its growth. When a shown parent's growth is spread over several smaller parts (three or more,
/// each under half of it, like the devices in `XCTestDevices`), the parts are left out instead, so
/// one place does not fill the list. Scan roots never stand in for their parts.
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
    let shown: Vec<&Growth> = grown.iter().filter(|a| !explained_by_child(a)).collect();
    let small_parts = |p: &Growth| -> Vec<&str> {
        let prefix = format!("{}/", p.path);
        shown
            .iter()
            .filter(|a| a.path.starts_with(&prefix) && (a.delta as f64) < 0.5 * p.delta as f64)
            .map(|a| a.path.as_str())
            .collect()
    };
    let collapsed: HashSet<&str> = shown
        .iter()
        .filter(|p| !cur.roots.contains(&p.path))
        .map(|p| small_parts(p))
        .filter(|parts| parts.len() >= 3)
        .flatten()
        .collect();
    let mut out: Vec<Growth> = shown
        .into_iter()
        .filter(|a| !collapsed.contains(a.path.as_str()))
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

    #[test]
    fn reclaimable_leaves_out_blocks_shared_with_clones() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("clones/sub")).unwrap();
        // incompressible, so APFS really allocates it
        let data: Vec<u8> = (0..8u32 << 20)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let write = |p: &Path, data: &[u8]| {
            // flushed, so the blocks are allocated before we look
            let mut f = fs::File::create(p).unwrap();
            std::io::Write::write_all(&mut f, data).unwrap();
            f.sync_all().unwrap();
        };
        write(&root.join("original"), &data);
        for to in ["clones/a", "clones/sub/b"] {
            let ok = std::process::Command::new("cp")
                .arg("-c")
                .arg(root.join("original"))
                .arg(root.join(to))
                .status()
                .unwrap();
            assert!(ok.success(), "cp -c failed");
        }
        write(&root.join("clones/own"), &data[..1 << 20]);
        let dir = root.join("clones");
        let far = Instant::now() + std::time::Duration::from_secs(60);

        let allocated = measure(&dir);
        let private = reclaimable(std::slice::from_ref(&dir), far).expect("APFS reports it");
        assert!(allocated >= 17 << 20, "allocated {allocated}");
        assert!(
            (1 << 20..2 << 20).contains(&private),
            "only the file of its own counts: {private}"
        );
        // a plain file is all private
        let own = reclaimable(&[dir.join("own")], far).unwrap();
        assert!(own >= 1 << 20 && own <= measure(&dir.join("own")), "{own}");
        // out of time: not measured rather than wrong
        assert_eq!(reclaimable(&[dir], Instant::now()), None);
    }

    fn snap(entries: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            time: 0,
            roots: vec!["/r".into()],
            exclude: vec![],
            entries: entries.iter().map(|(p, b)| (p.to_string(), *b)).collect(),
            min: 0,
            max_depth: 0,
        }
    }

    #[test]
    fn folders_the_walk_found_small_are_not_walked_again() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        for sub in ["big", "small", "skip", "a/b/deep"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(root.join("big/f"), vec![1u8; 2 << 20]).unwrap();
        fs::write(root.join("small/f"), vec![1u8; 10]).unwrap();
        let (snap, _) = snapshot(
            std::slice::from_ref(&root),
            &[root.join("skip")],
            1 << 20,
            2,
        );
        let sizer = Sizer { snap };
        assert!(sizer.covers(&root.join("small")));
        assert!(sizer.covers(&root.join("a/b")));
        assert!(
            !sizer.covers(&root.join("a/b/deep")),
            "deeper than max_depth"
        );
        assert!(!sizer.covers(&root.join("skip/x")), "excluded");
        assert!(
            !sizer.covers(Path::new("/Applications")),
            "outside the roots"
        );
        assert!(sizer.size_at_least(&root.join("big"), 1 << 20).is_some());
        assert_eq!(sizer.size_at_least(&root.join("small"), 1 << 20), None);
        // Not recorded but not known to be small either: measured.
        fs::write(root.join("skip/f"), vec![1u8; 2 << 20]).unwrap();
        assert!(sizer.size_at_least(&root.join("skip"), 1 << 20).is_some());
        // A threshold below what the snapshot records needs a measurement too.
        assert!(sizer.size_at_least(&root.join("small"), 1).is_some());
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
    fn growth_leaves_out_small_parts_of_a_shown_parent() {
        // XCTestDevices grew as a whole, each device a little.
        let g = 1_000_000_000;
        let d = "/r/XCTestDevices";
        let mut prev = vec![("/r", 100 * g), (d, 0)];
        let mut cur = vec![("/r", 200 * g), (d, 61 * g)];
        let devices: Vec<String> = (0..8).map(|i| format!("{d}/dev{i}/data")).collect();
        for p in &devices {
            prev.push((p.as_str(), 0));
            cur.push((p.as_str(), 4 * g));
        }
        // Two parts are not "spread": they stay, and so do the root's parts.
        prev.extend([("/r/b", 0), ("/r/b/big", 0), ("/r/b/other", 0)]);
        cur.extend([("/r/b", 10 * g), ("/r/b/big", 6 * g), ("/r/b/other", 4 * g)]);
        prev.extend([("/r/c", 0), ("/r/e", 0)]);
        cur.extend([("/r/c", 5 * g), ("/r/e", 5 * g)]);
        let paths: Vec<_> = growth(&snap(&prev), &snap(&cur), 3 * g)
            .into_iter()
            .map(|x| x.path)
            .collect();
        assert_eq!(
            paths,
            ["/r", d, "/r/b", "/r/b/big", "/r/c", "/r/e", "/r/b/other"]
        );
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
