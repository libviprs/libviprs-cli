# Releasing libviprs-cli

This is how I cut a release of `viprs`. It follows the core's `release` branch convention: merging into `release` is what publishes. Everything up to that merge is reversible, the release itself mostly is not (a published tag and its assets are public the moment they exist), so the order matters.

The CLI is not published to crates.io. `Cargo.toml` has no `publish` setting, but it also has no `description` or `license`, which `cargo publish` insists on, and nothing in the workflow runs it. If that ever changes it is its own piece of work, not a step here.

## 1. Open the issue

Every PR needs a closing keyword, and the cut is a PR. Open "Release libviprs-cli X.Y.Z", assign it to spdrman, and say what the cut carries.

## 2. Cut on a branch off `main`

Name it `cut/X.Y.Z`. One commit, `release: cut X.Y.Z`:

- `Cargo.toml`: bump `version` (skip it if it already says X.Y.Z).
- `Cargo.lock`: this one is tracked here, and the release builds with `--locked`, so it has to agree. `cargo check` after the bump rewrites the `libviprs-cli` entry, then commit it.
- `CHANGELOG.md`: leave a bare `## [Unreleased]`, put the notes under `## [X.Y.Z] — YYYY-MM-DD` right below it. The workflow takes everything under that heading, up to the next `## [`, as the release notes, and refuses to run if it is empty. There are no link references in this file, so there is nothing to add there.

Before pushing, run what CI runs: `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release --locked` on Rust 1.99. The guard in `tests/release_workflow.rs` is part of `cargo test`, and it is the thing that notices if someone edits the workflow in a way that breaks the release (wrong trigger, a target dropped, the Rust pin drifting from `rust-version`, an unpinned action).

## 3. Merge the cut PR

PR to `main` with `Closes #N` in the body, then merge it. `main` now has the release commit.

## 4. Merge `main` into `release`

```
git fetch origin
git worktree add ../rel -B rel origin/release     # first time: git worktree add ../rel -b rel origin/main
cd ../rel && git merge -m 'Merge main for X.Y.Z' origin/main && git push origin HEAD:release
```

The very first release has no `release` branch yet, so the push creates it from `main`. After that it is a plain merge. This push is the one that publishes, so I only do it when I mean it.

## 5. What the workflow does

`.github/workflows/release.yml` runs on that push:

1. Reads the version from `Cargo.toml`, requires a non-empty `## [X.Y.Z]` section in `CHANGELOG.md`, and stops if the tag `vX.Y.Z` already exists.
2. Builds `x86_64-unknown-linux-gnu` (Ubuntu 22.04) and `aarch64-apple-darwin` (macOS 14) with Rust 1.99.0, `cargo build --release --locked`, default features. Default means `pdfium`, which binds libpdfium at runtime, so nothing is bundled.
3. Strips the binary, packs `libviprs-cli-vX.Y.Z-<target>.tar.gz` (the `viprs` binary, `README.md`, `LICENSE`), and smoke tests the unpacked copy: layout, `--help`, `features`, and a `test-image` then `info` round trip. None of that needs libpdfium. (`viprs` has no `--version` flag.)
4. Writes `SHA256SUMS` over both tarballs and checks it.
5. Creates the annotated tag `vX.Y.Z` on the pushed commit and the release `libviprs-cli X.Y.Z` with the CHANGELOG section as notes and the two tarballs and `SHA256SUMS` attached. Only this last job has `contents: write`, and it uses just `GITHUB_TOKEN`.

A manual run (Actions, "release", Run workflow) has `dry_run` on by default. It does steps 1 to 4 and publishes nothing. GitHub only offers that button for a workflow file that is on the default branch, so it works once the workflow is on `main`, and it should be run against `release` or `main` before the real push.

## 6. Check the release

- The release page has exactly three assets: the two tarballs and `SHA256SUMS`.
- `git ls-remote --tags origin vX.Y.Z` shows a tag object (an annotated tag has a different hash from the commit it points at).
- Download both tarballs and `SHA256SUMS` into a clean directory and run `shasum -a 256 --check SHA256SUMS`.
- On a Mac, unpack the arm64 one and run `viprs features`.

## Traps

- **A failed publish can leave a tag behind.** The workflow refuses a tag that exists, so if the tag step worked and the release step did not, the tag has to be removed (by someone allowed to) before the run is retried. Everything before the tag step leaves nothing behind.
- **The build job never sees `../libviprs`.** CI's lint and test jobs clone the core next to this repo, but the release build uses the `libviprs` from `Cargo.lock`, which is what a user building from source gets as well.
- **`--locked` fails if `Cargo.lock` is stale.** That is the point. Fix it in the cut PR, not in the workflow.
- **No signing.** The binaries are not code signed or notarised. The README says what that means on macOS.
