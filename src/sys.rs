//! Talking to the OS: disk, memory, processes, power, permissions, external commands, notifications.

use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{IsTerminal, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn home() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from("/"),
    }
}

/// `~` and `~/x` to absolute paths.
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Absolute path back to `~/x` for display.
pub fn tilde(p: &Path) -> String {
    match p.strip_prefix(home()) {
        Ok(r) if r.as_os_str().is_empty() => "~".into(),
        Ok(r) => format!("~/{}", r.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn now() -> i64 {
    chrono::Local::now().timestamp()
}

pub fn fmt_time(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// Decimal units, like Finder.
pub fn fmt_bytes(b: u64) -> String {
    if b == 0 {
        return "0 B".into();
    }
    let f = b as f64;
    if f >= 1e9 {
        format!("{:.1} GB", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.0} MB", f / 1e6)
    } else {
        format!("{:.0} KB", f / 1e3)
    }
}

/// Binary units for memory, like Activity Monitor (36 GB of RAM, not 38.7).
pub fn fmt_mem(b: u64) -> String {
    let f = b as f64 / (1u64 << 30) as f64;
    if f >= 10.0 {
        format!("{f:.0} GB")
    } else if f >= 1.0 {
        format!("{f:.1} GB")
    } else {
        format!("{:.0} MB", b as f64 / (1u64 << 20) as f64)
    }
}

pub fn fmt_delta(d: i64) -> String {
    format!(
        "{}{}",
        if d < 0 { "-" } else { "+" },
        fmt_bytes(d.unsigned_abs())
    )
}

pub fn days_since(t: i64) -> i64 {
    (now() - t).max(0) / 86_400
}

pub fn mtime(p: &Path) -> Option<i64> {
    let m = std::fs::symlink_metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

// ---- disk ----

#[derive(Clone, Copy, Default)]
pub struct Disk {
    pub total: u64,
    /// Free space (`statfs`, like `df`). Thresholds and alerts use this one.
    pub avail: u64,
    /// Free plus purgeable space, the "available" of Finder. None when unknown.
    pub avail_incl_purgeable: Option<u64>,
}

/// The data volume (where user files live). Same numbers as `df`; purgeable space is not counted as free.
pub fn disk() -> Disk {
    for mount in [c"/System/Volumes/Data", c"/"] {
        // SAFETY: `statfs` is plain old data, so all-zero is a valid value; the path is a
        // NUL-terminated literal and `s` is a valid out-pointer. Fields are read only on success.
        unsafe {
            let mut s: libc::statfs = std::mem::zeroed();
            if libc::statfs(mount.as_ptr(), &raw mut s) == 0 {
                let b = u64::from(s.f_bsize);
                return Disk {
                    total: s.f_blocks * b,
                    avail: s.f_bavail * b,
                    avail_incl_purgeable: avail_incl_purgeable(),
                };
            }
        }
    }
    Disk::default()
}

/// Free plus purgeable space when it adds something visible next to free space (0.1 GB or more).
pub fn purgeable_shown(avail: u64, incl_purgeable: Option<u64>) -> Option<u64> {
    incl_purgeable.filter(|&p| p >= avail + 100_000_000)
}

/// Available space as Finder and System Settings show it: free space plus purgeable space
/// (caches, local snapshots, …) macOS clears when something important needs room.
/// None when the volume does not report it.
pub fn avail_incl_purgeable() -> Option<u64> {
    use objc2_foundation::{
        NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str("/System/Volumes/Data"));
    let mut value = None;
    // SAFETY: `value` is the out-parameter type the method declares (an optional object), and
    // the key is Foundation's own constant, which is valid for the life of the process.
    unsafe {
        url.getResourceValue_forKey_error(
            &mut value,
            NSURLVolumeAvailableCapacityForImportantUsageKey,
        )
        .ok()?;
    }
    let n = value?.downcast::<NSNumber>().ok()?;
    u64::try_from(n.longLongValue()).ok().filter(|&b| b > 0)
}

/// Bytes of a file that are not shared with APFS clones: what deleting it frees
/// (`ATTR_CMNEXT_PRIVATESIZE`, macOS 10.15+). None if the file system does not report it.
/// Does not follow symbolic links.
pub fn private_size(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    /// What getattrlist writes: the length, the attributes it returned, then the private size.
    /// Every field is naturally aligned, so `repr(C)` matches the kernel's packing.
    #[repr(C)]
    struct Buf {
        length: u32,
        returned: libc::attribute_set_t,
        private_size: libc::off_t,
    }
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: both are plain old data, so all-zero is a valid value.
    let (mut req, mut buf) = unsafe {
        (
            std::mem::zeroed::<libc::attrlist>(),
            std::mem::zeroed::<Buf>(),
        )
    };
    req.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    req.commonattr = libc::ATTR_CMN_RETURNED_ATTRS;
    // With FSOPT_ATTR_CMN_EXTENDED the `forkattr` group selects extended common attributes.
    req.forkattr = libc::ATTR_CMNEXT_PRIVATESIZE;
    // SAFETY: the path is NUL-terminated, `req` is a valid attribute list, and the kernel
    // writes at most `size_of::<Buf>()` bytes into `buf`, which is read only on success.
    let r = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&raw mut req).cast(),
            (&raw mut buf).cast(),
            size_of::<Buf>(),
            libc::FSOPT_NOFOLLOW | libc::FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    // Volumes that cannot tell leave the attribute out of `returned`.
    (r == 0 && buf.returned.forkattr & libc::ATTR_CMNEXT_PRIVATESIZE != 0)
        .then(|| u64::try_from(buf.private_size).unwrap_or(0))
}

// ---- memory ----

fn sysctl_bytes(name: &str) -> Option<Vec<u8>> {
    let c = CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    // SAFETY: first call asks only for the length (null buffer); the second passes a buffer of
    // exactly that length and the kernel writes at most `len` bytes, which we then truncate to.
    unsafe {
        if libc::sysctlbyname(
            c.as_ptr(),
            std::ptr::null_mut(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(
            c.as_ptr(),
            buf.as_mut_ptr().cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        Some(buf)
    }
}

fn sysctl_int(name: &str) -> Option<i64> {
    let b = sysctl_bytes(name)?;
    match b.len() {
        4 => Some(i64::from(i32::from_ne_bytes(b[..4].try_into().ok()?))),
        8 => Some(i64::from_ne_bytes(b[..8].try_into().ok()?)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub struct Mem {
    pub total: u64,
    /// Same as "System-wide memory free percentage" of `memory_pressure`.
    pub free_pct: Option<i64>,
    /// 1 = normal, 2 = warning, 4 = critical.
    pub pressure: i64,
    pub swap_used: u64,
}

pub fn mem() -> Mem {
    // vm.swapusage is struct xsw_usage { u64 total, avail, used; ... }
    let swap_used = sysctl_bytes("vm.swapusage")
        .and_then(|b| b.get(16..24).and_then(|s| s.try_into().ok()))
        .map_or(0, u64::from_ne_bytes);
    Mem {
        total: sysctl_int("hw.memsize").unwrap_or(0) as u64,
        free_pct: sysctl_int("kern.memorystatus_level"),
        pressure: sysctl_int("kern.memorystatus_vm_pressure_level").unwrap_or(0),
        swap_used,
    }
}

pub fn pressure_label_i18n(level: i64) -> String {
    use crate::i18n::t;
    match level {
        1 => t("pressure.normal"),
        2 => t("pressure.warn"),
        4 => t("pressure.critical"),
        _ => t("pressure.unknown"),
    }
}

pub fn os_version() -> String {
    sysctl_bytes("kern.osproductversion")
        .map(|b| {
            String::from_utf8_lossy(&b)
                .trim_end_matches('\0')
                .to_string()
        })
        .unwrap_or_default()
}

/// The Mac's processor as Apple tools name it, also when roomy itself runs under Rosetta.
pub fn host_arch() -> &'static str {
    if sysctl_int("hw.optional.arm64") == Some(1) {
        "arm64"
    } else {
        "x86_64"
    }
}

// ---- processes ----

pub struct AppMem {
    pub name: String,
    pub rss: u64,
    pub procs: usize,
}

/// Resident memory grouped by app bundle, so helper processes count toward their app.
/// Shared pages are counted more than once, so this is an estimate.
pub fn top_apps(n: usize) -> Vec<AppMem> {
    let Some(out) = run(&["ps", "-axo", "rss=,comm="], None, Duration::from_secs(10)) else {
        return vec![];
    };
    let mut by_app: HashMap<String, (u64, usize)> = HashMap::new();
    for line in out.stdout.lines() {
        let Some((rss, comm)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(kb) = rss.parse::<u64>() else { continue };
        let e = by_app.entry(app_name(comm.trim())).or_default();
        e.0 += kb * 1024;
        e.1 += 1;
    }
    let mut v: Vec<AppMem> = by_app
        .into_iter()
        .map(|(name, (rss, procs))| AppMem { name, rss, procs })
        .collect();
    v.sort_by(|a, b| b.rss.cmp(&a.rss).then_with(|| a.name.cmp(&b.name)));
    v.truncate(n);
    v
}

pub fn app_name(comm: &str) -> String {
    comm.split('/')
        .find_map(|c| c.strip_suffix(".app"))
        .map_or_else(
            || comm.rsplit('/').next().unwrap_or(comm).to_string(),
            str::to_string,
        )
}

/// Command lines of all processes, for "is it running" checks.
pub fn cmdlines() -> Vec<String> {
    run(
        &["ps", "-axww", "-o", "command="],
        None,
        Duration::from_secs(10),
    )
    .map(|o| o.stdout.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

/// Whether a process command line matches a "running" pattern. A pattern containing `/` or a
/// space is looked for anywhere (`/Xcode.app/Contents/MacOS/Xcode`). A plain name must be a whole
/// word: the file name of a program or argument (`/usr/bin/xcodebuild`, `…/Homebrew/brew.rb`) or
/// the last part of a Java class (`org.gradle.launcher.daemon.bootstrap.GradleDaemon`), so
/// `xcodebuild` does not match `xcodebuildmcp`.
pub fn cmdline_matches(cmdline: &str, pattern: &str) -> bool {
    if pattern.contains(['/', ' ']) {
        return cmdline.contains(pattern);
    }
    cmdline.split_whitespace().any(|word| {
        let word = word.trim_matches(['"', '\'', ';', '(', ')']);
        let name = word.rsplit('/').next().unwrap_or(word);
        name == pattern || name.strip_suffix(pattern).is_some_and(|s| s.ends_with('.'))
    })
}

/// Working directories of the current user's processes, for "is this worktree in use" checks.
pub fn process_cwds() -> Vec<PathBuf> {
    let uid = uid().to_string();
    run(
        &["lsof", "-a", "-d", "cwd", "-u", &uid, "-Fn"],
        None,
        Duration::from_secs(30),
    )
    .map(|o| {
        o.stdout
            .lines()
            .filter_map(|l| l.strip_prefix('n'))
            .map(PathBuf::from)
            .collect()
    })
    .unwrap_or_default()
}

/// The current user's id.
pub fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

// ---- power ----

pub struct Power {
    pub on_ac: bool,
    pub battery_pct: Option<u32>,
}

pub fn power() -> Power {
    run(&["pmset", "-g", "batt"], None, Duration::from_secs(5)).map_or(
        Power {
            on_ac: true,
            battery_pct: None,
        },
        |o| parse_pmset(&o.stdout),
    )
}

pub fn parse_pmset(s: &str) -> Power {
    let on_ac = !s.contains("'Battery Power'");
    let battery_pct = s
        .lines()
        .find(|l| l.contains("InternalBattery"))
        .and_then(|l| {
            let pct = l.split('\t').nth(1)?.split('%').next()?;
            pct.trim().parse().ok()
        });
    Power { on_ac, battery_pct }
}

// ---- permissions ----

/// Full Disk Access of this process (or of the terminal it runs in).
/// The TCC database is readable only with FDA.
pub fn has_full_disk_access() -> bool {
    File::open(home().join("Library/Application Support/com.apple.TCC/TCC.db")).is_ok()
}

/// Folders guarded by TCC. Touching them without permission shows a dialog, so background
/// scans without Full Disk Access stay out of them.
pub fn tcc_protected() -> Vec<PathBuf> {
    [
        "~/Desktop",
        "~/Documents",
        "~/Downloads",
        "~/Library/Mail",
        "~/Library/Messages",
        "~/Library/Safari",
        "~/Library/Calendars",
        "~/Library/Containers",
        "~/Library/Group Containers",
        "~/Library/Application Support/AddressBook",
        "~/Library/Application Support/CallHistoryDB",
        "~/Library/Application Support/MobileSync",
        "~/Library/Application Support/com.apple.TCC",
        "~/Library/Mobile Documents",
        "~/Pictures/Photos Library.photoslibrary",
    ]
    .iter()
    .map(|p| expand(p))
    .collect()
}

/// Lower our disk I/O priority so scans do not slow down the user's work.
pub fn throttle_io() {
    // SAFETY: the declaration matches <sys/resource.h>; it takes plain integers and only changes
    // this process's I/O policy, so calling it cannot cause undefined behaviour.
    unsafe extern "C" {
        safe fn setiopolicy_np(
            iotype: libc::c_int,
            scope: libc::c_int,
            policy: libc::c_int,
        ) -> libc::c_int;
    }
    const IOPOL_TYPE_DISK: libc::c_int = 0;
    const IOPOL_SCOPE_PROCESS: libc::c_int = 0;
    const IOPOL_THROTTLE: libc::c_int = 3;
    setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_PROCESS, IOPOL_THROTTLE);
}

/// Exclusive lock held while the returned file is alive. None if another roomy holds it.
#[must_use = "the lock is released as soon as the returned File is dropped"]
pub fn try_lock(path: &Path) -> Option<File> {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    // SAFETY: `f` is open for the duration of the call, so its descriptor is valid.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    (r == 0).then_some(f)
}

// ---- external commands ----

pub struct Out {
    pub ok: bool,
    pub stdout: String,
}

/// Runs a command with a timeout. None if it cannot start or times out.
pub fn run<S: AsRef<OsStr>>(argv: &[S], cwd: Option<&Path>, timeout: Duration) -> Option<Out> {
    let mut cmd = Command::new(&argv[0]);
    // Keep git from rewriting index files while we only look (ignored by everything else).
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let buf = reader.join().ok()?;
                return Some(Out {
                    ok: status.success(),
                    stdout: String::from_utf8_lossy(&buf).into_owned(),
                });
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
}

/// Finds a command in PATH (plus the usual Homebrew locations, which launchd's PATH lacks).
pub fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':')
        .map(PathBuf::from)
        .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from))
        .map(|d| d.join(cmd))
        .find(|p| p.is_file())
}

/// PATH for launchd jobs: the standard directories plus Homebrew and per-user tool locations.
pub fn agent_path() -> String {
    let mut dirs: Vec<String> = vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    for d in ["~/.local/bin", "~/.cargo/bin"] {
        dirs.push(expand(d).display().to_string());
    }
    dirs.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(String::from));
    dirs.join(":")
}

// ---- trash ----

/// Moves to the Trash with Finder's semantics (Put Back works).
pub fn trash(path: &Path) -> Result<(), String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let s = path.to_str().ok_or("path is not UTF-8")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(s));
    let fm = NSFileManager::defaultManager();
    fm.trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| e.localizedDescription().to_string())
}

// ---- notifications ----

pub fn notifier() -> &'static str {
    if which("terminal-notifier").is_some() {
        "terminal-notifier"
    } else {
        "osascript"
    }
}

/// `ROOMY_NO_NOTIFY=1` silences every notification (the test sandbox sets it).
pub fn notifications_disabled() -> bool {
    std::env::var("ROOMY_NO_NOTIFY").is_ok_and(|v| !v.is_empty() && v != "0")
}

pub fn notify(subtitle: &str, body: &str) {
    if notifications_disabled() {
        return;
    }
    let t = Duration::from_secs(10);
    if let Some(tn) = which("terminal-notifier") {
        let group = format!("{}-{subtitle}", crate::app::NAME);
        let args: [&OsStr; 9] = [
            tn.as_os_str(),
            "-title".as_ref(),
            crate::app::NAME.as_ref(),
            "-subtitle".as_ref(),
            subtitle.as_ref(),
            "-message".as_ref(),
            body.as_ref(),
            "-group".as_ref(),
            group.as_ref(),
        ];
        let _ = run(&args, None, t);
        return;
    }
    let script = format!(
        "display notification (item 2 of argv) with title \"{}\" subtitle (item 1 of argv)",
        crate::app::NAME
    );
    let _ = run(
        &[
            "osascript",
            "-e",
            "on run argv",
            "-e",
            &script,
            "-e",
            "end run",
            subtitle,
            body,
        ],
        None,
        t,
    );
}

// ---- terminal styling ----

pub struct Sty {
    on: bool,
}

impl Sty {
    pub fn new() -> Self {
        Sty {
            on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
}

/// Columns a string takes in a terminal (CJK characters are two columns wide).
pub fn width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Width of a column that fits every item, for labels whose length depends on the language.
pub fn column_width<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> usize {
    items
        .into_iter()
        .map(|s| width(s.as_ref()))
        .max()
        .unwrap_or(0)
}

/// Pads to a display width.
pub fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

pub fn pad_left(s: &str, w: usize) -> String {
    format!("{}{s}", " ".repeat(w.saturating_sub(width(s))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_patterns_match_whole_names() {
        let m = cmdline_matches;
        assert!(m("/usr/bin/xcodebuild -scheme App test", "xcodebuild"));
        assert!(m("xcodebuild build", "xcodebuild"));
        assert!(m("sh -c \"xcodebuild test\"", "xcodebuild"));
        assert!(!m("npm exec xcodebuildmcp@latest mcp", "xcodebuild"));
        assert!(!m(
            "node /Users/me/.npm/_npx/1/node_modules/.bin/xcodebuildmcp mcp",
            "xcodebuild"
        ));
        assert!(m(
            "/opt/homebrew/opt/ruby/bin/ruby /opt/homebrew/Library/Homebrew/brew.rb upgrade",
            "brew.rb"
        ));
        assert!(m(
            "java -cp x.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.5",
            "GradleDaemon"
        ));
        assert!(!m("java MyGradleDaemonTool", "GradleDaemon"));
        // Patterns with a path or a space are looked for anywhere.
        assert!(m(
            "/Applications/Xcode.app/Contents/MacOS/Xcode -psn_0",
            "/Xcode.app/Contents/MacOS/Xcode"
        ));
        assert!(m("/Users/me/.cargo/bin/cargo build", "/cargo "));
    }

    #[test]
    fn pmset_on_battery() {
        let s = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=123)\t85%; discharging; 5:00 remaining present: true\n";
        let p = parse_pmset(s);
        assert!(!p.on_ac);
        assert_eq!(p.battery_pct, Some(85));
    }

    #[test]
    fn pmset_desktop() {
        let p = parse_pmset("Now drawing from 'AC Power'\n");
        assert!(p.on_ac);
        assert_eq!(p.battery_pct, None);
    }

    #[test]
    fn app_names() {
        assert_eq!(
            app_name(
                "/Applications/Google Chrome.app/Contents/Frameworks/Helper (Renderer).app/Contents/MacOS/x"
            ),
            "Google Chrome"
        );
        assert_eq!(app_name("/usr/libexec/logd"), "logd");
        assert_eq!(app_name("next-server"), "next-server");
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(fmt_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(fmt_bytes(320_000_000), "320 MB");
        assert_eq!(fmt_delta(-2_000_000_000), "-2.0 GB");
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_mem(36 << 30), "36 GB");
        assert_eq!(fmt_mem(3 << 29), "1.5 GB");
    }

    #[test]
    fn available_incl_purgeable_is_reported() {
        // Every APFS data volume reports it; purgeable space only ever adds to free space,
        // but both numbers move, so allow some slack.
        let d = disk();
        let p = d.avail_incl_purgeable.expect("not reported");
        assert!(p <= d.total, "{p} > total {}", d.total);
        assert!(p + 1_000_000_000 >= d.avail, "{p} < free {}", d.avail);
    }

    #[test]
    fn private_size_of_clones() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        // incompressible, so APFS really allocates it
        let data: Vec<u8> = (0..4u32 << 20)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        // flushed, so the blocks are allocated before we look
        let mut f = File::create(&a).unwrap();
        std::io::Write::write_all(&mut f, &data).unwrap();
        f.sync_all().unwrap();
        assert!(private_size(&a).expect("APFS reports it") >= 4 << 20);
        let ok = Command::new("cp")
            .arg("-c")
            .arg(&a)
            .arg(&b)
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(
            private_size(&b).unwrap() < 1 << 20,
            "a clone shares its blocks"
        );
        assert_eq!(private_size(&d.path().join("missing")), None);
    }

    #[test]
    fn padding_counts_wide_chars() {
        assert_eq!(pad("安全", 6), "安全  ");
        assert_eq!(pad_left("1 GB", 6), "  1 GB");
    }

    #[test]
    fn column_fits_the_widest_label() {
        assert_eq!(
            column_width(["状態フォルダ", "フルディスクアクセス", "Rules"]),
            20
        );
        assert_eq!(column_width(["Disk", "Top memory"]), 10);
        assert_eq!(column_width(Vec::<String>::new()), 0);
    }
}
