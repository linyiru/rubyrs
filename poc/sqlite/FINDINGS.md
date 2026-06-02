# menu item 4 (SQLite + Sequel-lite) — Phase 1 Discovery

Date: 2026-06. No code shipped this phase. Discovery only —
ADR scan + dep survey + open-question inventory. Drives the
scope of Phase 2 (likely `docs/adr/0027-battery-sqlite.md`)
and Phase 3+ implementation commits.

## What's already decided (in existing ADRs)

Surprisingly substantial. Half of the menu-item-4 design space
is already locked by ADR 0019 (Tier 2/3 boundary) — the
spike's job is to surface the remaining unknowns, not redo
the architecture.

### Locked by ADR 0019

| Decision | Source | Status |
|---|---|---|
| Vendor crate: `rusqlite` (not `sqlx-sqlite`) | ADR 0019 line 446 | Locked — Cargo dep not yet added; first PR adds it |
| Load path: `require "rubyrs/sqlite"` (not bare `sqlite3`) | ADR 0019 rule 8 + Bun-prefix precedent | Locked |
| Deviation class: `a` (owned-resource I/O — caller-supplied path) | ADR 0019 rule 4 line 158 | Locked |
| FS sandbox gate: `Config::sqlite_allow_paths` | ADR 0019 line 158 | Locked — `Config` field doesn't exist yet but the contract is named |
| Single-layer (NO `Native.sqlite_open` primitive + `SQLite3::Database` Ruby wrapper split) | ADR 0019 line 692 ("Rejected") | Locked — battery owns the surface |
| Each Tier 3 battery gets its own ADR | ADR 0019 rule 7 | Required — menu item 4 implies ADR 0027 |
| ADR-per-battery enforced by `scripts/check-battery-adrs.sh` | ADR 0019 line 230 | Required — adding `_sqlite` without ADR fails CI |
| Bundled in `cli-defaults` feature aggregate | ADR 0019 line 313 + 504 | Locked — but `cli-defaults` doesn't exist in `Cargo.toml` yet |

### Locked by ADR 0026 v2

| Decision | Source | Status |
|---|---|---|
| DSL shape: Sequel-lite / ROM-lite query DSL, **NOT ActiveRecord** | ADR 0026 v2 row 4 | Locked — scope cap |
| "Sinatra + a DB" is the first real app shape | Ditto | Drives the parity-fixture target |
| Stateful-lifecycle hook in diff_framework is a prerequisite | ADR 0026 v2 line 168 | Required — that's the M27 D S4 still-deferred work |

## What's still open

The Phase 2 ADR will need to lock down:

### A. Battery surface (Rust side)

1. **Connection model.** Single-conn per `Database#new`? Pool?
   `rusqlite::Connection` is not Send — if pooling, needs
   `Send + Sync` wrapper or single-thread enforcement.
   Bun's `bun:sqlite` is single-conn-per-`Database` per
   thread; that's the simplest precedent.

2. **Transaction semantics.** Explicit `BEGIN` / `COMMIT` /
   `ROLLBACK` is `Connection::execute(...)`. Nested
   transactions (SAVEPOINT)? Auto-rollback on Ruby
   exception inside the `transaction { ... }` block?
   The CRuby `sqlite3` gem's `Database#transaction` accepts
   a block + auto-commits / auto-rollbacks; that's the
   target shape.

3. **Prepared statement caching.** Per-connection
   `HashMap<sql_str, PreparedStatement>` so the same SQL
   string reused inside a loop doesn't re-parse every time.
   Cap on cache size? CRuby's gem caches up to 100 by
   default. ~5-line LRU is sufficient.

4. **Type marshalling.** SQLite has 5 column types (`NULL`,
   `INTEGER`, `REAL`, `TEXT`, `BLOB`). Mapping to Ruby is
   obvious for the first four; `BLOB` needs the binary-safe
   `Value::Str::from_bytes` shape (which we have post-cext
   work). Reverse direction: Ruby Integer / Float / String /
   Symbol → SQLite param. Symbol as TEXT or error? CRuby
   gem errors; we should too.

