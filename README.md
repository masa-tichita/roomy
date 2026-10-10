# roomy

**Your Mac's tidy roommate that keeps it roomy.**

roomy watches storage and memory on your Mac, warns you *before* they run out, and tells you
what is safe to delete — with the evidence for each decision. It understands where developer
Macs actually fill up: git worktrees, Xcode simulators, package caches, old tool versions,
editor extensions, Docker, and AI models.

[日本語の README はこちら](README.ja.md)

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
- **Warns early.** A small background agent checks every 15 minutes and notifies you when free
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

From source (Rust 1.88 or newer):

```sh
cargo install --git https://github.com/masa-tichita/roomy
```

Homebrew and prebuilt universal binaries are planned for v0.2.

## Quick start

```sh
roomy                      # dashboard (in a terminal)
roomy status               # free space, memory, swap, top apps
roomy scan                 # walk the disk, list candidates and growth
roomy clean 1 3 --dry-run  # see exactly what would happen
roomy clean 1 3            # do it (asks first)
roomy service install      # background checks and a daily scan
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

[thresholds]
disk_warn_gb = 50
disk_critical_gb = 25

[worktrees]
roots = ["~/dev", "~/work"]
managed = [{ path = "~/orca/workspaces", name = "Orca" }]   # shown as a hint

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

`roomy rules` lists every rule; `roomy config validate` catches typos.

## For scripts and agents

`status`, `scan`, `report`, `top`, `history`, `rules` and `doctor` accept `--json`. Every JSON
document is an object with `"schema": 1`; text fields are always English. Fields may be added
within a schema version; anything else bumps it.

| Exit code | Meaning |
|---|---|
| 0 | success |
| 1 | error, or `clean` could not do everything |
| 2 | invalid command-line usage |
| 3 | `status --exit-code`: free space or memory is over a threshold |

## Uninstall

```sh
roomy service uninstall --purge   # stop the agent, delete reports and history
cargo uninstall roomy
rm -r ~/.config/roomy             # optional: your configuration
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
roomy by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
