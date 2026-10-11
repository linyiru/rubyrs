#!/usr/bin/env bash
# scripts/doctor.sh — is this checkout able to produce a trustworthy result?
#
#   bash scripts/doctor.sh            report; exit 0 unless something is broken
#   bash scripts/doctor.sh --strict   also exit 1 on warnings (for check scripts)
#   bash scripts/doctor.sh --hook     Claude Code SessionStart mode: compact
#                                     report on stdout (lands in the agent's
#                                     context), persist the environment from
#                                     scripts/dev-env.sh via CLAUDE_ENV_FILE,
#                                     always exit 0 so a session can still open
#
# Every check here is a failure mode that has looked like a rubyrs regression
# on a real machine: a toolchain that ignores rust-toolchain.toml, a CRuby
# oracle inside the 3.4.0/3.4.1 Prism bug window (docs/SUBSET.md), a missing
# rostdown submodule, a root disk that filled mid-build. The point is to say
# so BEFORE a 10-minute build, in words that name the fix.
#
# Exit codes: 0 ok (or --hook), 1 at least one FAIL (or WARN under --strict).

set -uo pipefail

MODE=report
case "${1:-}" in
  --hook)   MODE=hook ;;
  --strict) MODE=strict ;;
  "") ;;
  *) echo "usage: scripts/doctor.sh [--hook|--strict]" >&2; exit 2 ;;
esac

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2
# shellcheck source=scripts/dev-env.sh
. "$REPO_ROOT/scripts/dev-env.sh"

fails=0; warns=0
ok()   { printf 'ok    %s\n' "$1"; }
warn() { printf 'WARN  %s\n' "$1"; warns=$((warns+1)); }
fail() { printf 'FAIL  %s\n' "$1"; fails=$((fails+1)); }

# --- Rust toolchain -------------------------------------------------------
pin="$(sed -nE 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' rust-toolchain.toml | head -1)"
if ! command -v cargo >/dev/null 2>&1; then
  fail "cargo not on PATH"
else
  cargo_ver="$(cargo --version 2>/dev/null | awk '{print $2}')"
  cargo_path="$(command -v cargo)"
  if [[ "$cargo_ver" == "$pin" || "$cargo_ver" == "$pin".* ]]; then
    ok "cargo $cargo_ver matches rust-toolchain.toml ($pin) via $cargo_path"
  else
    fail "cargo $cargo_ver at $cargo_path; rust-toolchain.toml pins $pin and is being ignored (not a rustup proxy). Put \${CARGO_HOME:-~/.cargo}/bin first on PATH, or install rustup."
  fi
fi

# --- CRuby oracle ---------------------------------------------------------
want_ruby="$(tr -d '[:space:]' < .ruby-version 2>/dev/null)"
if ! command -v ruby >/dev/null 2>&1; then
  warn "ruby not on PATH: the diff suite (tests/diff_cruby.rs) will SKIP, not fail"
else
  ruby_ver="$(ruby -e 'print RUBY_VERSION' 2>/dev/null)"
  case "$ruby_ver" in
    3.4.0|3.4.1)
      fail "ruby $ruby_ver is inside the CRuby 3.4.0/3.4.1 Prism bug window (docs/SUBSET.md, ensure-walk corner): fixtures fail against a wrong oracle. .ruby-version asks for $want_ruby; install it or set RBENV_VERSION=$want_ruby." ;;
    3.4.*)
      if [[ "$ruby_ver" == "$want_ruby" ]]; then ok "ruby $ruby_ver (= .ruby-version)"
      else warn "ruby $ruby_ver; .ruby-version asks for $want_ruby (fine for CRuby 3.4.x >= 3.4.2, but not what CI runs)"; fi ;;
    *)
      fail "ruby $ruby_ver; rubyrs is pinned to CRuby 3.4 behaviour, install $want_ruby (.ruby-version)" ;;
  esac
fi

# --- Submodule ------------------------------------------------------------
if [[ -f crates/rostdown/Cargo.toml ]]; then
  ok "crates/rostdown submodule present"
