//! Xcode leftovers that need logic: old `DeviceSupport` versions and unavailable simulators.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::rules::{children, expand_glob};
use crate::sys::{expand, host_arch, mtime, os_version, run, tilde, which};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    for dir in expand_glob("~/Library/Developer/Xcode/*DeviceSupport") {
        if let Some(c) = device_support(ctx, &dir) {
            out.push(c);
        }
    }
    if let Some(c) = unavailable_sims(ctx, false) {
        out.push(c);
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    let fresh = if c.id == "xcode.unavailable-sims" {
        unavailable_sims(ctx, true)
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

/// The selected developer folder, only when it is a full Xcode: without one, `xcrun` may pop up
/// an install dialog.
fn developer_dir() -> Option<PathBuf> {
    let o = run(&["xcode-select", "-p"], None, Duration::from_secs(5))?;
    let dir = o.stdout.trim();
    (o.ok && dir.contains(".app/Contents/Developer")).then(|| PathBuf::from(dir))
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

/// Simulator runtimes and device types installed on this Mac, by identifier.
#[derive(Default)]
struct Installed {
    /// Only runtimes whose profile allows this macOS version and processor.
    runtimes: HashSet<String>,
    device_types: HashSet<String>,
}

impl Installed {
    /// Where the simulator service finds them: mounted runtime images, the system profiles, and
    /// the Xcode bundle itself (Xcode 14 and older). A location missed here only means `simctl`
    /// is asked.
    fn find(developer: &Path) -> Installed {
        let mut runtime_dirs = expand_glob(
            "/Library/Developer/CoreSimulator/Volumes/*/Library/Developer/CoreSimulator/Profiles/Runtimes",
        );
        runtime_dirs.push("/Library/Developer/CoreSimulator/Profiles/Runtimes".into());
        let in_xcode =
            developer.join("Platforms/*.platform/Library/Developer/CoreSimulator/Profiles");
        runtime_dirs.extend(expand_glob(&format!("{}/Runtimes", in_xcode.display())));
        let mut type_dirs = vec![PathBuf::from(
            "/Library/Developer/CoreSimulator/Profiles/DeviceTypes",
        )];
        type_dirs.extend(expand_glob(&format!("{}/DeviceTypes", in_xcode.display())));
        Installed::read(&runtime_dirs, &type_dirs, &os_version(), host_arch())
    }

    fn read(runtime_dirs: &[PathBuf], type_dirs: &[PathBuf], os: &str, arch: &str) -> Installed {
        let bundles = |dirs: &[PathBuf], ext: &str| -> Vec<PathBuf> {
            dirs.iter()
                .flat_map(|d| children(d))
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .collect()
        };
        let mut found = Installed::default();
        for b in bundles(runtime_dirs, "simruntime") {
            if let Some(id) = bundle_id(&b)
                && runs_here(&b, os, arch)
            {
                found.runtimes.insert(id);
            }
        }
        for b in bundles(type_dirs, "simdevicetype") {
            found.device_types.extend(bundle_id(&b));
        }
        found
    }
}

/// A property list (XML or binary) whose top level is a dictionary.
fn plist_dict(path: &Path) -> Option<plist::Dictionary> {
    plist::Value::from_file(path).ok()?.into_dictionary()
}

fn plist_str<'a>(d: &'a plist::Dictionary, key: &str) -> Option<&'a str> {
    d.get(key)?.as_string()
}

fn bundle_id(bundle: &Path) -> Option<String> {
    let info = plist_dict(&bundle.join("Contents/Info.plist"))?;
    plist_str(&info, "CFBundleIdentifier").map(str::to_string)
}

/// "26.0.1" as numbers without trailing zeros, so that "26" and "26.0" compare equal.
fn version(s: &str) -> Option<Vec<u32>> {
    let mut v = s
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<u32>>>()?;
    while v.last() == Some(&0) {
        v.pop();
    }
    Some(v)
}

/// The runtime's profile allows this macOS version and processor. Unreadable means no.
fn runs_here(bundle: &Path, os: &str, arch: &str) -> bool {
    let Some(profile) = plist_dict(&bundle.join("Contents/Resources/profile.plist")) else {
        return false;
    };
    let Some(os) = version(os).filter(|v| !v.is_empty()) else {
        return false;
    };
    let bound = |key: &str, ok: fn(&Vec<u32>, &Vec<u32>) -> bool| match profile.get(key) {
        None => true,
        Some(v) => v.as_string().and_then(version).is_some_and(|b| ok(&os, &b)),
    };
    let archs_ok = match profile.get("supportedArchs") {
        None => true,
        Some(v) => v
            .as_array()
            .is_some_and(|a| a.is_empty() || a.iter().any(|x| x.as_string() == Some(arch))),
    };
    bound("minHostVersion", |os, min| os >= min)
        && bound("maxHostVersion", |os, max| os <= max)
        && archs_ok
}

/// True when every simulator in `devices` surely is available: its runtime and device type are
/// installed, and nothing about it is unusual. Then `simctl` has nothing to report. Anything
/// unexpected answers false, so that `simctl` decides.
fn all_available(devices: &Path, installed: &Installed) -> bool {
    children(devices).iter().filter(|d| d.is_dir()).all(|d| {
        let Some(p) = plist_dict(&d.join("device.plist")) else {
            return false;
        };
        let installed_as = |key: &str, set: &HashSet<String>| {
            plist_str(&p, key).is_some_and(|id| set.contains(id))
        };
        matches!(p.get("isDeleted"), None | Some(plist::Value::Boolean(false)))
            // "System": any installed runtime with this identifier will do
            && p.get("runtimePolicy").is_none_or(|v| v.as_string() == Some("System"))
            && installed_as("runtime", &installed.runtimes)
            && installed_as("deviceType", &installed.device_types)
    })
}

/// Simulators `simctl` reports as unavailable. Starting the simulator service to ask can take most
/// of a minute on Macs with many runtimes (#2), so a scan first reads the device folders and the
/// installed runtimes itself and asks only when a device may be unavailable. A re-check before
/// removal always asks.
fn unavailable_sims(ctx: &Ctx, always_ask: bool) -> Option<Candidate> {
    let devices = expand("~/Library/Developer/CoreSimulator/Devices");
    if ctx.is_off_limits(&devices) || !devices.is_dir() || which("xcrun").is_none() {
        return None;
    }
    let developer = developer_dir()?;
    if !always_ask && all_available(&devices, &Installed::find(&developer)) {
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

    // ---- reading simulator files instead of asking simctl ----

    use plist::{Dictionary, Value};

    const RT: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-5";
    const TY: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17";

    fn dict(pairs: &[(&str, Value)]) -> Dictionary {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    /// The simulator service writes XML and Xcode ships binary plists: fixtures use both.
    fn write(path: &Path, d: Dictionary, binary: bool) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let v = Value::Dictionary(d);
        if binary {
            v.to_file_binary(path).unwrap();
        } else {
            v.to_file_xml(path).unwrap();
        }
    }

    fn bundle(dir: &Path, name: &str, id: &str, profile: Option<Dictionary>) {
        let contents = dir.join(name).join("Contents");
        write(
            &contents.join("Info.plist"),
            dict(&[("CFBundleIdentifier", s(id))]),
            true,
        );
        if let Some(p) = profile {
            write(&contents.join("Resources/profile.plist"), p, true);
        }
    }

    fn device(devices: &Path, udid: &str, extra: &[(&str, Value)]) {
        let mut d = dict(&[
            ("UDID", s(udid)),
            ("deviceType", s(TY)),
            ("isDeleted", Value::Boolean(false)),
            ("runtime", s(RT)),
            ("runtimePolicy", s("System")),
            ("state", Value::Integer(1.into())),
        ]);
        for (k, v) in extra {
            d.insert((*k).to_string(), v.clone());
        }
        write(&devices.join(udid).join("device.plist"), d, false);
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        runtimes: PathBuf,
        types: PathBuf,
        devices: PathBuf,
    }

    /// An installed iOS 26.5 runtime and iPhone 17 device type, and an empty device set.
    fn fixture() -> Fixture {
        let d = tempfile::tempdir().unwrap();
        let f = Fixture {
            runtimes: d.path().join("Runtimes"),
            types: d.path().join("DeviceTypes"),
            devices: d.path().join("Devices"),
            _dir: d,
        };
        let profile = dict(&[
            ("minHostVersion", s("14.0.0")),
            (
                "supportedArchs",
                Value::Array(vec![s("arm64"), s("x86_64")]),
            ),
        ]);
        bundle(&f.runtimes, "iOS 26.5.simruntime", RT, Some(profile));
        bundle(&f.types, "iPhone 17.simdevicetype", TY, None);
        std::fs::create_dir_all(&f.devices).unwrap();
        write(
            &f.devices.join("device_set.plist"),
            dict(&[("Version", Value::Integer(0.into()))]),
            false,
        );
        f
    }

    impl Fixture {
        fn installed(&self) -> Installed {
            Installed::read(
                std::slice::from_ref(&self.runtimes),
                std::slice::from_ref(&self.types),
                "26.0.1",
                "arm64",
            )
        }

        fn all_available(&self) -> bool {
            all_available(&self.devices, &self.installed())
        }
    }

    #[test]
    fn installed_runtimes_and_device_types_from_bundles() {
        let f = fixture();
        bundle(&f.runtimes, "notes.txt", "x", None); // not a bundle
        let i = f.installed();
        assert_eq!(i.runtimes, HashSet::from([RT.to_string()]));
        assert_eq!(i.device_types, HashSet::from([TY.to_string()]));
    }

    #[test]
    fn runtimes_this_mac_cannot_run_do_not_count() {
        let f = fixture();
        let other = |name: &str, id: &str, profile: Option<Dictionary>| {
            bundle(&f.runtimes, name, id, profile);
        };
        other(
            "new.simruntime",
            "rt.new",
            Some(dict(&[("minHostVersion", s("27.1"))])),
        );
        other(
            "old.simruntime",
            "rt.old",
            Some(dict(&[("maxHostVersion", s("15.99"))])),
        );
        other(
            "intel.simruntime",
            "rt.intel",
            Some(dict(&[("supportedArchs", Value::Array(vec![s("x86_64")]))])),
        );
        other(
            "bad.simruntime",
            "rt.bad",
            Some(dict(&[("minHostVersion", s("x"))])),
        );
        other("noprofile.simruntime", "rt.noprofile", None);
        other(
            "exact.simruntime",
            "rt.exact",
            Some(dict(&[("maxHostVersion", s("26.0.1"))])),
        );
        other("any.simruntime", "rt.any", Some(dict(&[])));
        let mut got: Vec<String> = f.installed().runtimes.into_iter().collect();
        got.sort();
        assert_eq!(got, [RT, "rt.any", "rt.exact"]);
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(version("26.0"), version("26"));
        assert!(version("26.0.1") > version("26"));
        assert!(version("10.15") < version("14.0.0"));
        assert_eq!(version("26.x"), None);
    }

    #[test]
    fn no_devices_or_all_installed_needs_no_simctl() {
        let f = fixture();
        assert!(f.all_available(), "an empty device set");
        device(&f.devices, "A", &[]);
        device(
            &f.devices,
            "B",
            &[(
                "lastBootedAt",
                Value::Date(std::time::SystemTime::UNIX_EPOCH.into()),
            )],
        );
        device(&f.devices, "C", &[]);
        // older devices have neither key
        let c = f.devices.join("C/device.plist");
        let mut d = plist_dict(&c).unwrap();
        d.remove("isDeleted");
        d.remove("runtimePolicy");
        write(&c, d, true);
        assert!(f.all_available());
    }

    #[test]
    fn any_doubt_asks_simctl() {
        let cases: [(&str, &[(&str, Value)]); 5] = [
            ("runtime gone", &[("runtime", s("rt.gone"))]),
            ("device type gone", &[("deviceType", s("ty.gone"))]),
            ("no runtime key", &[("runtime", Value::Boolean(true))]),
            ("being deleted", &[("isDeleted", Value::Boolean(true))]),
            ("pinned to a build", &[("runtimePolicy", s("Build"))]),
        ];
        for (why, extra) in cases {
            let f = fixture();
            device(&f.devices, "A", &[]);
            device(&f.devices, "B", extra);
            assert!(!f.all_available(), "{why}");
        }
        let f = fixture();
        device(&f.devices, "A", &[]);
        std::fs::write(f.devices.join("A/device.plist"), "garbage").unwrap();
        assert!(!f.all_available(), "unreadable device.plist");
        let f = fixture();
        std::fs::create_dir_all(f.devices.join("Z/data")).unwrap();
        assert!(!f.all_available(), "a folder without device.plist");
    }
}
