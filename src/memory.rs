//! Memory cleanup: swap, which apps hold compressed or swapped memory, and quitting them.
//!
//! Swap cannot be deleted: it is the memory of running apps written to disk. macOS shrinks the
//! swap files once those apps quit, so the cleanup is "quit what holds it". Only the user's own
//! processes are listed; system processes and the session roomy runs in are never offered.

use crate::i18n::{Msg, m, mf};
use crate::sys::{self, run};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A macOS app; quitting it quits its helpers too.
    App { bundle: PathBuf },
    /// Any other process (node, python, …).
    Process,
}

#[derive(Clone, Debug)]
pub struct Holder {
    /// Stable within one listing: `app:<bundle path>` or `pid:<pid>`.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The process to ask to quit (for apps: the main executable).
    pub main_pid: Option<i32>,
    pub pids: Vec<i32>,
    /// Compressed or swapped-out memory (top's CMPRS).
    pub compressed: u64,
    pub resident: u64,
    /// For processes: the command line and working directory.
    pub detail: Option<String>,
    /// Why it is not offered for quitting.
    pub protected: Option<Msg>,
}

struct Proc {
    ppid: i32,
    uid: u32,
    /// Full executable path (`ps -o comm`).
    exe: String,
}

/// `ps` for every process: pid -> (ppid, uid, executable).
fn processes() -> HashMap<i32, Proc> {
    let Some(out) = run(
        &["ps", "-axo", "pid=,ppid=,uid=,comm="],
        None,
        Duration::from_secs(10),
    ) else {
        return HashMap::new();
    };
    out.stdout
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let uid = it.next()?.parse().ok()?;
            let exe = it.collect::<Vec<_>>().join(" ");
            Some((pid, Proc { ppid, uid, exe }))
        })
        .collect()
}

/// Sizes in `top` look like `1004M`, `829M+`, `12K-`, `1.2G`, `0B`.
pub fn parse_top_size(s: &str) -> Option<u64> {
    let s = s.trim_end_matches(['+', '-']);
    let (num, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic())?);
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// `top -stats pid,cmprs,mem` -> pid -> (compressed, resident).
pub fn parse_top(s: &str) -> HashMap<i32, (u64, u64)> {
    let mut out = HashMap::new();
    let mut in_table = false;
    for line in s.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() == Some(&"PID") {
            in_table = true;
            continue;
        }
        if !in_table || cols.len() < 3 {
            continue;
        }
        if let (Ok(pid), Some(c), Some(r)) = (
            cols[0].parse::<i32>(),
            parse_top_size(cols[1]),
            parse_top_size(cols[2]),
        ) {
            out.insert(pid, (c, r));
        }
    }
    out
}

/// The outermost `.app` bundle in an executable path.
pub fn bundle_of(exe: &str) -> Option<PathBuf> {
    let i = exe.find(".app/")?;
    Some(PathBuf::from(&exe[..i + 4]))
}

/// Whether `exe` is the main executable of an app named `app_dir` (e.g. `Arc.app`), wherever
/// that app lives: `…/Arc.app/Contents/MacOS/<exe>`, not a helper nested deeper.
pub fn is_main_executable(exe: &str, app_dir: &str) -> bool {
    let marker = format!("/{app_dir}/Contents/MacOS/");
    match exe.find(&marker) {
        Some(i) => {
            bundle_of(exe).is_some_and(|b| b.to_string_lossy().len() == i + 1 + app_dir.len())
        }
        None => false,
    }
}

fn is_system(exe: &str) -> bool {
    ["/System/", "/usr/", "/sbin/", "/bin/", "/Library/Apple/"]
        .iter()
        .any(|p| exe.starts_with(p))
}

/// Our own process chain (roomy, its shell, an agent, the terminal …), without launchd.
fn ancestors(procs: &HashMap<i32, Proc>) -> Vec<i32> {
    let mut chain = vec![];
    let mut pid = std::process::id() as i32;
    while pid > 1 && chain.len() < 64 {
        chain.push(pid);
        pid = procs.get(&pid).map_or(1, |p| p.ppid);
    }
    chain
}

fn chain_of(pid: i32, procs: &HashMap<i32, Proc>) -> Vec<i32> {
    let mut chain = vec![];
    let mut p = pid;
    while p > 1 && chain.len() < 64 {
        chain.push(p);
        p = procs.get(&p).map_or(1, |x| x.ppid);
    }
    chain
}

/// Command line and working directory of a process, for recognising dev servers and the like.
fn describe_process(pid: i32) -> Option<String> {
    let cmd = run(
        &["ps", "-o", "command=", "-p", &pid.to_string()],
        None,
        Duration::from_secs(5),
    )?
    .stdout
    .trim()
    .to_string();
    let cwd = run(
        &["lsof", "-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"],
        None,
        Duration::from_secs(5),
    )
    .and_then(|o| {
        o.stdout
            .lines()
            .find_map(|l| l.strip_prefix('n').map(str::to_string))
    });
    let short: String = cmd.chars().take(120).collect();
    // The folder first: it is what tells a dev server or an MCP server apart.
    Some(match cwd {
        Some(d) => format!("{} · {short}", sys::tilde(Path::new(&d))),
        None => short,
    })
}

