# Releasing roomy

Releases are cut from `main` by pushing a tag; `.github/workflows/release.yml` does the rest.

1. Open a PR that:
   - bumps `version` in `Cargo.toml` (and `Cargo.lock` via `cargo check`),
   - renames `## [Unreleased]` in `CHANGELOG.md` to `## [X.Y.Z] - YYYY-MM-DD` and starts a new
     empty `## [Unreleased]` above it.
2. Merge it after CI passes.
3. Tag and push:

   ```sh
   git switch main && git pull
   git tag -a vX.Y.Z -m "roomy X.Y.Z"
   git push origin vX.Y.Z
   ```

The workflow then

- checks that the tag matches `Cargo.toml` and runs the tests,
- builds aarch64 and x86_64, combines them with `lipo`, signs the result ad hoc (the linker
  signs only the arm64 slice) and verifies the signature,
- packs `roomy-vX.Y.Z-universal-apple-darwin.tar.gz` with a `.sha256`, attests its build
  provenance, and publishes a GitHub release with the CHANGELOG section as notes,
- calls `.github/workflows/homebrew.yml`, which renders `packaging/homebrew/roomy.rb.in` with the
  release's checksum, pushes it to
  [masa-tichita/homebrew-tap](https://github.com/masa-tichita/homebrew-tap), then installs from
  the tap and runs `brew test` on a fresh runner.

If only the Homebrew step failed, fix it on `main` and redo it for the existing tag — never
re-tag a published version:

```sh
gh workflow run homebrew.yml -f tag=vX.Y.Z
```

## One-time setup

The tap update needs a repository secret `HOMEBREW_TAP_TOKEN`: a fine-grained personal access
token with **Contents: Read and write** on `masa-tichita/homebrew-tap` only. Without it the
release is still published and the Homebrew workflow fails with a clear error.

## Verify a release

```sh
gh attestation verify roomy-vX.Y.Z-universal-apple-darwin.tar.gz --repo masa-tichita/roomy
brew install masa-tichita/tap/roomy && brew test roomy
```
