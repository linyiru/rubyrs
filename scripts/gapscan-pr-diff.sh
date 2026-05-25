#!/usr/bin/env bash
# Render a Markdown summary of how the current working tree's rubyrs
# changes the gapscan picture against `master`, across the 10
# canonical scanned codebases.
#
# Run locally:
#   ./scripts/gapscan-pr-diff.sh > /tmp/diff.md
#
# Run in CI: see .github/workflows/gapscan-pr.yml. The CI posts the
# output as a sticky PR comment.
#
# Output goes to stdout. Diagnostic noise goes to stderr.
#
# Caching: target codebases are cloned blobless with sparse-checkout
# (only the relpath we scan is materialised — see `ensure_target`)
# into ${GAPSCAN_CACHE:-/tmp/gapscan-targets}. Re-runs against the
# same pins are nearly free (we `git fetch` + checkout the pinned SHA).
#
# Pin philosophy: each target is pinned to a specific commit so the
# diff measures *the PR's effect on rubyrs* — not "what the target
# codebase did upstream since the last diff". Bump pins explicitly
# when the docs/gap-reports/ snapshots get regenerated.

set -euo pipefail

# --- targets -------------------------------------------------------
# Format: key|repo_url|commit|relative_path
# Keep pins in sync with the SHAs recorded in docs/gap-reports/*.md
# headers so the per-codebase Δ here lines up with what those reports
# would change to.
# Full 40-character SHAs are required: `git fetch origin <abbrev>`
# is not supported by the protocol — only full hashes (or named
# refs) work for direct-SHA fetches. Using abbrevs silently falls
# back to a full `git fetch origin`, defeating the optimization.
TARGETS=(
  "jekyll|https://github.com/jekyll/jekyll|202df571314ba1d18e9fccd81d12aaad4a703c38|lib"
  "liquid|https://github.com/Shopify/liquid|742ac3dbf5432c6c2689ea00da604d7379f73799|lib"
  "sinatra|https://github.com/sinatra/sinatra|5236d3459b8b9015e5ce21ddd0c6beb0db4081d4|lib"
  "dry-struct|https://github.com/dry-rb/dry-struct|26eb60f8e320f8e0717d92f9bf6bb0ea98eb3f7b|lib"
  "rake|https://github.com/ruby/rake|5cea175679e5b692d5fc35255548c297d56b35d2|lib"
  "bundler|https://github.com/rubygems/rubygems|5c535b050b0f528d21569302026d7bb1bdcfb668|bundler/lib"
  "tilt|https://github.com/jeremyevans/tilt|6a0dae17cdeaab877339d475cf075618ad9250d1|lib"
  "stdlib-set|https://github.com/ruby/ruby|48d4efcb85000e1ebae42004e963b5d0cedddcf2|lib/set.rb"
  "stdlib-optparse|https://github.com/ruby/ruby|48d4efcb85000e1ebae42004e963b5d0cedddcf2|lib/optparse.rb"
  "stdlib-uri|https://github.com/ruby/ruby|48d4efcb85000e1ebae42004e963b5d0cedddcf2|lib/uri"
)

# --- config -------------------------------------------------------
CACHE_DIR="${GAPSCAN_CACHE:-/tmp/gapscan-targets}"
BASE_REF="${GAPSCAN_BASE_REF:-origin/master}"
# Honour CARGO_TARGET_DIR if the caller (or a shell-level export)
# redirects cargo's build output — otherwise the default `./target`
# matches what `cargo build` produces in this repo's root.
GAPSCAN_BIN_HEAD="${GAPSCAN_BIN_HEAD:-${CARGO_TARGET_DIR:-./target}/release/rubyrs-gapscan}"
if [[ -n "${GAPSCAN_WORK:-}" ]]; then
  # Caller manages lifecycle (CI passes a path that participates in
  # actions/cache; we mustn't delete it on exit).
  WORK="$GAPSCAN_WORK"
else
  WORK=$(mktemp -d)
  # Clean up auto-created WORK on exit, including the git worktree
  # registration the base build creates. Without this, repeated
  # local runs leave $TMPDIR/tmp.* dirs and stale .git/worktrees/
  # entries behind.
  cleanup_work() {
    if [[ -d "$WORK/base-worktree" ]]; then
      git worktree remove --force "$WORK/base-worktree" 2>/dev/null || true
    fi
    rm -rf "$WORK"
  }
  trap cleanup_work EXIT
fi

log() { printf "[gapscan-pr-diff] %s\n" "$*" >&2; }