/// Apps and processes holding the most compressed or swapped memory, biggest first.
pub fn holders(limit: usize) -> Vec<Holder> {
    let procs = processes();
    let Some(top) = run(
        &[
            "top",
            "-l",
            "1",
            "-o",
            "cmprs",
            "-stats",
            "pid,cmprs,mem",
            "-n",
            "400",
        ],
        None,
        Duration::from_secs(15),
    ) else {
        return vec![];
    };
    let sizes = parse_top(&top.stdout);
    let uid = sys::uid();
    let me = ancestors(&procs);
    // The terminal app at the top of our chain is shared by every tab; everything below it is
    // "this session".
    let session: HashSet<i32> = me
        .iter()
        .take(me.len().saturating_sub(1))
        .copied()
        .collect();

    let mut groups: BTreeMap<String, Holder> = BTreeMap::new();
    for (&pid, &(compressed, resident)) in &sizes {
        let Some(p) = procs.get(&pid) else { continue };
        if p.uid != uid || pid == std::process::id() as i32 {
            continue;
        }
        let (id, name, kind) = match bundle_of(&p.exe) {
            // Grouped by bundle name, not path: an app launched from App Translocation runs its
            // main process from a temporary copy while helpers may come from /Applications.
            Some(b) => {
                let name = b
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (format!("app:{name}"), name, Kind::App { bundle: b })
            }
            None => (
                format!("pid:{pid}"),
                p.exe.rsplit('/').next().unwrap_or(&p.exe).to_string(),
                Kind::Process,
            ),
        };
        let h = groups.entry(id.clone()).or_insert_with(|| Holder {
            id,
            name,
            kind: kind.clone(),
            main_pid: None,
            pids: vec![],
            compressed: 0,
            resident: 0,
            detail: None,
            protected: None,
        });
        h.pids.push(pid);
        h.compressed += compressed;
        h.resident += resident;
    }

    for h in groups.values_mut() {
        h.pids.sort_unstable();
        match &h.kind {
            Kind::App { bundle } => {
                let app_dir = bundle
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                h.main_pid = procs
                    .iter()
                    .filter(|(_, p)| p.uid == uid)
                    .find(|(_, p)| is_main_executable(&p.exe, &app_dir))
                    .map(|(pid, _)| *pid);
                let exe = bundle.to_string_lossy();
                if is_system(&exe) {
                    h.protected = Some(m("memory.protected_system"));
                } else if h.main_pid.is_none() {
                    h.protected = Some(m("memory.protected_no_main"));
                }
            }
            Kind::Process => {
                let pid = h.pids[0];
                h.main_pid = Some(pid);
                let exe = procs.get(&pid).map(|p| p.exe.as_str()).unwrap_or_default();
                if is_system(exe) {
                    h.protected = Some(m("memory.protected_system"));
                }
            }
        }
        let in_session = h
            .pids
            .iter()
            .chain(h.main_pid.iter())
            .any(|pid| chain_of(*pid, &procs).iter().any(|a| session.contains(a)));
        if h.protected.is_none() && in_session {
            h.protected = Some(m("memory.protected_session"));
        }
    }

    let mut list: Vec<Holder> = groups.into_values().filter(|h| h.compressed > 0).collect();
    list.sort_by_key(|h| std::cmp::Reverse(h.compressed));
    list.truncate(limit);
    for h in &mut list {
        if h.kind == Kind::Process {
            h.detail = describe_process(h.pids[0]);
        }
    }
    list
}

/// Space used by other APFS volumes on the same disk as the data volume (swap), and by
/// simulator runtime images.
#[derive(Clone, Debug)]
pub struct Outside {
    pub name: String,
    pub kind: &'static str,
    pub bytes: u64,
}

pub fn outside_space() -> Vec<Outside> {
    let Some(out) = run(
        &["diskutil", "apfs", "list", "-plist"],
        None,
        Duration::from_secs(15),
    ) else {
        return vec![];
    };
    parse_apfs_list(out.stdout.as_bytes())
}

pub fn parse_apfs_list(bytes: &[u8]) -> Vec<Outside> {
    let Ok(v) = plist::Value::from_reader(std::io::Cursor::new(bytes)) else {
        return vec![];
    };
    let mut out = vec![];
    let containers = v
        .as_dictionary()
        .and_then(|d| d.get("Containers"))
        .and_then(|c| c.as_array());
    for c in containers.into_iter().flatten() {
        let vols = c
            .as_dictionary()
            .and_then(|d| d.get("Volumes"))
            .and_then(|v| v.as_array());
        for vol in vols.into_iter().flatten() {
            let Some(d) = vol.as_dictionary() else {
                continue;
            };
            let name = d
                .get("Name")
                .and_then(|n| n.as_string())
                .unwrap_or_default()
                .to_string();
            let roles: Vec<&str> = d
                .get("Roles")
                .and_then(|r| r.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_string()).collect())
                .unwrap_or_default();
            let bytes = d
                .get("CapacityInUse")
                .and_then(plist::Value::as_unsigned_integer)
                .unwrap_or(0);
            let kind = if roles.contains(&"VM") {
                "swap"
            } else if name.contains("Simulator") {
                "simulator_runtime"
            } else {
                continue;
            };
            out.push(Outside { name, kind, bytes });
        }
    }
    out.sort_by_key(|o| std::cmp::Reverse(o.bytes));
    out
}

