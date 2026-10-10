//! `roomy guide`: how AI agents should use roomy, as plain text, a Claude Code skill, or an
//! AGENTS.md snippet. The text is compiled in, so it always matches this version.

use std::fs;
use std::path::PathBuf;

pub const GUIDE: &str = include_str!("../docs/agents/guide.md");

const SKILL_FRONTMATTER: &str = "---
name: roomy
description: Check Mac storage and memory and free up disk space safely with the roomy CLI. Use when the disk is full or low, the user asks what takes up space, wants to clean caches, git worktrees, Xcode or Docker data, or asks about memory pressure or swap.
---

";

const AGENTS_MD: &str = "## Mac disk space and memory

Use `roomy` for questions about disk space or memory on this Mac. Run `roomy guide` once to
learn the workflow. Never remove files yourself; propose candidates from `roomy report --json`
and remove them with `roomy clean <id> --json --yes` only after the user approves.
";

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Format {
    /// The guide as Markdown
    Text,
    /// A Claude Code skill (SKILL.md)
    Skill,
    /// A section to paste into AGENTS.md / CLAUDE.md
    AgentsMd,
}

pub fn render(format: Format) -> String {
    match format {
        Format::Text => GUIDE.to_string(),
        Format::Skill => format!("{SKILL_FRONTMATTER}{GUIDE}"),
        Format::AgentsMd => AGENTS_MD.to_string(),
    }
}

/// Where Claude Code looks for personal skills.
pub fn claude_skill_path() -> PathBuf {
    crate::sys::home().join(".claude/skills/roomy/SKILL.md")
}

/// Writes the Claude Code skill. Keeps an existing file unless `force`.
pub fn install_claude_skill(force: bool) -> anyhow::Result<PathBuf> {
    let path = claude_skill_path();
    if path.exists() && !force {
        anyhow::bail!(crate::i18n::tf(
            "guide.exists",
            &[("path", &path.display())]
        ));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, render(Format::Skill))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_frontmatter_and_the_guide() {
        let s = render(Format::Skill);
        assert!(s.starts_with("---\nname: roomy\ndescription: "));
        assert!(s.contains("roomy clean <id>... --json --yes"));
    }

    /// Keep the guide small: agents read it into their context.
    #[test]
    fn guide_stays_short() {
        assert!(GUIDE.len() < 4000, "guide is {} bytes", GUIDE.len());
    }
}