5. **Error mapping.** rusqlite errors fall into ~7
   categories (`SqliteFailure`, `InvalidParameterName`,
   `QueryReturnedNoRows`, ...). CRuby's gem maps these
   to a class hierarchy under `SQLite3::Exception`. We
   match that for `rescue SQLite3::ConstraintException`
   portability.

6. **Threading.** rubyrs is single-threaded (Tier 1 doesn't
   model OS threads; Thread is a stub). Battery doesn't
   need any threading machinery. Future Tier 2 `_thread`
   would intersect — but that's a separate ADR.

### B. Sequel-lite DSL (pure-Ruby side)

The ADR's "Sequel-lite" framing is intentionally vague.
Real Sequel is enormous (associations, validations, plugins,
migrations). For menu item 4 we need the subset Sinatra+DB
apps actually reach for:

| Operation | Sequel surface | Worth including? |
|---|---|---|
| `db[:users].all` | `Dataset#all` returns `Array<Hash>` | Yes (Tier A) |
| `db[:users].where(name: "x")` | `Dataset#where` returns chainable Dataset | Yes (Tier A) |
| `db[:users].where{ id > 5 }` | virtual-row block form | DEFERRED — needs `Sequel::SQL::VirtualRow` machinery |
| `db[:users].order(:name)` / `.limit(10)` | Yes (Tier A) | Yes |
| `db[:users].insert(name: "x")` | `Dataset#insert` | Yes (Tier A) |
| `db[:users].update(name: "x")` | bulk update | Yes (Tier A) |
| `db[:users].delete` | bulk delete on filtered dataset | Yes (Tier A) |
| `db[:users].join(:posts, user_id: :id)` | basic INNER JOIN | Yes (Tier B) |
| Associations (`one_to_many`) | Model layer | DEFERRED — needs Model class |
| Migrations | `Sequel.migration { up { ... }; down { ... } }` | DEFERRED — needs schema-walker DSL |
| Plugins | The whole gem ecosystem | OUT OF SCOPE |

Sketched scope: ~Tier A queries (`where` / `order` / `limit`
/ `insert` / `update` / `delete`) + ~Tier B basic `join`. No
model layer, no migrations, no associations. Roughly the
shape `bun:sqlite`'s `Database#query` + `Statement#all` give
JavaScript users, plus chainable filtering.

### C. diff_framework S4 — stateful lifecycle hooks

Currently `manifest.json` declares `script` / `server` and
the harness runs them once. SQLite fixtures need:

1. **`setup_sql` array** — SQL statements to run BEFORE
   the scenarios (schema seed). Could also be a path to
   a `.sql` file.
2. **`teardown_sql`** — symmetric tear-down (rare; `:memory:`
   DBs vanish automatically, but file-backed needs it).
3. **`db_path` placeholder substitution** — `manifest.json`
   declares `db_path: "test.db"`; harness injects an
   absolute path via env var, identical on both runtimes.
4. **Post-scenario dump diff** — after the script runs,
   dump the final DB state (`SELECT * FROM each_table ORDER BY ...`)
   and byte-diff that across runtimes. The
   harness already byte-diffs stdout; dump-diff is the
   stateful equivalent.

Implementation surface: ~80 LOC in `tests/diff_framework.rs`
+ ~30 LOC of manifest schema. Independent of the battery
itself — could land BEFORE the battery if there's a simpler
fixture to pilot it on (in-memory `:memory:` DB exercised
by a hand-rolled `rusqlite` test spike).

### D. Cargo feature aggregates

ADR 0019 talks about `cli-defaults` and `everything` features
but neither is in `Cargo.toml` today. Adding `_sqlite` is a
natural moment to also add:

