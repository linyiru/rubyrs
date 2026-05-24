# Changelog

All notable changes to rubyrs will be recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/), and this project
follows [Semantic Versioning](https://semver.org/) once we hit 0.1.

## [Unreleased]

### Added
- **Built-in exception class hierarchy** (P2-C-2). `Runtime::new`
  now `eval`s a small preamble that defines `Exception`,
  `StandardError`, `RuntimeError`, `NoMethodError`,
  `ArgumentError`, `TypeError`, `NameError`, `ResourceExhausted`,
  with `Exception` providing `initialize`, `message`, `to_s`.
  Each `StandardError` descendant inherits from the level above.
  This is deliberately *Ruby code at the Ruby level* (no special-
  cased C structs), so user-defined `class MyErr < StandardError;
  end` Just Works and `raise MyErr.new("x"); rescue => e;
  puts e.message` produces the same output as CRuby.
- New diff fixture `custom_exception.rb` covers user-defined
  exception subclasses, `e.message` / `e.to_s`, raise-in-method
  + rescue-in-caller, and multiple sequential rescue blocks
  for different classes. Byte-identical to CRuby.

Known divergence: CRuby's `Exception#message` reads an internal
mesg slot set by the C-level Exception.new, not the `@message`
ivar. Our preamble-based implementation reads `@message`, so a
user override of `initialize` that sets `@message` directly is
visible to `message` in rubyrs but not in CRuby. Documented;
won't be fixed until we have a clear use case demanding parity.

### Added
- **Class inheritance** (P2-C-1). `class Foo < Bar` is now parsed
  and a `Class` stores its superclass (`Rc<Class>`). Method lookup
  walks the chain via the existing `lookup_method_cached` helper
  (now generalised), and `initialize` lookup during `Class.new` also
  uses the chain so `Dog.new("Rex")` invokes `Animal#initialize`
  when `Dog < Animal`. A `class_is_a` predicate is added for the
  rescue-by-class filter that lands next (kept `#[allow(dead_code)]`
  for now).
- New diff fixture `inheritance.rb` exercises three-level chain
  (Animal → Dog → Puppy) with inherited initialize, inherited
  instance methods, and class-own methods. Byte-identical to CRuby.

### Changed
- **Statement-position avoids redundant `Dup`/`Pop`** (Tier1-5).
  `compile_body` now distinguishes the last expression of a body
  (whose value is the body's value) from intermediate ones (whose
  value is discarded). Intermediate `Expr::LVarWrite` /
  `Expr::IVarWrite` emit `StoreLocal` / `StoreIvar` directly with
  no preceding `Dup`. The `Inc*` ops get matching `NoPush`
  variants emitted in stmt position. Microbench: fizzbuzz 1M 332
  ms → **327 ms**.
- **`Op::BinOpInt` fuses `LoadConstInt + BinOp`** (Tier1-4). For
  any binary op where the right-hand side is a literal integer
  (`n % 15`, `... == 0`, `i <= 1000000`, …), the compiler now
  emits a single `BinOpInt(kind, i64)` op instead of the previous
  pair. Each fused expression saves one dispatch and one stack
  round-trip. Microbench: fizzbuzz 1M 364 ms → **332 ms (~9%)**.
  rubyrs vs CRuby on fizzbuzz: **1.80×** (was 1.94×).
- **`Op::IncIvar` fast path for `@x = @x + 1`** (Tier1-3).
  Symmetric to Tier1-2 but on the receiver's ivar table. Hot
  path: in-place increment of the Int; slow path: synth a `+` call.
  Microbench: Counter.inc × 1M (the `@count = @count + 1` inside
  `inc`): 179 ms → **153 ms (~15%)**. rubyrs is now within
  **1.42×** of CRuby's interpreter on method-dispatch-dominated
  workloads. Fizzbuzz unchanged (no ivars in the hot path).
- **`Op::IncLocal` fast path for `i = i + 1`** (Tier1-2). The
  compiler now recognises the syntactic pattern
  `name = name + 1` (literal `+ 1`) and emits a single
  `IncLocal(slot)` op instead of the previous 5-op sequence
  (LoadLocal, LoadConstInt(1), BinOp::Add, Dup, StoreLocal).
  Read-modify-write happens in place against the slot. On
  non-Int payloads the op falls back to a synthesised
  `+`-call so user-defined types with their own `+` keep
  working (CRuby semantics preserved). Microbench: fizzbuzz
  386 ms → **369 ms**; Counter.inc × 1M loop 203 ms → **179 ms**
  (the outer `i = i + 1` benefits; the inner `@count = @count + 1`
  still goes through the generic path — IncIvar is a follow-up).
- **Single-slot inline method cache** (Tier1-1). `Vm.call_cache`
  holds the (class identity, method name, resolved `Method`) of
  the last successful `Value::Object` dispatch; subsequent calls
  with matching class + name skip the `HashMap<SymId, _>::get`.
  Invalidated conservatively on `Op::DefMethod`. Helper
  `Vm::lookup_method_cached` is shared across all four
  Object-dispatch sites (do_call no_recv, do_call with recv,
  do_call_block no_recv, do_call_block with recv).
  Microbench: fizzbuzz 408 ms → **386 ms** (~5%, BinOp-dominated
  workload limits the cache's reach). Counter.inc × 1M
  (method-dispatch-dominated): 203 ms vs CRuby's 108 ms — we
  are now within **1.87×** of CRuby's interpreter on this shape
  of workload.

### Added
- **CRuby differential testing harness** (P2-B-1). New
  `tests/diff_cruby.rs` runs each `tests/diff/*.rb` under both rubyrs
  and the system `ruby` binary; stdout must match byte-for-byte. CI
  pins Ruby 3.4 via `ruby/setup-ruby@v1` so the comparison is
  reproducible. Seeded with 10 fixtures (integer/string/array/hash/
  block/class/symbol/interpolation/rescue/fizzbuzz). Running the
  fixtures immediately caught a parser gap (`ParenthesesNode` was
  unsupported); fixed in the same commit.

### Added
- **Resource caps for untrusted scripts** (P1-D). `Config` gains
  three optional knobs: `fuel: Option<u64>` (per-op limit, enforced
  inside `dispatch_until` so block bodies can't bypass it),
  `max_heap_objects: Option<usize>` (live Instance/Array/Hash
  count, checked after `maybe_gc`), and `max_frames: Option<usize>`
  (frame stack depth, checked before every `frames.push`). Hitting
  any returns `RubyError::ResourceExhausted`. Defaults are `None`
  for backward-compat; embedders running untrusted DSLs should set
  all three.
- CLI env vars: `RUBYRS_FUEL`, `RUBYRS_MAX_OBJECTS`,
  `RUBYRS_MAX_FRAMES`.
- 5 new `tests/embed.rs` tests covering fuel exhaustion in a tight
  loop, fuel inside a block, unlimited-fuel happy path, heap-cap
  with retained allocations, and frame-cap with deep recursion.

### Added
- **Host embedding API** (P1-C). New `src/lib.rs` exposes `Runtime`,
  `Value`, `Trap`, `RubyError`, etc. as a public Rust API:
  - `Runtime::new()` / `with_config(Config)`
  - `rt.eval(source, filename)` and `rt.eval_file(path)` —
    incremental: class/method defs persist across calls
  - `rt.register_fn(name, |args| ...)` — host functions callable
    from Ruby
  - `rt.set_stdout(Box<dyn Write>)` — capture/redirect `puts`/`print`
  - `rt.format_trap(&trap)` — CRuby-style backtrace formatting
- `examples/embed.rs` demonstrating all four capabilities
- `tests/embed.rs` locking down the API surface (7 tests)
- ADR 0007: Host embedding API design

### Changed
- `src/main.rs` shrinks to a 20-line CLI wrapper around `Runtime`.
  Behaviour is identical to before.

### Changed
- **Global string interner** (P1-B). Method names, ivar names, class
  names, and string literals all live in a single Vm-owned `Interner`
  and are referenced by `SymId(u32)`. `Proto.strings` is gone;
  `Value::Sym` carries a `SymId` instead of `Rc<String>`;
  `Value::Str` carries `Rc<str>`. `Class.methods`, `Instance.ivars`,
  and `Vm.classes` are now keyed on `SymId`. Symbol equality is a
  single u32 compare; method dispatch hashes on a tight key.
  Microbench: 1M fizzbuzz 484 ms → **408 ms (1.18× faster)**;
  distance to CRuby + YJIT 3.44× → 2.82×.

### Added
- ADR 0006: Global string interner with SymId.

### Added
- **CRuby-style error format with backtrace** (P0-B-3). Trap output
  now prints `file:line:in 'method': msg (Class)` plus one
  `\tfrom file:line:in 'method'` line per frame, structurally
  matching CRuby. File and line resolve against the source via
  `error::line_col`.
- New `tests/fixtures/errors/` directory + `run_error_fixture()` in
  the integration harness. Each `.rb` has an `.expected_err` golden
  for stderr; the test expects a non-zero exit. Seeded with
  `nomethod`, `wrong_args`, `yield_no_block`.

### Changed
- **User errors no longer panic the host process** (P0-B-2). Undefined
  method, wrong arity, and `yield` outside a block now build a `Trap`
  that bubbles up through every dispatch path (`Result<_, Trap>`
  everywhere), is printed at process exit, and returns a non-zero
  exit code. Internal invariants (heap UAF, empty frame stack while
  dispatching, stack underflow) remain `panic!` but are now marked
  `"ICE: ..."` to make the distinction explicit when one fires.

### Internal
- **Error / Span infrastructure** (P0-B-1): new `src/error.rs` with
  `Span` (byte offset into source), `RubyError` (closed set covering
  SyntaxError / NoMethodError / ArgumentError / TypeError / RuntimeError
  / NameError), `TrapFrame`, and `Trap` (error + backtrace). `Expr`
  is now wrapped as `Spanned<Expr>` (alias `SExpr`); `Proto` gets a
  parallel `op_spans: Vec<Span>` and a `filename: Rc<str>`. The
  panic→Trap migration itself is the next commit; this one just
  wires the plumbing so spans flow from Prism → Expr → Op without
  changing observable behaviour.
- **Module split** (P1-A): `src/main.rs` (1600 lines) split into
  `ast.rs`, `value.rs`, `heap.rs`, `bytecode.rs`, `compiler.rs`,
  `vm.rs`, plus a 55-line CLI `main.rs`. Move-only refactor:
  fixtures emit stdout bit-identical to the pre-split binary. Sets
  up the seam for the upcoming `lib.rs` / embedding API (P1-C).
- `Op` and `BinOpKind` now derive `Copy` (P0-C). The dispatch loop's
  `code[ip].clone()` becomes a plain `Copy`. The previous `clone()` was
  already optimised away by LLVM (all payloads were already POD), so
  this is a structural correctness change rather than a measurable
  speedup. Future Op variants must remain POD or carry a `SymId` /
  similar index instead of an `Rc<str>`.

### Fixed
- **GC root hole in native-driven iterators** (P0-A). `Array#map`,
  `Array#each`, and `Hash#each` accumulated state in Rust-local `Vec`s
  that weren't visible to the mark phase; a sufficiently large `map`
  could read use-after-free objects. Now uses an explicit `Vm.pinned`
  root list. `STRESS_GC=1 cargo test` exercises this in CI.

### Added
- `STRESS_GC=1` env flag forces a full collection on every potential
  GC point. Wired into CI as a second job.
- ADR 0005: pinned stack for native-driven loops.
- Symbol literal (`:foo`) and shorthand hash key syntax (`{name: "x"}`)
- String interpolation: `"hello #{name}"`, mixed with method calls and math
- `Nil#to_s` / `inspect` / `nil?`, `Bool#to_s` to round out built-ins
  needed by interpolation
- GitHub Actions CI: Linux + macOS, build + test on every push and PR
- LICENSE files: dual MIT OR Apache-2.0
- Crate metadata in `Cargo.toml` (description, license, repository)
- `docs/` directory with structured project documentation
- Architecture Decision Records (`docs/adr/`)
- `CHANGELOG.md` and `CONTRIBUTING.md`

### Internal
- Specialised `Op::BinOp(BinOpKind)` for `+ - * / % == != < <= > >=` —
  Int+Int fast path avoids generic method dispatch
- 1M-fizzbuzz: 0.67 s → 0.44 s (2.3× of CRuby's interpreter)

## [0.0.x — development]

Initial PoC and milestones leading up to this point. All work pre-tag is
in the commit log; the changelog is canonical from here forward.
