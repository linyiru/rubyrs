# Releasing rubyrs

This document is the operational SOP for cutting a tagged
release. Read this start-to-finish before tagging the FIRST
release; for subsequent ones, the checklist at the top is
usually enough.

Per [`CHANGELOG.md`](CHANGELOG.md)'s header, rubyrs follows
[SemVer](https://semver.org/) starting at 0.1.0:

  - **Patch** (`0.1.0 → 0.1.1`): bug fixes, new Ruby methods
    that don't break existing embed code, internal refactors.
  - **Minor** (`0.1.0 → 0.2.0`): pre-1.0 breaks of any
    embedding API (`Runtime` / `Config` / `RubyError` /
    `Trap` / `Value` / `register_fn` / `set_stdout` / `eval` /
    `format_trap`). Per SemVer 0.y.z, breaks land in `y`.
  - **Major** (`1.0.0`): stable embedding API commitment.
    Not happening soon — too much surface area still moving
    (BigInt phase B, regex feature, Tier 2 boundary, the
    research-mode `do_call` / `compile_expr` splits in
    issues #152 / #153).

The "Ruby script-side surface" (subset of Ruby 3.4 the
interpreter accepts) is NOT part of the SemVer contract.
Closing a `gapscan` Missing class is always additive; new
Ruby methods land as patch bumps. Diverging behaviour from
CRuby is documented in `docs/SUBSET.md`.

---

## Pre-flight checklist

Run from a clean checkout of master HEAD. If any item below
fails, fix it BEFORE tagging — don't accept "I'll patch in
0.1.1 right after".

  - [ ] `git pull --rebase origin master` — local matches
        upstream
  - [ ] `git status` shows clean working tree (no
        uncommitted changes, no `.claude/worktrees/` or
        `target/` files staged)
  - [ ] **CI green on master HEAD for ≥ 24 hours.** Check
        `gh run list --branch master --limit 5` — if any of
        the last 5 runs failed and weren't followed by a
        passing run, investigate. A green run that's the
        immediate consequence of an "emergency fix" commit is
        NOT enough — let master bake.
  - [ ] `cargo test --workspace --release` — 0 failures
  - [ ] `cargo clippy --workspace --all-targets -- -D
        warnings` — clean
  - [ ] `STRESS_GC=1 cargo test --release -p rubyrs --test
        diff_cruby` — all fixtures pass
  - [ ] `cargo build --workspace --release` — no warnings
  - [ ] `CHANGELOG.md` `[Unreleased]` section reflects what's
        actually in this release (no stale entries from
        abandoned PRs, no missing entries from recent
        merges). Spot-check via `git log
        v<previous-tag>..HEAD --oneline | head -30`.
  - [ ] No open issues labelled `release-blocker`
        (`gh issue list --label release-blocker`)

If pre-flight is green, proceed.

## Workspace + per-crate versioning

The workspace version (`Cargo.toml`'s `[workspace.package]
version`) is the source of truth. All 7 member crates inherit
via `version.workspace = true`. Bump in one place.

Publishable crates (5 of 7):

  - `rubyrs-cext` — CRuby-shape C ABI; foundational, has no
    rubyrs deps
  - `rubyrs` — interpreter core; optionally depends on
    `rubyrs-cext`
  - `rubyrs-gapscan` — depends on `rubyrs` for the supported-
    nodes manifest
  - `rubyrs-spec-extract` — independent tooling
  - `rubund` — CLI runner (the `rubyrs` binary is bundled
    here too)

Non-publishable (`publish = false`):

  - `rubyrs-wasm-embed`, `rubyrs-wasm-timer`

For the **first release (0.1.0)**, defer publishing
`rubyrs-spec-extract` / `rubund` / `rubyrs-gapscan` to
crates.io unless we have specific reason to publish them.
Publish only `rubyrs-cext` + `rubyrs`. The non-published
crates still get the GitHub tag — anyone who clones the
repo at the tag SHA gets the full workspace.

## Step-by-step

### 1. Bump the workspace version

Edit the top-level `Cargo.toml`:

```toml
[workspace.package]
version = "0.1.0"     # ← target version
```

Run `cargo build` once to refresh `Cargo.lock`. Verify
nothing else changed:

```bash
git diff --stat
# Expect: Cargo.toml + Cargo.lock only.
```

### 2. Finalise CHANGELOG.md

Rename the `## [Unreleased]` section to `## [0.1.0] - YYYY-MM-DD`
(use today's date). Verify the `### Release highlights`
sub-section still applies (the high-level summary at the top
of the section — see `dde140d` for the convention).

Open a NEW empty `## [Unreleased]` block at the top, ready to
catch the next round of entries:

```markdown
## [Unreleased]

### Added
### Changed
### Fixed
### Internal

## [0.1.0] - 2026-MM-DD

### Release highlights
...
```

### 3. Commit the bump + CHANGELOG finalisation

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release: 0.1.0"
git push origin master
```

Wait for CI to go green on this commit before tagging. If
the version bump broke any dep version expectation, this is
where it surfaces.

### 4. Create the annotated tag

```bash
git tag -a v0.1.0 -m "rubyrs 0.1.0

$(awk '/^## \[0\.1\.0\]/,/^## \[/' CHANGELOG.md | head -n -1)
"
git push origin v0.1.0
```

The annotated tag carries the release-section content as its
message; viewable later via `git tag -v v0.1.0` /
`git show v0.1.0`.

### 5. Create the GitHub release

```bash
gh release create v0.1.0 \
  --title "rubyrs 0.1.0" \
  --notes-file <(awk '/^## \[0\.1\.0\]/,/^## \[/' CHANGELOG.md | head -n -1)
```

This creates the release page on GitHub with the same body
as the tag message. Use `--draft` if you want to review
before publishing; remove `--draft` (or `gh release edit
v0.1.0 --draft=false`) when ready to ship.

### 6. Publish to crates.io (optional, see policy above)

For the first release, only these two:

```bash
cargo publish -p rubyrs-cext
# Wait for crates.io to index — usually 30-60 seconds.
# `cargo search rubyrs-cext` should show the new version.
cargo publish -p rubyrs
```

The order matters: `rubyrs` declares
`rubyrs-cext = { path = "...", version = "0.1.0", optional = true }`,
so `rubyrs-cext = "0.1.0"` must already exist on crates.io
for `cargo publish -p rubyrs` to resolve.

If we decide to publish `rubyrs-gapscan` too:

```bash
cargo publish -p rubyrs-gapscan
# Depends on rubyrs; must run AFTER rubyrs is on crates.io.
```

### 7. Post-release

The empty `## [Unreleased]` section from step 2 is already in
place — new commits land there until the next release.

Bump the workspace version to the next anticipated minor (or
patch) tag as an alpha placeholder, so any contributor running
`cargo publish` accidentally doesn't ship 0.1.0 a second time:

```toml
version = "0.2.0-alpha.0"  # or "0.1.1-alpha.0"
```

Commit + push:

```bash
git commit -am "release: open 0.2.0-alpha development cycle"
git push origin master
```

## Troubleshooting

### "cargo publish: dependency `rubyrs` has no version specified"

A `path = "..."` dependency without a sibling `version = "..."`
fails the publish dry-run. Fix the offending `Cargo.toml`
entry to the standard pair form:

```toml
rubyrs-cext = { path = "../rubyrs-cext", version = "0.1.0", optional = true }
```

### "cargo publish: failed to publish — already exists"

A previous attempt succeeded mid-flight. Bump the version
(crates.io is immutable per (name, version) tuple — you
can't republish the same coordinate). Patch-bump:
`0.1.0 → 0.1.1`. Don't try to delete the bad release;
crates.io's `cargo yank` is a soft deprecation, not a
deletion.

### CI fails on the release commit but didn't fail on master

The version bump touched `Cargo.toml` / `Cargo.lock`; a
member crate's `version =` line probably wasn't using
`workspace = true`. Sync it up.

### Need to retract a release

`cargo yank --vers 0.1.0 rubyrs` (and `-p rubyrs-cext` if
that one's bad). Yanking keeps the version resolvable for
existing `Cargo.lock`s but stops new lockfiles from
selecting it. Cut a `0.1.1` with the fix. Update the
GitHub release with a "yanked — use 0.1.1" notice; don't
delete the GH release (preserves the historical record).

## Why this SOP exists

The first 0.1.0 release of rubyrs accumulated 1500+ lines of
unreleased changelog over 5+ months of pre-tag development.
The release-readiness review surfaced several "obvious in
hindsight" steps (path-dep version pairing, two duplicate
`### Changed` sections, crates.io publish order) that would
have caused friction if discovered at tag time. Documenting
them here means the second release ships in a clean morning,
not a frustrating afternoon.

If you're cutting the second release and find a step in this
doc to be wrong or stale, fix it in the same PR that
finalises the release. The SOP is meant to track reality, not
to be archaeology.
