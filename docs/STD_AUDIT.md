# `use std::` audit — ADR 0018 Phase 0 deliverable

This document inventories every `std::` reference in the
current `crates/rubyrs/src/` tree and tags each with its
destination per [ADR 0018](adr/0018-workspace-migration.md)
Phase 1's `rubyrs-core` extraction.

Updated 2026-05-27 by ADR 0019 v3 / 0020 follow-up:
extends the original three-tag scheme with a fourth tag for
ADR 0019's Tier 3 native batteries.

## Tag scheme

| Tag | Meaning | Phase 1 action |
|---|---|---|
| **`tier-1-replaceable`** | `std::` reference can be rewritten to `core::` or `alloc::` with no behaviour change. | Mass-replace in the Phase 1 PR. |
| **`tier-1-replaceable-via-hashbrown`** | `std::collections::{HashMap,HashSet}` — depends on `std::collections::hash_map::RandomState`. Replace via the `hashbrown` crate's API-compatible `HashMap`/`HashSet`. | Add `hashbrown` (no_std + alloc) as a `rubyrs-core` dep, rewrite imports. |
| **`tier-2-host-io`** | Legitimately needs `std`. Belongs in `rubyrs-language` (Phase 3) or — for the CLI/embed surface — stays in the `rubyrs` facade crate. | Stays in `rubyrs` facade or moves to `rubyrs-language` per migration. |
| **`tier-3-battery-<name>`** | Will move into the corresponding Tier 3 battery crate per ADR 0019 v3 (e.g. `_io`, `_thread`, `_process`). | Stays put until the battery's own PR, then moves with that battery. |

## Summary frequency

Aggregated reference counts (use + inline) across
`crates/rubyrs/src/**/*.rs`:

