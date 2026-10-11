#!/usr/bin/env bash
# scripts/gc-worktrees.sh — remove what agent sessions leave behind.
#
#   bash scripts/gc-worktrees.sh                 dry-run: show the plan, change nothing
#   bash scripts/gc-worktrees.sh --apply         remove merged, clean, unlocked worktrees
#                                                and their branches
#   bash scripts/gc-worktrees.sh --apply --branches
#                                                also delete local branches that are merged
#                                                and not checked out anywhere
#   bash scripts/gc-worktrees.sh --apply --target-root
#                                                also delete RUBYRS_TARGET_ROOT/<dir>s whose
#                                                .source checkout no longer exists
#   --base <ref>                                 what "merged" means (default origin/master)
#   --fetch                                      `git fetch origin` first
#
# A worktree is removed only when ALL of: its HEAD is an ancestor of --base,
# `git status --porcelain` is empty (untracked files count as dirty), it is
# not locked, and it is not the one this script runs in. Anything else is
# listed with the reason it was kept. Nothing is ever forced, and remote
# branches are never touched: deleting on origin is an outward-facing
# action, do it by hand.
#
# Why: 2026-10-10 on lawrences-mac-studio this repo had 86 worktrees (about
# 75 of them agent-*, 15 GB, two with 5-7 GB target dirs on a root disk with
# 36 GB free) and 501 of 564 local branches already merged. Nothing cleaned
# up after a session, and the fleet's agent-worktree trap doc records work
# lost from worktrees that LOOKED abandoned, so this refuses to guess:
# unmerged or dirty means kept.

set -euo pipefail

APPLY=0; BRANCHES=0; TARGET_ROOT=0; FETCH=0; BASE=origin/master
while (( $# )); do
  case "$1" in
    --apply)       APPLY=1 ;;
    --branches)    BRANCHES=1 ;;
    --target-root) TARGET_ROOT=1 ;;
    --fetch)       FETCH=1 ;;
    --base)        shift; BASE="${1:?--base needs a ref}" ;;
    -h|--help)     sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"
# shellcheck source=scripts/dev-env.sh
. "$here/scripts/dev-env.sh"

(( FETCH )) && git fetch origin --quiet
base_sha="$(git rev-parse --verify --quiet "$BASE^{commit}")" || { echo "no such ref: $BASE" >&2; exit 2; }
self="$(git rev-parse --show-toplevel)"
main_wt="$(git worktree list --porcelain | awk 'NR==1 && $1=="worktree" {print $2}')"

say()  { printf '%s\n' "$*"; }
plan() { if (( APPLY )); then say "  DO    $*"; else say "  WOULD $*"; fi; }

removed=0; kept=0; candidates=0

# --- Worktrees --------------------------------------------------------------
say "worktrees (base $BASE = ${base_sha:0:8}):"
path=""; head=""; branch=""; locked=0; prunable=0
flush() {
  [[ -n "$path" ]] || return 0
  local short="${path#"$main_wt"/}" reason="" ahead
  if [[ "$path" == "$main_wt" ]]; then :
  elif (( prunable )); then
    reason="prunable (directory gone) -> git worktree prune"
    candidates=$((candidates+1)); plan "prune registration of $short"
    (( APPLY )) && git worktree prune
  elif [[ "$path" == "$self" ]]; then reason="this script runs here"
  elif (( locked )); then reason="locked (a session is using it)"
  elif [[ ! -d "$path" ]]; then reason="directory missing but not yet prunable"
  elif ! git merge-base --is-ancestor "$head" "$base_sha"; then
    ahead="$(git rev-list --count "$base_sha..$head")"
    reason="unmerged: $ahead commit(s) not in $BASE"
  elif [[ -n "$(git -C "$path" status --porcelain 2>/dev/null)" ]]; then
    reason="dirty working tree (tracked changes or untracked files)"
  else
    candidates=$((candidates+1))
    plan "remove worktree $short${branch:+ and branch $branch}"
    if (( APPLY )); then
      if git worktree remove "$path"; then
        removed=$((removed+1))
        [[ -n "$branch" && "$branch" != master ]] && git branch -d "$branch" >/dev/null && say "        deleted branch $branch"
      else
        say "        git worktree remove refused; left in place"
      fi
    fi
  fi
  if [[ -n "$reason" ]]; then kept=$((kept+1)); say "  keep  $short: $reason"; fi
  path=""; head=""; branch=""; locked=0; prunable=0
}
while IFS= read -r line; do
  case "$line" in
    "worktree "*) flush; path="${line#worktree }" ;;
    "HEAD "*)     head="${line#HEAD }" ;;
    "branch "*)   branch="${line#branch refs/heads/}" ;;
    locked*)      locked=1 ;;
    prunable*)    prunable=1 ;;
    "")           ;;
  esac
done < <(git worktree list --porcelain)
flush

# --- Local branches -----------------------------------------------------------
if (( BRANCHES )); then
  say "local branches merged into $BASE and not checked out:"
  checked_out="$(git worktree list --porcelain | sed -n 's#^branch refs/heads/##p' | sort -u)"
  while IFS= read -r b; do
    [[ -z "$b" || "$b" == master || "$b" == main ]] && continue
    if grep -qxF "$b" <<<"$checked_out"; then continue; fi
    candidates=$((candidates+1)); plan "delete branch $b"
    (( APPLY )) && git branch -d "$b" >/dev/null
  done < <(git for-each-ref --format='%(refname:short)' --merged "$base_sha" refs/heads/)
fi

# --- Orphaned per-worktree target dirs ------------------------------------------
if (( TARGET_ROOT )); then
  if [[ -z "${RUBYRS_TARGET_ROOT:-}" ]]; then
    say "target dirs: RUBYRS_TARGET_ROOT not set (see scripts/dev-env.sh), skipping"
  else
    say "target dirs under $RUBYRS_TARGET_ROOT whose checkout is gone:"
    for d in "$RUBYRS_TARGET_ROOT"/*/; do
      [[ -f "$d.source" ]] || continue
      src="$(<"$d.source")"
      [[ -d "$src" ]] && continue
      size="$(du -sh "$d" 2>/dev/null | cut -f1)"
      candidates=$((candidates+1)); plan "rm -rf $d ($size, was $src)"
      (( APPLY )) && rm -rf "$d"
    done
  fi
fi

# --- Summary --------------------------------------------------------------------
if (( APPLY )); then
  say "done: $removed worktree(s) removed, $kept kept, $candidates action(s) taken"
else
  say "dry-run: $candidates action(s) planned, $kept worktree(s) kept. Re-run with --apply to execute."
fi
