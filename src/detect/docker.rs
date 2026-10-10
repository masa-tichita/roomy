//! Reclaimable Docker data. Asked only when a Docker engine is already running, so that roomy
//! never starts Docker Desktop / `OrbStack` / Colima by itself.

use super::Ctx;
use crate::i18n::{Msg, m};
use crate::model::{Action, Candidate, Safety};
use crate::sys::{run, which};
use std::time::Duration;

const ENGINES: &[&str] = &[
    "/OrbStack.app/",
    "/Docker.app/",
    "com.docker.backend",
    "colima",
    "/Rancher Desktop.app/",
    "podman machine",
];

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    out.extend(candidates(ctx));
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    candidates(ctx)
        .into_iter()
        .find(|x| x.id == c.id)
        .ok_or_else(|| m("recheck.gone"))
}

fn candidates(ctx: &Ctx) -> Vec<Candidate> {
    if which("docker").is_none() || !ENGINES.iter().any(|e| ctx.running(e)) {
        return vec![];
    }
    let Some(out) = run(
        &["docker", "system", "df", "--format", "{{json .}}"],
        None,
        Duration::from_secs(20),
    ) else {
        return vec![];
    };
    if !out.ok {
        return vec![];
    }
    let mut v = vec![];
    for (kind, bytes) in parse_df(&out.stdout) {
        let (id, title, safety, argv, note) = match kind.as_str() {
            "Build Cache" => (
                "docker.build-cache",
                m("docker.build_title"),
                Safety::Safe,
                &["docker", "builder", "prune", "-f"][..],
                m("docker.build_note"),
            ),
            "Images" => (
                "docker.images",
                m("docker.images_title"),
                Safety::Review,
                &["docker", "image", "prune", "-a", "-f"][..],
                m("docker.images_note"),
            ),
            "Local Volumes" => (
                "docker.volumes",
                m("docker.volumes_title"),
                Safety::Review,
                &["docker", "volume", "prune", "-f"][..],
                m("docker.volumes_note"),
            ),
            _ => continue,
        };
        v.push(Candidate {
            id: id.into(),
            source: "docker".into(),
            path: "docker".into(),
            category: Msg::plain("Docker"),
            title,
            bytes,
            safety,
            evidence: vec![m("docker.evidence")],
            notes: vec![note],
            action: Action::Command {
                argv: argv.iter().map(ToString::to_string).collect(),
                cwd: None,
            },
        });
    }
    v
}

/// `docker system df --format '{{json .}}'` -> (Type, reclaimable bytes)
pub fn parse_df(s: &str) -> Vec<(String, u64)> {
    s.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            let kind = v["Type"].as_str()?.to_string();
            let rec = v["Reclaimable"]
                .as_str()?
                .split_whitespace()
                .next()?
                .to_string();
            Some((kind, parse_size(&rec)?))
        })
        .collect()
}

/// Docker prints decimal sizes like "0B", "1.2kB", "325.1MB", "2.633GB".
pub fn parse_size(s: &str) -> Option<u64> {
    let i = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = s.split_at(i);
    let n: f64 = num.parse().ok()?;
    let mult = match unit.to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("1.2kB"), Some(1200));
        assert_eq!(parse_size("2.633GB"), Some(2_633_000_000));
        assert_eq!(parse_size("??"), None);
    }

    #[test]
    fn df_lines() {
        let s = r#"{"Active":"6","Reclaimable":"0B (0%)","Size":"5.23GB","TotalCount":"6","Type":"Images"}
{"Active":"9","Reclaimable":"2.633GB (27%)","Size":"9.604GB","TotalCount":"28","Type":"Local Volumes"}"#;
        assert_eq!(
            parse_df(s),
            vec![
                ("Images".into(), 0),
                ("Local Volumes".into(), 2_633_000_000)
            ]
        );
    }
}
