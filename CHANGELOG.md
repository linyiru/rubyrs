# Changelog

All notable changes to rubyrs will be recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/), and this project
follows [Semantic Versioning](https://semver.org/) once we hit 0.1.

> **Adding an entry?** Put it under `## [Unreleased]` in the matching
> `### Added` / `### Changed` / `### Fixed` / `### Internal` bucket. Keep it to
> **one user-facing sentence** — design rationale, implementation notes, and
> benchmarks belong in the commit / PR / ADR, not here. Link the PR or issue,
> plus any ADR or `tests/diff/*.rb` fixture. An entry that links a
> `tests/diff/*.rb` fixture is verified byte-identical to CRuby by the
> `diff_cruby` harness, so you don't need to repeat "byte-identical to CRuby".
> Template:
>
> ```
> - **`Thing#added`** — what it does for users; key divergence if any.
>   ([#PR](https://github.com/linyiru/rubyrs/pull/PR), ADR 00NN, `fixture.rb`)
> ```

## [Unreleased]

> Backfill in progress: the post-0.2.0 JIT arc landed across ~500 commits
> without per-PR changelog entries. The headlines are captured below from
> their ADRs; finer-grained entries are still being reconstructed.

### Added

- **Native JIT (`jit-native`, opt-in)** — a Cranelift method/loop JIT that
  deopts to the always-correct interpreter on any non-fast shape; covers
  integer methods, recursive calls, and Int/Float `Enumerable` drivers
  (`sum` / `map` / `select` / `find` / `group_by` / `each_with_object` / …).
  Off by default. (ADR [0030](docs/adr/0030-jit-tier.md),
  [0032](docs/adr/0032-jit-native-surpass.md),
  [0034](docs/adr/0034-jit-first-surpass-yjit.md))
- **`Fiber`** — `Fiber.new` / `#resume` / `Fiber.yield` / `#alive?` over the
  `_fiber` battery.

### Changed

- **Generational mark-sweep GC** — young/old regions with minor/major
  collections and a write barrier, replacing the flat mark-sweep
  (substantially less collection churn on object-heavy workloads).
- **Battery preambles load through the preamble bytecode cache** — the
  `_sqlite` / `_socket` / `_openssl` / `_bcrypt` / `_oj` Ruby surfaces
  (~1,050 lines) are now compiled as cached preamble chunks at `Runtime`
  construction instead of being re-parsed by `register_*_host_fns` on
  every boot (measured −12% warm-boot wall on a `_socket,_openssl`
  build). The classes now exist in every build carrying the feature —
  registration only wires the host-fn backend — and survive
  `Runtime::reset()` as part of the post-preamble baseline.

### Fixed

- **`break`/`next` escaping an ensure crossed by a local `return` now
  matches CRuby ≥ 3.4.2** — the break lands at the loop join and cancels the
  pending return (rubyrs previously mimicked CRuby 3.4.0/3.4.1's prism bug
  window, [Bug #21001](https://bugs.ruby-lang.org/issues/21001), where the
  method returned the break value); 15 shapes re-mainlined into
  `ensure_walk_break_return.rb`, leaving only the walk-survives-block-`next`
  family (D3/K1/K4, where modern CRuby hangs forever) pinned in
  `tests/embed/ensure_walk_divergences.rs`.
- **`$!` now reverts to the enclosing scope's errinfo when a
  `break`/`next`/`return` cancels an in-flight exception** — a control
  transfer out of an exception-entered ensure body (or out of a rescue body)
  previously left the cancelled exception in `$!`, so a later bare `raise`
  resurrected it; 27 shapes added to `ensure_walk_break_return.rb`
  (sections M/N: the full next×exception-source matrix plus the errinfo
  restore family).

### Internal

- **Dispatch-core fast paths** — primitive / index / `Proc#call` fast paths
  that bypass the method-name cascade on hot framework dispatch. (ADR
  [0031](docs/adr/0031-dispatch-core.md))
- **CI: ensure-walk fixture unpinned from the CRuby 3.4.0/3.4.1 prism bug
  window** — 14 `break`/`next`-in-suspended-ensure shapes whose CRuby output
  flipped in 3.4.2 ([Bug #21001](https://bugs.ruby-lang.org/issues/21001);
  one shape hangs modern CRuby forever, which hung CI's floating "3.4"
  oracle) moved from `ensure_walk_break_return.rb` to pinned goldens in
  `tests/embed/ensure_walk_divergences.rs`.
- **CI: C-extension `dlopen` from test binaries works on Linux** — build.rs
  now emits `--export-dynamic` for ELF test targets too, so in-process
  `Runtime` tests that `require` a cext (e.g. `cext_typeddata`) resolve
  `rb_*` symbols instead of failing with `undefined symbol: rb_cObject`.

## [0.2.0] - 2026-06-14

### Release highlights

Two big arcs landed since 0.1.0: **true async streaming for the
`_http_server` battery** (ADR 0023, completed pre-0.1.0 + Risk #1
shipped post-tag via Phase 5b) and **full signal-handling
infrastructure** (ADR 0025, Phase 0–5 + round-3 follow-ups). The
combination unlocks the canonical CRuby trap-flow chain in
`rubyrs script.rb`:

  at_exit { cleanup }
  Signal.trap("INT") { puts "graceful shutdown"; exit 0 }
  __rubyrs_http_serve_with_app("127.0.0.1:9292", 60, app,
                              { install_signal_handler: true })

Ctrl+C → trap fires → exit raises SystemExit → at_exit drain →
embedder sees Trap with the cleanup already done. Streaming
responses honor `body.close` exactly-once on client disconnect
+ server shutdown (Drop-Vm-free contract + `SuppressInterruptGuard`
holding off concurrent SIGINT during close).

### Added

- **`Signal.trap(name, handler = nil)` + block form** — install a handler (String/Symbol/Integer name, optional "SIG" prefix; "DEFAULT"/"IGNORE"/`SIG_IGN`/Proc/block/`nil`→IGNORE per CRuby); returns the previous handler in matching shape; SIGKILL/SIGSTOP rejected. Trap blocks run at the `dispatch_until` safe-point. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Kernel#sleep(secs)` / bare `Kernel#sleep` are now interruptible** — polls the interrupt flag in chunks (CLI uses 50ms) and raises `Interrupt` on flag-flip; bare `sleep` requires `install_signal_handler: true` or raises ArgumentError. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Kernel#exit` / `Kernel#exit!` / `Kernel#abort`** — `exit(status = true)` raises SystemExit (ensure + at_exit fire); `exit!(status = false)` exits immediately; `abort(msg = nil)` writes msg then `exit(1)`. Status shapes match CRuby (`Integer | true | false | nil`). ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Kernel#at_exit { ... }`** — LIFO stack drained at the end of each eval (embed-model analog of CRuby's process end); errors override the result ("last-error-wins") and the drain is panic-safe per handler. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Interrupt < SignalException < Exception`** and **`SystemExit < Exception`** class hierarchy — all sit outside `StandardError` so a bare `rescue` can't swallow Ctrl+C or `exit`; `SystemExit` carries `#status`/`#success?`, `SignalException` adds a 2-arg constructor + `#signo`. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Signal` module preamble** — wraps the `__rubyrs_signal_trap` Kernel builtin. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **CLI signal opt-ins** — the CLI wires real sleep, `process_exit`, and `install_signal_handler: true`. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`_http_server` streaming bodies** — interruptible body close now guards `body.close` against a concurrent SIGINT during client-disconnect (ADR 0023 Risk #1). ([ADR 0023](docs/adr/0023-true-async-streaming.md))
- **`Kernel#sleep` accepts `Rational`** — CRuby parity. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))

### Changed

- **`Config::sleep_for` signature** — now takes the interrupt flag and returns the slept duration. Breaking for embed users with custom sleep closures. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Config::default()` Tier 1 defaults preserved** — new capability slots default off: no host-side process termination, no signal capture, and `sleep` raises without injection. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`install_signal_handler` flag parsing** — now accepts Ruby `true`/`false` alongside `0`/`1`, and the rejection message lists all four accepted shapes. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))

### Fixed

- **"no implicit conversion" TypeError spellings match CRuby across the
  conversion surface** — nil/true/false now render as value words ("no
  implicit conversion of nil into String") instead of `NilClass`/"Boolean";
  num2long-shaped Integer ops get CRuby's distinct "no implicit conversion
  from nil to integer" wording; `Kernel#Integer/#Float/#Rational` and
  sprintf `%d`/`%f` operands use the "can't convert X into Y" family;
  `const_get`/`const_defined?`/`const_source_location` name String (not
  Symbol) and `const_set`/`autoload`/`autoload?` name args raise "X is not
  a symbol nor a string"; `Integer#chr(enc)` names String (not Encoding);
  a BigInt receiver's `chr` is always `RangeError`; `class_eval(src, nil)`
  accepts the nil filename. (`conv_type_name_messages.rb`)
- **Stack overflow at startup in debug builds on small main-thread stacks** (#356, our first user-filed issue) — building a `Runtime` compiles the preamble through a recursive AST→IR translator whose unoptimised frames overflowed the 1 MB default main-thread stack on Windows (Linux/macOS 8 MB and release builds were unaffected). The translator now grows the native stack on demand; native-only, wasm keeps plain recursion, and a `windows-latest` debug CI job guards the regression. ([#356](https://github.com/linyiru/rubyrs/issues/356))
- **ADR 0023 Risk #1 — `body.close` on client disconnect now actually fires** — the Drop-initiated close is shielded from a concurrent SIGINT so it can't abort mid-flight, fixing the ensure-leak shape. ([ADR 0023](docs/adr/0023-true-async-streaming.md))
- **`at_exit` handler drain is panic-safe** — a panicking handler no longer abandons the rest of the LIFO queue; the panic converts to a RuntimeError that flows through "last-error-wins". ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **Interrupt-suppression guard now catches panics** — a panic inside a trap block previously disabled SIGINT delivery permanently for the Vm; the real RAII guard fixes it. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`Signal.trap("INT", nil)` now installs IGNORE** — was treated as query mode; the 1-arg query form no longer misroutes the block form (CRuby parity). ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`SystemExit.new` no-args message** — changed from "SystemExit" to "exit" (CRuby parity). ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))

### Internal

- **Process-wide signal flag** — opted-in Runtimes share one flag; opted-out Runtimes get a fresh one so they don't leak SIGINT writes to each other. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **Per-Vm trap-block storage** — keyed by normalized Unix signal number. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **Suppress-interrupt window** — a panic-safe RAII guard marks must-complete cleanup regions. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`at_exit` block-id stack** — drained after eval returns. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **Safe-point interrupt dispatch** — both dispatch loops consult an interrupt action at op-boundary; the three actions cover Default/Ignore/Block trap states, gated by the suppress counter and cext depth. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **New `signals.rs` module** — `signal-hook`-backed install with a Tier-1 portable signal subset; Unix-only, Windows is a no-op stub (deferred per ADR 0025). ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **New deps** — `signal-hook` (unix target) and `libc` (unix dev-dep for the SIGINT smoke test). ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **Memory-ordering contract** — Relaxed-load + SeqCst-store suffices for the single flag, with a documented upgrade checklist for future paired state. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))
- **`#[doc(hidden)]` test accessors** — for the Phase 1/2/4 integration tests, serialized by a signal-test mutex. ([ADR 0025](docs/adr/0025-signal-handling-interruptible-primitives.md))

### Deferred (tracked in ADR 0025 v7)

- **cext-depth gating** — SIGINT during a real cext call IS currently delivered mid-cext.
- **`Kernel#abort` no-args** — needs `$!` exposure to host functions.
- **`parse_signal_name` integer range** — accepts 1..=64 unconditionally; bogus numbers fail at install rather than parse.

## [0.1.0] - 2026-05-28

### Release highlights

The first tagged release of rubyrs — an embeddable Ruby 3.4
interpreter built for safe evaluation of untrusted scripts inside
a host process. Headlines:

- **Substantial Ruby 3.4 Tier 1 surface**: numerics (Int / Float /
  BigInt), strings, arrays, hashes, ranges, blocks, exceptions,
  classes / modules / mixins / `singleton_class`, regex, exceptions
  + `rescue` hierarchy walk, `method_missing` / `define_method` /
  `attr_accessor`. 229+ `diff_cruby` fixtures lock byte-identical
  stdout against MRI; embed tests cover the host-facing API. See
  the `### Added` section below for the complete inventory.
- **Untrusted-input safety model** (ADR 0008): every workload gets
  a configurable cap on fuel (instruction count), heap object
  count, frame depth, wall-clock deadline, interner size, and
  per-value byte size. Caps default to "unlimited" for trusted
  embed; `Config::fuel = Some(N)` / friends are the opt-in. A
  cap trap surfaces as `ResourceExhausted` — deliberately
  `< Exception` (not `< StandardError`) so a bare `rescue` can't
  swallow it.
- **Deterministic-by-default capabilities** (ADR 0017): `Time.now` /
  `Random.new` / `SecureRandom.seed=` / `ENV` / `$$` /
  `Runtime::set_stdout` are all explicitly host-injected via
  `Config`. With no injection, scripts get a `RuntimeError`
  pointing at the missing capability — no silent leak of the
  host's wall clock, entropy, env, or pid. The CLI binary
  `rubyrs` wires every capability through; library embedders
  inherit nothing by default.
- **CRuby C-extension compatibility** (Spike L0–L3): real
  `rb_funcallv` + handle-translation bridge via `dlopen`.
  msgpack-ruby, flori_json, and bcrypt all load and round-trip
  against their published gem source unchanged. ADR 0009 +
  ADR 0013 document the panic / aliasing policy.
- **Wasm cold start < 10 ms**: `wasm32-wasip1` build with
  `wizer` pre-init + `wasm-opt -Oz` + AOT-precompiled `.cwasm`
  starts in ~7 ms on Apple Silicon (vs CRuby's ~78 ms). The
  wasi env-var bypass + raw `environ_get` syscall keeps
  `wasmtime run --env=KEY=VAL` working post-wizer.
- **Performance vs CRuby's interpreter**: 1M-fizzbuzz at
  0.44 s (~2.3× of `ruby --disable-gems`) and ~5× lighter
  peak RSS, both on the same M-series Mac. Bench corpus and
  baselines live in `perf/` with a CI ratchet so regressions
  surface in the PR that introduced them.
- **Embeddable API** (`Runtime` / `Config` / `register_fn` /
  `set_stdout` / `eval` / `format_trap`): host fns can be
  registered as Ruby toplevel methods, stdout routes to any
  `impl Write`, traps cross the embedding boundary as
  `RubyError` variants whose `is(class_name)` / `is_a(class_name)`
  predicates lift the script's exception class name into a
  Rust-side check (with hierarchy walk against the built-in
  Tier 1 exception tree).
- **ADR-driven architecture**: 17+ Architecture Decision
  Records in `docs/adr/` documenting design choices
  (worktree migration, untrusted-input caps, cext panic policy,
  Tier 1 deterministic boundary, BigInt placement, …).
  The interpreter is split into a 20-file `vm/*.rs` module
  set, each mirroring a CRuby compilation unit
  (`string.c` → `vm/string.rs`, etc.) — see `ARCHITECTURE.md`.
- **CI ratchets**: per-file panic budget (no `.unwrap()` /
  `.expect()` / `panic!` regression allowed silently),
  per-workload peak-RSS budget, miri smoke for the cext
  re-entrance aliasing pattern, `wasm32-wasip1` build green,
  `-D warnings` clippy enforced.

The detailed log below is append-order within each section. For
breaking-change semantics, see the `### Changed` entries.

### Added
- **`String#unpack1(fmt)`** — first-element shorthand for `String#unpack`, skipping `.unpack(...).first`. Ruby-3.1+ `offset:` kwarg not implemented (SUBSET.md). ([`string_unpack1.rb`](crates/rubyrs/tests/diff/string_unpack1.rb))
- **`Array#pack` / `String#unpack` signed-int + hex directives** — adds signed 16/32-bit `s`/`l` (with `<`/`>` endian forms) and hex strings `H`/`h` (count in nibbles, `*` = rest, odd-length pads trailing nibble). ([`pack_directives_extra.rb`](crates/rubyrs/tests/diff/pack_directives_extra.rb))
- **`Kernel.instance_method(:name)` + `RUBY_VERSION` / `RUBY_PLATFORM`** — Gemfile-shape prelude probing; the two constants are populated at boot and frozen. ([`kernel_instance_method.rb`](crates/rubyrs/tests/diff/kernel_instance_method.rb))
- **`Mutex` single-threaded no-op stub** — `Mutex.new` / `synchronize { }` / `locked?` for cache-guard patterns. Single-threaded divergences: `synchronize` just yields, `locked?` always `false`, re-entrant succeeds rather than deadlocking; direct `lock`/`try_lock`/`unlock` out of scope. Not Tier 1 per [ADR 0017](docs/adr/0017-tier1-boundary.md) Rule 4. ([`mutex_stub.rb`](crates/rubyrs/tests/diff/mutex_stub.rb))
- **Global variables — `$foo` read + write** — globals persist across `eval` calls and are a GC root; uninitialised reads default to `nil`. Special: `$$` reads host PID ([ADR 0017](docs/adr/0017-tier1-boundary.md) Rule 1 deviation, target tier 2), `$0` reads script name. ([`global_variables.rb`](crates/rubyrs/tests/diff/global_variables.rb))
- **`break` / `next` through `ensure`** — the unwinder now runs every intervening `ensure` handler before landing, matching CRuby's structured-jump semantics (previously skipped or trapped); composes with `rescue`. ([`break_next_ensure.rb`](crates/rubyrs/tests/diff/break_next_ensure.rb))
- **`ConstantPath` op-write family** — `Foo::Bar ||=` / `&&=` / `+=` (and full arithmetic op-write set) work end-to-end, including a fallback for dynamic-head constant paths.
- **cext spike: BigInt protocol round-trip via msgpack/bigint.rb (A5 / A6a–A6d)** — loads a real upstream gem `lib/` helper byte-identical to MRI. Scope is Tier 1 protocol-compat per [ADR 0015](docs/adr/0015-concentric-architecture.md): i64-range values round-trip; beyond-i64 saturates, BigInt arithmetic stays Tier 2 deferred.
  - **A5 — `require ".rb"` loads Ruby source** — detects/auto-appends `.rb` and routes through a shared loader; cext stays the fallback. Resolved cwd-relative; gem-style LOAD_PATH walking deferred. (`tests/require_rb.rs`)
  - **A6a — pack/unpack endian modifiers** (`L>`/`L<`/`S>`/`S<`/`Q>`/`Q<`/`q>`/`q<`) — `>`/`<` suffix normalised to canonical directives. ([`pack_endian.rb`](crates/rubyrs/tests/diff/pack_endian.rb))
  - **A6b — `Integer#[]` bit access** (single + two-arg) — bit at position `i`, or `n[offset, length]` bitfield, two's-complement for negatives. Divergence: `length == 64` with negative receiver saturates to `-1` where CRuby returns unsigned `2^64 - 1`. ([`integer_bit_index.rb`](crates/rubyrs/tests/diff/integer_bit_index.rb))
  - **A6c — `Class#instance_method` for primitive classes** — synthesises an UnboundMethod for the 14 well-known primitive classes instead of raising NameError; user classes still raise on unknown methods. ([`class_instance_method_primitive.rb`](crates/rubyrs/tests/diff/class_instance_method_primitive.rb))
  - **A6d — msgpack `lib/msgpack/bigint.rb` round-trip** — vendored upstream (Apache-2.0, unmodified), exercised via `cext_msgpack_bigint`, 8 cases byte-identical to MRI. Skipped: `i64::MIN` (magnitude overflow). Test is Rust-integration because CRuby uses nested-module lookup; rubyrs Tier 1 flattens nested modules (deferred per ADR 0015 Tier 2).
  - **Small dependencies along the way** — `Array#shift`/`#pop`/`#reverse_each`, `nil.to_i`/`nil.to_f`.
- **cext spike: msgpack ext-type chain (L3-J / L3-K / A3 / A4)** — closes the "ship a custom Ruby class through msgpack's `register_type`" use case end-to-end.
  - **L3-J — `Symbol` crosses the cext FFI** — real `rb_id2sym`/`rb_sym2id`/`rb_sym2str` over the intern table (was stubbed); `rb_value_type` returns `T_SYMBOL`. `cext_msgpack_symbol` round-trips 5 Symbols byte-identical to MRI.
  - **L3-K — Proc/Block crosses the cext FFI** — forwards through `rb_funcallv(proc, :call, …)`. Fixed three pre-existing gaps: `OBJ_FROZEN` hard-coded to `1` (everything looked frozen), variadic `rb_ary_new3` returning empty (stable Rust can't take extern-C variadics; replaced with arity-specialised helpers), and linker stripping the new helpers (needed `#[used]` statics). `cext_msgpack_proc` verifies callback through cext → Vm.
  - **A3 — Class handle dedup against sentinels** — interns `Value::Class(name)` to the seeded sentinel handle when it matches the 21-slot prelude, so msgpack's `ext_module == rb_cSymbol` branch fires and Symbols pack as ext-type `0x00` (was fixstr). `cext_msgpack_symbol_ext` matches MRI byte-for-byte; user classes still intern fresh.
  - **A4 — application-defined ext-types** — pins `register_type_internal` for user classes; `cext_msgpack_app_ext` round-trips a `Color` (ext `0x10`) and `Stamp` (ext `-1`, Time's reserved id), plus mixed-frame coexistence. Real `Time` support is a separate subset addition (same Proc shape applies when it lands).

  Net: 20 cext tests across 13 files green; perf within 35–50% headroom; Miri (SB + TB) clean.
- **Method / UnboundMethod / Proc reflection chain** ([ADR 0016](docs/adr/0016-method-reflection-chain.md)) — full surface for captured-method objects, in atomic commits:
  - **`Method#unbind` / `UnboundMethod#bind(obj)`** — strip/rebind receiver; bind checks `is_a?` (TypeError on mismatch); subclass instances bind fine. ([`unbound_method.rb`](crates/rubyrs/tests/diff/unbound_method.rb))
  - **`Method#arity` / `#parameters`** — CRuby's keyword arity rule and `[[kind, name], …]` pairs (`:req`/`:opt`/`:rest`/`:keyreq`/`:key`/`:keyrest`); builtins fall back to `arity == -1` / `[[:rest]]`. ([`method_introspect.rb`](crates/rubyrs/tests/diff/method_introspect.rb))
  - **`Method#==` / `UnboundMethod#==`** — receiver-identity + name; UnboundMethod compares resolved method by pointer so inherited methods compare equal. ([`method_equality.rb`](crates/rubyrs/tests/diff/method_equality.rb))
  - **`Method#>>` / `#<<` composition** — accepts `Method` or `Proc` on either side. ([`method_compose.rb`](crates/rubyrs/tests/diff/method_compose.rb))
  - **`Method#curry` / `Proc#curry`** — gathers args across `.call`/`.[]`/`.()` until arity is hit; reports `Proc`; explicit `m.curry(n)` honoured. ([`method_curry.rb`](crates/rubyrs/tests/diff/method_curry.rb), [`proc_curry_compose.rb`](crates/rubyrs/tests/diff/proc_curry_compose.rb))
  - **`Method#to_proc`** — explicit form of `&m` coercion. ([`method_to_proc_explicit.rb`](crates/rubyrs/tests/diff/method_to_proc_explicit.rb))
  - **`Class#instance_method(:sym)`** — direct UnboundMethod construction; NameError if absent from the chain. ([`class_instance_method.rb`](crates/rubyrs/tests/diff/class_instance_method.rb))
  - **`Method#owner` / `#receiver`** — owner is the defining class (not receiver's class) for inherited methods; UnboundMethod#receiver raises NoMethodError to match CRuby. ([`method_owner_receiver.rb`](crates/rubyrs/tests/diff/method_owner_receiver.rb))
  - **`Method#hash` + `#source_location`** — hash from receiver-identity + name so `==` Methods collide; `source_location` returns `[file, line]` for user methods, `nil` for builtins. ([`method_hash_source.rb`](crates/rubyrs/tests/diff/method_hash_source.rb))

  Also fixed a pre-existing toplevel-block slot collision where a lambda's param slot clobbered an outer local (block builders now propagate `n_locals` to the parent).
- **SUBSET-roadmap fill-ins** — a batch of small atomic additions plugging high-priority SUBSET gaps:
  - **`Integer#digits([base])` / `#bit_length`** — LSB-first digit Array (base ≥ 2); two's-complement bit_length (`-1` → `0`, `-256` → `8`).
  - **`String#squeeze([charset])`** — collapses consecutive identical chars. Char-set ranges (`"a-z"`) and `^`-negation NOT expanded, same as `tr` (SUBSET.md).
  - **`String#scan` regex + block form** — adds Regex patterns with CRuby's capture-group rule; block yields each match and returns the receiver.
  - **`Enumerable#chunk_while`** — partition into runs by an adjacent-pair block; returns a materialised Array (no Enumerator type).
  - **`Enumerable#min_by(n)` / `#max_by(n)`** — top-n via sort+truncate; edges match CRuby (`n=0 → []`, `n > len → all`, `n<0 → ArgumentError`).
  - **`String#center` / `#ljust` / `#rjust`** — pad to width (default `" "`, cycles when multichar); odd-total center puts the extra char right; empty pad raises ArgumentError.
  - **`Array#bsearch`** — block-form binary search in both CRuby modes (find-minimum, find-any); other block returns raise TypeError.
  - **`Hash#transform_keys` / `#transform_values`** — non-mutating; key collisions follow CRuby's later-wins order.
  - **`Hash#except` / `#slice`** — `except` drops keys in receiver order; `slice` keeps keys in ARGUMENT order (CRuby semantics).
  - **`Array#take_while` / `#drop_while`** — `drop_while` stops at the first falsy return and keeps the remainder (block not re-invoked).
  - **`Array#tally`** — counts into a Hash by first appearance. Divergence: uses `==` (collapses `1`/`1.0`) where CRuby uses `eql?`; `tally_by` (Ruby proposal #16504) not shipped.
  - **`Comparable#clamp(Range)`** — Range-arg form with one-sided nil bounds; 2-arg form still works. Numeric primitives don't include Comparable yet, so the fixture uses user classes.
  - **`Float#round(n)` / `#truncate(n)`** — precision-arg forms (`n>0` → Float, `n==0`/`n<0` → Int); ordered before the Float coercion arm to avoid shadowing.
  - **`Hash#compact` / `#compact!` + `Array#filter_map` / `Hash#filter_map`** — `compact!` returns `nil` when unchanged; `filter_map` uses strict truthiness; `Hash#filter_map` collects into a flat Array.
  - **`Array#combination(n)` / `#permutation([n])`** — lexicographic; permutation defaults to full length; edges `n=0 → [[]]`, `n > len → []`.
  - **`Array#assoc` / `#rassoc`** — first sub-Array matching on `[0]`/`[1]`; non-Array elements skipped.
  - **`Range#cover?(Range)` + `Range#step` block form** — `cover?` true iff fully contained (empty sub-ranges don't cover); `step` block-form yields each value and returns the receiver.
  - **`Object#methods` / `#instance_variables`** — walks the user-class chain returning Symbols; primitives return `[]` (no per-Kernel enumeration — documented divergence).
  - **`String#encode` / `#force_encoding` (stubs)** — no-ops returning the receiver since the subset has no per-string encoding tag (raw `Vec<u8>` backing since [#53](https://github.com/linyiru/rubyrs/pull/53)); transliteration out of scope.
  - **`String#unpack` + `Array#pack` (subset)** — directives `C`/`c`, `n`/`N`, `v`/`V`, `q`/`Q`, `a`/`A`/`Z`; counts and `*` honoured, whitespace ignored, unsupported directives raise ArgumentError. `String#bytes` shipped alongside.

  Net (Method-reflection wave + SUBSET fill-ins): ~33 atomic commits, 134 byte-identical fixtures; full Miri (Stacked + Tree Borrows) and perf baseline clean throughout.
- **`String#sub` / `#gsub` / `#tr`** — literal-pattern forms with CRuby's `tr` stretch/delete rule; honours `max_value_bytes`. Regex forms and `tr` char-ranges deferred (SUBSET.md). ([`string_transform.rb`](crates/rubyrs/tests/diff/string_transform.rb))
- **`<=>` spaceship** — ordering (`-1`/`0`/`1`) across Int/Float/String/Symbol/Bool/nil with numeric coercion; user-defined `<=>` dispatched normally; non-comparable and cross-type pairs return `nil`. ([`spaceship.rb`](crates/rubyrs/tests/diff/spaceship.rb))
- **`attr_accessor` / `attr_reader` / `attr_writer`** — compile-time desugar to getter/setter `def`s; the setter returns the assigned value; composes with inheritance and default args. ([`attr_accessor.rb`](crates/rubyrs/tests/diff/attr_accessor.rb))
- **Universal `!` / `!@`** — `!recv` is `true` iff `recv` is `nil` or `false`, for any receiver type.
- **`Float` type (MVP)** — `f64` literals, arithmetic, Int/Float coercion ("Float wins"), cross-numeric `==`, plus conversions and predicates (`nan?` / `finite?` / `infinite?` / `floor` / `ceil` / `round`). Scientific notation (≥ `1e16`, `< 1e-3`) diverges from CRuby's formatter (SUBSET.md). ([`float_basics.rb`](crates/rubyrs/tests/diff/float_basics.rb))
- **`Object#class`, `Class#name` / `#to_s` / `#==` / `#!=`** — `class` works on every receiver via preamble stub classes; `Class` identity is `Rc::ptr_eq` (reopened classes share their `Rc`). Unblocks `e.class.name == "MyError"` and `obj.class == MyClass`. ([`object_class.rb`](crates/rubyrs/tests/diff/object_class.rb))
- **`Object#respond_to?(name)`** — duck-typed feature detection (Symbol or String arg); walks the class chain for user objects, an enumerated list for built-ins. ([`respond_to.rb`](crates/rubyrs/tests/diff/respond_to.rb))
- **Default method arguments** (literal defaults) — `def foo(x, y = 1, msg = "hello")` compiles; defaults restricted to literal Values (`Int`/`String`/`true`/`false`/`nil`), other shapes surface a `SyntaxError` Trap at translation time rather than miscompiling. Unblocks Gemfile-style DSL methods and `def initialize(x, y = nil)`. ([`default_args.rb`](crates/rubyrs/tests/diff/default_args.rb))
- **`Object#nil?` returns `false` for every non-nil receiver** — catch-all arm matching CRuby; previously `"abc".nil?` raised NoMethodError.
- **`docs/SECURITY.md`** (P2-15) — trust model, semi-trusted-profile config recipe, attack surface per cap, and an explicit "hardening layer, not a sandbox" boundary (WebAssembly + `wasmtime` is the answer for untrusted code). Catalogues residual risks the caps don't cover; cross-linked from the README.
- **Per-value byte cap** (P2-14c) — new `Config::max_value_bytes`; individual String/Array/Hash values can't grow past `n` bytes, checked at the mutation points (`String#+`/`#*`, `Array#push`/`#<<`/`#[]=`, `Hash#[]=` for new keys). Closes the `"a" * 10_000_000` and `arr << i`-loop RAM-hog vectors. CLI: `RUBYRS_MAX_VALUE_BYTES=N`.
- **Interner cap** (P2-14b) — new `Config::max_symbols`; runtime intern paths trap with `ResourceExhausted` past `N` fresh symbols (re-interning existing strings is free; compile-time intern uncapped). CLI: `RUBYRS_MAX_SYMBOLS=N`; `Runtime::symbol_count()` helps size it against the preamble baseline.
- **Wall-clock deadline cap** (P2-14a) — new `Config::deadline`; `eval` traps with `ResourceExhausted` past the budget. Amortised (checked every 1024 ops, no `Instant::now()` in the no-deadline case) and per-`eval` (re-anchored each call). CLI: `RUBYRS_DEADLINE_MS=N`.
- **`rescue ClassName => e` (class-filtered) + multiple `rescue` clauses** (P1-10) — the unwinder pops past handlers whose class filter doesn't match the raised exception's class chain; clauses checked in source order; bare `rescue` still filters on `StandardError`. `raise SomeError` and `raise SomeError, "msg"` supported (the latter desugars to `SomeError.new("msg")` so `initialize` runs). ([`rescue_by_class.rb`](crates/rubyrs/tests/diff/rescue_by_class.rb))
- **`docs/SUBSET.md § Divergences`** — documents intentional divergences (unresolved class in `rescue`, `ResourceExhausted` un-catchability, single-class-only in multi-class rescue, `Foo::Bar` falling back to the trailing segment), each pinned by a test.
- **`docs/PANIC_AUDIT.md`** (P0-4) — classifies every `panic!`/`.unwrap()`/`.expect(…)` into ICE / ICE-but-fuzzy / user-reachable buckets; current totals all 🟢 or 🟡.
- **CI `panic-budget` job** (P0-5) — counts panics per file and fails if any rises above the `PANIC_AUDIT.md` threshold (doc-comments excluded); budgets ratchet down only.
- **Hash extras + short-circuit `||` and `&&`** (P3-B-3) — new Hash methods `merge` / `to_h` / `to_a` / `delete` / `invert` / `store` (+ `each` aliases `each_pair`); short-circuit `OrNode`/`AndNode`; sort comparator now orders Symbols lexicographically so `hash.keys.sort` works on symbol-keyed hashes. ([`hash_extras.rb`](crates/rubyrs/tests/diff/hash_extras.rb))
- **Array combination & iteration extras** (P3-B-2) — no-block `reverse` / `uniq` / `compact` / `flatten` (depth 1) / `join` / `+` / `-` / `concat` / `take` / `drop` / `to_a`; block-taking `each_with_index` and `sort_by`; adds `Integer#-@`/`+@` so `arr.sort_by { |n| -n }` works. ([`array_extras.rb`](crates/rubyrs/tests/diff/array_extras.rb))
- **Integer predicates + iteration + String basics** (P3-B-1) — `Integer`: `even?`/`odd?`/`abs`/`zero?`/`positive?`/`negative?`/`succ`/`next`/`pred` + block `upto`/`downto`; `String`: `length`/`size`/`empty?`/`upcase`/`downcase`/`reverse`/`strip` family/`include?`/`start_with?`/`end_with?`/`to_i` (CRuby-lenient)/`*`/lexicographic comparisons/`chars`/`split`/`to_sym`. ([`int_string_basics.rb`](crates/rubyrs/tests/diff/int_string_basics.rb))
- **Enumerable aggregation: `inject`/`reduce`, `sum`, `count`, `min`/`max`, `sort`** (P3-A-3) — all three `inject`/`reduce` shapes (block-only, block+init, symbol-shorthand); `sum` optional init; `count` no-arg/needle/block forms; `min`/`max`/`sort` no-comparator on homogeneous Int/String arrays (comparator forms deferred); `Range#sum` uses the closed form. ([`enumerable_aggregate.rb`](crates/rubyrs/tests/diff/enumerable_aggregate.rb))
- **Enumerable filtering: `select`/`reject`/`find`/`any?`/`all?`/`none?`/`include?`** across Array, Hash, Range (P3-A-2) — aliases `filter`/`detect`/`has_key?`/`key?`/`member?`; `Hash#find` returns `[k, v]`; empty-collection cases preserve vacuous-truth semantics. ([`enumerable_filter.rb`](crates/rubyrs/tests/diff/enumerable_filter.rb))
- **`Range` values + `Range#each` + Range basics** (P3-A-1) — `1..5` / `1...5`, new `Value::Range` (GC-walked). Integer-endpoint ranges support `each`/`map`/`to_a`/`size`/`first`/`last`/`min`/`max` (respects `exclude_end?`)/`include?`/`exclude_end?`. Non-Int endpoints out of scope (fall through to NoMethodError). ([`range_basics.rb`](crates/rubyrs/tests/diff/range_basics.rb))
- **Brewfile DSL demo + benchmark** (P2-A) — `examples/brewfile/`, a 50-line Brewfile-shaped script hosted via `Runtime::register_fn`. Headline: **42× faster end-to-end** (1.8 ms vs CRuby 3.4's ~75 ms; YJIT doesn't help since CRuby's time is startup). README and `docs/BENCHMARKS.md` lead with this number.
- **`return` / `break` / `next`** (P2-C-4) — compile to frame-pop; `break` sets a flag that iteration drivers (`Array#each`/`#map`, `Hash#each`, `Integer#times`) consult, using the block's last value as the iterator return. ([`control_flow.rb`](crates/rubyrs/tests/diff/control_flow.rb))
- **`ensure` clause** (P2-C-3) — `begin … ensure … end` runs the ensure body on both normal-exit and exception paths; composes freely with `rescue`. ([`ensure_basics.rb`](crates/rubyrs/tests/diff/ensure_basics.rb))
- **`raise "msg"` auto-wraps to `RuntimeError.new("msg")`** — matches CRuby's Kernel#raise so `e.message` works; already-Exception instances pass through unchanged.
- **Built-in exception class hierarchy** (P2-C-2) — `Runtime::new` evals a preamble defining `Exception`/`StandardError`/`RuntimeError`/`NoMethodError`/`ArgumentError`/`TypeError`/`NameError`/`ResourceExhausted`, each descendant inheriting from the level above. Deliberately Ruby-at-the-Ruby-level so user `class MyErr < StandardError` Just Works. Known divergence: CRuby's `Exception#message` reads an internal mesg slot set by C-level `Exception.new`; rubyrs reads `@message`, so a user `initialize` override that sets `@message` is visible to `message` here but not in CRuby (documented; deferred until a use case demands parity). ([`custom_exception.rb`](crates/rubyrs/tests/diff/custom_exception.rb))
- **Class inheritance** (P2-C-1) — `class Foo < Bar` parsed; method and `initialize` lookup walk the superclass chain (so `Dog.new` invokes `Animal#initialize`). ([`inheritance.rb`](crates/rubyrs/tests/diff/inheritance.rb))
- **CRuby differential testing harness** (P2-B-1) — `tests/diff_cruby.rs` runs each `tests/diff/*.rb` under rubyrs and the system `ruby`, requiring byte-for-byte stdout; CI pins Ruby 3.4. Seeded with 10 fixtures; immediately caught and fixed a `ParenthesesNode` parser gap.
- **Resource caps for untrusted scripts** (P1-D) — `Config` gains `fuel` (per-op limit, enforced inside `dispatch_until`), `max_heap_objects` (live count, checked after `maybe_gc`), and `max_frames` (depth, checked before each push); hitting any returns `ResourceExhausted`. Defaults `None`; embedders running untrusted DSLs should set all three. CLI: `RUBYRS_FUEL` / `RUBYRS_MAX_OBJECTS` / `RUBYRS_MAX_FRAMES`. See [ADR 0008](docs/adr/0008-resource-caps-for-untrusted-scripts.md).
- **Host embedding API** (P1-C) — `src/lib.rs` exposes a public Rust API: `Runtime::new()`/`with_config`, incremental `eval`/`eval_file` (defs persist across calls), `register_fn`, `set_stdout`, `format_trap`. With `examples/embed.rs`, `tests/embed.rs`, and [ADR 0007](docs/adr/0007-host-embedding-api.md).
- **[ADR 0006](docs/adr/0006-global-string-intern.md)** — global string interner with SymId.
- **CRuby-style error format with backtrace** (P0-B-3) — Trap output prints `file:line:in 'method': msg (Class)` plus one `from …` line per frame, structurally matching CRuby. New `tests/fixtures/errors/` with `.expected_err` goldens (seeded `nomethod`/`wrong_args`/`yield_no_block`).
- **`STRESS_GC=1`** — forces a full collection on every potential GC point; wired into CI as a second job.
- **[ADR 0005](docs/adr/0005-pinned-stack-for-native-driven-loops.md)** — pinned stack for native-driven loops.
- **Symbol literal (`:foo`) + shorthand hash keys (`{name: "x"}`)**.
- **String interpolation** — `"hello #{name}"`, mixed with method calls and math.
- **`Nil#to_s` / `inspect` / `nil?`, `Bool#to_s`** — round out built-ins needed by interpolation.
- **GitHub Actions CI** — Linux + macOS, build + test on every push and PR.
- **LICENSE files** — dual MIT OR Apache-2.0.
- **Crate metadata in `Cargo.toml`** — description, license, repository.
- **`docs/` documentation, Architecture Decision Records (`docs/adr/`), `CHANGELOG.md` and `CONTRIBUTING.md`**.

### Changed
- **Wasm cold start via `wizer` pre-init** — snapshots a default Runtime (class registrations + preamble bytecode) at build time, applied with the host Config on each invocation; M-series cold start ~7.6→7.2 ms. Optional in `perf/wasm_check.sh` (graceful fallback when wizer absent).
- **Wasi env-var bypass** — `main.rs` reads `environ_get` directly via FFI instead of `std::env::vars()`, which returns empty on wizer'd builds because wizer snapshots wasi-libc's `__environ` before the C runtime populates it; the direct syscall makes env propagation work pre- and post-wizer.
- **Wasm perf gate measures AOT-precompiled `.cwasm`** — build prelude does `wasm-opt -Oz` (optional, needs `binaryen`) → `wasmtime compile`, eliminating per-run JIT; `startup_floor.rb` budget tightened 300→100 ms. The `.cwasm` is host-arch + wasmtime-version specific, so it is NOT a shipping artifact; raw-`.wasm` numbers stay in README/BENCHMARKS.md.
- **`Regexp` / `/pat/` literals are opt-in via the `regex` Cargo feature** — default install drops the `regex` crate (~300 KB + ReDoS surface), unsuited to the sandbox-host niche; with the feature off, regex literals raise a parse-time error pointing at the flag (same UX as `require` without `cext`). Embedders enable the feature or register a host fn. ([ADR 0017](docs/adr/0017-tier1-boundary.md) Rule 3, [#75](https://github.com/linyiru/rubyrs/pull/75))
- **`[profile.release] lto = "thin"`** — recovers the ~7% fizzbuzz regression from the CRuby-mirrored vm.rs module split (372→350 ms, within noise) by re-enabling cross-module inlining; release build +3s, dev/test unaffected.
- **`BlockHandle` now lives in the GC heap** (P2-13) — `Value::Block` is an `ObjId` and the collector marks captured/self values, putting blocks on the same mark/sweep footing as Array/Hash/Range. Eliminates a future Rc-cycle hazard for self-capturing blocks (e.g. `proc`/`lambda`, not yet in subset), so this is largely preventive; iterator paths exercise the new plumbing every test run. Regression test loops `[1,2,3].each` under stress-GC with a tight heap cap to prove reclamation.
- **GC mark walks children in place instead of cloning** (P0-3) — the mark phase previously cloned each Array/Hash/Instance.ivars on every visit, turning collection of one large container into quadratic work; now split-borrows slots vs marks and iterates by reference, same semantics. Win is on workloads with many or large containers; fizzbuzz unaffected (not GC-bound).
- **`Vm.pinned` managed by a `PinGuard` RAII type** (P0-2) — native iterator/aggregation drivers and the `Class.new` allocator used hand-rolled push/pop that could be skipped once `?` early-returns were added, slowly leaking pinned values into every GC cycle; `Drop` now pops exactly what was pinned on both success and unwind paths. A `debug_assert!` checks pinned-stack balance per call (skipped in release); regression test raises inside `[1,2,3].map` 50× under stress-GC.
- **Per-call-site method inline cache** (P1-B upgrade) — replaces the single-slot cache with a per-site cache (each call op carries a compile-time slot id), so polymorphic call sites dispatching on different classes no longer thrash each other; invalidated via a method generation counter. Monomorphic gains small (fizzbuzz 1M 327→322 ms, 1.72× vs CRuby); the structural win is polymorphic dispatch.
- **Statement-position avoids redundant `Dup`/`Pop`** (Tier1-5) — the compiler distinguishes a body's last expression from discarded intermediates, emitting stores/increments without the preceding `Dup`; fizzbuzz 1M 332→327 ms.
- **Right-literal integer binops fused** (Tier1-4) — a binary op with a literal-int RHS (`n % 15`, `i <= 1000000`, …) compiles to a single fused op, saving one dispatch and stack round-trip per expression; fizzbuzz 1M 364→332 ms (~9%), 1.80× vs CRuby (was 1.94×).
- **`@x = @x + 1` ivar increment fast path** (Tier1-3) — in-place increment of an Int ivar, falling back to a synthesised `+` call otherwise; Counter.inc × 1M 179→153 ms (~15%), within 1.42× of CRuby on dispatch-dominated workloads. Fizzbuzz unchanged (no ivars in hot path).
- **`i = i + 1` local increment fast path** (Tier1-2) — recognises the literal `+ 1` pattern and emits a single in-place op instead of a 5-op sequence, falling back to a synthesised `+` call on non-Int payloads so user types keep CRuby semantics; fizzbuzz 386→369 ms, Counter.inc × 1M 203→179 ms.
- **Single-slot inline method cache** (Tier1-1) — caches the last successful Object dispatch (class identity, method name, resolved method), skipping the method-table lookup on a matching repeat call; invalidated conservatively on method definition. Fizzbuzz 408→386 ms (~5%, BinOp-dominated limits reach); Counter.inc × 1M within 1.87× of CRuby.
- **`src/main.rs` shrinks to a ~20-line CLI wrapper around `Runtime`** — behaviour identical.
- **Global string interner** (P1-B) — method/ivar/class names and string literals live in one Vm-owned interner referenced by `SymId(u32)`, making symbol equality a single u32 compare and method dispatch hash on a tight key; 1M fizzbuzz 484→408 ms (1.18× faster), distance to CRuby+YJIT 3.44×→2.82×.
- **User errors no longer panic the host process** (P0-B-2) — undefined method, wrong arity, and `yield` outside a block now build a `Trap` that bubbles through every dispatch path, prints at exit, and returns a non-zero code; internal invariants (heap UAF, empty frame stack, stack underflow) stay `panic!` but are marked `"ICE: ..."`.

### Fixed
- **Block-local variables are now fresh per invocation** — a var first-assigned inside a `do…end`/`proc {}`/`lambda {}` body is reset to `nil` each call (was leaking across iterations/`.call`s, e.g. `proc { n ||= 0; n += 1 }.call` counted 1,2,3 instead of 1,1,1, and `y = 100 if cond` kept the prior iteration's `y` when `cond` was false). Outer-scope vars and block params keep their values. ([`block_local_freshness.rb`](crates/rubyrs/tests/diff/block_local_freshness.rb))
- **`String#inspect` control-character escapes match CRuby** — previously only `\\ \" \n \r \t \0` were escaped and other control bytes were emitted raw (`"\x00".inspect` gave `"\0"`; `\a \b \v \f \e` came out as raw control chars, garbling binary dumps). Now uses CRuby 3.4's full table: eight named escapes plus `\u00NN` (uppercase hex) for remaining low control bytes and `\x7F`. ([`string_inspect_control.rb`](crates/rubyrs/tests/diff/string_inspect_control.rb))
- **String literal high-byte preservation** — `\xNN` escapes producing non-UTF-8 bytes (e.g. `"\xFF\xFF"`) were each replaced with U+FFFD at translation time, ballooning a 2-byte literal to 6 bytes and corrupting binary-protocol parsers. Invalid bytes are now preserved verbatim. SUBSET note: `String#length` on a high-byte literal still counts U+FFFD-replaced chars because rubyrs doesn't model String encoding tags (this fixes byte preservation, not the per-char length semantic). ([`string_high_byte_literal.rb`](crates/rubyrs/tests/diff/string_high_byte_literal.rb))
- **GC rooting holes around `maybe_gc` (6 latent sites)** — a heap-bearing value held only in a Rust local across a collection could be swept and its slot reused (use-after-free), surfacing as a `class_of` ICE or a self-referential-slot stack overflow in inspect; sites included `Object#method`/`invoke_block` rest-slot, `Array#combination`/`Array#permutation`/`String#scan` accumulators, and `UnboundMethod#bind`. All wrapped in `PinGuard`. ([#90](https://github.com/linyiru/rubyrs/issues/90); repros [`proc_curry_compose.rb`](crates/rubyrs/tests/diff/proc_curry_compose.rb), [`array_combinatorics.rb`](crates/rubyrs/tests/diff/array_combinatorics.rb), [`string_scan.rb`](crates/rubyrs/tests/diff/string_scan.rb), [`kernel_instance_method.rb`](crates/rubyrs/tests/diff/kernel_instance_method.rb))
- **CI unbreak — clippy, panic budget, wasm dead_code** — fixed two `doc_lazy_continuation` lint regressions, ratcheted four per-file panic budgets up for new ICE-class invariant asserts, and gated `Vm.loaded_features` behind `cfg(not(target_os = "wasi"))` (dead code there since `require` traps).
- **Integer literals no longer truncate to i32** — literals past ~2.1 billion (decimal or hex) silently became `0` (e.g. `0x0102030405060708`, `72623859790382856`); now parsed as full i64. SUBSET note: values beyond i64 saturate to `i64::MIN`/`i64::MAX` (no BigInt promotion, per SUBSET.md). ([`integer_literal_i64.rb`](crates/rubyrs/tests/diff/integer_literal_i64.rb))
- **`return` from inside a block now correctly exits the enclosing method** — every `return` previously compiled as non-local, so a `return value` in a helper called from a block escaped out through the block to the helper's caller instead of just exiting the helper. Method-body `return` is now local while block-body `return` stays non-local. A stale "Divergences" entry in `docs/SUBSET.md` should be removed in follow-up. ([`nonlocal_return.rb`](crates/rubyrs/tests/diff/nonlocal_return.rb))
- **Cross-type `==` / `!=` no longer raises NoMethodError** — `"x" == nil`, `5 == "5"`, `[] == ""` etc. crashed with `undefined method '=='`; cross-type compares now follow `Object#==` identity (false for mismatched types). Side benefit: `Hash == Hash` (order-insensitive, O(n*m)) and `Range == Range` (begin/end/exclusive) now work. ([`cross_type_eq.rb`](crates/rubyrs/tests/diff/cross_type_eq.rb))
- **Uncaught Ruby exceptions no longer kill the host process** — an unmatched exception called `process::exit(1)`, fatal for embedders; uncaught exceptions now surface as `RubyError::Uncaught { class_name, message }` through the normal `Trap` path so hosts can log/retry/continue, with output formatted to match CRuby's `(ClassName)` tag. The CLI still prints and exits. Closes the largest residual attack surface in `docs/SECURITY.md`.
- **`eval` no longer inherits leftover dispatch state from a previous Trap** — a prior `eval` that ended in a Trap (uncaught exception, fuel exhaustion, deadline) left frames, operand-stack residue, and pins on the `Vm`, so the next call could fall back into abandoned frames and run stale bytecode. Dispatch state is now cleared at the start of every call; classes and the heap still persist (the embedding contract).
- **ADR 0008 retraction** — the earlier draft promised `rescue Exception => e` would catch `ResourceExhausted` after P1-10; it doesn't and shouldn't, because the resource trap is a host-level `Trap`, not a Ruby-level `raise`, so it bypasses unwinding entirely. The ADR and a matching test now lock the actual contract in.
- **Unsupported AST nodes return `SyntaxError` instead of panicking** (P0-4) — any Prism node outside the supported subset (e.g. `case/when`, regex literals, lambdas) used to `panic!`, tearing down the host process; translation now records the error and `eval` returns a `RubyError::SyntaxError` Trap, closing a denial-of-service surface for evaluating arbitrary third-party Ruby.
- **`ResourceExhausted` can no longer be swallowed by `rescue => e`** (P0-1) — it had subclassed `StandardError`, so a bare `rescue` could silently catch the kill switch and keep burning fuel/heap. It's now re-rooted directly under `Exception` (alongside `SystemExit`/`Interrupt`), and rescue handlers now carry a `StandardError` filter class so non-matching exceptions are skipped while `ensure` still runs unconditionally. ADR 0008 updated.
- **GC root hole in `Hash#to_a`** — each `[k, v]` pair is a fresh heap Array accumulated in a Rust-local Vec across `maybe_gc`; under stress-GC earlier pairs were swept and reused, yielding a self-referential Array that blew the stack in display. Source Hash and pairs are now pinned for the alloc window.
- **GC root hole during `Class.new` arg drain** — `Class.new(args...)` popped `args` into a Rust local before `maybe_gc`, so under stress-GC a heap value in `args` could be swept and the new Instance reuse its slot (leaving `args` pointing at the new Instance). `args` is now pinned around the alloc window.
- **Block param shadows outer scope correctly** — when a block param's name collided with an enclosing local (e.g. outer `x`, then `each { |x| ... }`), the block reused the outer slot and read garbage; block params now always allocate fresh slots, matching CRuby's block-local-variable semantics.
- **GC root hole in native-driven iterators** (P0-A) — `Array#map`, `Array#each`, and `Hash#each` accumulated state in Rust-local Vecs invisible to the mark phase, so a large enough `map` could read use-after-free objects; they now use an explicit `Vm.pinned` root list, exercised under `STRESS_GC=1` in CI.

### Internal
- **`msgpack/bigint.rb` round-trip becomes a `diff_cruby` fixture** — PR #89's lexical constant scoping (dual-write into bare and prefixed keys) closed the nested-module namespace gap, so `MessagePack::Bigint.to_msgpack_ext` now resolves through its proper nested path and the Rust integration-test workaround retires. New fixture asserts the same 8 i64-range cases against CRuby; first concrete downstream simplification of #89. ([`cext_msgpack_bigint.rb`](crates/rubyrs/tests/diff/cext_msgpack_bigint.rb), [#89](https://github.com/linyiru/rubyrs/pull/89))
- **GC rooting lint gate** — `scripts/lint-gc-rooting.sh` scans every `vm/*.rs` for the dangerous `maybe_gc` + `heap.alloc` + unrooted-Value-drain shape and fails CI (ahead of clippy) unless a `PinGuard` or an inline `// allow: gc-rooting` annotation is present, after 8 structurally-identical incidents. Also lands the site-#8 fix (`Kernel#Array(other)`, `args[0]` unrooted across `maybe_gc`, ICE under `STRESS_GC=1`) with a reproducing fixture. ([#90](https://github.com/linyiru/rubyrs/issues/90), [`kernel_array_coerce.rb`](crates/rubyrs/tests/diff/kernel_array_coerce.rb))
- **ADR 0017 — Tier-1 boundary specification** — concrete contract for what is / isn't in Tier 1, with four inclusion rules (determinism, no OS capabilities, no regex, no OS threads), an out-of-Tier-1 table, and a "current deviations" tracker (the regex deviation was closed by PR #86); SUBSET.md cross-links it as the formal contract. ([ADR 0017](docs/adr/0017-tier1-boundary.md))
- **Centralised cext-bundle build helper** — new `tests/common/mod.rs` exposes `build_cext_bundle` + a `RUBY_DLEXT` const, removing inlined `bash build.sh` / existence-check / platform-suffix logic from 12 cext integration tests; `msgpack-cext` and `counter-cext` build scripts gained `flock` + atomic-rename serialisation against racing parallel `cargo test`.
- **`rubyrs-spec-extract` v0.3** — adds 3 extractor-derived specs (+12 examples) for `Array#compact`, `Array#take`, `Hash#keys`, and tightens extraction-context heuristics and skip-log messages.
- **CRuby-mirrored `vm.rs` split** — the 6593-line `vm.rs` split into per-type `vm/*.rs` submodules mirroring CRuby's file layout (`string.c`→`vm/string.rs`, …), behaviour-preserving; `vm.rs` drops to ~440 lines after follow-up extractions. Cross-module boundaries cost ~7% on the fizzbuzz microbench, recovered by switching `[profile.release]` to `lto = "thin"`.
- **Error / Span infrastructure** (P0-B-1) — new `src/error.rs` adds `Span`, `RubyError` (closed error set), `TrapFrame`, and `Trap`; `Expr` becomes `Spanned<Expr>` and `Proto` carries op-spans + filename, wiring spans Prism → Expr → Op without changing observable behaviour ahead of the panic→Trap migration.
- **Module split** (P1-A) — `src/main.rs` (1600 lines) split into `ast.rs`, `value.rs`, `heap.rs`, `bytecode.rs`, `compiler.rs`, `vm.rs` plus a 55-line CLI `main.rs`; move-only, setting up the seam for the upcoming `lib.rs` / embedding API (P1-C).
- **`Op` / `BinOpKind` derive `Copy`** (P0-C) — the dispatch loop's `code[ip].clone()` becomes a plain `Copy`; a structural correctness change (LLVM already elided the clone since all payloads were POD). Future `Op` variants must stay POD or carry an index instead of an `Rc<str>`.
- **Specialised `Op::BinOp(BinOpKind)`** for `+ - * / % == != < <= > >=` — Int+Int fast path avoids generic method dispatch.
- **1M-fizzbuzz** — 0.67 s → 0.44 s (2.3× of CRuby's interpreter).

## [0.0.x — development]

Initial PoC and milestones leading up to this point. All work pre-tag is
in the commit log; the changelog is canonical from here forward.

[unreleased]: https://github.com/linyiru/rubyrs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/linyiru/rubyrs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/linyiru/rubyrs/releases/tag/v0.1.0