# Files whose contents determine whether the gapscan classifier
# produces different output. If nothing in this set differs between
# BASE and HEAD, the diff is guaranteed empty — we skip the BASE
# build entirely. Single biggest CI saving.
CLASSIFIER_PATHS=(
  "crates/rubyrs/src/ast.rs"
  "crates/rubyrs/data/supported_prism_nodes.txt"
  "crates/rubyrs/data/rides_along_prism_nodes.txt"
  # rubyrs's build.rs is what *emits* SUPPORTED_PRISM_NODES /
  # RIDES_ALONG_PRISM_NODES from the data files — changes to it
  # can shift classification even when the data files don't.
  "crates/rubyrs/build.rs"
  "crates/rubyrs-gapscan/src/"
  "crates/rubyrs-gapscan/data/"
  "crates/rubyrs-gapscan/build.rs"
  # Dependency manifests + lockfile: bumping the `ruby_prism` crate
  # (which we depend on for AST node discrimination) or changing
  # its features can shift the universe of node classes — and
  # therefore the classification — without touching any src/ or
  # data/ files. Include all Cargo.toml + Cargo.lock so dep-only
  # PRs don't sneak past the fast-path.
  "Cargo.toml"
  "Cargo.lock"
  "crates/rubyrs/Cargo.toml"
  "crates/rubyrs-gapscan/Cargo.toml"
)

# Returns 0 (true) when there's a difference that could affect
# classifier output between $BASE_REF and HEAD.
has_classifier_change() {
  ! git diff --quiet "$BASE_REF" -- "${CLASSIFIER_PATHS[@]}"
}

# --- clone or refresh each target into CACHE_DIR ------------------
# Clones are *blobless* partial clones (`--filter=blob:none`) with
# sparse-checkout restricted to the scanned relpaths. The combination
# means: clone keeps full commit + tree history (so arbitrary SHAs
# resolve without re-fetching) but checkout only materialises blobs
# for files inside the listed relpaths.
#
# Same-repo targets share one underlying clone, keyed on repo URL.
# ruby/ruby in particular has three TARGETS rows (stdlib-set,
# stdlib-optparse, stdlib-uri) all pinned to the same SHA — cloning
# it once instead of three times saves ~200M of cache and the
# corresponding clone time. The sparse-checkout grows incrementally
# as each new relpath is added.
#
# Echoes the resolved checkout directory on stdout so the caller can
# compose `$dir/$relpath` for scanning.
repo_cache_dir() {
  # Stable, filesystem-safe directory name from the repo URL.
  # `sha1sum` ships with Linux coreutils but not vanilla macOS,
  # which has `shasum -a 1` instead. Probe at call time so the
  # script works in both local-dev (macOS) and CI (Linux) contexts.
  local hasher
  if command -v sha1sum >/dev/null 2>&1; then
    hasher="sha1sum"
  else
    hasher="shasum -a 1"
  fi
  printf "%s\n" "$1" | $hasher | cut -c1-12
}

ensure_target() {
  local key="$1" repo="$2" sha="$3" relpath="$4"
  local dir="$CACHE_DIR/$(repo_cache_dir "$repo")"
  if [[ ! -d "$dir/.git" ]]; then
    log "cloning $repo (blobless + sparse) into shared cache dir"
    git clone --quiet --no-checkout --filter=blob:none --no-tags "$repo" "$dir"
    # Non-cone mode handles single-file paths like `lib/set.rb`.
    git -C "$dir" sparse-checkout set --no-cone "/$relpath"
  else
    # Repo already cloned for an earlier same-repo target — extend
    # the sparse-checkout to include this target's relpath too.
    # `add` is idempotent; safe to call when relpath is already in.
    # Note `add` doesn't accept --no-cone (it inherits whatever mode
    # the initial `set --no-cone` left in effect).
    git -C "$dir" sparse-checkout add "/$relpath"
  fi
  if ! git -C "$dir" rev-parse --verify --quiet "$sha^{commit}" >/dev/null; then
    log "fetching $sha for $repo"
    # Fetch only the pinned SHA when the host supports it
    # (GitHub does, via uploadpack.allowReachableSHA1InWant);
    # fall back to a full fetch otherwise.
    git -C "$dir" fetch --quiet --no-tags --filter=blob:none origin "$sha" \
      || git -C "$dir" fetch --quiet --no-tags --filter=blob:none origin
  fi
  git -C "$dir" -c advice.detachedHead=false checkout --quiet "$sha"
  echo "$dir"
}

# --- build the HEAD gapscan binary --------------------------------
build_head_gapscan() {
  log "building rubyrs-gapscan from HEAD"
  cargo build --quiet --release -p rubyrs-gapscan >&2
}

