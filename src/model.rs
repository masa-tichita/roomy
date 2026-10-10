//! Data shared by scanning, reporting and cleaning. Serialized into report.json and `--json` output.

use crate::i18n::Msg;
use crate::walk::Growth;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    /// Comes back by itself or is re-downloaded when needed.
    Safe,
    /// Probably unneeded, but look before removing.
    Review,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Gone for good (frees space immediately). For things that regenerate.
    Delete,
    /// Moved to the Trash (space is freed when the Trash is emptied). For user data.
    Trash,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Paths {
        paths: Vec<String>,
        method: Method,
    },
    Command {
        argv: Vec<String>,
        cwd: Option<String>,
    },
    /// roomy does not remove this itself; `hint` says how.
    Manual {
        hint: Msg,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Candidate {
    /// Stable id, usable with `clean`.
    pub id: String,
    /// Which detector produced it; `clean` asks the same detector to re-check.
    pub source: String,
    /// Main path, for display and re-checking.
    pub path: String,
    pub category: Msg,
    pub title: Msg,
    pub bytes: u64,
    pub safety: Safety,
    /// Facts the decision is based on ("pushed", "no uncommitted changes", ...).
    pub evidence: Vec<Msg>,
    /// Caveats.
    pub notes: Vec<Msg>,
    pub action: Action,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Excluded {
    pub path: String,
    pub reason: Msg,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct WalkInfo {
    pub seconds: f64,
    pub unreadable: u64,
    /// Protected folders were skipped (background scan without Full Disk Access).
    pub skipped_protected: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Report {
    pub schema: u32,
    pub version: String,
    pub time: i64,
    pub disk_total: u64,
    pub disk_avail: u64,
    pub candidates: Vec<Candidate>,
    pub growth: Vec<Growth>,
    /// Time of the snapshot `growth` is compared against.
    pub growth_since: Option<i64>,
    pub excluded: Vec<Excluded>,
    pub walk: WalkInfo,
}

impl Report {
    pub fn total(&self, safety: Safety) -> u64 {
        self.candidates
            .iter()
            .filter(|c| c.safety == safety)
            .map(|c| c.bytes)
            .sum()
    }
}
