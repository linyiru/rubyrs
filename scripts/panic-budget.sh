#!/usr/bin/env bash
# scripts/panic-budget.sh — the per-file panic-site ratchet (CI job
# `panic-budget`, docs/PANIC_AUDIT.md).
#
# Counts `panic!`, `.unwrap()` and `.expect(` in each crates/rubyrs/src
# file and fails if any count exceeds its budget in
# scripts/panic-budgets.tsv. Budgets may only go down; raising one needs a
# comment above its row and lands in the same commit as the new site.
#
# FAIL-CLOSED (2026-06-29): a file with a row keeps that budget; every
# OTHER crates/rubyrs/src/*.rs is held at 0, so a new file (or the first
# panic site in a file that has none today) fails until someone adds a
# reviewed row. That replaced an allowlist which silently ignored ~37 of
# ~59 source files.
#
# The budgets moved here from an inline step in ci.yml on 2026-10-10,
# because every budget change edited the 1,700-line workflow file.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
budgets=scripts/panic-budgets.tsv

fail=0
checked=" "
check() {
  local file="$1" budget="$2"
  checked="$checked$file "
  if [[ ! -f "$file" ]]; then
    echo "::error file=$budgets::$file has a budget row but does not exist; delete the row."
    fail=1
    return
  fi
  # Strip lines from the first `#[cfg(test)]` onward: in-tree unit tests
  # routinely call `.unwrap()` / `.expect(...)` / `panic!(...)` for fixture
  # assertions, and counting those would ratchet test churn instead of
  # ICE-class production sites. The convention is a `#[cfg(test)] mod tests
  # { ... }` at the bottom of each file, so stripping at the first
  # occurrence is safe. Doc-comment lines (`///`, `//!`, indented or not)
  # are text, not code, and are subtracted.
  local prod count doc
  prod=$(awk '/^#\[cfg\(test\)\]/ {exit} {print}' "$file")
  count=$(printf '%s\n' "$prod" | grep -cE 'panic!|\.unwrap\(\)|\.expect\(' || true)
  doc=$(printf '%s\n' "$prod" | grep -E '^[[:space:]]*(///|//!)' | grep -cE 'panic!|\.unwrap\(\)|\.expect\(' || true)
  count=$((count - doc))
  echo "$file: $count (budget $budget)"
  if [ "$count" -gt "$budget" ]; then
    echo "::error file=$file::Panic budget exceeded: $count > $budget. See docs/PANIC_AUDIT.md."
    fail=1
  fi
}

while IFS=$'\t' read -r file budget _note || [[ -n "$file" ]]; do
  case "$file" in ''|'#'*) continue ;; esac
  if ! [[ "$budget" =~ ^[0-9]+$ ]]; then
    echo "::error file=$budgets::row for $file has no numeric budget (columns are tab-separated)"
    fail=1
    continue
  fi
  check "$file" "$budget"
done < "$budgets"

# Fail-closed sweep: everything without a row is held at 0.
while IFS= read -r f; do
  case "$checked" in *" $f "*) continue ;; esac
  check "$f" 0
done < <(find crates/rubyrs/src -name '*.rs' | sort)
exit "$fail"
