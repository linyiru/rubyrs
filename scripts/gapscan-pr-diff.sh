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
TARGETS=(
  "jekyll|https://github.com/jekyll/jekyll|202df57|lib"
  "liquid|https://github.com/Shopify/liquid|742ac3d|lib"
  "sinatra|https://github.com/sinatra/sinatra|5236d34|lib"
  "dry-struct|https://github.com/dry-rb/dry-struct|26eb60f|lib"
  "rake|https://github.com/ruby/rake|5cea175|lib"
  "bundler|https://github.com/rubygems/rubygems|5c535b0|bundler/lib"
  "tilt|https://github.com/jeremyevans/tilt|6a0dae1|lib"
  "stdlib-set|https://github.com/ruby/ruby|48d4efc|lib/set.rb"
  "stdlib-optparse|https://github.com/ruby/ruby|48d4efc|lib/optparse.rb"
  "stdlib-uri|https://github.com/ruby/ruby|48d4efc|lib/uri"
)

# --- config -------------------------------------------------------
CACHE_DIR="${GAPSCAN_CACHE:-/tmp/gapscan-targets}"
BASE_REF="${GAPSCAN_BASE_REF:-origin/master}"
GAPSCAN_BIN_HEAD="${GAPSCAN_BIN_HEAD:-./target/release/rubyrs-gapscan}"
WORK="${GAPSCAN_WORK:-$(mktemp -d)}"

log() { printf "[gapscan-pr-diff] %s\n" "$*" >&2; }

# Files whose contents determine whether the gapscan classifier
# produces different output. If nothing in this set differs between
# BASE and HEAD, the diff is guaranteed empty — we skip the BASE
# build entirely. Single biggest CI saving.
CLASSIFIER_PATHS=(
  "crates/rubyrs/src/ast.rs"
  "crates/rubyrs/data/supported_prism_nodes.txt"
  "crates/rubyrs/data/rides_along_prism_nodes.txt"
  "crates/rubyrs-gapscan/src/"
  "crates/rubyrs-gapscan/data/"
  "crates/rubyrs-gapscan/build.rs"
)

# Returns 0 (true) when there's a difference that could affect
# classifier output between $BASE_REF and HEAD.
has_classifier_change() {
  ! git diff --quiet "$BASE_REF" -- "${CLASSIFIER_PATHS[@]}"
}

# --- clone or refresh each target into CACHE_DIR ------------------
# Clones are *blobless* partial clones (`--filter=blob:none`) with
# sparse-checkout restricted to the scanned relpath. The combination
# means: clone keeps full commit + tree history (so arbitrary SHAs
# resolve without re-fetching) but on first checkout only downloads
# the blobs for files inside relpath. For ruby/ruby this is a
# multi-hundred-MB saving — we only materialise lib/set.rb,
# lib/optparse.rb, or lib/uri/ depending on which target.
ensure_target() {
  local key="$1" repo="$2" sha="$3" relpath="$4"
  local dir="$CACHE_DIR/$key"
  if [[ ! -d "$dir/.git" ]]; then
    log "cloning $key from $repo (blobless + sparse on $relpath)"
    git clone --quiet --no-checkout --filter=blob:none --no-tags "$repo" "$dir"
    # Non-cone mode handles single-file paths like `lib/set.rb`.
    git -C "$dir" sparse-checkout set --no-cone "$relpath" "/$relpath"
  fi
  if ! git -C "$dir" rev-parse --verify --quiet "$sha^{commit}" >/dev/null; then
    log "fetching $sha for $key"
    # Fetch only the pinned SHA when the host supports it
    # (GitHub does, via uploadpack.allowReachableSHA1InWant);
    # fall back to a full fetch otherwise.
    git -C "$dir" fetch --quiet --no-tags --filter=blob:none origin "$sha" \
      || git -C "$dir" fetch --quiet --no-tags --filter=blob:none origin
  fi
  git -C "$dir" -c advice.detachedHead=false checkout --quiet "$sha"
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
repo = os.environ.get('GITHUB_REPOSITORY', '')
sha = os.environ.get('GITHUB_SHA', 'master')
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
  ensure_target "$key" "$repo" "$sha" "$relpath"
  scan_path="$CACHE_DIR/$key/$relpath"
  before="$WORK/${key}-base.json"
  after="$WORK/${key}-head.json"
  scan_json "$GAPSCAN_BIN_BASE" "$scan_path" "$before"
  scan_json "$GAPSCAN_BIN_HEAD" "$scan_path" "$after"
  aggregate "$key" "$before" "$after" >> "$per_target_jsonl"
done

render_markdown "$per_target_jsonl"