# --- build a separate gapscan against the base commit, in a worktree
build_base_gapscan() {
  local wt="$WORK/base-worktree"
  local base_sha
  base_sha=$(git rev-parse "$BASE_REF")
  log "base ref $BASE_REF resolves to $base_sha"
  # Handle the "stale registration" case where the prior worktree
  # directory was removed but git still has it on its list — `add`
  # will refuse with "missing but already registered". Prune first;
  # if a directory exists but isn't recognised, force-remove and
  # recreate.
  git worktree prune >/dev/null 2>&1 || true
  if [[ ! -d "$wt" ]]; then
    git worktree add --quiet --detach "$wt" "$base_sha"
  else
    if ! git -C "$wt" -c advice.detachedHead=false checkout --quiet "$base_sha" 2>/dev/null; then
      log "stale worktree at $wt, removing and recreating"
      git worktree remove --force "$wt" 2>/dev/null || rm -rf "$wt"
      git worktree add --quiet --detach "$wt" "$base_sha"
    fi
  fi
  log "building rubyrs-gapscan from $BASE_REF"
  ( cd "$wt"
    CARGO_TARGET_DIR="$WORK/base-target" cargo build --quiet --release -p rubyrs-gapscan >&2
  )
  GAPSCAN_BIN_BASE="$WORK/base-target/release/rubyrs-gapscan"
}

# --- scan one (binary, codebase path) into a JSON file ------------
# Lets stderr pass through so gapscan's own diagnostics
# (unknown-class warnings, scan-failure messages) survive into the
# CI log — silencing them was actively unhelpful for debugging
# (per PR #18 review).
scan_json() {
  local bin="$1" path="$2" out="$3"
  "$bin" scan "$path" --format json -o "$out"
}

# --- aggregate per-codebase diffs ---------------------------------
aggregate() {
  local key="$1" before="$2" after="$3"
  python3 - "$key" "$before" "$after" <<'PY'
import json, sys
key, before_p, after_p = sys.argv[1], sys.argv[2], sys.argv[3]
b = json.load(open(before_p)); a = json.load(open(after_p))
bt = b['totals']; at = a['totals']
ds = at['supported'] - bt['supported']
dr = at['rides_along'] - bt['rides_along']
dm = at['missing'] - bt['missing']
# Closed: classes that were Missing in `before` and are no longer
# Missing in `after`. Honour scan-time classification.
def missing_set(rep):
    return {h['class']: h['count'] for h in rep['histogram']
            if h.get('classification') == 'Missing'}
bm = missing_set(b); am = missing_set(a)
closed = [(c, bm[c]) for c in bm if am.get(c, 0) == 0]
new = [(c, am[c]) for c in am if bm.get(c, 0) == 0]
closed.sort(key=lambda x: -x[1])
new.sort(key=lambda x: -x[1])
out = {
    'key': key,
    'before_supported_pct': bt['supported'] / max(sum(bt.values()), 1) * 100,
    'after_supported_pct':  at['supported'] / max(sum(at.values()), 1) * 100,
    'supported_delta': ds,
    'rides_along_delta': dr,
    'missing_delta': dm,
    'closed': closed[:5],
    'new': new[:5],
}
print(json.dumps(out))
PY
}

