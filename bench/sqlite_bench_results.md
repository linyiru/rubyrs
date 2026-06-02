# SQLite benchmark — `_sqlite` battery vs CRuby `sqlite3` gem

Workload: `:memory:` DB, 8000 rows pre-seeded; four operation
shapes timed 2000 iters × 3 runs, min total / iters reported.
Driver: `bench/sqlite_bench.rb`. Run on the same script that
`require`s `"rubyrs/sqlite"` (rubyrs) or `"sqlite3"` (CRuby);
runtime-aware shim picks the right one.

Numbers from 2026-06-02, ARM macOS / Apple Silicon, release
builds. CRuby 3.4 + sqlite3 gem 2.5.0 (bundled libsqlite3
3.47.2). rubyrs built with `--features _sqlite,stdlib` (Phase
3 commit `2cfa9c5e`).

| Workload                  | CRuby + sqlite3 gem | rubyrs `_sqlite` (Phase 3) | vs CRuby |
|---------------------------|---------------------|----------------------------|----------|
| `bulk_insert`             | ~3.2 µs/iter        | **~2.4 µs/iter**           | **0.74× (35 % faster)** |
| `select_one_cached`       | ~1.25 µs/iter       | ~1.4 µs/iter               | 1.13× (12 % slower) |
| `select_one_uncached`     | ~2.8 µs/iter        | **~2.35 µs/iter**          | **0.84× (20 % faster)** |
| `select_many` (8000 rows) | ~2300 µs/iter       | **~1650 µs/iter**          | **0.72× (40 % faster)** |

Multiplier vs CRuby (lower = faster). **Bold** = rubyrs beats
CRuby.

## Takeaways

- **rubyrs's `_sqlite` battery beats CRuby on 3 of 4 shapes
  out of the gate**, on a Phase-3 PoC with no perf tuning. The
  underlying libsqlite3 is the same library in both (CRuby's
  gem bundles 3.47.2; rusqlite's bundled feature ships its
  own; both are recent 3.x). The win is at the language↔C
  boundary: rubyrs's host-fn dispatch (Ruby Op::Call →
  HostFnSlot::V1 closure → rusqlite call → Value return)
  pays less per-call overhead than CRuby's sqlite3 gem's
  Ruby↔C bridge for the bulk / multi-row shapes.
- **`select_one_cached` is 12 % slower** — the one shape where
  CRuby's prepare-once + `.execute(id)` pattern outpaces ours.
  Our cached path does Ruby method dispatch
  (`db.query_cached`) → host-fn boundary → `SQLITE_CONNS`
  thread_local borrow → `HashMap<i64, ConnState>` lookup →
  LRU get → bind + step + row marshalling. CRuby skips the
  per-call SQL string lookup (the caller holds the
  `SQLite3::Statement` object directly). Closing this gap
  needs a similar shape — a Ruby-visible `SQLite3::Statement`
  class users hold across iterations — which is a follow-up
  (Phase 5b's Sequel-lite `Dataset` already constructs
  reusable prepared statements internally; that's where the
  win compounds).
- **`select_many` 40 % win** is the most interesting result:
  pure row-materialisation throughput. The inner loop is
  rusqlite stepping rows + `vm.heap.alloc(HeapObj::Array(...))`
  per row + outer-array alloc; nothing Ruby-visible until the
  full result array is returned. ~8000 rows × the per-row
  cost dominates; rubyrs's heap alloc is monomorphic and
  cheap.
- **`bulk_insert` 35 % win** suggests the host-fn boundary's
  per-call cost is genuinely lower than CRuby's gem's
  Ruby↔C bridge cost, which translates straight into write
  throughput in tight INSERT loops (the canonical
  data-migration shape).

## Reproducing

```bash
# Build rubyrs with the battery on (matches the `cli-defaults`
# aggregate from ADR 0019; same shape `cargo install rubyrs`
# would ship)
cargo build --release --features _sqlite,stdlib -p rubyrs

# Run the bench on both runtimes
ruby                    bench/sqlite_bench.rb       # CRuby + sqlite3 gem
target/release/rubyrs   bench/sqlite_bench.rb       # rubyrs Phase 3
```

Environment knobs: `ITERS=2000 RUNS=3` (defaults). The bench
uses `:memory:` DB so no FS / journal-mode variance enters.

## Surface differences exercised

The bench's runtime-aware branches surface a couple of API
shape mismatches between the two SQLite bindings that the
omakase-menu Sequel-lite DSL (Phase 5b) is meant to paper
over:

- **rubyrs splits `execute` / `query`** (the former for
  non-SELECT; the latter for SELECT). **CRuby's gem unifies
  on `execute`** which returns row arrays for SELECT and an
  empty array for INSERT/UPDATE/DELETE. ADR 0027 §3 documents
  the split as intentional — rusqlite's `raw_execute` errors
  on statements that return rows, and conflating the two would
  push the "is this a SELECT?" check into Ruby-side surface.
  Sequel-lite hides both behind `Dataset#all` /
  `Dataset#insert` so users don't have to choose.
- **rubyrs `execute(sql, *params)` splat** vs **CRuby
  `execute(sql, [params_array])` array-only**. Both round-trip
  to the same SQL underneath but the Ruby-side ergonomics
  differ. Sequel-lite again abstracts.
- **rubyrs `execute_cached` / `query_cached`** are opt-in for
  the LRU; bare `execute` / `query` skip the cache (ADR 0027
  §4). **CRuby's gem auto-caches** based on
  `prepared_statement_cache_size` for every `execute`.

## What this doesn't measure

- **No transaction overhead**: each workload runs without
  `db.transaction { ... }`. The actual per-INSERT cost without
  a transaction is 10-100× higher (commit-per-row), so a
  realistic data-migration workload pays both the
  `bulk_insert` cost AND the rare COMMIT cost. The bench's
  3 µs/iter is the transaction-amortised cost.
- **No concurrent access** — rubyrs is single-threaded so
  `BUSY_TIMEOUT` handling, WAL contention, etc. are out of
  scope until `_thread` ships.
- **No prepared-statement-lifetime fanout**: the bench's
  cached path uses ONE SQL string. A workload with N distinct
  SQL strings exercises LRU eviction; rubyrs's LRU(100)
  default matches CRuby's, but the eviction cost has not
  been benched.

## Perf milestones (cumulative)

This is Phase 3's first day of measurement; no tuning yet. The
trajectory mirrors the JSON-bench progression — initial PoC
is already competitive; targeted optimisations close the
remaining gaps.

| Pass | Result |
|------|--------|
| Phase 3 baseline (Ruby-host-fn + LRU + heap alloc per row) | 3 of 4 shapes beat CRuby; `select_one_cached` 12 % behind |

Future passes (Phase 5b Sequel-lite + Phase 7 optional bench
+ Phase X tuning): close the `select_one_cached` gap via a
prepared-statement value type users can hold across iters
(Sequel's `Dataset` shape), and chase the remaining 10-20 %
margin via inlining the row-array allocation in
`collect_rows` (currently one alloc per row + one for the
outer array).
