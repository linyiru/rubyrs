# 0027: `_sqlite` battery — single-conn rusqlite wrapper + Sequel-lite DSL

## Status

Proposed (2026-06). **v2** — revised against a three-agent
parallel review of v1. v1 kept in git history. v2 changes:

- **§1**: rusqlite version pin bumped 0.32 → 0.39 (v1 was 7
  minor versions stale; 0.38–0.39 land ErrorCode and Statement
  lifetime helper changes that affect the marshalling and
  prepared-statement code).
- **§2**: `rusqlite::Connection` correctly described as
  **`Send + !Sync`**, not `!Send` (v1 was wrong on the
  load-bearing rationale). The single-conn decision still
  holds — enforced by `!Sync` (one-thread-at-a-time access),
  not `!Send`.
- **§3**: ships `busy_timeout = 5000ms` as the default
  (matches Bun :sqlite's documented value) — two `Database.new`
  against the same file in the same process is a real case
  that hits `SQLITE_BUSY` without it. Reviewer flagged v1's
  "deferred" stance as a shipped footgun.
- **§3**: Bun :sqlite comparison reframed honestly — Bun's
  `db.transaction(fn)` returns a wrapped function called later
  AND supports SAVEPOINT nesting out of the box; our shape
  matches the *outer-form* of Bun's API but is not API-identical.
- **§4**: `ConnState` struct field order fixed — `stmts` MUST
  appear before `conn` per Rust's declaration-order Drop rule.
  v1 sketch had them reversed → would have UB'd on shutdown
  via `sqlite3_finalize` against a freed Connection.
- **§4**: per-call cache footgun documented — `db.execute(sql, ...)`
  with a fresh interpolated SQL string never hits the LRU.
  v2 ships `db.execute_cached(sql, *params)` as the opt-in
  reuse shape and exposes `db.statement_cache_hits` /
  `_misses` counters.
- **§6**: exception hierarchy expanded to the **full 25-class
  CRuby `sqlite3` gem surface**, not the 7-class subset v1
  proposed. The seven I shipped were a truncation; "ship all 25
  empty subclasses" is the cheap-and-right call (parity is
  worth ~200 LOC of empty `class FooException <
  SQLite3::Exception; end`). One v1 name was also wrong:
  `TypeMismatchException` is the invented name — the real
  CRuby gem class is `MismatchException`.
- **§7**: `:memory:` URI documentation corrected. v1 said
  `file::memory:` is supported; the canonical URI forms are
  actually `file::memory:?cache=shared` and
  `file:name?mode=memory`. URI parsing also requires
  `SQLITE_USE_URI` (or `URI=ON` in the connection opts). v1's
  unconditional allow on `file::memory:` would have created a
  literal file named `:memory:` when URI mode was off.
- **§new (Heap result cap)**: `Config::sqlite_max_result_bytes`
  added. v1's "query returns full result array" stance has no
  guard against `SELECT * FROM big_table`; ADR 0019's class
  `f` (heap-cap) exists exactly so batteries don't bypass it.