```toml
cli-defaults = ["stdlib", "_sqlite", "_http"]
everything = ["cli-defaults", "_json_native", "_fiber", "_http_server"]
```

Roughly. The `cli-defaults` aggregate is in ADR 0019; the
`everything` shape mirrors it on the kitchen-sink end. Both
are aliases — no runtime cost. Adding them in the menu-
item-4 commit is cheap; deferring leaves ADR 0019 partly
unimplemented.

## Dependency survey

### `rusqlite` (chosen by ADR 0019)

- **Version**: latest stable as of 2026-06. Pinning to a
  specific minor in `Cargo.toml`.
- **Bundled SQLite**: feature flag `bundled` ships
  libsqlite3 inside the crate (~2 MB compiled). Without
  `bundled`, dynamic-link against system libsqlite3 (Mac
  ships it; Linux usually too; portable Docker images may
  not).
- **Decision needed in ADR 0027**: bundled or
  system-linked. Bundled is simpler for the Bun-class
  `cargo install rubyrs` promise (works out of the box on
  any host); ~2 MB tax. System-linked saves space but
  fails on minimal containers without `apt-get install
  libsqlite3-dev`. **Lean: bundled.** Matches ADR 0019's
  "≤ 40 MB cli-defaults" budget; the ADR's calculation
  already assumed bundled.
- **Workspace fit**: no other Cargo dep links libsqlite3;
  no clash. Adds `rusqlite` + `libsqlite3-sys` to the
  workspace deps tree.

### Alternatives considered (and why rejected)

- `sqlx-sqlite`: async-first API. rubyrs is single-threaded,
  doesn't run tokio outside `_http_server`; `sqlx`'s
  async API would force us to spin up an executor per
  query. Unjustified cost.
- `libsqlite3-sys` direct binding: rusqlite IS this plus
  ergonomics. No reason to re-write it.
- WASM-compatible alternatives (e.g. `wasmer-sqlite`):
  rubyrs's WASM target (`wasm32-wasip1`) doesn't bundle
  the `_sqlite` battery — see ADR 0017 row "wasm32-wasip1"
  where Tier-3 batteries are gated off. Out of scope.

## Phase plan

Concrete commit ladder, derived from the above:

### Phase 2 — ADR 0027 design

One commit (~one ADR file, ~150-200 lines following the
ADR 0022 template). Locks:

- Vendor crate: rusqlite, bundled features
- Connection model: single-conn per `Database`
- Transaction surface: `Database#transaction { ... }` block
  with auto-rollback on exception
- Type marshalling table (5 SQLite types × Ruby Value)
- Exception hierarchy (`SQLite3::Exception` → ~7
  subclasses)
- Sequel-lite DSL Tier A scope (~10 methods listed above)
- `Config::sqlite_allow_paths` field shape
- `cli-defaults` / `everything` feature aggregates added
- diff_framework S4 manifest schema additions

### Phase 3 — battery PoC

One commit (~400 LOC Rust + ~50 LOC Ruby surface). Lands
`crates/rubyrs/src/sqlite.rs` with:

- `__rubyrs_sqlite_open(path) → handle`
- `__rubyrs_sqlite_exec(handle, sql) → nil` (no result; for
  DDL / single-shot)
- `__rubyrs_sqlite_query(handle, sql, params) → Array<Hash>`
- `__rubyrs_sqlite_close(handle) → nil`

Plus a tiny Ruby wrapper: `class SQLite3::Database; def
initialize(path); @h = __rubyrs_sqlite_open(path); end; ...`.
No DSL yet. Schema seed + raw query.

### Phase 4 — diff_framework S4 hooks

One commit (~100 LOC of harness + a `sqlite_smoke`
fixture). New manifest schema:

```json
{
  "script": { ... },
  "setup_sql": [ "CREATE TABLE ...", "INSERT INTO ..." ],
  "db_dump_tables": ["users", "posts"]
}
```

