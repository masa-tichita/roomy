# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `status`, `scan`, `report`, `clean`, `top`, `history`, `rules`, `config`, `doctor` commands.
- Interactive dashboard (`roomy` in a terminal, or `roomy ui`) with candidate selection and removal.
- Detectors for git worktrees, Xcode leftovers, package caches, app caches, old tool versions,
  obsolete editor extensions, Docker, unused large apps and old downloads.
- Data-driven rules (`rules/builtin.toml`) with user rules, overrides and disabling.
- Growth detection between scans.
- Background service (`roomy service install`) with notifications for low or fast-dropping free
  space, memory pressure and swap, plus a daily scan.
- Safety: re-check before removal, path guard, audit log, Trash for user data.
- English and Japanese messages; JSON output with a schema version.
- AI agent support: compact `--json` summaries, `explain`, JSON plans and results for `clean`,
  guard rails for unattended removal (`--allow-review`, `[clean] max_unattended_gb`),
  `roomy guide` (agent instructions, Claude Code skill, AGENTS.md snippet) and an MCP server
  (`roomy mcp`).
- `disabled_detectors` turns off built-in detectors (for example `["docker"]`).

### Fixed

- `doctor`, `status` and `rules` size their label column to the widest label, so long Japanese
  labels (フルディスクアクセス, 最終スキャン) no longer push their line out of alignment.
- Scans no longer measure app folders the scan did not walk (outside `[scan] roots` or in
  `exclude`), nor app folders the walk already found small; `lsof` runs only when a worktree is
  found.
