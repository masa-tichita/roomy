//! Files under `~/Library/Application Support/roomy/`. Private to the user (0700 / 0600).

use crate::app::state_dir;
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub fn report_path() -> PathBuf {
    state_dir().join("report.json")
}

pub fn agent_state_path() -> PathBuf {
    state_dir().join("agent.json")
}

pub fn history_path() -> PathBuf {
    state_dir().join("history.jsonl")
}

pub fn audit_path() -> PathBuf {
    state_dir().join("audit.jsonl")
}

pub fn snapshots_dir() -> PathBuf {
    state_dir().join("snapshots")
}

pub fn lock_path() -> PathBuf {
    state_dir().join("lock")
}

pub fn log_path() -> PathBuf {
    state_dir().join("logs/agent.log")
}

fn ensure_dir(d: &Path) -> std::io::Result<()> {
    fs::create_dir_all(d)?;
    fs::set_permissions(d, fs::Permissions::from_mode(0o700))
}

pub fn load<T: DeserializeOwned>(p: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(p).ok()?).ok()
}

/// Atomic write (temp file + rename).
pub fn save<T: Serialize>(p: &Path, v: &T) -> anyhow::Result<()> {
    if let Some(d) = p.parent() {
        ensure_dir(d)?;
    }
    let tmp = p.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec(v)?)?;
        f.sync_all()?;
    }
    fs::rename(tmp, p)?;
    Ok(())
}

pub fn append_line<T: Serialize>(p: &Path, v: &T) -> anyhow::Result<()> {
    if let Some(d) = p.parent() {
        ensure_dir(d)?;
    }
    let mut f = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(p)?;
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    f.write_all(&line)?;
    Ok(())
}

pub fn read_lines<T: DeserializeOwned>(p: &Path) -> Vec<T> {
    fs::read_to_string(p)
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Keeps a line-based file under `max_bytes` by dropping the oldest half.
pub fn trim_lines(p: &Path, max_bytes: u64) {
    let Ok(md) = fs::metadata(p) else { return };
    if md.len() <= max_bytes {
        return;
    }
    if let Ok(s) = fs::read_to_string(p) {
        let lines: Vec<&str> = s.lines().collect();
        let keep = lines[lines.len() / 2..].join("\n") + "\n";
        let _ = fs::write(p, keep);
    }
}

/// Rotates a log file to `<name>.1` when it grows past `max_bytes`.
pub fn rotate_log(p: &Path, max_bytes: u64) {
    if fs::metadata(p).is_ok_and(|m| m.len() > max_bytes) {
        let _ = fs::rename(p, p.with_extension("log.1"));
    }
}

pub fn ensure_log_dir() {
    if let Some(d) = log_path().parent() {
        let _ = ensure_dir(d);
    }
}

/// Snapshot files, newest first (names are timestamps).
pub fn snapshot_files() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(snapshots_dir())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

pub fn prune_snapshots(keep: usize) {
    for old in snapshot_files().into_iter().skip(keep.max(2)) {
        let _ = fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_keeps_newest_half() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.jsonl");
        fs::write(&p, "1\n2\n3\n4\n").unwrap();
        trim_lines(&p, 4);
        assert_eq!(fs::read_to_string(&p).unwrap(), "3\n4\n");
    }

    #[test]
    fn save_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/x.json");
        save(&p, &vec![1, 2]).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(load::<Vec<i32>>(&p), Some(vec![1, 2]));
    }
}
