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
# Caching: target codebases are cloned shallow into
# ${GAPSCAN_CACHE:-/tmp/gapscan-targets}. Re-runs against the same
# pins are nearly free (we `git fetch` + checkout the pinned SHA).
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

# --- clone or refresh each target into CACHE_DIR ------------------
ensure_target() {
  local key="$1" repo="$2" sha="$3"
  local dir="$CACHE_DIR/$key"
  if [[ ! -d "$dir/.git" ]]; then
    log "cloning $key from $repo"
    git clone --quiet --filter=blob:none "$repo" "$dir"
  fi
  # Fetch the pinned SHA explicitly (depth=1 from the SHA only).
  # Some git hosts refuse arbitrary-SHA fetches without protocol v2;
  # if that happens fall back to a full fetch.
  if ! git -C "$dir" rev-parse --verify --quiet "$sha^{commit}" >/dev/null; then
    log "fetching $sha for $key"
    git -C "$dir" fetch --quiet origin "$sha" 2>/dev/null \
      || git -C "$dir" fetch --quiet origin
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
  if [[ ! -d "$wt" ]]; then
    git worktree add --quiet --detach "$wt" "$base_sha"
  else
    git -C "$wt" -c advice.detachedHead=false checkout --quiet "$base_sha"
  fi
  log "building rubyrs-gapscan from $BASE_REF"
  ( cd "$wt"
    CARGO_TARGET_DIR="$WORK/base-target" cargo build --quiet --release -p rubyrs-gapscan >&2
  )
  GAPSCAN_BIN_BASE="$WORK/base-target/release/rubyrs-gapscan"
}

# --- scan one (binary, codebase path) into a JSON file ------------
scan_json() {
  local bin="$1" path="$2" out="$3"
  "$bin" scan "$path" --format json -o "$out" 2>/dev/null
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
# Takes the aggregated JSONL path as $1. We can't read it via stdin
# redirection because the python source itself comes from a heredoc
# on stdin (`python3 -`), which would consume the redirect.
render_markdown() {
  python3 - "$1" <<'PY'
import json, os, sys
rows = []
with open(sys.argv[1]) as f:
    for line in f:
        line = line.strip()
        if not line: continue
        rows.append(json.loads(line))

total_supported = sum(r['supported_delta'] for r in rows)
total_missing = sum(r['missing_delta'] for r in rows)
any_change = any(r['supported_delta'] or r['missing_delta']
                 or r['rides_along_delta'] or r['closed'] or r['new']
                 for r in rows)

print("## gapscan PR diff")
print()
if not any_change:
    print(f"This PR doesn't change any node-classification across the {len(rows)} canonical scan targets — `rubyrs::SUPPORTED_PRISM_NODES` is unchanged.")
    print()
    print("_See [docs/gap-reports/](../docs/gap-reports/README.md) for the dataset and methodology._")
    sys.exit(0)

print(f"vs `{os.environ.get('GAPSCAN_BASE_REF','origin/master')}`, across {len(rows)} canonical scan targets:")
print()
print(f"- **Σ Missing → Supported across all targets: {-total_missing:+d}**")
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
print("Pins live in [`scripts/gapscan-pr-diff.sh`](../scripts/gapscan-pr-diff.sh); bump them when [`docs/gap-reports/`](../docs/gap-reports/) snapshots are regenerated.")
PY
}

# --- main ---------------------------------------------------------
mkdir -p "$CACHE_DIR" "$WORK"

build_head_gapscan
build_base_gapscan

per_target_jsonl="$WORK/agg.jsonl"
: > "$per_target_jsonl"

for target in "${TARGETS[@]}"; do
  IFS='|' read -r key repo sha relpath <<< "$target"
  ensure_target "$key" "$repo" "$sha"
  scan_path="$CACHE_DIR/$key/$relpath"
  before="$WORK/${key}-base.json"
  after="$WORK/${key}-head.json"
  scan_json "$GAPSCAN_BIN_BASE" "$scan_path" "$before"
  scan_json "$GAPSCAN_BIN_HEAD" "$scan_path" "$after"
  aggregate "$key" "$before" "$after" >> "$per_target_jsonl"
done

render_markdown "$per_target_jsonl"
