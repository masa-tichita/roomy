//! Old extension versions that VS Code-family editors have marked obsolete in `extensions/.obsolete`.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Method, Safety};
use crate::sys::{expand, tilde};
use std::fs;
use std::path::{Path, PathBuf};

const EDITORS: &[(&str, &str)] = &[
    ("VS Code", "~/.vscode/extensions"),
    ("VS Code Insiders", "~/.vscode-insiders/extensions"),
    ("VSCodium", "~/.vscode-oss/extensions"),
    ("Cursor", "~/.cursor/extensions"),
    ("Windsurf", "~/.windsurf/extensions"),
    ("Antigravity", "~/.antigravity/extensions"),
    ("Kiro", "~/.kiro/extensions"),
];

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>) {
    for (label, dir) in EDITORS {
        if let Some(c) = evaluate(ctx, label, &expand(dir)) {
            out.push(c);
        }
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    let (label, dir) = EDITORS
        .iter()
        .find(|(_, d)| tilde(&expand(d)) == c.path)
        .ok_or_else(|| m("recheck.rule_gone"))?;
    evaluate(ctx, label, &expand(dir)).ok_or_else(|| m("recheck.gone"))
}

/// Folder names listed in `.obsolete` (a JSON object `{ "<folder>": true }`).
pub fn obsolete(dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(dir.join(".obsolete")) else {
        return vec![];
    };
    let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) else {
        return vec![];
    };
    map.keys()
        .filter(|k| !k.is_empty() && !k.contains('/') && *k != "." && *k != "..")
        .map(|k| dir.join(k))
        .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .collect()
}

fn evaluate(ctx: &Ctx, label: &str, dir: &Path) -> Option<Candidate> {
    if ctx.is_off_limits(dir) {
        return None;
    }
    let old = obsolete(dir);
    if old.is_empty() {
        return None;
    }
    let slug = label.to_ascii_lowercase().replace(' ', "-");
    Some(Candidate {
        id: format!("editor.obsolete:{slug}"),
        source: "editor".into(),
        path: tilde(dir),
        category: m("category.devtools"),
        title: mf("editor.title", &[("editor", &label), ("n", &old.len())]),
        bytes: old.iter().map(|p| ctx.sizer.size(p)).sum(),
        safety: Safety::Safe,
        evidence: vec![m("editor.evidence")],
        notes: vec![],
        action: Action::Paths {
            paths: old
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            method: Method::Delete,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_obsolete_list_and_ignores_bad_names() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        fs::create_dir_all(dir.join("ext-1.0.0")).unwrap();
        fs::create_dir_all(dir.join("ext-2.0.0")).unwrap();
        fs::write(
            dir.join(".obsolete"),
            r#"{"ext-1.0.0":true,"../evil":true,"gone-0.1":true}"#,
        )
        .unwrap();
        assert_eq!(obsolete(dir), vec![dir.join("ext-1.0.0")]);
    }
}
