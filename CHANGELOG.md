# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `[service] notifications = false` mutes the background service without stopping it;
  `[service] memory_alerts = false` keeps only disk alerts. `ROOMY_NO_NOTIFY=1` mutes everything.
- Worktrees in folders another tool manages can be removed through that tool
  (`[[worktrees.managed]] remove = ["orca", "worktree", "rm", "--worktree", "path:{path}"]`).
- Worktree evidence shows a merged pull request (via `gh`), which catches squash merges.

### Changed

- A memory "warning" is reported only when it lasts two checks in a row (critical: at once).

### Fixed

- Two scans within the same second no longer overwrite each other's snapshot, which made
  growth and `top` deltas disappear.
- End-to-end tests no longer send real notifications.

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
- Shell completions (`roomy completions <shell>`) and manual pages (`roomy man`), for packaging.
- What a removal really frees: for candidates that remove paths, roomy measures the bytes not
  shared with APFS clones (`ATTR_CMNEXT_PRIVATESIZE`). Sizes, totals and ordering lead with that
  number; the allocated size is shown alongside when it is clearly larger (`reclaimable` in JSON).
- Finder-style available space including purgeable space in `status`, the dashboard and JSON
  (`disk.available_incl_purgeable`); thresholds still use free space.

### Fixed

- `doctor`, `status` and `rules` size their label column to the widest label, so long Japanese
  labels (フルディスクアクセス, 最終スキャン) no longer push their line out of alignment.
- Scans no longer measure app folders the scan did not walk (outside `[scan] roots` or in
  `exclude`), nor app folders the walk already found small; `lsof` runs only when a worktree is
  found.
