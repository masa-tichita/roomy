# roomy — guide for AI agents

roomy finds what is safe to delete on this Mac and removes it with safety checks. Use it
instead of ad-hoc `du`, `find` or `rm`: results are deterministic and compact, and every removal
is re-checked right before it happens and logged.

## Workflow

1. `roomy status --json` — free space, memory pressure, swap, last scan. Instant.
2. `roomy report --json` — the last scan (instant), or `roomy scan --json` for a fresh one
   (about 30 seconds). Each candidate has `id`, `size`, `safety`, `why` (evidence), `notes`
   and `action`.
3. `roomy explain <id> --json` — one candidate in detail (paths, command, caveats).
4. Tell the user what you suggest and why, using `why` and `notes`. **Never remove anything
   without the user's approval.**
5. `roomy clean <id>... --json` — a dry run: the plan after re-checking, and what is refused.
6. After the user approves: `roomy clean <id>... --json --yes`. Add `--allow-review` only for
   "review" candidates the user explicitly approved.

## Meaning of the fields

- `safety: "safe"` — comes back by itself or is re-downloaded when needed.
- `safety: "review"` — probably unneeded, but the user must look first (worktrees holding
  `.env` files, Docker volumes, models that are slow to download, old downloads).
- `bytes` / `size` — allocated size of what would be removed (like `du`).
- `reclaimable` — what removing it really frees: APFS clones share blocks, so this can be much
  smaller than `bytes` (simulator devices, copies made with `cp -c`). Missing or `null` when not
  measured (commands, other file systems). Totals (`reclaimable.safe` / `.review`, a plan's
  `frees_bytes`) use it where measured. Quote this number when telling the user what they gain.
- `disk.free` — unused space (like `df`); warnings use it. `disk.available_incl_purgeable` also
  counts purgeable space macOS clears on demand, which is what Finder shows as available.
- `action.kind: "manual"` — roomy will not remove it (e.g. apps); pass on `action.hint`.
- `refused` in a clean result — not removed, with the reason. Do not work around it.

## Rules

- Never delete files yourself with `rm`. If roomy refuses, report the reason to the user.
- If the report is more than a day old, run `roomy scan --json` first.
- Unattended removals are capped in size; for bigger cleanups, ask the user to run
  `roomy clean` (or the `roomy` dashboard) themselves.

## Other commands

- `roomy top [path] --json` — largest folders from the last scan, no rescan.
- `roomy history --json` — lowest free space per day.
- `roomy doctor --json` — permissions, tools, background service.
- Exit codes: 0 ok, 1 error or partial failure, 2 usage error, 3 over a threshold
  (`status --exit-code`).
- `roomy mcp` serves the same operations as MCP tools.
