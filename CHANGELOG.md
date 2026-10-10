# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-10

First release.

### Finding what to remove

- One parallel walk records every large folder; the next scan shows what grew, pointing at the
  folder that caused it.
- Detectors for git worktrees (clean, pushed, idle, not in use; files outside Git listed;
  merged pull requests via `gh`, which catches squash merges), Xcode leftovers, package and app
  caches, old versions of self-updating tools, obsolete editor extensions, Docker, large unused
  apps and old downloads.
- Data-driven rules (`rules/builtin.toml`) that users extend, override or disable;
  `disabled_detectors` turns off built-in detectors.
- What a removal really frees: the bytes not shared with APFS clones. Sizes, totals and
  ordering lead with it; the allocated size is shown alongside when clearly larger.
- Finder-style available space (including purgeable) next to free space.

### Removing safely

- Nothing is removed automatically. `clean` (or `c` in the dashboard) re-checks every
  candidate right before removal, refuses paths outside home, protected folders and symbolic
  links, prefers each tool's own cleanup command, moves user data to the Trash, and logs every
  removal.
- Worktrees managed by another tool (e.g. Orca) can be removed through that tool.

### Using it

- Commands: `status`, `scan`, `report`, `explain`, `clean`, `top`, `history`, `rules`,
  `config`, `doctor`, `service`, `guide`, `mcp`, `completions`, `man`.
- Interactive dashboard (`roomy` in a terminal): candidates, growth, free-space trend, memory,
  a folder explorer, selection and removal.
- Background service (`brew services start roomy` or `roomy service install`): checks every 15
  minutes, notifies on low or fast-dropping free space, sustained memory pressure and swap, scans
  daily; quiet hours, `notifications = false` and `memory_alerts = false` to tone it down.
- English and Japanese.

### For AI agents

- Versioned, English, compact `--json` on every reporting command; summaries first, details via
  `explain`; JSON plans and results for `clean`; errors as JSON. The shape is pinned by snapshot
  tests.
- Guard rails for unattended removal: "review" candidates need `--allow-review`, one run is capped
  by `[clean] max_unattended_gb`.
- `roomy mcp`: a Model Context Protocol server with the same operations (`--read-only` without
  removal); `roomy guide` for instructions, a Claude Code skill and an AGENTS.md snippet.

### Distribution

- Universal binary (Apple Silicon and Intel, macOS 13+), ad-hoc signed, with build provenance,
  on GitHub Releases and the Homebrew tap (`brew install masa-tichita/tap/roomy`), including shell
  completions and manual pages.

[Unreleased]: https://github.com/masa-tichita/roomy/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/masa-tichita/roomy/releases/tag/v0.1.0