Harness injects `HARNESS_DB_PATH` env, runs setup_sql via
the battery's exec, runs the script, then dumps each table
via `SELECT * FROM <name> ORDER BY rowid` and includes
that in the byte-diff transcript.

### Phase 5 — Sequel-lite DSL

One commit (~250 LOC pure Ruby). `src/stdlib_vendor/sequel_lite.rb`
with the Tier A surface (Dataset, chainable `where` / `order`
/ `limit`, `all` / `insert` / `update` / `delete`). Sits on
top of the Phase 3 battery — single layer per ADR 0019 rule.

### Phase 6 — parity fixture

One commit. `tests/diff_framework/fixtures/sequel_canon/`
mirroring `as_lite_canon` / `json_canon` shape. CRuby side
loads the real `sequel` gem; rubyrs side loads our
`sequel_lite.rb`. Byte-diff stdout + DB dump.

### Phase 7 (optional, deferred) — basic JOIN + bench

Tier-B work. Defer until a fixture actually needs it.

## Risks / unknowns the PoC didn't kill

These remain genuine open questions Phase 2's ADR has to take a position on:

1. **`Send`-ness of `rusqlite::Connection`**. The connection
   is `!Send`. Our heap-alloc model stores Ruby values
   transitively through `vm.heap`. If we put a Connection
   in `HeapObj::TypedData`, the heap stays single-threaded
   (already true), but the typed-data destructor runs on
   sweep — needs to be the right thread. Single-threaded VM
   means this isn't a real issue today, but the ADR should
   document why it isn't (in case a future `_thread`
   battery changes the constraint).

2. **Prepared-statement lifetime vs. Connection.** rusqlite
   `Statement<'conn>` borrows from the Connection. If we
   cache statements per-Connection, the cache outlives
   each prepare call. Borrow-checker dance. Likely solution:
   own statements via `unsafe transmute` to `'static` paired
   with a runtime invariant ("Statement is dropped before
   Connection"), OR use a fresh prepare every call (slower,
   simpler). ADR should pick.

3. **`Config::sqlite_allow_paths` interaction with `:memory:`.**
   The string `":memory:"` is a SQLite-special path that
   doesn't touch the FS. Sandbox should allow it unconditionally,
   matching what `_http_server`'s `127.0.0.1` does for the
   bind address.

4. **`PRAGMA` statements.** Many Sinatra-class apps run
   `PRAGMA foreign_keys = ON` etc. on connect. Battery
   needs to allow pragmas; they're stateful. Sequel-lite
   DSL needs a sensible escape hatch (`db.execute("PRAGMA ...")`?
   `db.pragma(:foreign_keys, true)`?).

5. **JSON-in-SQLite.** SQLite has built-in JSON functions
   (`json_extract`, `json_array`). Cross-feature with our
   `_json_native` accelerator — should the battery expose
   any JSON-shaped helpers, or stay strictly SQL? Lean
   strictly SQL; users compose via `Database#execute`.

6. **WAL mode + concurrent readers.** SQLite's WAL mode
   allows N readers + 1 writer concurrently. rubyrs is
   single-threaded so this is irrelevant for the runtime,
   but file-backed DBs left in WAL mode are visible to
   other processes that open the file. ADR should document
   default journal mode (likely don't set one explicitly;
   inherit the DB's existing mode).

## Recommendation: ship Phase 2 next

The discovery above shows the menu item is **well-defined**
(ADR 0019 locks 80% of the architecture) but **broad**
(roughly 5 atomic commits before a parity fixture lights up).
Writing ADR 0027 next is the cheapest way to get
reviewer-grade clarity on the 6 open questions above before
sinking the ~1000 LOC the battery + DSL will need.

ADR-first is the right shape here because — unlike the
JSON / AS-lite menu items — there's a real Rust dep being
chosen, a real `Config` field being shaped, and a real
exception hierarchy being committed to. Those decisions
deserve to be `git blame`d to a single ADR commit, not
spread across the battery PR's diff.
