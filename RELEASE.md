# Releasing

This project uses [cargo-release](https://rust-lang.github.io/cargo-release/).
The settings are in `release.toml`. The order of the steps below was checked
with cargo-release 0.25.22; check it again after upgrading.

```bash
cargo install cargo-release
```

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
3. Runs the tests on the source, and stops if any fail
4. Commits as `Release X.Y.Z`
5. Publishes to crates.io. Before uploading, Cargo builds the crate as it was
   packaged; that build runs no tests
6. Tags `vX.Y.Z`, once publishing has worked
7. Pushes the commit and the tag

Pushing the tag starts `.github/workflows/release.yml`. It runs the tests on
Linux, macOS and Windows, and only if they all pass builds binaries for Linux
(x86-64 and arm64), macOS (Intel and Apple silicon) and Windows, and attaches
them to a GitHub release with their checksums in `SHA256SUMS`. The crate is on
crates.io by then, so failing tests there hold back the binaries, not the
crate.

Say `patch` for a fix, `minor` for a new flag or a new behaviour, and `major`
for anything that changes what an existing command already does. On its own,
`cargo release` tries to release the version already in `Cargo.toml`.

## Before you start

- Everything committed: `git status`
- The tests pass: `cargo test`
- Formatted: `cargo fmt --check`
- Signed in to crates.io: `cargo login`
- Anything worth reading about is under `## [Unreleased]` in `CHANGELOG.md`.
  Nothing writes that for you.

## Doing less than all of it

```bash
cargo release patch --execute --no-publish   # skip crates.io
cargo release patch --execute --no-push      # keep it local
```

## If it goes wrong

First see how far it got:

```bash
git log -1 --oneline           # is the last commit "Release X.Y.Z"?
git status --short             # is anything left uncommitted?
cargo search servio --limit 1  # the newest version on crates.io
```

Before anything was pushed:

- There is no release commit, and `Cargo.toml`, `Cargo.lock` and
  `CHANGELOG.md` are left changed: the tests stopped the release. Put those
  three files back:

  ```bash
  git checkout -- Cargo.toml Cargo.lock CHANGELOG.md
  ```

- The last commit is `Release X.Y.Z`, nothing else is uncommitted, and X.Y.Z is
  not on crates.io. Undo the commit, and the tag if there is one:

  ```bash
  git reset --hard HEAD~1
  git tag -d vX.Y.Z
  ```

  Only when the last commit is the release commit: otherwise this throws away
  a commit of your own.

- X.Y.Z is on crates.io, but the tag or the push is missing. Keep the commit
  and do only the steps that are left, not the whole release again:

  ```bash
  cargo release tag --execute
  cargo release push --execute
  ```

  The tag step skips a tag that already exists, and says no packages were
  selected. If there is one, check that `git log -1 --oneline vX.Y.Z` shows
  the release commit, then run the push step.

After the tag was pushed:

- If a build failed, or a test failed by chance, run the failed jobs again from
  the Actions page; the tag can stay. A test that fails for a real reason means
  the crate on crates.io has the same fault: fix it and release the next patch.
- To take the tag back:

  ```bash
  git push origin :refs/tags/vX.Y.Z
  ```

  That does not take the crate off crates.io, or back from anyone who already
  downloaded a binary.

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
