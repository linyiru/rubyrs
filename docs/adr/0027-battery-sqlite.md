# 0027: `_sqlite` battery — single-conn rusqlite wrapper + Sequel-lite DSL

## Status

Proposed (2026-06). **v1**. First battery ADR after ADR 0022
(`_http_server`); together they establish the template ADR 0019
rule 7 referenced. Driven by ADR 0026 v2 menu item 4 ("a data
layer on `_sqlite` — Sequel-lite / ROM-lite query DSL, not
ActiveRecord"). Discovery phase notes at `poc/sqlite/FINDINGS.md`.

Locks the 6 design questions Phase 1 Discovery surfaced
(connection model, transaction surface, prepared-statement
caching, type marshalling, exception hierarchy, rusqlite feature
selection) plus the 4 ancillary items (`Config::sqlite_allow_paths`
shape, PRAGMA escape, `:memory:` sandbox special-case, Cargo
feature aggregates). Phase 3 (battery PoC), Phase 4
(diff_framework S4 hooks), Phase 5 (Sequel-lite DSL), Phase 6
(parity fixture) commits land against this ADR.

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
rusqlite = { version = "0.32", features = ["bundled"], optional = true }
```

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
- `rusqlite::Connection` is `!Send`. Stored as a `HeapObj::TypedData`
  payload on the rubyrs heap. The VM's single-threaded constraint
  makes this safe; if `_thread` later relaxes it, the `Database`
  class re-becomes per-thread-bound (matching Bun's policy).

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
SAVEPOINT-nesting is a Sequel feature; deferred. Documented as a
class-`c` divergence (process-state — see "Deviations" below).

### 4. Prepared statement caching: per-connection LRU, cap 100

```rust
struct ConnState {
    conn: rusqlite::Connection,
    stmts: lru::LruCache<String, rusqlite::Statement<'static>>,
}
```

- Cache key = SQL string (exact match — caller is responsible for
  reusing the same string across iterations).
- Eviction = LRU at cap 100. Cap matches the CRuby `sqlite3`
  gem's default (`Database#prepared_statement_cache_size = 100`).
- Borrow-checker dance: `rusqlite::Statement<'conn>` borrows from
  the Connection. We store statements with their lifetime
  transmuted to `'static` paired with the runtime invariant
  "Statement is dropped strictly before Connection." Sealed by
  the ConnState struct that owns both — dropping ConnState drops
  the LRU first (dropping all statements), then the Connection.
  No external code can observe a Statement outliving its
  Connection.
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
mirror the CRuby `sqlite3` gem's `SQLite3::Errors` module so
`rescue SQLite3::ConstraintException` clauses port:

```
SQLite3::Exception
├── SQLite3::SQLException        (compile / syntax errors)
├── SQLite3::ConstraintException (UNIQUE / CHECK / FK / NOT NULL)
├── SQLite3::BusyException       (file lock contention — rare in single-conn)
├── SQLite3::CantOpenException   (filesystem reach failure)
├── SQLite3::ReadOnlyException   (write to RO db)
├── SQLite3::CorruptException    (DB file format violation)
└── SQLite3::TypeMismatchException (param/column type clash, our marshalling-table failures)
```

Mapping from `rusqlite::Error` enum variants is straightforward —
the library exposes `ErrorCode` for the ~30 SQLite-native codes,
which roll up into the 7 buckets above per CRuby's mapping.
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

`file::memory:` URI form (SQLite's URI-style in-memory) also
allowed unconditionally — same semantics.

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

| Class | Item | Detail |
|---|---|---|
| **a** (caller-supplied path) | All FS reach | User supplies `path` to `Database.new`. No `Database.new` form picks a path implicitly. |
| **c** (process state) | Transaction nesting depth | v1 supports only outer-only transactions (inner blocks execute in-context, don't get their own SAVEPOINT). Real CRuby `sqlite3` gem supports SAVEPOINT-nesting; deferred to Tier B. |
| **h** (pure-Ruby semantic-parity) | Bool round-trip | Ruby `true` round-trips as Integer `1`, not Bool `true` — SQLite has no native Bool type. Matches CRuby `sqlite3` gem behaviour byte-for-byte. |
| **h** | Float precision at SQLite limits | SQLite `Real` is IEEE-754 double, same as Ruby `Float` — but ADR 0019's deviation list class-`h` already covers Float-precision divergences from the underlying impl. |
| **g** (tokio threads) | Connection pool | Not modelled. Single-conn per Database. Future `_thread` battery + revision can lift. |

Nothing in classes b, d, e, f.

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
`limit` / etc.) is a separate Ruby-side artefact (Phase 5 commit)
NOT part of the battery's freeze surface. The DSL sits over the
battery's stable API and can evolve independently.

## What v1 ships

- `crates/rubyrs/src/sqlite.rs` — host-fn implementations, ~400 LOC
- `crates/rubyrs/src/preamble/sqlite_database.rb` — Ruby `SQLite3::Database`
  + exception hierarchy, ~150 LOC
- `crates/rubyrs/src/stdlib_vendor/sequel_lite.rb` — Sequel-lite DSL,
  ~250 LOC (Phase 5 commit, separate)
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
class wrapping the primitive. Considered + **rejected by ADR
0019 rule 692** because it doubles the per-battery cost (two
review surfaces, two test surfaces, two doc surfaces) for no
runtime benefit. ADR 0019's "Single-layer discipline" is the
explicit Decision; this ADR honours that.

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
| 4 | diff_framework S4 hooks (manifest schema + harness extension) + `sqlite_smoke` fixture | pending |
| 5 | Sequel-lite DSL — `src/stdlib_vendor/sequel_lite.rb` + register in `stdlib_vendor.rs` + `is_stdlib_stub_name` whitelist | pending |
| 6 | `sequel_canon` parity fixture | pending |
| 7 (optional) | JOIN + bench | deferred until consumer needs |

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
