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

The workflow then runs four jobs; nothing is public until the third:

1. **draft** — checks the tag matches `Cargo.toml` and that `CHANGELOG.md` has a section for it
   (otherwise it stops before building), then creates a draft release with that section as notes.
2. **build** — tests, builds aarch64 and x86_64, combines them with `lipo`, signs the result ad
   hoc (the linker signs only the arm64 slice) and verifies it, packs
   `roomy-vX.Y.Z-universal-apple-darwin.tar.gz` with a `.sha256`, attests its build provenance,
   and uploads both to the draft.
3. **publish** — checks both assets are attached, then publishes the release.
4. **homebrew** — `.github/workflows/homebrew.yml` renders `packaging/homebrew/roomy.rb.in` with
   the release's checksum, pushes it to
   [masa-tichita/homebrew-tap](https://github.com/masa-tichita/homebrew-tap), then installs from
   the tap and runs `brew test` on a fresh runner.

If a run fails before *publish*, fix the cause and re-run the failed jobs: the draft is reused and
assets are replaced. A published release is never modified.

If only the Homebrew step failed, fix it on `main` and redo it for the existing tag — never
re-tag a published version:

```sh
gh workflow run homebrew.yml -f tag=vX.Y.Z
```

## One-time setup

The Homebrew workflow pushes to the tap with a **deploy key**: an SSH key that can write to
`masa-tichita/homebrew-tap` and nothing else, and never expires.

```sh
ssh-keygen -t ed25519 -N "" -C "roomy release workflow -> homebrew-tap" -f tapkey
gh repo deploy-key add tapkey.pub -R masa-tichita/homebrew-tap --allow-write --title "roomy release workflow"
gh secret set HOMEBREW_TAP_DEPLOY_KEY -R masa-tichita/roomy < tapkey
rm tapkey tapkey.pub
```

To rotate it, delete the old deploy key in the tap's settings and repeat. Without the secret
the release is still published and the Homebrew workflow fails with a clear error.

### npm (`roomy-cli`)

The *npm* job publishes `roomy-cli` with npm trusted publishing (OIDC), so no npm token is
stored. One-time setup on npmjs.com, after the package exists (the very first version has to be
published by hand: `packaging/npm/build.sh vX.Y.Z out && cd out && npm publish --access public`):

- package `roomy-cli` → Settings → Trusted publisher → GitHub Actions
- organization or user `masa-tichita`, repository `roomy`, workflow `release.yml`,
  environment `release`, and allow `npm publish`

The `release` environment in this repository only accepts `v*` tags. A newly added trusted
publisher must be used successfully within two days.

## Verify a release

```sh
gh attestation verify roomy-vX.Y.Z-universal-apple-darwin.tar.gz --repo masa-tichita/roomy
brew install masa-tichita/tap/roomy && brew test roomy
```
