//! The `LaunchAgent` that runs `roomy agent` periodically (for installs not managed by Homebrew).

use crate::app;
use crate::config::Config;
use crate::i18n::{t, tf};
use crate::state;
use crate::sys::{self, run};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn domain() -> String {
    format!("gui/{}", sys::uid())
}

pub fn is_loaded(label: &str) -> bool {
    run(
        &["launchctl", "print", &format!("{}/{label}", domain())],
        None,
        Duration::from_secs(5),
    )
    .is_some_and(|o| o.ok)
}

/// Whether `brew services` runs roomy (under either label Homebrew has used).
pub fn brew_loaded() -> bool {
    app::BREW_LABELS.iter().any(|l| is_loaded(l))
}

/// Homebrew installs live in `<prefix>/Cellar/roomy/<version>/bin`; the stable path is `<prefix>/opt/roomy/bin`.
pub fn stable_exe(exe: &Path) -> PathBuf {
    let s = exe.to_string_lossy();
    if let Some((prefix, rest)) = s.split_once(&format!("/Cellar/{}/", app::NAME))
        && let Some((_, tail)) = rest.split_once('/')
    {
        return PathBuf::from(format!("{prefix}/opt/{}/{tail}", app::NAME));
    }
    exe.to_path_buf()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn plist(exe: &Path, interval_min: u32) -> String {
    let log = xml_escape(&state::log_path().to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>agent</string></array>
  <key>StartInterval</key><integer>{secs}</integer>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>{path}</string></dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        label = app::AGENT_LABEL,
        exe = xml_escape(&exe.to_string_lossy()),
        secs = interval_min.max(5) * 60,
        path = xml_escape(&sys::agent_path()),
    )
}

pub fn install(cfg: &Config, force: bool) -> anyhow::Result<()> {
    let exe = stable_exe(&std::env::current_exe()?.canonicalize()?);
    let s = exe.to_string_lossy();
    if !force && (s.contains("/target/debug/") || s.contains("/target/release/")) {
        anyhow::bail!(tf("service.dev_binary", &[("exe", &s)]));
    }
    if !force && s.contains("/opt/") && s.contains(&format!("/opt/{}/", app::NAME)) {
        anyhow::bail!(t("service.use_brew"));
    }
    if brew_loaded() {
        anyhow::bail!(t("service.brew_loaded"));
    }
    let path = app::launch_agent_path();
    fs::create_dir_all(sys::home().join("Library/LaunchAgents"))?;
    state::ensure_log_dir();
    fs::write(&path, plist(&exe, cfg.agent.interval_min))?;
    let target = format!("{}/{}", domain(), app::AGENT_LABEL);
    let _ = run(
        &["launchctl", "bootout", &target],
        None,
        Duration::from_secs(10),
    );
    let out = run(
        &["launchctl", "bootstrap", &domain(), &path.to_string_lossy()],
        None,
        Duration::from_secs(10),
    );
    if !out.is_some_and(|o| o.ok) {
        anyhow::bail!(tf(
            "service.bootstrap_failed",
            &[("plist", &path.display())]
        ));
    }
    println!(
        "{}",
        tf("service.installed", &[("minutes", &cfg.agent.interval_min)])
    );
    println!("{}", tf("service.fda_hint", &[("exe", &exe.display())]));
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let target = format!("{}/{}", domain(), app::AGENT_LABEL);
    let _ = run(
        &["launchctl", "bootout", &target],
        None,
        Duration::from_secs(10),
    );
    let path = app::launch_agent_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    println!("{}", t("service.uninstalled"));
    Ok(())
}

pub fn status_line() -> String {
    let own = is_loaded(app::AGENT_LABEL);
    let brew = brew_loaded();
    match (own, brew) {
        (true, true) => t("service.both"),
        (true, false) => t("service.running_own"),
        (false, true) => t("service.running_brew"),
        (false, false) => t("service.not_running"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homebrew_paths_become_stable() {
        let p = Path::new("/opt/homebrew/Cellar/roomy/0.1.0/bin/roomy");
        assert_eq!(
            stable_exe(p),
            PathBuf::from("/opt/homebrew/opt/roomy/bin/roomy")
        );
        let q = Path::new("/Users/x/.cargo/bin/roomy");
        assert_eq!(stable_exe(q), q);
    }

    #[test]
    fn plist_escapes_and_has_agent_args() {
        let p = plist(Path::new("/a&b/roomy"), 15);
        assert!(p.contains("/a&amp;b/roomy"));
        assert!(p.contains("<string>agent</string>"));
        assert!(p.contains("<integer>900</integer>"));
    }
}