- **§Deviations**: reclassified per ADR 0019's actual closed
  taxonomy. v1 misused class `c` (which is "Network reach
  beyond a single URL", not "process state") for
  transaction-nesting; reclassed to `h`. v1's "no connection
  pool" under class `g` ("Native-thread spawn") inverted the
  class — moved out of Deviations entirely into "what v1
  explicitly defers", where it belongs.
- **§Alternative A**: citation fixed. v1 cited "ADR 0019 rule
  692" (which doesn't exist) — should be Alternative 6 at
  ADR 0019 §"Alternatives considered," ~lines 689-695.
- **§Migration plan**: Phase 5 split into 5a (registry
  plumbing + empty `Sequel::Dataset` skeleton) + 5b (DSL body).
  v1's 5 bundled DSL design + register + whitelist into one
  commit — bisectability suffered if the DSL shipped a bug.

First battery ADR after ADR 0022 (`_http_server`); together
they establish the template ADR 0019 rule 7 referenced. Driven
by ADR 0026 v2 menu item 4 ("a data layer on `_sqlite` —
Sequel-lite / ROM-lite query DSL, not ActiveRecord"). Discovery
phase notes at `poc/sqlite/FINDINGS.md`.

Locks the 6 design questions Phase 1 Discovery surfaced
(connection model, transaction surface, prepared-statement
caching, type marshalling, exception hierarchy, rusqlite feature
selection) plus the 4 ancillary items (`Config::sqlite_allow_paths`
shape, PRAGMA escape, `:memory:` sandbox special-case, Cargo
feature aggregates), PLUS the v2 additions
(`Config::sqlite_max_result_bytes`, `busy_timeout` default,
`execute_cached` opt-in). Phase 3 (battery PoC), Phase 4
(diff_framework S4 hooks), Phase 5a (registry plumbing), Phase
5b (Sequel-lite DSL body), Phase 6 (parity fixture) commits land
against this ADR.

## Context

### Why a battery, not a pure-Ruby canon

Unlike menu items 2 (JSON) and 3 (ActiveSupport-lite), the SQLite
data layer has no pure-Ruby substitute. The closest analogue
would be a hand-rolled in-memory store with manual indexing —
which would diverge from real-SQLite semantics on every edge
case (collation order, NULL sorting, TEXT-vs-INTEGER type
affinity, etc.). ADR 0019 Rule 6 ("pure-Ruby canon") explicitly
exempts batteries whose semantics ARE the underlying native
implementation; SQLite belongs there.

### Why now

Menu items 0–3 ship without a database — Rack contract, Sinatra,
JSON, ActiveSupport-lite are all stateless. The first "real
Sinatra app" — the Bun-class demo ADR 0019 was sized around —
needs a place to persist data. That's menu item 4. Without it,
the omakase menu plateaus at "stateless API surface" — which is
where MRI Ruby's gems already shine; there's no "rubyrs lever"
to pull.

### Why ADR-first this time

For menu items 2 / 3 the spike → canon path was tight enough
to skip an ADR. SQLite has:

- A real Rust crate choice (rusqlite vs sqlx — locked here)
- A real `Config` field being shaped (`sqlite_allow_paths`)
- A real exception hierarchy committed to (`SQLite3::Exception`
  + ~7 subclasses, the surface user `rescue` clauses target)
- A real diff_framework harness extension (S4 stateful hooks,
  deferred since M27 D)
- A first-time-needed Cargo aggregate (`cli-defaults` / `everything`
  per ADR 0019)

Those decisions deserve a single `git blame` target rather than
five locations inside the battery PR diff.

## Decision

### 1. Vendor crate: `rusqlite` with `bundled` feature

```toml
rusqlite = { version = "0.39", features = ["bundled"], optional = true }
```

Version pin: **0.39** (latest stable as of 2026-06). v1 pinned
0.32; the 7-version gap covered a `Statement` lifetime helper
shift (0.33 introduced `Statement::raw_bind_parameter`'s
borrow-checker-friendlier form) and `ErrorCode` enum additions
(0.36+ added SQLITE-3.44 codes). Pin a specific minor (`= 0.39`
not `^0.39`) so the supply chain stays deterministic.

**Bundled** (ships libsqlite3 C source inside the crate, ~2 MB
compiled) rather than system-linked. Rationale:

- Matches the Bun-class promise: `cargo install rubyrs` on a
  minimal container (no `apt-get install libsqlite3-dev`)
  Just Works.
- ADR 0019's `cli-defaults ≤ 40 MB` budget already absorbs the
  ~4 MB SQLite footprint (line 371).
- System-linked is fragile across host SQLite version drift;
  bundled pins the SQLite version to the version rusqlite
  vendored, so cross-host parity is deterministic.

Not chosen:

- **`sqlx-sqlite`**: async-first API would force a tokio executor
  per query in rubyrs's single-threaded VM. ADR 0019 line 216
  identified this trade-off; locked rusqlite here.
- **`libsqlite3-sys` direct**: rusqlite IS this plus ergonomics.
- **WASM-compat SQLite (e.g. `sqlite-wasm`)**: `wasm32-wasip1`
  target gates Tier-3 batteries off (ADR 0017 row "wasm32-wasip1");
  out of scope.

### 2. Connection model: single connection per `SQLite3::Database`

```ruby
db = SQLite3::Database.new("app.db")   # opens connection
db.execute("INSERT INTO users ...")    # queues on the one conn
db.close                                # explicit close
```

Single `rusqlite::Connection` per `Database` instance. NOT a
pool. Rationale:

- rubyrs is single-threaded (Tier 1 doesn't model OS threads —
  `Thread` is a stub per ADR 0017). A pool only buys throughput
  under concurrent access; concurrent access requires threads.
  Defer to a future `_thread` battery + revision of this ADR.
- Bun's `bun:sqlite` is single-conn-per-`Database` per thread;
  same shape, same rationale.
- `rusqlite::Connection` is `Send` but `!Sync` (the C handle
  is movable across threads, but the internal `RefCell` blocks
  concurrent access). rusqlite enforces "only one thread at a
  time uses a Connection" via the `!Sync` bound. Stored as a
  `HeapObj::TypedData` payload on the rubyrs heap. The VM's
  single-threaded constraint makes this safe; if `_thread`
  later lands, `Database` instances can move between threads
  but stay non-shareable — matches what every other Ruby
  sqlite binding does (and what Bun's `bun:sqlite` does at the
  JS layer).

### 3. Transaction surface: block-form with auto-rollback on exception

```ruby
db.transaction do
  db.execute("INSERT INTO users ...")
  db.execute("INSERT INTO posts ...")
end
# normal exit  → COMMIT
# raised Ruby  → ROLLBACK; re-raise
```

Mirrors the CRuby `sqlite3` gem's `Database#transaction` shape
(itself mirrored by ActiveRecord, Sequel, ROM, etc.). Block-form
is the only supported shape in v1; explicit `BEGIN` / `COMMIT` /
`ROLLBACK` via `execute("BEGIN")` etc. work as raw SQL but bypass
the rollback hook (caller is on their own).

Nested `transaction { ... }` calls v1: outer-only — inner block
runs WITHOUT a SAVEPOINT (the SQL gets executed inside the outer
transaction; commit/rollback decisions stay with the outermost).
SAVEPOINT-nesting is a Sequel + Bun :sqlite feature; we defer it
to Tier B. Documented as a class-`h` divergence (semantic-parity
gap from `sequel`/`bun:sqlite` — see "Deviations" below).

**Bun :sqlite comparison nuance**: Bun's `db.transaction(fn)`
returns a wrapped function that the caller invokes later (i.e.
deferred execution). Our `db.transaction { … }` evaluates the
block immediately. Both shapes produce the same commit /
rollback semantics; the difference is when the SQL runs.
Reviewers flagged v1's "matches Bun byte-for-byte" framing as
overclaiming — we match the outer-form (block-shape + auto-
rollback on exception) but not the calling convention.

**Default `busy_timeout = 5000ms`**: set unconditionally at
`Database.new` time via `rusqlite::Connection::busy_timeout`.
Matches Bun's default. The "single-conn so contention is rare"
argument is true for one Database against the underlying file —
BUT two `Database.new("app.db")` in the same process (e.g.
test-harness setup code + the SUT) hit `SQLITE_BUSY` on
overlapping writes with no retry under default-zero timeout.
Shipping 5000ms eliminates the most common immediate footgun;
embedders who want explicit no-retry can call
`db.busy_timeout = 0` after construction.

### 4. Prepared statement caching: per-connection LRU, cap 100

```rust
struct ConnState {
    // Field order is LOAD-BEARING: Rust drops struct fields in
    // declaration order, so `stmts` (containing
    // Statement<'static> values whose true borrow is `conn`)
    // MUST come first. Reversing this order produces UB on
    // shutdown: `sqlite3_finalize` would run against a Connection
    // already freed by an earlier field-drop.
    stmts: lru::LruCache<String, rusqlite::Statement<'static>>,
    conn: rusqlite::Connection,
}
```

- Cache key = SQL string (exact match). Per-call cached SQL
  reuse goes through an OPT-IN method:

  ```ruby
  db.execute_cached(sql_const, *params)  # hits LRU
  db.execute(sql_string, *params)        # bypasses LRU
  ```

  Rationale: in real apps the SQL passed to `execute(...)` is
  often a freshly-interpolated string per call (`"INSERT INTO
  #{table} VALUES (?)"`); each lookup misses the cache AND
  evicts a real cached entry. Silently caching every `execute`
  call would turn the LRU into a thrashing footgun. The
  explicit `execute_cached` form makes intent unambiguous, and
  `db.statement_cache_hits` / `db.statement_cache_misses`
  counters let users diagnose when they think they're reusing
  but aren't.
- Eviction = LRU at cap 100. Cap matches the CRuby `sqlite3`
  gem's default (`Database#prepared_statement_cache_size = 100`).
  Tunable via `db.prepared_statement_cache_size = N` (matches
  CRuby gem's API).
- Borrow-checker dance: `rusqlite::Statement<'conn>` borrows from
  the Connection. We store statements with their lifetime
  transmuted to `'static` paired with the runtime invariant
  "Statement is dropped strictly before Connection." Sealed by
  the ConnState struct field order above — dropping ConnState
  drops `stmts` first (dropping all Statements), then `conn`.
  No external code can observe a Statement outliving its
  Connection.
- **Re-entrancy hazard**: if user code somehow triggers
  `execute_cached` recursion (e.g. a UDF callback running SQL
  on the same `db`) during an in-progress `prepare`, LRU
  eviction can fire mid-borrow — Stacked Borrows violation
  even though the C-level state is fine. Mitigated by a
  per-Database `prepare_active: bool` flag checked on entry
  to `execute_cached`; recursive entry traps with
  `SQLite3::MisuseException`. Pinned by a Miri test in the
  Phase 3 PoC.
- `unsafe transmute` justification documented at the source site
  per the project's panic-policy doc rules — same shape as
  `_http_server`'s tokio-runtime self-references.

### 5. Type marshalling table

Bi-directional between SQLite column types and rubyrs `Value`:

| SQLite ← Ruby (param bind) | Ruby → SQLite (column read) |
|---|---|
| `Value::Nil` → `Null` | `Null` → `Value::Nil` |
| `Value::Bool(b)` → `Integer(0/1)` | (SQLite has no Bool — comes back as Integer) |
| `Value::Int(n)` → `Integer(n)` | `Integer(n)` → `Value::Int(n)` |
| `Value::Float(f)` → `Real(f)` | `Real(f)` → `Value::Float(f)` |
| `Value::Str(s)` → `Text(s.to_string())` | `Text(s)` → `Value::Str(s)` |
| `Value::Sym(s)` → `Text(s.to_s)` | n/a (SQLite has no Sym) |
| `Value::Array` / `Value::Hash` | **TypeError** — composite types don't bind to columns |
| (everything else) | **TypeError** at bind time |

Symmetric except for the Bool round-trip (Ruby `true` round-trips
as `1`, not `true` — documented as a class-`h` divergence per
ADR 0019's deviation taxonomy; matches CRuby `sqlite3` gem
behaviour byte-for-byte).

BLOB columns read back as `Value::Str` constructed via
`new_str_bytes(Vec<u8>)` — preserves arbitrary bytes (binary
safety same as `_http_server`'s body-handling path).

### 6. Exception hierarchy

Top-level `SQLite3::Exception < StandardError`. Subclasses
ship the FULL CRuby `sqlite3` gem (`SQLite3::Errors`) surface —
25 named subclasses — so `rescue SQLite3::FullException` /
`SQLite3::ConstraintException` / `SQLite3::MismatchException`
clauses port byte-for-byte without porting code knowing which
subset rubyrs implements:

```
SQLite3::Exception
├── SQLite3::SQLException             (compile / syntax errors)
├── SQLite3::InternalException        (internal logic error in SQLite)
├── SQLite3::PermissionException      (FS perms denied opening DB)
├── SQLite3::AbortException           (callback abort)
├── SQLite3::BusyException            (file lock contention)
├── SQLite3::LockedException          (table locked)
├── SQLite3::MemoryException          (malloc failed)
├── SQLite3::ReadOnlyException        (write to RO db)
├── SQLite3::InterruptException       (operation cancelled)
├── SQLite3::IOException              (disk I/O error)
├── SQLite3::CorruptException         (DB file format violation)
├── SQLite3::NotFoundException        (table or record not found)
├── SQLite3::FullException            (disk full)
├── SQLite3::CantOpenException        (filesystem reach failure)
├── SQLite3::ProtocolException        (db protocol error — rare)
├── SQLite3::EmptyException           (no data — historical)
├── SQLite3::SchemaChangedException   (schema mutated mid-query)
├── SQLite3::TooBigException          (string/BLOB exceeds limit)
├── SQLite3::ConstraintException      (UNIQUE / CHECK / FK / NOT NULL)
├── SQLite3::MismatchException        (datatype mismatch — our marshalling failures)
├── SQLite3::MisuseException          (library mis-use, incl. re-entrancy)
├── SQLite3::UnsupportedException     (feature unavailable in this build)
├── SQLite3::AuthorizationException   (authorizer rejected statement)
├── SQLite3::FormatException          (auxiliary database format error)
├── SQLite3::RangeException           (bind parameter out of range)
└── SQLite3::NotADatabaseException    (file isn't an SQLite db)
```

Mapping from `rusqlite::Error` / `ErrorCode` enum variants is
mechanical — libsqlite3's ~25 native primary error codes
correspond 1-to-1 with the class names above (rusqlite exposes
them as `ErrorCode` variants since 0.36). The 25 classes are
empty subclasses (`class FooException < SQLite3::Exception;
end`) — ~3 LOC each, ~80 LOC total. Cheap.

Why ship all 25 instead of the high-traffic subset (Constraint,
Busy, CantOpen, ReadOnly, Mismatch, Corrupt, SQLException):
users `rescue` clauses written against the real gem reference
classes like `FullException` (disk full — relevant on tight
containers), `LockedException`, `IOException`, `MisuseException`.
Shipping the subset means those rescues silently fail (the
exception falls through to a generic `Exception` clause if any,
or unwinds to top-level). Truncating the exception surface
would be a class-`h` divergence we could document, but at ~80
LOC the parity ratio is unbeatable. Reviewer (v1 review)
flagged the 7-class subset as a hidden defect surface.

Error message: SQLite's native string (rusqlite forwards verbatim
from libsqlite3 — matches the CRuby gem's messages).

### 7. `Config::sqlite_allow_paths` — sandbox gate

```rust
pub struct Config {
    // ... existing fields ...
    pub sqlite_allow_paths: Option<Vec<PathBuf>>,
}
```

When `Some(prefixes)`, `Database.new(path)` only succeeds if
`path` is `:memory:` OR lexically-resolves under one of the
prefixes. Out-of-scope opens raise `SQLite3::CantOpenException`
with a "sandbox blocked" message.

When `None`, no sandbox — any path allowed (or rejected by
`Config::allow_filesystem_io = false`, which is checked first
and OVERRIDES `sqlite_allow_paths`).

`:memory:` SPECIAL CASE: the literal string `":memory:"` is a
SQLite-internal handle for an in-memory DB. It doesn't touch the
FS, so it's allowed unconditionally — same shape as
`_http_server`'s unconditional `127.0.0.1` bind for the loopback
adapter. Documented at the field's source comment.

**URI in-memory forms** (`file::memory:?cache=shared`,
`file:foo?mode=memory`, `file:foo?mode=memory&cache=shared`):
allowed unconditionally only when URI parsing is enabled.
SQLite's URI-filename parser requires `SQLITE_OPEN_URI` (or
`sqlite3_config(SQLITE_CONFIG_URI, 1)`); rusqlite's
`Connection::open_with_flags(...)` lets us set that per-open.
The battery's open path enables URI mode for any path matching
`/^file:/` AND falls back to literal-filename mode for everything
else — which means bare `:memory:` Just Works, named-URI in-mem
DBs Just Work, AND a stray `file:` prefix on a path doesn't
silently turn into a literal-file-named-`file:foo`. Reviewer
flagged v1's text as misleading on this point: `file::memory:`
without URI mode would have created a file literally named
`:memory:` in the cwd.

### 7b. `Config::sqlite_max_result_bytes` — heap-cap on query results

```rust
pub struct Config {
    // ...
    pub sqlite_max_result_bytes: Option<usize>,
}
```

`db.query(sql, *params)` returns the entire result set as
`Array<Hash>` — no streaming-iterator shape until Fiber
integration lands (deferred to Tier B). Without a cap, a
runaway `SELECT * FROM big_table` materialises unbounded heap.
ADR 0019's class `f` (heap-cap deviations) exists exactly so
batteries don't bypass `Config::max_value_bytes`; the SQLite
battery needs its own cap because individual cells fit under
`max_value_bytes` while the assembled `Array<Hash>` blows past.

When set, the battery accumulates row bytes during `query`
materialisation and traps with `SQLite3::TooBigException` when
the running total exceeds the cap. The trap fires BEFORE the
oversized Hash is allocated, so no partial-allocation cleanup
needed. Default `None` (unbounded) matches the CRuby `sqlite3`
gem's default; embedders running untrusted scripts set
`Some(16 * 1024 * 1024)` (16 MB) or whatever fits their RSS
budget.

Sequel-lite DSL's `Dataset#each` Phase 5b iterator-shaped form
side-steps this cap — true streaming, only one row materialised
at a time. Pure `Dataset#all` goes through `query` and pays
the cap.

### 8. `cli-defaults` and `everything` feature aggregates

```toml
[features]
cli-defaults = ["stdlib", "_sqlite", "_http_server"]
everything   = ["cli-defaults", "_json_native", "_fiber"]
```

- `cli-defaults` matches ADR 0019 line 313's contract: fresh
  `cargo install rubyrs` user gets the Bun-class
  `require "rubyrs/sqlite"` demo working.
- `everything` is the kitchen-sink for embedders who want all
  available batteries pre-wired.
- Both are aliases — no runtime cost. Add them in the
  feature-flag commit alongside `_sqlite`. ADR 0019 line 504
  was the original commitment; this discharges it.

### 9. PRAGMA support

`db.execute("PRAGMA foreign_keys = ON")` works as raw SQL via
the standard `execute` path — PRAGMAs are SQL statements that
return optional rows. Battery exposes no dedicated `pragma`
method in v1; users use `execute` / `query`. The Sequel-lite
DSL may add `db.pragma(:foreign_keys, true)` sugar later (Tier
B), but that's a DSL decision, not a battery decision.

### 10. Default journal mode: inherit

No `PRAGMA journal_mode = ...` set by the battery. SQLite's
default for new file-backed DBs is rollback journal; WAL mode
requires explicit opt-in via the user's first PRAGMA. Documented
in the SQLite3::Database doc comment.

## Capability host-fns consumed

Per ADR 0019 rule 7's checklist. The battery consumes ONE host
capability:

- **Filesystem reach**, gated by `Config::allow_filesystem_io`
  AND `Config::sqlite_allow_paths`. Both checks happen at
  `Database.new` time; subsequent `execute` calls don't re-check
  since SQLite operates on the already-opened file descriptor.

The battery exposes FOUR host-fns to scripts (registered via
`Runtime::register_fn` when `_sqlite` is built in):

```
__rubyrs_sqlite_open(path: String, opts: Hash) → handle (Integer)
__rubyrs_sqlite_close(handle: Integer) → nil
__rubyrs_sqlite_execute(handle: Integer, sql: String, params: Array) → Integer (rows changed)
__rubyrs_sqlite_query(handle: Integer, sql: String, params: Array) → Array<Array<Value>>
```

`handle` is an opaque integer index into a per-Vm
`HashMap<i64, ConnState>`. Allocations + deallocations happen
inside the host-fn boundary; the heap holds a thin Integer
wrapper. This is the "owned-resource I/O" shape ADR 0019 rule 4a
documents — the user supplies a path, the battery opens it, the
battery closes it on `Database#close` or GC.

The Ruby-side `SQLite3::Database` class wraps these into the
user-facing API (constructor, `transaction`, `execute`, `query`,
etc.). Sits in `preamble/sqlite_database.rb` for the Ruby surface
parallel to how `_http_server` puts user-facing constants in a
preamble shim.

## Deviations (per ADR 0019 Rule 4 taxonomy)

Carefully scoped to the 8-class closed taxonomy ADR 0019 Rule 4
defines. v1 misused classes `c` and `g` — those entries are
moved out of this table per v2 review:
- Transaction-nesting (v1 had as class `c`): NOT actually
  "Network reach beyond a single URL" (the real class `c`
  meaning); it's a semantic-parity gap from Sequel/Bun's
  SAVEPOINT-nesting. Reclassed to class `h` below.
- Connection pool (v1 had as class `g`): "Native-thread spawn"
  is what `g` means, and we DON'T spawn threads, so claiming
  `g` inverts the class. The right home is "what v1 defers"
  further down — it's a missing feature, not a deviation
  from a documented behaviour.

| Class | Item | Detail |
|---|---|---|
| **a** (caller-supplied path) | All FS reach | User supplies `path` to `Database.new`. No `Database.new` form picks a path implicitly. |
| **h** (pure-Ruby semantic-parity) | Bool round-trip | Ruby `true` round-trips as Integer `1`, not Bool `true` — SQLite has no native Bool type. Matches CRuby `sqlite3` gem behaviour byte-for-byte. |
| **h** | Transaction nesting depth | v1 supports only outer-only transactions (inner blocks execute in-context, don't get their own SAVEPOINT). Sequel + Bun :sqlite both support SAVEPOINT-nesting. Deferred to Tier B. |
| **h** | Float precision at SQLite limits | SQLite `Real` is IEEE-754 double, same as Ruby `Float`. ADR 0019's class-`h` already covers Float-precision divergences from the underlying impl. |
| **f** (heap-cap) | `query` materialises full result set into Array<Hash> | `Config::sqlite_max_result_bytes` is the gate (see §7b). When set, an oversized result raises `SQLite3::TooBigException` before allocation. Default `None` matches the CRuby gem. |

Nothing in classes b, c, d, e, g.

## Surface freeze policy

Per ADR 0019 rule 7. The Ruby-side API surfaces:

- `SQLite3::Database.new(path, opts = {})`
- `SQLite3::Database#execute(sql, *params)` — returns rows-changed
- `SQLite3::Database#query(sql, *params)` — returns `Array<Hash>`
- `SQLite3::Database#transaction { ... }` — block-form
- `SQLite3::Database#close`
- `SQLite3::Database#closed?`
- `SQLite3::Exception` + 7 subclasses listed above

Status: **unstable** until the battery has shipped in one tagged
release with no API change requests. Promotes to **stable**
(semver-tracked) thereafter; removing a stable method requires a
new ADR per ADR 0019 rule 7.

The Sequel-lite DSL (`Dataset`, chainable `where` / `order` /
`limit` / etc.) is a separate Ruby-side artefact (Phase 5a + 5b commits)
NOT part of the battery's freeze surface. The DSL sits over the
battery's stable API and can evolve independently.

## What v1 ships

- `crates/rubyrs/src/sqlite.rs` — host-fn implementations, ~400 LOC
- `crates/rubyrs/src/preamble/sqlite_database.rb` — Ruby `SQLite3::Database`
  + exception hierarchy, ~150 LOC
- `crates/rubyrs/src/stdlib_vendor/sequel_lite.rb` — Sequel-lite DSL,
  ~250 LOC (Phase 5b commit, separate)
- `Cargo.toml` — `rusqlite` dep, `_sqlite` + `cli-defaults` +
  `everything` features
- `crates/rubyrs/tests/diff_framework/` — S4 lifecycle-hook
  manifest schema extension + `sqlite_smoke` and `sequel_canon`
  fixtures (Phase 4 + 6 commits)
- `lib.rs` — `register_sqlite_host_fns` public export, paralleling
  `register_http_server_host_fns`

Targeted ratio: ~600 LOC Rust + ~400 LOC Ruby. Comparable to
JSON canon + native combined; smaller than `_http_server` v1's
~2 KLOC because no protocol parsing, no async runtime.

## What v1 explicitly defers

- **Migrations** (`Sequel.migration { up { ... } }`) — schema-
  walker DSL with its own up/down semantics. Deferred to a
  follow-up commit IF a fixture needs it; out of menu-item-4
  v1 scope per ADR 0026 v2's "NOT ActiveRecord" cap.
- **Models** (`class User < Sequel::Model`) — association
  graph, validations, callbacks. Substantial scope; deferred.
- **Plugins** — Sequel's plugin ecosystem (pagination,
  json_serializer, etc.). The DSL exposes hook points but ships
  no plugins. Out of scope.
- **Connection pool** (multi-conn) — needs `_thread` battery
  first.
- **WAL mode auto-enable** — user opts in via PRAGMA.
- **Backup API** (`Database#backup`) — useful but rare; deferred.
- **`Database#busy_timeout`** — relevant for multi-conn (which
  we don't support v1); deferred.
- **Streaming row iteration** (`db.query("SELECT ...").each`) —
  v1 returns the full result array. Streaming-iterator shape
  needs Fiber integration (`_fiber` battery); deferred.

## Open questions resolved in Phase 3+ commits

The 6 residual risks `FINDINGS.md` listed get concrete answers
inside the Phase 3 PoC commit:

1. **`Send`-ness of `rusqlite::Connection`** — single-threaded
   VM, no cross-thread access path. Documented at the source
   site. (Resolved by ADR's section 2 above.)
2. **Statement-vs-Connection borrow dance** — `unsafe transmute`
   to `'static` + invariant-via-struct-ownership. (Resolved by
   ADR's section 4 above.)
3. **`:memory:` sandbox interaction** — special-case allow.
   (Resolved by ADR's section 7 above.)
4. **PRAGMA support** — works via raw `execute`. (Resolved by
   ADR's section 9 above.)
5. **JSON-in-SQLite cross-feature** — battery stays strictly
   SQL; users compose via raw `execute("SELECT json_extract(...)")`.
   `_json_native` and `_sqlite` are orthogonal. Documented here.
6. **WAL mode default** — inherit (no explicit set). (Resolved
   by ADR's section 10 above.)

## Consequences

### Positive

- Menu item 4 unblocks: "Sinatra + DB" Bun-class demo can ship.
- `cli-defaults` + `everything` aggregates finally exist —
  discharges ADR 0019's `cargo install rubyrs` + Bun-class
  promise.
- Single-layer architecture (no `Native.*` Rust primitives + Ruby
  wrapper split) keeps the diff_framework parity gate tight: the
  same `SQLite3::Database` Ruby class is what users `require`
  AND what the parity fixture exercises.
- `_sqlite` becomes the second concrete instance of ADR 0019 rule
  7's per-battery ADR pattern (`_http_server` was the first),
  validating that the template scales beyond ADR 0022's specific
  shape.

### Negative

- `cli-defaults` build absorbs ~4 MB from rusqlite + bundled
  libsqlite3. Already accounted for in ADR 0019's `≤ 40 MB`
  budget but worth surfacing — default `cargo install rubyrs`
  jumps from ~25 MB (current pure-canon-only) to ~38 MB.
- `unsafe transmute` for the prepared-statement cache adds one
  more `unsafe` block to the codebase. Same shape as
  `_http_server`'s tokio self-references; reviewable.
- Single-conn model means future scale-out (a real web server
  handling concurrent requests against the same DB) needs
  `_thread` first. Bun's model has the same constraint.
- Sequel-lite DSL is in `stdlib_vendor/sequel_lite.rb` —
  evolving its API independent from the battery surface needs
  discipline (the battery's "stable" promise doesn't extend to
  the DSL layer).

## Alternatives considered

### A. Two-layer split (Rust primitives + Ruby `SQLite3::Database`)

`Native.sqlite_open(path) → handle` + pure-Ruby `SQLite3::Database`
class wrapping the primitive. Considered + rejected by **ADR
0019 §"Alternatives considered" Alternative 6** (~lines 689-695):
"Pure-Ruby native shim layer in Tier 2. […] Two-layer
discipline; doubles the per-battery cost. This is exactly what
CRuby does (C ext + Ruby wrapper) and what makes its stdlib
boundary perpetually fuzzy. Rejected." ADR 0019's "Single-layer
discipline" is the explicit Decision; this ADR honours that.
(v1 cited "ADR 0019 rule 692" — a fictional rule number; v2
fixes the citation.)

### B. `sqlx-sqlite` instead of rusqlite

Async-first API. Rejected because:
- Forces a tokio executor per query. rubyrs is single-threaded
  outside `_http_server`'s tokio runtime; spinning up a per-
  query executor is wasteful.
- ADR 0019 line 216 already noted the trade-off; this ADR
  confirms the rusqlite path.

### C. Multi-conn / connection pool

Rejected v1 because:
- rubyrs has no threads (Tier 1 stub per ADR 0017).
- Single-conn matches Bun's `bun:sqlite` model.
- `_thread` battery (Tier 2, deferred) is the prerequisite;
  this ADR can be revised when that lands.

### D. Bundled vs system-linked SQLite

Already addressed in §1. Bundled wins for the Bun-class
"works on a fresh container" demo.

### E. SAVEPOINT-nested transactions in v1

Real Sequel supports this. Rejected v1 because:
- Adds non-trivial Ruby-side state machinery (savepoint depth
  counter, name generation, etc.).
- Documented as a class-`c` deviation; users that need
  nesting can `execute("SAVEPOINT ...")` manually or wait
  for the Tier-B follow-up.

## Migration plan

No migration needed — `_sqlite` is a new feature. Path forward:

| Phase | Commit | Status |
|---|---|---|
| 1 | `poc/sqlite/FINDINGS.md` | shipped at `e54fac79` |
| 2 | This ADR (`docs/adr/0027-battery-sqlite.md`) | **this commit** |
| 3 | Battery PoC — `src/sqlite.rs` + `preamble/sqlite_database.rb` + `Cargo.toml` deps + `lib.rs` export | pending |
| 4 | diff_framework S4 hooks (manifest schema + harness extension) + `sqlite_smoke` fixture | **shipped (2026-06)** — `tests/diff_framework/fixtures/sqlite_smoke/{app.rb, compat.rb, manifest.json}` + `#[cfg(feature = "_sqlite")] fn sqlite_smoke()` runner registration + framework-parity CI job gains `_sqlite` to its feature set and `sqlite3` to its gem install. Covers open / execute / query / prepare → Statement / block-form transaction COMMIT + ROLLBACK / ConstraintException catch / clean shutdown |
| 5a | Sequel-lite registry plumbing — empty `sequel_lite.rb` stub + `stdlib_vendor.rs` register + `is_stdlib_stub_name` whitelist + the test-fixture-shape proof that `require "sequel"` resolves to the stub | **deferred — see note below** |
| 5b | Sequel-lite DSL body — `Dataset` chainable `where` / `order` / `limit` / `all` / `each` / `insert` / `update` / `delete` (Tier A, ~250 LOC) | **deferred — see note below** |
| 6 | `sequel_canon` parity fixture | **deferred** (no Dataset to canonise) |
| 7 (optional) | JOIN + bench | deferred until consumer needs |

### 2026-06-02 — Phases 5–6 deferred at Phase 3.1 close

Menu item 4 is closing at Phase 3.1 (`SQLite3::Statement` Ruby
class + bench). Rationale:

- The Phase 3.1 SQLite bench has rubyrs ahead of CRuby by 20–37 %
  on three of four workloads and within ~10 % noise on the
  fourth (`select_one_cached`). The bench's `bench/sqlite_bench_results.md`
  ships with the data. The *raw* `_sqlite` battery is already
  competitive — Sequel-lite was originally framed as the lever
  to flip `select_one_cached` into a full sweep by amortising
  Ruby-side dispatch over `Dataset` chains.
- Sequel-lite is a **subset DSL**, not a Sequel mirror. The
  small surface (hash-form `where` / scalar ops / order / limit
  / all+first+each) implies a tiny SQL compiler (~300–500 LOC),
  but the **test + documentation surface** for "what works,
  what doesn't, why" is large enough that we don't want to ship
  it without a real consumer driving the shape. Building it
  speculatively risks landing a mid-state "looks like Sequel
  but isn't" that's harder to revise once published.
- The bench gap is already documented as **acceptable** in
  `bench/sqlite_bench_results.md` (the within-noise note). No
  end user is currently asking for the Dataset DSL.

Re-open trigger: a concrete consumer (an example app, a
benchmark target, or a Tier-3 stdlib that wants ORM-shape
syntax) lands a request for `Dataset` chainable shape. At that
point Phase 5a → 5b → 6 resume in order; the deferral is a
postpone, not a delete. The ADR design (single-conn,
per-thread `SQLITE_CONNS`, prepared-statement cache, exception
hierarchy, etc.) carries forward unchanged — only the DSL
veneer on top is parked.

Menu item 4's "battery designed, not built" line in ADR 0026 v2
becomes "battery built (`SQLite3::Database` + `SQLite3::Statement`);
Dataset DSL designed, deferred". The cumulative `_sqlite` surface
through Phase 3.1 is what the menu item promises in practice.

Each phase = one atomic commit. Total ~5 commits + 1 ADR
before the parity fixture lights up. ADR 0026 v2's "battery
designed, not built" line on menu item 4 closes once Phase 6
lands.

## Related

- ADR 0017 — Tier 1 boundary (Tier 3 batteries placement)
- ADR 0019 v3 — Tier 2/3 boundary; rules 4 (deviation
  taxonomy), 7 (per-battery ADR template), 8 (`rubyrs/` namespace)
- ADR 0022 — `_http_server` battery (first ADR-per-battery
  instance; this ADR's template source)
- ADR 0026 v2 — omakase blessed-gem menu, row 4 "data layer on
  `_sqlite`"
- `poc/sqlite/FINDINGS.md` — Phase 1 Discovery output that
  drove this ADR
- ADR 0013 — `CURRENT_VM_PTR` raw-pointer escape (the cext
  bridge mechanism the battery's host-fn allocations use, same
  shape `json_native` and `_http_server` use)
- ADR 0024 — bytecode iter + block break (the Fiber-integration
  layer streaming-row iteration would build on; deferred)