# --- render the Markdown comment ----------------------------------
# Takes the aggregated JSONL path as $1 and an optional "fast-path"
# marker as $2 (empty string when the full path ran). We can't read
# the JSONL via stdin redirection because the python source itself
# comes from a heredoc on stdin (`python3 -`), which would consume
# the redirect.
render_markdown() {
  python3 - "$1" "${2:-}" <<'PY'
import json, os, sys
rows = []
with open(sys.argv[1]) as f:
    for line in f:
        line = line.strip()
        if not line: continue
        rows.append(json.loads(line))
fast_path = sys.argv[2] == "fast-path"

total_supported = sum(r['supported_delta'] for r in rows)
total_missing = sum(r['missing_delta'] for r in rows)
any_change = any(r['supported_delta'] or r['missing_delta']
                 or r['rides_along_delta'] or r['closed'] or r['new']
                 for r in rows)

# Render with absolute GitHub URLs so the links work in a PR
# comment context (where relative paths resolve against /pull/N/,
# not the repo root). Fall back to relative paths when not in CI.
#
# On `pull_request` events GitHub sets GITHUB_SHA to the synthetic
# merge commit (refs/pull/N/merge), not the PR head. Links built
# from that point at a tree the reader didn't intend. The workflow
# passes the actual head SHA via GAPSCAN_COMMIT_SHA; prefer it,
# fall through to GITHUB_SHA (e.g. push events) and finally to
# 'master' for local renders.
repo = os.environ.get('GITHUB_REPOSITORY', '')
sha = os.environ.get('GAPSCAN_COMMIT_SHA') or os.environ.get('GITHUB_SHA', 'master')
def url(rel):
    if repo:
        return f"https://github.com/{repo}/blob/{sha}/{rel}"
    return rel  # local mode

print("## gapscan PR diff")
print()
if fast_path:
    # This branch is reached when the script's pre-flight check
    # saw zero changes in classifier-relevant files (ast.rs,
    # supported_prism_nodes.txt, rides_along_prism_nodes.txt, the
    # gapscan crate). The classifier can't differ as a result.
    print(f"This PR doesn't touch any file that affects gapscan classification — `SUPPORTED_PRISM_NODES` / `RIDES_ALONG_PRISM_NODES` are unchanged. Skipped the base build and the {len(rows)} per-target scans.")
    print()
    print(f"_See [docs/gap-reports/]({url('docs/gap-reports/README.md')}) for the dataset and methodology._")
    sys.exit(0)
if not any_change:
    # Both binaries actually ran, both scanned all targets,
    # results matched. The classifier output may still have
    # changed for node classes that none of the 10 targets
    # happen to exercise — keep the wording honest about what
    # was observed.
    print(f"Both binaries produced identical histograms across the {len(rows)} canonical scan targets. (If the classifier changed for node classes that none of these targets exercise, this view won't catch it — the data-file diff would.)")
    print()
    print(f"_See [docs/gap-reports/]({url('docs/gap-reports/README.md')}) for the dataset and methodology._")
    sys.exit(0)

print(f"vs `{os.environ.get('GAPSCAN_BASE_REF','origin/master')}`, across {len(rows)} canonical scan targets:")
print()
# Renamed from "Σ Missing → Supported": missing_delta counts any
# Missing exit (Missing → Supported, Missing → RidesAlong, or
# Missing-class no-longer-appearing). The "→ Supported" framing
# overcounted. supported_delta is the strict "now classifies as
# Supported" view; keep both for transparency.
print(f"- **Σ Missing delta across all targets: {total_missing:+d}** (lower is better)")
print(f"- Σ Supported delta: {total_supported:+d}")
print()
print("| Codebase | %Sup before → after | Δ Missing | Closed (top 5) | New (top 5) |")
print("|---|---:|---:|---|---|")
for r in rows:
    closed_s = ", ".join(f"`{c}` ×{n}" for c, n in r['closed']) or "—"
    new_s = ", ".join(f"`{c}` ×{n}" for c, n in r['new']) or "—"
    arrow = f"{r['before_supported_pct']:.2f}% → {r['after_supported_pct']:.2f}%"
    sign = "+" if r['supported_delta'] >= 0 else ""
    print(f"| `{r['key']}` | {arrow} ({sign}{r['supported_delta']}) | {r['missing_delta']:+d} | {closed_s} | {new_s} |")
print()
print(f"Pins live in [`scripts/gapscan-pr-diff.sh`]({url('scripts/gapscan-pr-diff.sh')}); bump them when [`docs/gap-reports/`]({url('docs/gap-reports/')}) snapshots are regenerated.")
PY
}

# --- main ---------------------------------------------------------
mkdir -p "$CACHE_DIR" "$WORK"
per_target_jsonl="$WORK/agg.jsonl"

# Fast path: if the PR doesn't touch any file that can affect
# classifier output, the diff is guaranteed empty. Skip both the
# BASE rubyrs-gapscan build (~37s) and the per-target scans
# (~16s). Touch the empty jsonl so the renderer takes the
# "no change" branch.
if ! has_classifier_change; then
  log "no diff in classifier-relevant files vs $BASE_REF — skipping base build and scans"
  : > "$per_target_jsonl"
  # Emit one empty-but-present row per target so the renderer reports
  # the right `len(rows)` count in the "no change" message.
  for target in "${TARGETS[@]}"; do
    IFS='|' read -r key _ _ _ <<< "$target"
    printf '{"key":"%s","before_supported_pct":0,"after_supported_pct":0,"supported_delta":0,"rides_along_delta":0,"missing_delta":0,"closed":[],"new":[]}\n' "$key" >> "$per_target_jsonl"
  done
  render_markdown "$per_target_jsonl" "fast-path"
  exit 0
fi

build_head_gapscan
build_base_gapscan

: > "$per_target_jsonl"
for target in "${TARGETS[@]}"; do
  IFS='|' read -r key repo sha relpath <<< "$target"
  dir=$(ensure_target "$key" "$repo" "$sha" "$relpath")
  scan_path="$dir/$relpath"
  before="$WORK/${key}-base.json"
  after="$WORK/${key}-head.json"
  scan_json "$GAPSCAN_BIN_BASE" "$scan_path" "$before"
  scan_json "$GAPSCAN_BIN_HEAD" "$scan_path" "$after"
  aggregate "$key" "$before" "$after" >> "$per_target_jsonl"
done

render_markdown "$per_target_jsonl"
