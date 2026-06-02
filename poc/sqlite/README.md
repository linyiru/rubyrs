# PoC: SQLite + Sequel-lite (ADR 0026 v2 menu item 4)

> **Status (2026-06):** Phase 1 Discovery only. No code shipped.
> See `FINDINGS.md` for the dependency survey, what's locked by
> existing ADRs, the open-question inventory, and the proposed
> Phase 2+ commit ladder.

Mirrors `poc/sinatra/` (M27 D) and `poc/as_lite/` (menu item 3)
for menu item 4 — the Sinatra+DB shape ADR 0026 v2 framed as "the
first real app." Distinct from earlier menu items in three ways:

1. **Real Rust battery required** (not just pure-Ruby canon —
   rusqlite wraps libsqlite3, no pure-Ruby substitute is
   viable for the data layer)
2. **Stateful** — connection, transaction, prepared statements
   all carry state across the test boundary
3. **Two-component** — `_sqlite` battery (Tier 3 Rust) + Sequel-
   lite query DSL (pure Ruby, Tier 3) sit in a single layer
   (ADR 0019 rejected the split-layer C-ext / Ruby-wrapper
   pattern)

## Files

| File | Role |
|---|---|
| `FINDINGS.md` | Phase 1 deliverable — discovery output. ADR scan, dep survey, locked-vs-open decision inventory, phased commit ladder, residual risk list. |

Phase 2+ artefacts (`docs/adr/0027-battery-sqlite.md`, battery
sources, `sequel_lite.rb` canon, `sequel_canon` parity
fixture) land in follow-up commits as `FINDINGS.md` recommends.

## Why ADR-first this time

For JSON / ActiveSupport-lite the spike → canon path was tight
enough to skip a battery ADR. SQLite has:

- A real Rust crate choice (rusqlite vs sqlx — locked to
  rusqlite by ADR 0019, but feature-flag policy still open)
- A real `Config` field shape (`Config::sqlite_allow_paths`)
- A real exception hierarchy committed to (`SQLite3::Exception`
  → ~7 subclasses)
- A real diff_framework S4 addition (stateful lifecycle hooks
  — schema seed + dump diff) that's been deferred since M27 D

Those decisions deserve a single ADR commit that `git blame`
points at, not "five spots inside the battery PR diff."
`FINDINGS.md` proposes ADR 0027 as the Phase 2 commit.
