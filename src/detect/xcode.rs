//! Xcode leftovers that need logic: old `DeviceSupport` versions and unavailable simulators.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::rules::{children, expand_glob};
use crate::sys::{expand, mtime, run, tilde, which};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    for dir in expand_glob("~/Library/Developer/Xcode/*DeviceSupport") {
        if let Some(c) = device_support(ctx, &dir) {
            out.push(c);
        }
    }
    if let Some(c) = unavailable_sims(ctx) {
        out.push(c);
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    let fresh = if c.id == "xcode.unavailable-sims" {
        unavailable_sims(ctx)
    } else {
        device_support(ctx, &expand(&c.path))
    };
    fresh.ok_or_else(|| m("recheck.gone"))
}

/// Folder names look like "iPhone16,1 26.5 (23F77)": keep the newest per device model.
pub fn old_versions(entries: Vec<(PathBuf, i64)>) -> Vec<PathBuf> {
    let mut by_model: BTreeMap<String, Vec<(PathBuf, i64)>> = BTreeMap::new();
    for (p, t) in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let model = name.split_whitespace().next().unwrap_or("").to_string();
        by_model.entry(model).or_default().push((p, t));
    }
    let mut old = vec![];
    for (_, mut v) in by_model {
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        old.extend(v.into_iter().skip(1).map(|x| x.0));
    }
    old.sort();
    old
}

fn device_support(ctx: &Ctx, dir: &Path) -> Option<Candidate> {
    if ctx.is_off_limits(dir) {
        return None;
    }
    let entries = children(dir)
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| (p.clone(), mtime(&p).unwrap_or(0)))
        .collect();
    let old = old_versions(entries);
    if old.is_empty() {
        return None;
    }
    let kind = dir.file_name()?.to_string_lossy().into_owned();
    Some(Candidate {
        id: format!(
            "xcode.device-support:{}",
            kind.to_ascii_lowercase().replace(' ', "-")
        ),
        source: "xcode".into(),
        path: tilde(dir),
        category: Msg::plain("Xcode"),
        title: mf("xcode.ds_title", &[("kind", &kind), ("n", &old.len())]),
        bytes: old.iter().map(|p| ctx.sizer.size(p)).sum(),
        reclaimable: None,
        safety: Safety::Safe,
        evidence: vec![m("xcode.ds_evidence")],
        notes: vec![m("xcode.ds_note")],
        action: Action::Paths {
            paths: old
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            method: Method::Delete,
        },
    })
}

/// Only when a full Xcode is selected: without it, `xcrun` may pop up an install dialog.
fn xcode_selected() -> bool {
    run(&["xcode-select", "-p"], None, Duration::from_secs(5))
        .is_some_and(|o| o.ok && o.stdout.contains(".app/Contents/Developer"))
}

/// Device folders of simulators `simctl` reports as unavailable (their runtime is gone).
pub fn parse_unavailable(json: &str) -> Vec<PathBuf> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    let mut out = vec![];
    if let Some(m) = v["devices"].as_object() {
        for list in m.values() {
            for d in list.as_array().into_iter().flatten() {
                if d["isAvailable"].as_bool() == Some(true) {
                    continue;
                }
                if let Some(parent) = d["dataPath"].as_str().and_then(|p| Path::new(p).parent()) {
                    out.push(parent.to_path_buf());
                }
            }
        }
    }
    out
}

fn unavailable_sims(ctx: &Ctx) -> Option<Candidate> {
    let devices = expand("~/Library/Developer/CoreSimulator/Devices");
    if ctx.is_off_limits(&devices)
        || !devices.is_dir()
        || which("xcrun").is_none()
        || !xcode_selected()
    {
        return None;
    }
    let out = run(
        &["xcrun", "simctl", "list", "devices", "unavailable", "-j"],
        None,
        Duration::from_secs(30),
    )?;
    let dirs = parse_unavailable(&out.stdout);
    if dirs.is_empty() {
        return None;
    }
    Some(Candidate {
        id: "xcode.unavailable-sims".into(),
        source: "xcode".into(),
        path: tilde(&devices),
        category: Msg::plain("Xcode"),
        title: mf("xcode.sims_title", &[("n", &dirs.len())]),
        bytes: dirs.iter().map(|p| ctx.sizer.size(p)).sum(),
        reclaimable: None,
        safety: Safety::Safe,
        evidence: vec![m("xcode.sims_evidence")],
        notes: vec![],
        action: Action::Command {
            argv: ["xcrun", "simctl", "delete", "unavailable"]
                .map(String::from)
                .to_vec(),
            cwd: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_per_model() {
        let p = |s: &str| PathBuf::from(format!("/x/{s}"));
        let old = old_versions(vec![
            (p("iPhone16,1 26.4 (A)"), 1),
            (p("iPhone16,1 26.5 (B)"), 2),
            (p("iPad14,1 18.0 (C)"), 1),
        ]);
        assert_eq!(old, vec![p("iPhone16,1 26.4 (A)")]);
    }

    #[test]
    fn unavailable_devices_from_json() {
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-17-0":[
            {"udid":"A","isAvailable":false,"dataPath":"/s/Devices/A/data"},
            {"udid":"B","isAvailable":true,"dataPath":"/s/Devices/B/data"}]}}"#;
        assert_eq!(parse_unavailable(json), vec![PathBuf::from("/s/Devices/A")]);
        assert_eq!(parse_unavailable("not json"), Vec::<PathBuf>::new());
    }
}