| `std::` path | Count | Tag |
|---|---|---|
| `std::rc::Rc` | 50 | `tier-1-replaceable` → `alloc::rc::Rc` |
| `std::cmp::Ordering` | 34 | `tier-1-replaceable` → `core::cmp::Ordering` |
| `std::cell::RefCell` | 34 | `tier-1-replaceable` → `core::cell::RefCell` |
| `std::collections::HashSet` | 32 | `tier-1-replaceable-via-hashbrown` |
| `std::collections::HashMap` | 18 | `tier-1-replaceable-via-hashbrown` |
| `std::cell::Cell` | 14 | `tier-1-replaceable` → `core::cell::Cell` |
| `std::ffi::c_char` | 14 | `tier-1-replaceable` → `core::ffi::c_char` |
| `std::ffi::c_void` | 9 | `tier-1-replaceable` → `core::ffi::c_void` |
| `std::mem::size_of` | 9 | `tier-1-replaceable` → `core::mem::size_of` |
| `std::process::id` | 7 | `tier-2-host-io` (host capability — already gated via `Config::pid`, ADR 0017) |
| `std::mem::take` | 7 | `tier-1-replaceable` → `core::mem::take` |
| `std::ffi::c_long` | 7 | `tier-1-replaceable` → `core::ffi::c_long` |
| `std::path::Path` | 6 | `tier-2-host-io` (some sites) / `tier-3-battery-_io` (some sites) — see per-site table |
| `std::ffi::c_int` | 6 | `tier-1-replaceable` → `core::ffi::c_int` |
| `std::env::vars` | 6 | `tier-2-host-io` (host capability — already gated via `Config::env`, ADR 0017) |
| `std::time::Instant` | 5 | `tier-2-host-io` (deadline enforcement — Tier 1 *internal*; safe per ADR 0017 line 47) |
| `std::time::Duration` | 5 | `tier-2-host-io` (deadline) |
| `std::path::PathBuf` | 5 | `tier-3-battery-_io` (in `kernel.rs`'s require resolver) / `tier-2-host-io` (in `vm.rs`'s `loaded_features` set) |
| `std::io::sink` | 5 | `tier-2-host-io` (default stdout sink; ADR 0017's `set_stdout` mechanism) |
| `std::borrow::Cow` | 5 | `tier-1-replaceable` → `alloc::borrow::Cow` |
| `std::slice::from_ref` | 4 | `tier-1-replaceable` → `core::slice::from_ref` |
| `std::io::Write` | 4 | `tier-2-host-io` (stdout sink trait) |
| `std::fs::canonicalize` | 4 | `tier-3-battery-_io` |
| `std::time::SystemTime` | 3 | `tier-2-host-io` (wall clock — already gated via `Config::wall_clock`, ADR 0017) |
| `std::sync::Arc` | 3 | **AUDIT** — single-threaded VM should not need `Arc`. Probably dead code or cext-callback path; needs per-site verification |
| `std::io::stdout` | 3 | `tier-2-host-io` (`main.rs` only — facade-CLI surface) |
| `std::fs::metadata` | 3 | `tier-3-battery-_io` |
| `std::fmt::Write` | 3 | `tier-1-replaceable` → `core::fmt::Write` |
| `std::str::FromStr` | 2 | `tier-1-replaceable` → `core::str::FromStr` |
| `std::str::from_utf8` (`from_utf`) | 2 | `tier-1-replaceable` → `core::str::from_utf8` |
| `std::ptr::null_mut` | 2 | `tier-1-replaceable` → `core::ptr::null_mut` |
| `std::ptr::addr_of_mut` | 2 | `tier-1-replaceable` → `core::ptr::addr_of_mut` |
| `std::num::NonZeroU<N>` | 2 | `tier-1-replaceable` → `core::num::NonZero*` |
| `std::mem::replace` | 2 | `tier-1-replaceable` → `core::mem::replace` |
| `std::iter::repeat_n` | 2 | `tier-1-replaceable` → `core::iter::repeat_n` |
| `std::io::Result` | 2 | `tier-2-host-io` (sink trait return type) |
| `std::fs::read_to_string` | 2 | `tier-3-battery-_io` (require resolver in `lib.rs:1029`, `kernel.rs:1189`) |
| `std::collections::hash_map::DefaultHasher` | 1 | `tier-1-replaceable-via-hashbrown` (use `hashbrown::DefaultHashBuilder`) |
| `std::str::Chars` | 1 | `tier-1-replaceable` → `core::str::Chars` |
| `std::slice::from_raw_parts` | 1 | `tier-1-replaceable` → `core::slice::from_raw_parts` |
| `std::rc::Weak` | 1 | `tier-1-replaceable` → `alloc::rc::Weak` |
| `std::process::exit` | 1 | `tier-2-host-io` (`main.rs` only — CLI exit code path) |
| `std::panic` | 1 | `tier-2-host-io` (cext panic-catch — ADR 0009 territory; needed for cext crate, not core) |
| `std::ops::Deref` | 1 | `tier-1-replaceable` → `core::ops::Deref` |
| `std::mem::forget` | 1 | `tier-1-replaceable` → `core::mem::forget` |
| `std::iter::repeat_with` | 1 | `tier-1-replaceable` → `core::iter::repeat_with` |
| `std::hint::cold_path` | 1 | `tier-1-replaceable` → `core::hint::cold_path` |
| `std::hash` | 1 | `tier-1-replaceable` → `core::hash` |
| `std::fs::write` | 1 | `tier-3-battery-_io` |
| `std::fs::read` | 1 | `tier-3-battery-_io` |
| `std::ffi::CStr` | 1 | `tier-1-replaceable` → `core::ffi::CStr` |
| `std::ffi::c_ulong` | 1 | `tier-1-replaceable` → `core::ffi::c_ulong` |
| `std::env::vars_os` | 1 | `tier-2-host-io` (host capability) |
| `std::env::var` | 1 | `tier-2-host-io` (`STRESS_GC` test gate at `lib.rs:206` — should move to `cfg!(test)` discipline before Phase 1) |
| `std::env::current_dir` | 1 | `tier-3-battery-_io` (in `vm/fileops.rs`) |
| `std::char::from_digit` | 1 | `tier-1-replaceable` → `core::char::from_digit` |
| `std::any::type_name` | 1 | `tier-1-replaceable` → `core::any::type_name` |

## Aggregate counts by tag

| Tag | Sites (approx) | Action |
|---|---|---|
| `tier-1-replaceable` | ~230 | sed-replaceable in Phase 1 PR |
| `tier-1-replaceable-via-hashbrown` | ~51 | add hashbrown dep, rewrite imports |
| `tier-2-host-io` | ~40 | stays in `rubyrs` facade OR moves to `rubyrs-language` |
| `tier-3-battery-_io` | ~14 | moves to `_io` battery crate per ADR 0019 v3 |
| **Total** | **~335** | |

(The ADR 0018 original count was 54 — that was `use std::` lines only, not all references. The 335 figure is "every spelling of `std::` in any context.")

## Per-site detail for tier-2-host-io (the migration-critical ones)

These are the sites that DO NOT mass-replace via sed. Each
needs an explicit decision in Phase 1 about where it lands.

### Stays in `rubyrs` facade crate (CLI / embed surface)

These are the CLI binary and the embed API. They legitimately
need `std::io`, `std::time`, `std::env`, `std::process`:

- `main.rs:1` `use std::env` — CLI arg parsing
- `main.rs:2` `use std::path::Path` — script path arg
- `main.rs:3` `use std::process` — exit codes
- `main.rs:22, 30` `std::time::Instant` — `--time` flag, total runtime
- `main.rs:200` `std::time::Duration::from_millis` — `--deadline` flag parsing
- `main.rs:204` `std::process::id` (commented — wasi panics; CLI gates it)
- `main.rs:230` `std::time::SystemTime`, `UNIX_EPOCH` — `--wall-clock` flag
- `main.rs:260` `std::io::stdout` — `rubyrs script.rb` stdout sink
- `lib.rs:36` `use std::io::Write` — `Runtime::set_stdout(Box<dyn Write>)` API surface
- `lib.rs:37` `use std::path::Path` — `Runtime::eval_file` API
- `lib.rs:129, 303, 386, 1126` `std::time::Duration` — `Config::deadline` field
- `lib.rs:874, 878, 881` `std::io::sink` (in comments; default sink per ADR 0017)

### Moves to `rubyrs-language` (Phase 3) — VM-internal `std` use

These touch host capabilities but the *VM* needs them for its
own machinery (not just for exposing to scripts):

- `vm.rs:282` `std::collections::HashSet<std::path::PathBuf>` — `loaded_features` set for `require` deduplication. The `HashSet` is `tier-1-replaceable-via-hashbrown`; the `PathBuf` element is `tier-2-host-io` because path identity is OS-flavoured. **Decision**: stays in VM core when Phase 1 lands; the PathBuf can move under a feature gate later if pure-string identity becomes acceptable.
- `vm.rs:410` `pub(crate) stdout: Box<dyn std::io::Write>` — the Tier 1 stdout-sink mechanism. ADR 0017 line 47 explicitly permits this. **Decision**: stays in `rubyrs-core` (Tier 1) behind the existing `Runtime::set_stdout` API.
- `vm.rs:423` `Option<std::time::Instant>` — `deadline_at`. ADR 0017 line 47 permits this for cap enforcement (host-side, not script-visible). **Decision**: stays in `rubyrs-core`.
- `vm.rs:598` `std::io::sink()` — default stdout in the no-host-stdout case (per ADR 0017 the Tier 1 default). **Decision**: stays.
- `vm/gc.rs:59` `std::time::Instant::now()` — deadline check. Same rationale as `vm.rs:423`.
- `vm/iter.rs:2575-2580` `impl std::io::Write for Sink` — test-only impl. **Decision**: stays (test code).
- `vm/kernel.rs:10` `use std::io::Write` — `puts` / `print` / `p` calling into the stdout sink. Tier 1 mechanism per ADR 0017.

### Moves to `_io` battery (Tier 3, per ADR 0019 v3)

These are the script-exposed file-system operations. Today
they live in `vm/fileops.rs` + `vm/kernel.rs`'s require
resolver. Per ADR 0019 v3, they become the `_io` battery:

- `vm/fileops.rs:7, 134, 138, 166` — `Path`, `PathBuf`, `current_dir`, `canonicalize`
- `vm/fileops.rs:54, 67, 76, 83, 88` — `read`, `write`, `metadata` (all `std::fs::*`)
- `vm/kernel.rs:861, 891, 1070, 1116-1117, 1165, 1184, 1189, 1207` — require resolver's path canonicalisation and `read_to_string`
- `vm/cext.rs:920` — `use std::path::Path` (cext-related, stays with cext bridge)
- `vm/dispatch.rs:2155-2159` — `Path`, `canonicalize` for some dispatch path (likely require-related; verify in Phase 1)
- `lib.rs:1029` `std::fs::read_to_string` — `Runtime::eval_file` reads disk. **Decision**: stays in facade (Runtime API is Tier 1 facade) but the `fs::read_to_string` call could route through a host-fn for proper capability gating

### Capability-injected (already gated per ADR 0017)

These have an explicit `Config` injection slot; the `std::`
call is in the CLI binary, NOT in the VM. The VM consumes
the injected value:

- `lib.rs:132, 141` — `Config::env` (host fills from `std::env::vars()`)
- `lib.rs:147, 155, 160` — `Config::pid` (host fills from `std::process::id()`)
- `lib.rs:166, 185, 1001` — `Config::wall_clock` (host fills from `std::time::SystemTime::now()`; `deadline_at` from `Instant::now()`)
- `lib.rs:206` `std::env::var("STRESS_GC")` — test-time only. **Move to** `cfg!(test)` or a `Config::stress_gc: bool` field before Phase 1; the env-var read leaks the Tier-1-deviation through the library API today.
- `vm.rs:390-402, 423` — comments + types referencing the injected values
- `vm/step.rs:693, 956` — comments referencing the injected values (`env`, `pid`)

### Audit-required sites — RESOLVED 2026-05-27

- **`std::sync::Arc` (4 sites total)** — investigated, all legitimate:
  - `vm.rs:404`, `lib.rs:190`, `main.rs:223` — `Config::time_now: Option<std::sync::Arc<dyn Fn() -> (i64, u32) + Send + Sync>>`. The `Send + Sync` bound is required for the public `Config` API to be `Send` (an embedder may construct Config in one thread, hand it to a Runtime on another). **Tag: `tier-2-host-io`** (legitimately public API surface). NOT dead code; keep as `Arc`.
  - `vm/iter.rs:2582` `Arc::new(Mutex::new(...))` — test-only code (inside `#[cfg(test)]` block); the test fixture's `Sink` adapter needs `Arc` so the test thread can read the captured buffer after `eval`. **Tag: `tier-2-host-io` (test-only)**. Keep.
- **`std::panic` (1 site at `lib.rs:1670`)** — `use std::panic::{catch_unwind, AssertUnwindSafe}` is inside `#[cfg(test)] mod caps_guard_tests` testing `CapsGuard`'s drop-safety. Test-only, never reaches the production path. **Tag: `tier-2-host-io` (test-only)**. Keep.
- **`STRESS_GC` env read at `lib.rs:206`** — RESOLVED by removing the env read from `Config::default()` (commit `<this PR>`). The library API no longer leaks host env into a public field. Subprocess-based tests (diff_cruby, cext_*) still pick up `STRESS_GC` via the CLI's explicit `main.rs::env_lookup` read.

## Phase 1 migration order

Recommended sequencing within the Phase 1 PR (or PR chain):

1. ✅ **Test-time cleanup** — DONE in pre-Phase-1 cleanup PR:
   - `lib.rs:206`'s `STRESS_GC` env read removed from
     `Config::default()`; library API no longer leaks env
   - `std::sync::Arc` audited — all 4 sites legitimate
     (`Config::time_now` public Send+Sync bound; test-only
     Mutex wrapper)
   - `std::panic` confirmed test-only (`#[cfg(test)]`
     block in `lib.rs`); no production-path usage

2. **Phase 1 PR — mass `tier-1-replaceable` sweep**:
   - `sed`-replace ~230 `std::` sites to `core::` / `alloc::` equivalents. Single mechanical commit per category (Rc, RefCell, Cell, cmp::Ordering, mem::*, ffi::*).
   - Net diff: ~280 lines changed, mechanical only.

3. **Phase 1 PR — `hashbrown` introduction**:
   - Add `hashbrown` to `rubyrs-core/Cargo.toml`.
   - Replace `std::collections::HashMap` / `HashSet` imports.
   - One commit; ~50 sites.

4. **Phase 1 PR — `tier-2-host-io` placement**:
   - Apply the per-site decisions above.
   - For "stays in `rubyrs` facade" sites — the facade still uses `std`; only `rubyrs-core` goes `no_std`.
   - For "VM-internal `std`" sites (`vm.rs`'s stdout box, deadline) — these stay in `rubyrs-core` because ADR 0017 explicitly permits them as internal-use; the `#![no_std]` attribute on the crate root is enforced by **NOT** importing `std`, but `Box<dyn Write>` needs the `Write` trait from somewhere. Solution: define the `Write` trait abstraction inside `rubyrs-core` (`pub trait OutputSink: Write`-shape) such that the core depends on `core::fmt::Write` for the no_std contract, and the facade glues `std::io::Write` to the abstraction. This is the **only non-mechanical part** of Phase 1.

5. **Phase 2 (post Phase 1)** — `tier-3-battery-_io` is NOT touched in Phase 1. Those sites stay in `rubyrs-core` (or wherever they are today) until the `_io` battery PR opens; that PR migrates them out per ADR 0019 v3.

## Open questions to resolve before Phase 1 lands

1. **`Write` trait abstraction for the no_std core**: do we vendor a minimal `OutputSink` trait, or do we add a `[dependencies]` on a no_std-compatible IO crate? Recommend the former — single trait, ~20 lines, no dep.
2. ~~**`std::sync::Arc` sites**~~ RESOLVED — see "Audit-required sites" section above.
3. **`lib.rs:1029`'s `std::fs::read_to_string`**: should `Runtime::eval_file` itself be available in Tier 1 (today's reality) or move to a host-fn / Tier 3 `_io` battery surface? Recommend keeping in the facade for Phase 1; revisit when `_io` battery lands.
4. **`vm/cext.rs:920`'s `use std::path::Path`**: stays in the cext crate (which is itself Tier 4); just verify the import isn't visible from `rubyrs-core`.

## Verification at the end of Phase 1

CI gates (per ADR 0018 Phase 2):

- `cargo check -p rubyrs-core --no-default-features --target wasm32-unknown-unknown` — must build (`wasm32-unknown-unknown` has no `std`, so any leaked `std::` import fails)
- `cargo check -p rubyrs-core --target wasm32-unknown-unknown` (default features on) — catches `std` drift through default-on dependencies
- `cargo test --release` — green
- `STRESS_GC=1 cargo test --release -p rubyrs --test diff_cruby` — green
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- Binary size of facade `rubyrs --no-default-features` ≤ 6 MB (ADR 0015 Rule 7 ceiling, embed shape)

## Status

Audit complete. Ready for Phase 1 PR planning.

Next deliverable: the **`Write` trait abstraction** sketch
(Open Q #1) before Phase 1 PR opens — that's the single
piece that needs design work, not mechanical sed.
