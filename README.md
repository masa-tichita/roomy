# roomy

**English** | [日本語](README.ja.md)

**Your Mac's tidy roommate that keeps it roomy.**

roomy watches storage and memory on your Mac, warns you *before* they run out, and tells you
what is safe to delete — with the evidence for each decision. It understands where developer
Macs actually fill up: git worktrees, Xcode simulators, package caches, old tool versions,
editor extensions, Docker, and AI models.

**Built for AI agents as much as for people.** Claude Code, Codex and other agents call roomy
(CLI or MCP) instead of improvising with `du` and `rm`: they get a compact, deterministic report,
explain it to you, and remove only what you approve — through the same safety checks.

```
╭ roomy  Mac health report ──────────── Overview │ Folders ──────────── scanned 2026-10-10 15:48 ╮
│DISK     ███████████████████████████████████▉░░░░░░░░░░ 113.3 GB free of 494.4 GB (77% used)    │
│MEMORY   ███████████████████▍░░░░░░░░░░░░░░░░░░░░░░░░░░ 36 GB · 56% free · pressure normal      │
╰─────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ Candidates safe 7.7 GB · review 15.4 GB ───────────────────╮╭ Grew ─────────────────────────────╮
│▸     3.3 GB ● Old VS Code extensions (25)       Dev tools  ││  +6.2 GB …/Developer/XCTestDevices │
│      3.0 GB ○ app worktree: feature-x  ⚠ untracked files   ││  +3.1 GB ~/.cache/huggingface      │
│      2.7 GB ● Old versions of app-server-daemon (8)        │╰───────────────────────────────────╯
│      2.6 GB ○ Unused Docker volumes             Docker     │╭ Free space ───────────────────────╮
│      1.1 GB ○ Playwright browsers               Cache      ││▁▂▂▃▅▆▆▇█▇▇█▆▅▅▆▇                  │
╰────────────────────────────────────────────────────────────╯╰───────────────────────────────────╯
  ↑↓ move   ␣ mark   a all safe   ⏎ details   c clean   r rescan   tab switch tab   ? help   q quit
```

## Why roomy

- **Knows developer clutter.** A worktree is suggested only when it has no uncommitted changes,
  its commits are on a remote, nobody is working in it, and it has been idle for a while.
  Files outside Git that would be lost (like `.env`) are listed and the worktree is marked *review*.
- **Shows its reasons.** Every candidate says why: "pushed · no uncommitted changes · last
  touched 12 days ago", "keeps the newest 2 versions", "marked obsolete by the editor".
- **Finds what grew.** One fast parallel walk records the size of every large folder; the next
  scan shows which ones grew, pointing at the folder that caused it rather than its parents.
- **Counts what you really get back.** APFS clones (simulator devices, `cp -c` copies) share
  their blocks, so a folder can take 33 GB yet free only 10 GB. For removal candidates roomy
  measures what removing them really frees and shows it next to the size; totals use it.
- **Shows both kinds of free space.** *Free* is unused space, as `df` reports it; warnings use it.
  *Incl. purgeable* adds what macOS clears on demand (caches, local snapshots), which is what
  Finder and System Settings call available.
- **Warns early.** A small background service checks every 15 minutes and notifies you when free
  space is low or drops suddenly, or when memory pressure or swap get high.

## Safety promises

- **Nothing is deleted automatically.** Only `roomy clean` (or `c` in the dashboard) removes
  anything, after showing the plan and asking.
- **Everything is re-checked right before removal.** If a worktree got a new commit, an app
  started, or a folder vanished since the scan, that candidate is skipped.
- **A guard refuses** paths outside your home folder, too close to it, inside or above protected
  folders (`~/Documents`, `~/Desktop`, `~/.ssh`, Keychains, Mail, iCloud, …), or reached through a
  symbolic link.
- **Tools clean their own data** where they can (`npm cache clean`, `uv cache clean`,
  `brew cleanup`, `git worktree remove`, `xcrun simctl delete unavailable`, …).
- **Regenerable things are deleted; user data goes to the Trash.** Apps are never deleted — roomy
  points you to their uninstaller.
- **Every removal is logged** to `~/Library/Application Support/roomy/audit.jsonl`.
- **No telemetry.** Nothing leaves your Mac.

## Install

```sh
brew install masa-tichita/tap/roomy
brew services start roomy          # background checks and a daily scan
```

Universal binary for Apple Silicon and Intel, macOS 13 or newer. Also on npm, as `roomy-cli`
(the same binary, no install scripts):

```sh
npx roomy-cli                      # try it without installing
npm install -g roomy-cli           # then: roomy
```

Or build it yourself with Rust 1.88 or newer:

```sh
cargo install --git https://github.com/masa-tichita/roomy
roomy service install              # the same background checks without Homebrew
```

Homebrew installs shell completions and manual pages. With `cargo install`:

```sh
roomy completions zsh > ~/.zfunc/_roomy     # also bash, fish, elvish, powershell
roomy man --dir ~/.local/share/man/man1     # roomy.1 and one page per command
```

## Quick start