pub enum QuitOutcome {
    Quit,
    /// Asked to quit but still running after the wait (e.g. showing a save dialog).
    Asked,
    Failed(String),
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists and may be signalled.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Asks one holder to quit: apps through `NSRunningApplication.terminate` (the normal Quit, no
/// Automation permission needed), other processes with SIGTERM. Never forces.
pub fn quit(h: &Holder) -> QuitOutcome {
    if let Some(why) = &h.protected {
        return QuitOutcome::Failed(why.render());
    }
    let Some(pid) = h.main_pid else {
        return QuitOutcome::Failed(m("memory.protected_no_main").render());
    };
    let requested = match h.kind {
        Kind::App { .. } => {
            use objc2_app_kit::NSRunningApplication;
            match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
                Some(app) => app.terminate(),
                None => false,
            }
        }
        // SAFETY: plain signal delivery to a pid we listed for this user.
        Kind::Process => unsafe { libc::kill(pid, libc::SIGTERM) == 0 },
    };
    if !requested {
        return QuitOutcome::Failed(mf("memory.quit_refused", &[("pid", &pid)]).render());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !alive(pid) {
            return QuitOutcome::Quit;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    QuitOutcome::Asked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_sizes() {
        assert_eq!(parse_top_size("1004M"), Some(1004 << 20));
        assert_eq!(parse_top_size("829M+"), Some(829 << 20));
        assert_eq!(parse_top_size("12K-"), Some(12 << 10));
        assert_eq!(parse_top_size("0B"), Some(0));
        assert_eq!(parse_top_size("1.5G"), Some(3 << 29));
        assert_eq!(parse_top_size("n/a"), None);
    }

    #[test]
    fn top_table() {
        let s = "Processes: 600 total\nLoad Avg: 1.0\n\nPID    CMPRS MEM  \n28862  1002M 1899M\n3679   671M- 679M+\n";
        let t = parse_top(s);
        assert_eq!(t[&28862], (1002 << 20, 1899 << 20));
        assert_eq!(t[&3679], (671 << 20, 679 << 20));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn bundles() {
        let chrome = "/Applications/Google Chrome.app/Contents/Frameworks/X.framework/Helpers/Helper (Renderer).app/Contents/MacOS/Helper";
        assert_eq!(
            bundle_of(chrome),
            Some(PathBuf::from("/Applications/Google Chrome.app"))
        );
        assert_eq!(bundle_of("/opt/homebrew/bin/node"), None);
        assert!(is_system(
            "/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"
        ));
        // the main process of a translocated app, and helpers that are not the main executable
        let translocated =
            "/private/var/folders/x/T/AppTranslocation/ABC/d/Arc.app/Contents/MacOS/Arc";
        assert!(is_main_executable(translocated, "Arc.app"));
        assert!(is_main_executable(
            "/Applications/Arc.app/Contents/MacOS/Arc",
            "Arc.app"
        ));
        assert!(!is_main_executable(
            "/Applications/Arc.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper",
            "Arc.app"
        ));
        assert!(!is_system("/Applications/Arc.app/Contents/MacOS/Arc"));
    }

    #[test]
    fn apfs_volumes() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Containers</key><array>
 <dict><key>Volumes</key><array>
  <dict><key>Name</key><string>Data</string><key>Roles</key><array><string>Data</string></array><key>CapacityInUse</key><integer>375000000000</integer></dict>
  <dict><key>Name</key><string>VM</string><key>Roles</key><array><string>VM</string></array><key>CapacityInUse</key><integer>17182285824</integer></dict>
 </array></dict>
 <dict><key>Volumes</key><array>
  <dict><key>Name</key><string>iOS 26.5 Simulator</string><key>Roles</key><array/><key>CapacityInUse</key><integer>17571344384</integer></dict>
 </array></dict>
</array></dict></plist>"#;
        let v = parse_apfs_list(plist);
        assert_eq!(v.len(), 2);
        assert_eq!(
            (v[0].kind, v[0].bytes),
            ("simulator_runtime", 17_571_344_384)
        );
        assert_eq!((v[1].kind, v[1].bytes), ("swap", 17_182_285_824));
    }

    #[test]
    fn own_session_is_never_offered() {
        // This test process runs inside our chain; listing must not crash, and anything in the
        // session must come out protected.
        for h in holders(50) {
            if h.pids.contains(&(std::process::id() as i32)) {
                assert!(h.protected.is_some());
            }
        }
    }
}
