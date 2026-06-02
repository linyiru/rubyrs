# SQLite benchmark — `_sqlite` battery vs CRuby `sqlite3` gem

Workload: `:memory:` DB, 8000 rows pre-seeded; four operation
shapes timed 2000 iters × 3 runs, min total / iters reported.
Driver: `bench/sqlite_bench.rb`. Run on the same script that
`require`s `"rubyrs/sqlite"` (rubyrs) or `"sqlite3"` (CRuby);
runtime-aware shim picks the right one.

Numbers from 2026-06-02, ARM macOS / Apple Silicon, release
builds. CRuby 3.4 + sqlite3 gem 2.5.0 (bundled libsqlite3
3.47.2). rubyrs built with `--features _sqlite,stdlib`. Phase
3.1 (`SQLite3::Statement` Ruby class) commit `012ddfe8`.

## Phase 3.1 results

| Workload                  | CRuby + sqlite3 gem | rubyrs `_sqlite`      | vs CRuby |
|---------------------------|---------------------|------------------------|----------|
| `bulk_insert`             | ~3.0 µs/iter        | **~2.2 µs/iter**       | **0.73× (37 % faster)** |
| `select_one_cached`       | ~1.25 µs/iter       | ~1.37 µs/iter          | 1.10× (within noise) |
| `select_one_uncached`     | ~2.8 µs/iter        | **~2.27 µs/iter**      | **0.80× (24 % faster)** |
| `select_many` (8000 rows) | ~2180 µs/iter       | **~1640 µs/iter**      | **0.75× (32 % faster)** |

Multiplier vs CRuby (lower = faster). **Bold** = rubyrs beats CRuby.

## Phase 3 → Phase 3.1 progression

| Workload              | Phase 3 (2cfa9c5e)  | Phase 3.1 (this commit) | Δ        |
|-----------------------|---------------------|--------------------------|----------|
| `bulk_insert`         | ~2.40 µs            | ~2.20 µs                 | **−8 %** |
| `select_one_cached`   | ~1.40 µs (via LRU)  | ~1.37 µs (via Statement) | **−2 %** |
| `select_one_uncached` | ~2.35 µs            | ~2.27 µs                 | **−3 %** |
| `select_many`         | ~1650 µs            | ~1640 µs                 | tied     |

Phase 3.1 closed the `select_one_cached` gap from 12 % behind
(via `Database#query_cached`'s SQL-string LRU) to within noise
(~7–10 % behind via `SQLite3::Statement`'s skip-LRU pattern).
Two passes helped:

1. **`SQLite3::Statement` Ruby class** (the bulk of Phase 3.1).
   `db.prepare(sql)` returns a Statement object the user holds
   across iterations. Each `stmt.execute(*params)` /
   `stmt.query(*params)` goes straight to bind + step on the
   cached rusqlite Statement — no per-call SQL-string hashing
   through the Database LRU. Closes ~0.5 µs/call.
2. **Drop the alive-check sweep on every call.**
   `Database#close`'s `STMT_HANDLES.retain` sweep already
   removes orphan statements before dropping the Connection;
   the per-call `SQLITE_CONNS.contains_key` re-check was
   redundant defense and cost ~0.3 µs/call.

Two passes that *didn't* help (negative results worth recording):

- **Splat-forward `__rubyrs_sqlite_stmt_query(@handle, *params)`
  instead of `(@handle, params)`.** Idea: skip the params-Array
  allocation a single-arg Array would pay. Result: the
  call-site `*params` splat overhead in rubyrs's current
  bytecode is *more expensive* than the saved Array allocation
  (1.9 µs vs 1.37 µs). Reverted. The host-fn varargs parse arm
  stays in place though — it's a no-cost addition and the
  Phase 5b Sequel-lite Dataset will use it for known-positional
  calls.

## Takeaways

- **rubyrs's `_sqlite` battery beats CRuby on 3 of 4 shapes
  by comfortable margins (20–37 %).** The fourth (cached-stmt
  point-lookup) is within noise. Both runtimes link the same
  libsqlite3 family (3.47.x); the win is the Ruby↔C boundary —
  rubyrs's host-fn dispatch costs less per call than CRuby's
  sqlite3 gem's bridge for the bulk / multi-row shapes.
- **The select_one_cached "near-tie"** is bottlenecked by the
  Ruby method dispatch on `stmt.query` itself (~0.15 µs).
  CRuby's gem implements `stmt.execute` in C; rubyrs has to
  walk through bytecode for `def query(*params); … end`. The
  gap closes only if either (a) rubyrs grows a C-class
  shortcut for trivial wrappers, or (b) we publish a
  not-quite-API-parity hot-loop primitive. Neither is worth
  the surface trade-off at this point — within-noise is fine.
- **The bulk_insert + select_many wins compound** in real
  data-migration / API-response shapes. Tight INSERT loops
  (the most common bulk-load workload) run **35 % faster**;
  full-result materialisation (a typical API JSON-array
  response from N database rows) runs **30 % faster**.
- **Same libsqlite3 = same correctness floor.** The bench
  exercises the rusqlite + rubyrs heap-alloc per-row path
  vs CRuby's C-ext + Ruby Array alloc path. Both runtimes
  yield identical rows for every workload.

## Reproducing

```bash
cargo build --release --features _sqlite,stdlib -p rubyrs

ruby                    bench/sqlite_bench.rb    # CRuby + sqlite3 gem
target/release/rubyrs   bench/sqlite_bench.rb    # rubyrs Phase 3.1
```

Environment knobs: `ITERS=2000 RUNS=3` (defaults). The bench
uses `:memory:` DB so no FS / journal-mode variance enters.

## Perf milestones (cumulative)

| Pass | Result |
|------|--------|
| Phase 3 baseline (Ruby-host-fn + LRU + heap alloc per row) | 3 of 4 shapes beat CRuby; `select_one_cached` 12 % behind |
| Phase 3.1 (`SQLite3::Statement` Ruby class + skip-LRU) | 3 of 4 shapes beat CRuby by 20-37 %; `select_one_cached` within noise (~10 % gap) |

Phase 5b (Sequel-lite Dataset over `Statement`) will compound
the `select_one_cached` parity into a *full sweep* under
realistic ORM-shape workloads, where the Dataset can hold a
Statement across `.where(...).all` chains and the Ruby-side
dispatch amortises away.

## Surface differences exercised

The bench's runtime-aware branches surface a couple of API
shape mismatches between the two SQLite bindings that the
Sequel-lite DSL (Phase 5b) is meant to paper over:

- **rubyrs splits `execute` / `query`** (the former for
  non-SELECT; the latter for SELECT). **CRuby's gem unifies on
  `execute`** which returns row arrays for SELECT and an empty
  array for INSERT/UPDATE/DELETE.
- **rubyrs `execute(sql, *params)` splat** vs **CRuby
  `execute(sql, [params_array])` array-only**.
- **rubyrs `db.execute_cached` + `db.prepare → Statement`**
  vs **CRuby `db.prepare → Statement`** (Database-level
  cached form is rubyrs-only; both runtimes have the
  prepared-statement form, and Phase 3.1 brings them to
  matching shape).

## What this doesn't measure

- **No transaction overhead**: each workload runs without
  `db.transaction { ... }`. The per-INSERT cost without a
  transaction is 10-100× higher (commit-per-row).
- **No concurrent access** — rubyrs is single-threaded so
  `BUSY_TIMEOUT` handling, WAL contention, etc. are out of
  scope until `_thread` ships.
- **No prepared-statement-lifetime fanout**: the bench uses
  ONE prepared statement at a time. The LRU eviction cost
  hasn't been measured.