```sh
roomy                      # dashboard (in a terminal)
roomy status               # free space, memory, swap, top apps
roomy scan                 # walk the disk, list candidates and growth
roomy clean 1 3 --dry-run  # see exactly what would happen
roomy clean 1 3            # do it (asks first)
brew services start roomy  # background checks and a daily scan (or: roomy service install)
roomy doctor               # permissions, tools, agent, config
```

## Full Disk Access

roomy works without it. Without Full Disk Access, **background** scans skip folders macOS
protects (Desktop, Documents, Downloads, other apps' data) so you never get surprise permission
dialogs; scans you start from a terminal use the terminal's permissions.

To include those folders in background scans, add the `roomy` binary under
*System Settings › Privacy & Security › Full Disk Access*. Until roomy ships signed binaries,
macOS ties that permission to the exact build, so you need to add it again after each upgrade.

## Configuration

`roomy config init` writes a commented `~/.config/roomy/config.toml`. Every key is optional.

```toml
language = "ja"              # auto | en | ja
disabled_rules = ["cache.cargo-registry"]   # top-level keys go before any [table]
disabled_detectors = ["docker"]              # cache xcode versioned editor worktree docker apps downloads

[thresholds]
disk_warn_gb = 50
disk_critical_gb = 25

[service]
quiet_hours = [23, 7]        # no non-urgent notifications at night
memory_alerts = false        # only disk alerts
# notifications = false      # keep checking and recording history, never notify

[worktrees]
roots = ["~/dev", "~/work"]
# worktrees another tool manages: removed through that tool so it stays in sync
managed = [{ path = "~/orca/workspaces", name = "Orca", remove = ["orca", "worktree", "rm", "--worktree", "path:{path}"] }]

[apps]
keep = ["MATLAB"]            # never suggest these

[rule_overrides."claude.desktop-vm"]
safety = "safe"              # I never use Cowork

[[rules]]                    # your own rule
id = "my.renders"
title = "Old renders"
paths = ["~/Movies/Renders"]
target = "children"
safety = "review"
action = "trash"
```

Unused apps are looked for only in folders the scan walks (`[scan] roots`, by default `~` and
`/Applications`). `roomy rules` lists every rule; `roomy config validate` catches typos.

## Use it from AI agents

Agents get the same results as you, in a few kilobytes of JSON instead of pages of `du` output,
and they cannot delete anything roomy would not.

**MCP** (tools: `roomy_status`, `roomy_report`, `roomy_scan`, `roomy_explain`, `roomy_top`,
`roomy_history`, `roomy_doctor`, `roomy_plan_clean`, `roomy_clean`):

```sh
claude mcp add --scope user roomy -- roomy mcp          # Claude Code
```

```toml
# Codex: ~/.codex/config.toml
[mcp_servers.roomy]
command = "roomy"
args = ["mcp"]
```

Without a global install, MCP clients can start it through npm:

```json
{ "mcpServers": { "roomy": { "command": "npx", "args": ["-y", "roomy-cli", "mcp"] } } }
```

Use `roomy mcp --read-only` to leave out `roomy_clean`. Desktop apps that do not inherit your
shell's `PATH` need the full path of the binary (`which roomy`).

**CLI + instructions** (works with any agent that can run commands):

```sh
roomy guide --install-claude-skill     # Claude Code skill in ~/.claude/skills/roomy
roomy guide --format agents-md         # paste into AGENTS.md / CLAUDE.md
roomy guide                            # the workflow agents follow
```

The workflow: `report --json` → `explain <id> --json` → propose to the user →
`clean <id> --json` (a dry run) → after approval `clean <id> --json --yes`.

**Guard rails for unattended use.** With `--yes`, `--json` or over MCP, candidates marked
*review* need `--allow-review` (`allow_review`), and one run removes at most
`[clean] max_unattended_gb` (50 GB by default). Every removal is logged with `"unattended": true`.

**Output contract.** `status`, `scan`, `report`, `explain`, `clean`, `top`, `history`, `rules` and
`doctor` take `--json`. Every document is an object with `"schema": 1`; text is always English;
errors are `{"schema": 1, "error": "…"}`. JSON is compact when piped and indented at a terminal.
`report --json` is a summary (no path lists); `--full` gives everything.
Sizes: a candidate's `bytes` is its allocated size (like `du`) and `reclaimable` what removing it
frees once blocks shared with APFS clones are left out (absent or `null` when not measured, e.g.
for commands); `reclaimable` totals and `frees_bytes` in plans use it. `disk.free` is `statfs` free
space (thresholds use it) and `disk.available_incl_purgeable` adds purgeable space, like Finder. Fields may be added
within a schema version; anything else bumps it.

| Exit code | Meaning |
|---|---|
| 0 | success |
| 1 | error, or `clean` could not do everything |
| 2 | invalid command-line usage |
| 3 | `status --exit-code`: free space or memory is over a threshold |

## Uninstall

```sh
brew services stop roomy && brew uninstall roomy   # or: roomy service uninstall && cargo uninstall roomy
rm -r ~/Library/Application\ Support/roomy        # reports, snapshots, history, audit log
rm -r ~/.config/roomy                              # optional: your configuration
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
roomy by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
