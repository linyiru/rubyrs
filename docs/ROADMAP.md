# Roadmap

The roadmap is opinionated, not exhaustive. We work on what moves rubyrs
closest to its niche: **a fast-starting, memory-frugal, Rust-implemented
Ruby subset for embedded / WASM / edge use cases**. Anything that doesn't
serve that bar (e.g. JIT performance for long-running servers) is
explicitly out of scope — see [SUBSET.md](SUBSET.md).

## Quality bar: ruby/spec

Everything below is gated by **passing the relevant slice of `ruby/spec`**.
See [TESTING.md](TESTING.md) for the ingestion pipeline. We do not ship a
new feature without spec coverage that demonstrates we got the semantics
right.

## Done

These are landed and locked down by tests:

- **P0-A** Fix the GC root hole in native-driven iterators + `STRESS_GC=1`
  mode (commit `483f45d`)
- **P0-B** `panic!` → `Result<_, Trap>` with CRuby-style backtrace; new
  `tests/fixtures/errors/` golden harness (commits `d941d7a`–`cdc027f`)
- **P0-C** `Op` / `BinOpKind` derive `Copy` (commit `3fce237`)
- **P1-A** Single-file `main.rs` split into focused modules: `ast`,
  `value`, `heap`, `bytecode`, `compiler`, `vm`, plus `error`, `intern`,
  `lib` added later (commit `c37ded7`)
- **P1-B** Global `Interner` + `SymId`; method dispatch on u32 keys;
  `Value::Sym` becomes a `SymId`. Microbench fizzbuzz 484 ms → 408 ms
  (commit `dd7826c`)
- **P1-C** Host embedding API — `lib.rs`, `Runtime`, `register_fn`,
  `set_stdout`, `format_trap`. `tests/embed.rs` and `examples/embed.rs`
  pin the surface down (commit `9beded8`)
- **P1-D** Resource caps: `Config { fuel, max_heap_objects, max_frames }`.
  `RubyError::ResourceExhausted` covers all three. CLI env vars
  `RUBYRS_FUEL`, `RUBYRS_MAX_OBJECTS`, `RUBYRS_MAX_FRAMES` (commit
  `9172868`)

## Near term

In rough order of ROI for the embedding / DSL use case:

1. **`Range`** (`1..10`, `1...10`, `each`, `to_a`, `include?`,
   `each_with_index`)
2. **More `Enumerable`**: `select`, `reject`, `inject`/`reduce`, `find`,
   `any?`, `all?`, `include?`, `count`, `sort`, `sort_by`
3. **`String` methods**: `split`, `gsub`, `sub`, `chomp`, `strip`,
   `upcase`, `downcase`, `chars`, `start_with?`, `end_with?`
4. **`Module` + `include`** — at minimum enough to mix `Enumerable` once
5. **Class inheritance + `super`** — `class Foo < Bar` and method override
6. **`attr_reader / writer / accessor`** as built-in macros
7. **P2-A Pivot demo + benchmark**: pick a Ruby DSL (Brewfile leading
   candidate) and demonstrate it running on rubyrs.wasm under wasmtime
   with cold start + memory numbers vs CRuby and ruby.wasm. This is the
   *decision gate* for the embedding-niche thesis
8. **P2-B Spec ingestion v0.1** — `tools/spec_extract` from ruby/spec,
   first SPEC_STATUS.md report
9. **P2-C Exception class hierarchy + `ensure` + `return / break / next`**
10. **Method dispatch inline cache** (per-call-site monomorphic IC) — was
    P1-B in the original brief; deferred because `BinOp` fast path already
    skips the hot dispatch on integer ops. Becomes the bottleneck once
    class-method-heavy code dominates the benchmark mix

## Medium term

- **`Float` + mixed numeric arithmetic** with promotion rules
- **`Proc` and `lambda`** with `&block` parameter passing
- **Pattern matching** (`case ... in`)
- **WASM packaging story**: published as a `wasi-component`, with a small
  JS binding example for browsers
- **Better `String`**: byte-aware indexing, basic encoding tag
- **`HostCtx`** parameter on `register_fn` so host functions can allocate
  Arrays/Hashes that show up Ruby-side

## Long term

- **Run `mspec` inside rubyrs** — requires `method_missing` semantics or a
  carefully shimmed subset; enables running `ruby/spec` natively, the
  TruffleRuby / JRuby path
- **Inline ASM-level fast paths** for very hot operators (Cranelift?)
- **Concurrency story**: probably actor / message passing, not Threads
- **CRuby C-extension compat layer** — only if a clear use case appears.
  Mostly we expect to *not* do this; see [SUBSET.md](SUBSET.md).

## Not on the roadmap (explicitly)

- Running Rails, Sinatra, or any meaningful gem
- A full JIT or AOT compiler
- `Fiber`, `Thread`, `Ractor`
- Reading `Gemfile`, `require`, gem ecosystem integration

These are good things; CRuby is good at them. rubyrs is good at something
else.
