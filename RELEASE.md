# Releasing

cargo-release makes a release on your machine, and the release workflow checks,
builds and publishes it. The settings are in `release.toml` and
`.github/workflows/release.yml`. The order of the steps on your machine was
checked with cargo-release 0.25.22; check it again after upgrading.

```bash
cargo install cargo-release
```

## Once, before the first release from the workflow

The workflow publishes to crates.io with a token it is given for each run, so
no token is stored anywhere. crates.io has to trust the workflow first: in
servio's settings there, under Trusted Publishing, add GitHub with the owner
`alexylon`, the repository `servio` and the workflow `release.yml`.

## Making a release

Look first. Without `--execute`, nothing is changed, but the tests still run,
on the version you have now:

```bash
cargo release patch
```

Then do it:

```bash
cargo release patch --execute
```

That one command, in this order:

1. Sets the new version in `Cargo.toml` and `Cargo.lock`
2. Moves everything under `## [Unreleased]` in `CHANGELOG.md` into a section
   for the new version, dated today
3. Runs the tests, and stops if any fail
4. Commits as `Release X.Y.Z`
5. Tags `vX.Y.Z`
6. Pushes the commit and the tag

It prints `Publishing servio` on the way, but packages and uploads nothing:
publishing is the workflow's job.

The tag starts `.github/workflows/release.yml`, which:

1. Runs every check a push gets on the tagged commit, and checks that the tag
   names the version in `Cargo.toml`
2. Builds the programs for Linux (x86-64 and arm64), macOS (Intel and Apple
   silicon) and Windows
3. Publishes the crate to crates.io
4. Creates a GitHub release with the archives and their checksums in
   `SHA256SUMS`

Nothing reaches crates.io unless every check and build has passed.

Say `patch` for a fix, `minor` for a new flag or a new behaviour, and `major`
for anything that changes what an existing command already does. On its own,
`cargo release` tries to release the version already in `Cargo.toml`.

## Before you start

- Everything committed: `git status`
- Formatted and clean for Clippy: `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings`
- Anything worth reading about is under `## [Unreleased]` in `CHANGELOG.md`.
  Nothing writes that for you.

The tests need no run of their own: the release runs them.

## Doing less than all of it

```bash
cargo release patch --execute --no-push   # commit and tag, but keep them here
```

## If it goes wrong

On your machine, before anything was pushed:

- There is no release commit, and `Cargo.toml`, `Cargo.lock` and
  `CHANGELOG.md` are left changed: the tests stopped the release. Put those
  three files back:

  ```bash
  git checkout -- Cargo.toml Cargo.lock CHANGELOG.md
  ```

- The last commit is `Release X.Y.Z`, and nothing else is uncommitted. Undo the
  tag and the commit:

  ```bash
  git tag -d vX.Y.Z
  git reset --hard HEAD~1
  ```

  Only when `git log -1 --oneline` shows the release commit: otherwise this
  throws away a commit of your own.

In the release workflow, after the push:

- A job failed by chance, such as a download that timed out: open the run on
  the Actions page and choose to re-run the failed jobs, not all of them, since
  publishing the same version twice fails.
- A check, the version check or a build failed for a real reason: nothing was
  published. Fix it in a new commit, push it, and move the tag to it. Pushing
  the moved tag starts the workflow again:

  ```bash
  git push origin HEAD
  git tag -f -a vX.Y.Z -m "Release X.Y.Z"
  git push -f origin vX.Y.Z
  ```

- Publishing failed because crates.io does not trust the workflow yet: do the
  step at the top, then re-run the failed jobs.
- The crate is on crates.io but the GitHub release failed: re-run the failed
  jobs. Leave the tag where it is, since the crate came from that commit.

A version on crates.io cannot be deleted. It can only be yanked, which stops
new projects from picking it up:

```bash
cargo yank --version X.Y.Z
```

## Watch out for

`cargo publish` and `cargo package` leave a copy of the crate in
`target/package/`, and a later `cargo build` can decide that copy is the
source and skip rebuilding your edits. If a change seems to have no effect,
`rm -rf target/package`. The tests the release runs build in
`target/release-tests` instead, where that copy cannot get in the way.
