#!/usr/bin/env python3
"""Per-file line-coverage ratchet.

Reads an LCOV file (`cargo llvm-cov --lcov`), compares per-file line% against
a committed baseline JSON, and fails CI if any file drops below
`baseline - tolerance_pct`.

Baselines are rounded DOWN to whole percentage points so minor refactor noise
doesn't flap the gate. Hosts BUMP a baseline by re-running with --update after
adding tests; LOWERING a baseline is intentional and reviewed in the PR diff.

Companion to docs/PANIC_AUDIT.md / the panic-budget CI job — same ratchet
philosophy, different signal.

Usage:
    python3 scripts/coverage_ratchet.py \\
        --lcov lcov.info \\
        --baseline crates/rubyrs/coverage_baseline.json

    # Refresh baselines after improvements:
    python3 scripts/coverage_ratchet.py \\
        --lcov lcov.info \\
        --baseline crates/rubyrs/coverage_baseline.json \\
        --update
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path


def parse_lcov(path: Path) -> dict[str, tuple[int, int]]:
    """Return `{relative_source_path: (lines_hit, lines_found)}`.

    LCOV record shape (only the fields we need):

        SF:<source path>
        LF:<lines instrumented>
        LH:<lines hit>
        end_of_record
    """
    out: dict[str, tuple[int, int]] = {}
    src: str | None = None
    lh: int | None = None
    lf: int | None = None
    with path.open() as fh:
        for raw in fh:
            line = raw.strip()
            if line.startswith("SF:"):
                src = line[3:]
            elif line.startswith("LH:"):
                lh = int(line[3:])
            elif line.startswith("LF:"):
                lf = int(line[3:])
            elif line == "end_of_record":
                if src is not None and lh is not None and lf is not None:
                    out[src] = (lh, lf)
                src, lh, lf = None, None, None
    return out


def normalize(path: str, repo_root: Path) -> str:
    """LCOV emits absolute paths; baselines are repo-relative for portability."""
    p = Path(path)
    try:
        return str(p.resolve().relative_to(repo_root.resolve()))
    except ValueError:
        # Outside the repo (a dep) — leave absolute so it's skipped by the
        # `files` allowlist in the baseline.
        return str(p)


def pct(hit: int, found: int) -> float:
    if found == 0:
        return 100.0
    return hit / found * 100.0


def floor_to_int(x: float) -> int:
    """Round DOWN to whole percent — baseline absorbs sub-1% noise."""
    return int(math.floor(x))


def load_baseline(path: Path) -> dict:
    if not path.exists():
        return {"tolerance_pct": 1.0, "files": {}, "excluded_files": {}}
    with path.open() as fh:
        return json.load(fh)


def is_ratchet_source(rel: str) -> bool:
    """True iff `rel` (repo-relative path) is a source file the ratchet
    measures. Single source of truth for "what counts as a source file" —
    used by both the LCOV ingest filter and the source-tree walker, so
    the two can't drift.

    Skips:
      - Paths outside `crates/` (third-party deps).
      - `target/` — compiler output.
      - `tests/`, `examples/`, `benches/` — not library code.
        (`cargo llvm-cov --all-targets` CAN emit records for these; the
        cargo-llvm-cov default skips most of them, but Copilot review on
        PR #274 noted the ratchet's filter has to be defensive here.)
      - `build.rs` — build scripts.
      - `fuzz/` — separate cargo package.
    """
    if not rel.startswith("crates/"):
        return False
    if "/target/" in rel:
        return False
    if "/tests/" in rel or "/examples/" in rel or "/benches/" in rel:
        return False
    if rel.endswith("/build.rs"):
        return False
    if "/fuzz/" in rel:
        return False
    if "/src/" not in rel:
        return False
    return True


def discover_source_files(repo_root: Path) -> set[str]:
    """Walk `crates/*/src/**/*.rs` and return every file matching
    `is_ratchet_source`. The result is what we expect to see ACCOUNTED
    FOR by the baseline: every entry must be either in `files` (measured
    by LCOV with a coverage baseline) OR in `excluded_files` (explicitly
    declared as having no executable lines / behind a feature flag CI
    doesn't enable). Anything that's neither is a new source file the
    host forgot to register.
    """
    out: set[str] = set()
    for p in (repo_root / "crates").rglob("*.rs"):
        rel = str(p.relative_to(repo_root))
        if is_ratchet_source(rel):
            out.add(rel)
    return out


def write_baseline(path: Path, data: dict) -> None:
    # Sort keys for stable diffs.
    files_sorted = dict(sorted(data["files"].items()))
    data["files"] = files_sorted
    with path.open("w") as fh:
        json.dump(data, fh, indent=2, ensure_ascii=False)
        fh.write("\n")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--lcov", type=Path, required=True, help="Path to lcov.info from cargo llvm-cov")
    ap.add_argument("--baseline", type=Path, required=True, help="Path to coverage baseline JSON")
    ap.add_argument(
        "--update",
        action="store_true",
        help="Rewrite baseline with current measurements (rounded down to whole percent).",
    )
    ap.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="Repo root (default: parent of scripts/)",
    )
    args = ap.parse_args()

    if not args.lcov.exists():
        print(f"::error::lcov file not found: {args.lcov}", file=sys.stderr)
        return 2

    coverage = parse_lcov(args.lcov)
    # Filter to repo-relative paths and apply the same source-file filter
    # the tree-walker uses (single source of truth — `is_ratchet_source`).
    # This drops third-party deps, plus examples/benches/build.rs records
    # `cargo llvm-cov --all-targets` may emit for them — Copilot review on
    # PR #274 caught the asymmetry where the LCOV ingest admitted these
    # while `discover_source_files` excluded them, so a measured example
    # would trip "missing from baseline" against a baseline that
    # legitimately omitted it.
    rel = {}
    for path, (hit, found) in coverage.items():
        norm = normalize(path, args.repo_root)
        if is_ratchet_source(norm):
            rel[norm] = (hit, found, pct(hit, found))

    baseline = load_baseline(args.baseline)
    tolerance = float(baseline.get("tolerance_pct", 1.0))
    baselines = baseline.get("files", {})
    excluded = set(baseline.get("excluded_files", {}).keys())

    source_files = discover_source_files(args.repo_root)

    if args.update:
        new_files = {f: floor_to_int(v[2]) for f, v in rel.items()}
        baseline["files"] = new_files
        baseline.setdefault("tolerance_pct", tolerance)
        baseline.setdefault("excluded_files", {})
        baseline.setdefault(
            "_doc",
            "Per-file line-coverage baselines, percent points (whole numbers). "
            "Generated by scripts/coverage_ratchet.py --update. "
            "Files NEW to the project default to their first measured %; "
            "files DROPPED from the project must also be dropped here. "
            "Files that produce NO LCOV records (static-only, feature-gated) "
            "go in `excluded_files` with a one-line reason. See docs/COVERAGE.md.",
        )
        write_baseline(args.baseline, baseline)
        print(f"Wrote {len(new_files)} baselines to {args.baseline}")
        # Flag any source files that --update didn't cover AND aren't in
        # excluded_files — the host needs to decide which list to put them in.
        unaccounted = sorted(source_files - set(new_files) - excluded)
        if unaccounted:
            print(
                "\n::warning::Source files NOT in LCOV and NOT in excluded_files. "
                "Add each to baseline.excluded_files with a one-line reason:"
            )
            for f in unaccounted:
                print(f"  {f}")
        return 0

    # Audit mode: any file < baseline - tolerance is a regression.
    fail = 0
    regressed: list[str] = []
    measured_without_baseline: list[str] = []
    for fpath, (hit, found, current) in sorted(rel.items()):
        if fpath in excluded:
            # File is in excluded_files but ALSO appeared in LCOV — the
            # exclusion is now wrong. Tell the host to delete the entry.
            regressed.append(
                f"{fpath}: {current:.1f}% measured, but listed in excluded_files. "
                f"Remove the exclusion and run --update to capture as a real baseline."
            )
            fail = 1
            continue
        if fpath not in baselines:
            measured_without_baseline.append(
                f"{fpath}: {current:.1f}% ({hit}/{found}) — no baseline"
            )
            continue
        base = float(baselines[fpath])
        floor = base - tolerance
        if current + 1e-9 < floor:
            regressed.append(
                f"{fpath}: {current:.1f}% < baseline {base:.0f}% - {tolerance:.1f}% tolerance "
                f"(={floor:.1f}%); {hit}/{found} lines"
            )
            fail = 1
        else:
            arrow = "+" if current >= base else "~"
            print(f"  {arrow} {fpath}: {current:.1f}% (baseline {base:.0f}%)")

    # Files measured by LCOV but missing from baseline — fail; host adds them
    # via --update. Mirrors the panic-budget pattern (every file has an entry).
    if measured_without_baseline:
        fail = 1
        print("\n::error::Source files measured by LCOV but missing from baseline:")
        for line in measured_without_baseline:
            print(f"  {line}")
        print(
            "Fix: run `python3 scripts/coverage_ratchet.py --lcov lcov.info "
            f"--baseline {args.baseline} --update` to capture baselines."
        )

    # Source files on disk that aren't in LCOV AND aren't in baselines AND
    # aren't in excluded_files — the gap Copilot review flagged on PR #274.
    # A new static-only or feature-gated source file (e.g.
    # `_cext_link_keep_alive.rs`) produces no coverage records, so the
    # LCOV-driven check above never fires. Walking the tree catches it.
    measured_or_baselined = set(rel) | set(baselines) | excluded
    untracked_on_disk = sorted(source_files - measured_or_baselined)
    if untracked_on_disk:
        fail = 1
        print(
            "\n::error::Source files on disk that aren't measured AND aren't "
            "in excluded_files:"
        )
        for f in untracked_on_disk:
            print(f"  {f}")
        print(
            "Fix: each file must be in either `files` (if measured) or "
            "`excluded_files` (with a one-line reason — e.g. static-only, "
            "feature-gated). Add to `excluded_files` in the baseline JSON, "
            "or run --update if the file should be measurable on the default "
            "CI build."
        )

    # Baselines / exclusions for files no longer on disk — warn (don't fail).
    # An entry is stale when it appears in neither the LCOV records nor the
    # source-tree walk, i.e. the file was deleted but the registry didn't
    # catch up.
    stale = sorted((set(baselines) | excluded) - source_files - set(rel))
    if stale:
        print(
            "\n::warning::Baselines / exclusions for files that no longer "
            "exist in the project:"
        )
        for s in stale:
            print(f"  {s} (consider removing from baseline)")

    if regressed:
        print("\n::error::Coverage regressions:")
        for line in regressed:
            print(f"  {line}")
        print(
            "\nFix: either add tests so coverage recovers, OR (if the drop is "
            "intentional — e.g., production-code grew with deferred test work) "
            "rerun with --update and document in the PR body. Lowering a "
            "baseline is reviewed in the PR diff."
        )

    return fail


if __name__ == "__main__":
    sys.exit(main())
