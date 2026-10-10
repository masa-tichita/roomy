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
    /// Allocated size of what would be removed (like `du`).
    pub bytes: u64,
    /// What removing it would really free: `bytes` minus blocks shared with APFS clones.
    /// Measured for path removals only; None when not measured (commands, other file systems,
    /// too many files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimable: Option<u64>,
    pub safety: Safety,
    /// Facts the decision is based on ("pushed", "no uncommitted changes", ...).
    pub evidence: Vec<Msg>,
    /// Caveats.
    pub notes: Vec<Msg>,
    pub action: Action,
}

impl Candidate {
    /// Best estimate of the space removing it frees: the clone-aware measurement when there is one.
    pub fn frees(&self) -> u64 {
        self.reclaimable.unwrap_or(self.bytes)
    }

    /// The clone-aware size when it is clearly smaller than `bytes` (at least 10% and 10 MB),
    /// i.e. when it is worth showing both numbers.
    pub fn frees_less(&self) -> Option<u64> {
        self.reclaimable.filter(|&r| {
            let shared = self.bytes.saturating_sub(r);
            shared >= 10_000_000 && shared >= self.bytes / 10
        })
    }
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
    /// Free plus purgeable space, as Finder shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_avail_incl_purgeable: Option<u64>,
    pub candidates: Vec<Candidate>,
    pub growth: Vec<Growth>,
    /// Time of the snapshot `growth` is compared against.
    pub growth_since: Option<i64>,
    pub excluded: Vec<Excluded>,
    pub walk: WalkInfo,
}

impl Report {
    /// Space the candidates of one kind would free (clone-aware where measured).
    pub fn total(&self, safety: Safety) -> u64 {
        self.candidates
            .iter()
            .filter(|c| c.safety == safety)
            .map(Candidate::frees)
            .sum()
    }
}
