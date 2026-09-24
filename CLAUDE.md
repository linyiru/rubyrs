# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

rubyrs is a Ruby implementation in Rust, built on Prism (Ruby's official parser), that runs real, unmodified gems (Jekyll, RuboCop, Bridgetown, Rack, Sinatra). Behaviour is pinned to **CRuby 3.4** by differential testing: CRuby is the oracle, and any output difference is a rubyrs bug unless it is a deliberate divergence documented in `docs/SUBSET.md`. Out of scope: `Thread`/`Ractor` parallelism, an AOT compiler, arbitrary `bundle install`, and runtime `eval` of arbitrary strings on the WASM/embed path.

## Commands

Everything is built and tested with `--release` (CI does the same). The toolchain is pinned by `rust-toolchain.toml` (1.95). `crates/rostdown` is a git submodule, so run `git submodule update --init` on a fresh clone.

```bash
cargo build --release                      # CLI at target/release/rubyrs
./target/release/rubyrs script.rb
cargo test --release                       # whole workspace, diff_cruby included
cargo test --release -p rubyrs --test diff_cruby integer_basics   # one diff fixture
cargo test --release -p rubyrs --test diff_cruby -- --ignored     # quarantined "known:" fixtures
STRESS_GC=1 cargo test --release           # GC on every alloc point; required before pushing any new heap.alloc / maybe_gc site
cargo clippy --release --all-targets --workspace --locked -- -D warnings
bash scripts/lint-gc-rooting.sh            # CI lint: unrooted Value across maybe_gc + alloc
bash scripts/lint-doc-orientation.sh       # CI lint: doc comment re-attached to the wrong fn
bash perf/check.sh                         # peak-RSS / wall-time ratchet (perf/baselines.tsv)
```

CI also runs clippy per feature combination (`-p rubyrs --features stdlib,jit-native,_fiber,_json_native,mimalloc` and `--features everything,jit-native`) and builds with `RUSTFLAGS=-D warnings`.

It also runs the diff suite a second and third time under the JITs. To reproduce those runs locally:

```bash
RUBYRS_JIT_NATIVE=1 cargo test --release --features jit-native --test diff_cruby
RUBYRS_JIT_TIER2=1 RUBYRS_JIT_TIER2_THRESHOLD=1 cargo test --release --features jit-native --test diff_cruby
```

`THRESHOLD=1` makes tier 2 compile every method, not just hot ones. For the wasm32-wasip1 build, see `docs/DEVELOPMENT.md` (requires wasi-sdk 24 and `--no-default-features`, because `cext` has no dynamic loader on WASI and `build.rs` panics if it is enabled). Useful debug env vars: `RUBYRS_GC_STATS=1` (one stderr line per GC sweep), `RUBYRS_IC_STATS=1` (inline-cache hit rate on exit; needs the `ic-stats` feature), `RUBYRS_FUEL`/`RUBYRS_MAX_OBJECTS`/`RUBYRS_MAX_FRAMES` (resource caps).

## Tests

- **`crates/rubyrs/tests/diff/*.rb` + `tests/diff_cruby.rs`** is the main suite (well over a thousand fixtures). Each file runs under rubyrs and the system `ruby`, and stdout must match byte-for-byte. Fixtures are **not** auto-discovered: add a `#[test] fn name() { run_diff("name"); }` line to `diff_cruby.rs`. Use `run_diff_env` for fixtures that need a rubyrs-side kill switch (e.g. `RUBYRS_JSON_NO_NATIVE=1`). If `ruby` is not on PATH, the tests skip instead of failing.
- **Known-failure discipline:** a clean run has 0 failures. Mark an unimplemented feature `#[ignore = "known: …"]` and name the missing piece; never leave it red. A divergence that appears only under the specialized JIT goes in `JIT_KNOWN_DIVERGENCES` with a tracking note; that list is consulted only when `RUBYRS_JIT_NATIVE` is set, so it does not quarantine tier-2 failures. Delete the entry when the bug is fixed.
- The older fixture style lives in `tests/fixtures/*.rb` + `.expected`. Generate with `UPDATE_EXPECTED=1 cargo test --release -p rubyrs <name>`, then register in `tests/integration.rs`.
- Other suites: `tests/embed.rs` pins the public embedding API; `tests/diff_framework*` holds real vendored Sinatra/Rack gems; `tests/cext_*` covers C extensions (their example bundles build via `examples/*/build.sh`); `tests/wasm/*.sh` covers WASM.

## Architecture

Pipeline (`crates/rubyrs`): `ruby_prism::parse` → `ast::tr` translates Prism nodes into an owned `Spanned<Expr>`, dropping the parser's `'pr` lifetime at that boundary → `compiler` emits `Proto` bytecode (`bytecode.rs` `Op`) and a global `Interner` (`SymId(u32)` for every name) → `vm::Vm` runs it and returns `Result<Value, Trap>`. `lib.rs` exposes the embedding API (`Runtime`, `Config`). `main.rs` is the CLI.

- **VM layout mirrors CRuby's C files.** `vm.rs` holds `Vm`, `Frame`, and `PinGuard`. `vm/*.rs` maps one-to-one to CRuby compilation units: `dispatch.rs` ≈ `vm_eval.c`/`vm_insnhelper.c` (by far the largest), `step.rs` ≈ `vm_exec.c` (per-opcode `step`, `dispatch`/`dispatch_until` drivers), `lookup.rs` ≈ `vm_method.c` (inline `CallCache`, invalidated by `method_gen`), `gc.rs`, `string.rs`, and so on. To place new code, ask "where would CRuby put this?" and check `docs/VM_MODULE_MAP.md`.
- **Values and GC.** Immutable, non-cycling data sits behind `Rc`. Mutable, possibly-cyclic objects are `ObjId`s into a stop-the-world mark-sweep `Heap` (ADR 0003). Roots are the operand stack, frames, `vm.pinned`, globals, and constants. A `Value` popped into a Rust local is **unrooted**: across any `maybe_gc` + `heap.alloc` window it must be pinned via `PinGuard` (`g.vm.maybe_gc()` / `g.vm.heap.alloc(...)`). Violations show up only under `STRESS_GC=1`, which is why the lint and the stress CI run exist.
- **Preamble.** Much of the core library is Ruby source in `src/preamble/*.rb`, compiled at `Runtime::new`. The `preamble-cache` feature (`preamble_cache.rs`) caches the compiled bytecode, keyed to the binary. `src/stdlib_vendor/` and the `*_shim.rb` files hold vendored stdlib and gem shims.
- **Native accelerators.** `*_native.rs` modules (json, yaml, kramdown, liquid, rouge, prism, zlib, …) replace hot pure-Ruby gem paths. Each has a kill switch (`RUBYRS_*_NO_NATIVE`) so the diff suite can pin both the accelerator and the pure-Ruby canon against CRuby. Feature flags in `crates/rubyrs/Cargo.toml` gate many of these; `default = cext, regex, bignum, std-sink, preamble-cache`.
- **JIT (`jit-native` feature, Cranelift).** This is active work (ADR 0030 onward). `jit_native.rs` is the specialized tier: narrow eligibility, and it deopts to the interpreter on any guard failure. `jit_tier2.rs` is the frame-keeping baseline tier from ADR 0037. It keeps the real interpreter frame and operand stack, so VM state must equal the interpreter's at every point foreign code can observe (helper calls, bail points, branch edges). A bail switches modes and never re-executes anything. Admitted ops that aren't inlined run the interpreter's own `step()`. Enable at runtime with `RUBYRS_JIT_NATIVE=1` / `RUBYRS_JIT_TIER2=1`.
- **C extensions.** `crates/rubyrs-cext` provides the `rb_*` C ABI with opaque handles. `vm/cext.rs` bridges handles to `Value`s. Re-entrance into the Vm goes through the thread-local `CURRENT_VM_PTR` in `vm/vm_ptr.rs`. Safety contracts are in `docs/CEXT_SAFETY.md`, and Miri runs in CI over this surface.
- **Other workspace crates:** `rubund` (Bundler/Gemfile runner that drives the embedding API), `rubyrs-gapscan` (subset-coverage scanner over real Ruby corpora), `rubyrs-spec-extract` (ruby/spec ingestion), `rubyrs-wasm-embed`/`-timer` (WASM embedder), `carmine`, `liquidus`, `blusher-ext`, and `crates/rostdown` (submodule). `crates/rubyrs/fuzz` is excluded from the workspace (nightly; see `docs/FUZZING.md`).

Design rationale lives in `docs/adr/`. Scope and divergences are in `docs/SUBSET.md`, and the gate reference is in `docs/CI_GATES.md`.

## Rules that gate merges

- **Panic budget is a down-only ratchet.** Per-file counts of `panic!`/`.unwrap()`/`.expect(` in `crates/rubyrs/src` (excluding everything from the first `#[cfg(test)]` onward) are budgeted in `ci.yml`, and unlisted files default to 0. Convert panic sites to a `Trap` instead of adding one. Raising a budget needs justification and must land in the same commit as the new site (`docs/PANIC_AUDIT.md`).
- Coverage (`crates/rubyrs/coverage_baseline.json`) and perf (`perf/baselines.tsv`) are ratchets too.
- Any divergence from CRuby must be documented in a code comment and the CHANGELOG, ideally in an ADR as well.
- Update `CHANGELOG.md` under `[Unreleased]`: `Added`/`Changed`/`Fixed` for user-facing changes, `Internal` otherwise.
- Adding a dependency needs discussion first, because the project deliberately keeps its dependency closure small.
- `cargo fmt` is **not** used. The style is deliberately compact (one-line matches and tests), so match the surrounding code and don't reformat files.
- Commit messages: a ≤72-char summary, a blank line, then a body that explains *why*. No `Co-Authored-By:` trailers.
