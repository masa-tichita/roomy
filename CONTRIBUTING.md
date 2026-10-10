# Contributing to roomy

Thanks for helping! roomy deletes files, so correctness and caution matter more than speed.

## Build and test

```sh
cargo build
cargo test                                   # unit + end-to-end tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

End-to-end tests (`tests/e2e.rs`) run the real binary with `HOME` pointing at a temporary
folder, so they never touch your files. New behavior that removes anything needs such a test.

To see the dashboard without a terminal: `cargo run -- ui --print 120x36`.

## The JSON contract

Agents rely on `--json` and the MCP tools, so their shape is pinned by snapshot tests
(`json_contract_snapshots` in `tests/e2e.rs`, files in `tests/snapshots/`). Times, sizes and
anything that differs between Macs are redacted, so a snapshot changes only when a field is
added, removed, renamed or changes type or vocabulary. When you change the output on purpose:

```sh
cargo install cargo-insta   # once
cargo insta test --review   # or: cargo test, then cargo insta review
```

Commit the updated `.snap` files with the change. Adding a field is fine within a schema
version. Removing or renaming a field, or changing its type or meaning, is a breaking change:
bump `SCHEMA` in `src/app.rs`, and note it under *Breaking* in the changelog.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`,
`refactor:`, `test:`, `chore:`. They feed the changelog.

## Adding a rule

Most new cleanups are a few lines in [`rules/builtin.toml`](rules/builtin.toml):

```toml
[[rules]]
id = "cache.example"                     # category.name, stable forever
category = { en = "Cache", ja = "キャッシュ" }
title = { en = "Example cache", ja = "Example のキャッシュ" }
paths = ["~/Library/Caches/com.example.app"]   # must stay inside ~
target = "self"                          # or "children" to keep the folder
safety = "safe"                          # see below
action = "command"                       # delete | trash | command | none
command = ["example", "cache", "clean"]  # prefer the tool's own cleanup
requires = ["example"]                   # skip the rule if the tool is missing
skip_if_running = ["/Example.app/Contents/MacOS/Example"]
note = { en = "Downloaded again when needed.", ja = "必要になれば再ダウンロードされる。" }
```

Choosing `safety`:

- **safe** — the data comes back by itself or is re-downloaded, with no loss beyond time.
  Cite where that is documented in the pull request.
- **review** — probably unneeded, but there are cases where losing it hurts (models that take
  hours to download, database volumes, things offline work depends on).
- If it cannot be recreated at all (backups, user documents), use `action = "none"` with a `hint`.

Prefer `action = "command"` when the tool has a cleanup command; prefer `target = "children"`
when the app expects the folder itself to exist. Rules and messages need both English and
Japanese text; `cargo test` checks this.

Anything that needs logic (git state, versions, running processes) belongs in `src/detect/`,
with a `recheck` that re-evaluates one candidate right before removal.

## Translations

Messages live in `locales/en.toml` and `locales/ja.toml`. Every key must exist in both, with the
same `{placeholders}`; tests enforce it. New languages are welcome.

## Security

Please report anything that could make roomy delete the wrong thing privately — see
[SECURITY.md](SECURITY.md).
