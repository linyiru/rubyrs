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

## Near term (next ~10 commits)

In rough order of ROI for getting out of "toy" status:

1. **`Range`** (`1..10`, `1...10`, `each`, `to_a`, `include?`, `each_with_index`)
2. **More `Enumerable`**: `select`, `reject`, `inject`/`reduce`, `find`,
   `any?`, `all?`, `include?`, `count`, `sort`, `sort_by`
3. **`String` methods**: `split`, `gsub`, `sub`, `chomp`, `strip`, `upcase`,
   `downcase`, `chars`, `start_with?`, `end_with?`
4. **`Module` + `include`** — at minimum enough to define and mix
   `Enumerable` once
5. **Class inheritance + `super`** — `class Foo < Bar` and method override
6. **`attr_reader / writer / accessor`** as built-in macros
7. **Method dispatch inline cache** (per-call-site monomorphic IC)
8. **spec_extract v0.1** + first SPEC_STATUS.md
9. **Exception class hierarchy + `ensure`**
10. **`return / break / next`** — proper non-local exits

By the end of this list, the language is past "toy" — most short Ruby
programs work, spec coverage is publishable, and there's a feedback loop
that grows by itself.

## Medium term

- **`Float` + mixed numeric arithmetic** with promotion rules
- **`Proc` and `lambda`** with `&block` parameter passing
- **Pattern matching** (`case ... in`) — the modern way
- **WASM packaging story**: published as a `wasi-component`, with a small
  JS binding example for browsers
- **Embedding API**: `librubyrs` C-ABI and Rust API for hosts to call into
- **Better `String`**: byte-aware indexing, basic encoding tag

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
