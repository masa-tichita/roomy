//! Names and locations. Everything that depends on the tool's name is derived here.

use crate::sys::home;
use std::path::PathBuf;

pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// launchd label of the agent installed by `roomy service install`.
pub const AGENT_LABEL: &str = "io.github.masa-tichita.roomy.agent";
/// launchd labels used by `brew services`: `sh.brew.*` in current Homebrew, `homebrew.mxcl.*` in older ones.
pub const BREW_LABELS: [&str; 2] = ["sh.brew.roomy", "homebrew.mxcl.roomy"];
/// Exit codes. 2 is taken by clap for usage errors.
pub const EXIT_ERROR: u8 = 1;
pub const EXIT_THRESHOLD: u8 = 3;
/// Version of the JSON output and report file format.
pub const SCHEMA: u32 = 1;

/// `$XDG_CONFIG_HOME/roomy` or `~/.config/roomy`. `ROOMY_CONFIG_DIR` overrides (used by tests).
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ROOMY_CONFIG_DIR") {
        return d.into();
    }
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x).join(NAME),
        _ => home().join(".config").join(NAME),
    }
}

/// `~/Library/Application Support/roomy`. `ROOMY_STATE_DIR` overrides (used by tests).
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ROOMY_STATE_DIR") {
        return d.into();
    }
    home().join("Library/Application Support").join(NAME)
}

pub fn launch_agent_path() -> PathBuf {
    home()
        .join("Library/LaunchAgents")
        .join(format!("{AGENT_LABEL}.plist"))
}
