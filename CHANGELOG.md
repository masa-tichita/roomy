# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.1] - 2026-10-11

### Fixed

- `roomy memory` can quit apps whose main process starts with a bare name instead of a path
  (Circleback): executables are read with `proc_pidpath` when `ps` shows only a name. Such apps
  were listed as having no main process to quit (#30).

## [0.3.0] - 2026-10-11

### Added

- Memory cleanup: `roomy memory` (and a Memory tab in the dashboard, and an MCP tool) shows swap,
  space used outside the home folder (swap files, simulator runtimes) and the apps holding
  compressed or swapped memory; `roomy memory quit` (or `x` in the tab) quits them after
  confirmation, apps with the normal Quit. System processes and roomy's own session are never
  offered. `status` shows swap and space outside home.

### Changed

- "Grew" entries are measured for the folder's own data (`private` in JSON). Folders whose growth
  is mostly APFS clones — `XCTestDevices` showed +60.9 GB while only ~0.2 GB was new data — now
  say so in the report and the dashboard, and JSON marks them `mostly_shared` (#21).

### Fixed

- "In use" checks (`skip_if_running`) match whole program names: the `xcodebuildmcp` MCP server
  no longer counts as `xcodebuild` and turns Xcode caches into *review* (#20).
- The "grew" list no longer fills up with the parts of one folder that grew as a whole (eight
  simulator devices under `XCTestDevices`): when growth is spread over three or more smaller
  parts, only the folder is listed (#22).

## [0.2.0] - 2026-10-11

### Added

- npm package `roomy-cli`: `npx roomy-cli`, `bunx roomy-cli`, `pnpm dlx roomy-cli`, and MCP
  clients via `npx -y roomy-cli mcp`. It ships the universal binary itself (no install scripts,
  no downloads) and is published with npm trusted publishing and provenance.

### Changed

- Scans no longer start the simulator service when every simulator's runtime and device type
  are installed: roomy reads the device files itself and asks `xcrun simctl` only when a
  simulator may be unavailable. Asking took 42 s on a GitHub macOS runner with 17 Xcode
  versions (#2).
- Releases go through a draft and are published only when every asset is attached; the Homebrew
  formula is pushed with a repository-scoped deploy key; the toolchain and every GitHub Action
  are pinned, and workflows are checked with zizmor.

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

[Unreleased]: https://github.com/masa-tichita/roomy/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/masa-tichita/roomy/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/masa-tichita/roomy/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/masa-tichita/roomy/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/masa-tichita/roomy/releases/tag/v0.1.0