else
  fail "crates/rostdown is empty: run \`git submodule update --init\` (fresh worktrees do not inherit it)"
fi

# --- Disk -----------------------------------------------------------------
# df -Pk is POSIX; column 4 is available KiB. Thresholds: a release build of
# this workspace is 5-8 GB; under 5 GB free the build will not finish, and
# under 20 GB two parallel worktrees will not.
free_gb() { df -Pk "$1" 2>/dev/null | awk 'NR==2 {printf "%d", $4/1048576}'; }
target_dir="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
mkdir -p "$target_dir" 2>/dev/null || true
root_free="$(free_gb "$REPO_ROOT")"
tgt_free="$(free_gb "$target_dir")"
if [[ -n "$tgt_free" ]]; then
  if   (( tgt_free < 5 ));  then fail "${tgt_free} GB free where builds go ($target_dir): a release build needs 5-8 GB"
  elif (( tgt_free < 20 )); then warn "${tgt_free} GB free where builds go ($target_dir): one build fits, two parallel worktrees do not"
  else ok "${tgt_free} GB free where builds go ($target_dir)"; fi
fi
if [[ -n "$root_free" && "$(df -Pk "$REPO_ROOT" | awk 'NR==2{print $1}')" != "$(df -Pk "$target_dir" | awk 'NR==2{print $1}')" ]]; then
  if (( root_free < 10 )); then warn "${root_free} GB free on the checkout's own volume; git and TMPDIR still need room there"
  else ok "${root_free} GB free on the checkout's own volume"; fi
fi
if [[ -z "${CARGO_TARGET_DIR:-}" && -n "$root_free" ]] && (( root_free < 60 )); then
  warn "CARGO_TARGET_DIR is in-tree on a volume with ${root_free} GB free. Set RUBYRS_TARGET_ROOT in \${XDG_CONFIG_HOME:-~/.config}/rubyrs/env to build on a scratch volume (see scripts/dev-env.sh)."
fi

# --- Caches and hygiene (informational) -------------------------------------
if [[ -n "${RUSTC_WRAPPER:-}" ]]; then ok "RUSTC_WRAPPER=$RUSTC_WRAPPER"
else warn "no sccache: every worktree rebuilds all dependencies (vendored Prism C, Cranelift) from scratch"; fi

common_dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"
if [[ -n "$common_dir" ]]; then
  wt_count="$(git worktree list --porcelain 2>/dev/null | grep -c '^worktree ')"
  if (( wt_count > 20 )); then
    warn "$wt_count git worktrees registered; run \`bash scripts/gc-worktrees.sh\` (dry-run) to see which are merged and removable"
  else
    ok "$wt_count git worktrees registered"
  fi
fi

# --- Hook mode: persist the environment for the rest of the session ---------
if [[ "$MODE" == hook && -n "${CLAUDE_ENV_FILE:-}" ]]; then
  {
    printf 'export PATH=%q\n' "$PATH"
    [[ -n "${CARGO_TARGET_DIR:-}" ]] && printf 'export CARGO_TARGET_DIR=%q\n' "$CARGO_TARGET_DIR"
    [[ -n "${TMPDIR:-}" ]]           && printf 'export TMPDIR=%q\n' "$TMPDIR"
    [[ -n "${RUSTC_WRAPPER:-}" ]]    && printf 'export RUSTC_WRAPPER=%q\n' "$RUSTC_WRAPPER"
  } >> "$CLAUDE_ENV_FILE"
  echo "env   PATH, CARGO_TARGET_DIR, TMPDIR persisted for this session (scripts/dev-env.sh)"
fi

# --- Summary ----------------------------------------------------------------
echo "doctor: $fails fail, $warns warn — toolchain=$(cargo --version 2>/dev/null | awk '{print $2}') ruby=${ruby_ver:-none} target=${CARGO_TARGET_DIR:-in-tree}"
case "$MODE" in
  hook)   exit 0 ;;
  strict) (( fails == 0 && warns == 0 )) ;;
  *)      (( fails == 0 )) ;;
esac
