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

## Tooling

- **`rubyrs-gapscan`** — workspace crate that scans a Ruby codebase
  with Prism and classifies every AST node as Supported / RidesAlong /
  Missing against the rubyrs subset. Three layers of compile-time
  drift protection keep its classification in lockstep with `ast.rs`
  and Prism's node universe. Used to produce the gap reports below
  and to measure the delta after each gap-closing feature PR.
  See `crates/rubyrs-gapscan/`.

## Real-world gap data

Snapshots of `rubyrs-gapscan` against canonical Ruby projects —
these drive which "Near term" items below to attack first. Full
reports: [`docs/gap-reports/`](gap-reports/README.md).

| Codebase | Shape | % Supported (AST) | #1 missing |
|---|---|---:|---|
| Jekyll `lib/` | static-site framework | **84.03%** | `ConstantWriteNode` (×70) |
| Liquid `lib/` | template engine | **83.07%** | `ConstantWriteNode` (×141) |
| Sinatra `lib/` | web DSL | **82.46%** | `BlockParameterNode` (×58) |
| dry-struct `lib/` | modern data DSL | **82.38%** | `BlockParameterNode` (×11) |
| Rake `lib/` | task DSL | **83.30%** | `RegularExpressionNode` (×42) |

Tier 2 (Bundler / Tilt / stdlib slice) has now been scanned too —
see [gap-reports/README.md](gap-reports/README.md) for the
combined n=10 table.

**Headline:** all ten scans sit at 80.7–85.3% Supported regardless
of codebase shape. Bundler tops the list at **85.25%** despite
being by far the largest target (225 files, 106k AST nodes).
The previous #1 blocker `ModuleNode` (which dominated 3/5 scans
at PR #7 baseline) is now Supported on master — biggest single
feature impact gapscan has measured. Today's #1 missing class is
diverse: `ConstantWriteNode` ×3 (Jekyll, Liquid, stdlib URI),
`BlockParameterNode` ×3 (Sinatra, dry-struct, Tilt),
`KeywordHashNode` ×1 (Bundler — but ×430 occurrences),
`RegularExpressionNode` ×1 (Rake), `AliasMethodNode` ×1
(stdlib set), `RestParameterNode` ×1 (stdlib optparse). The
**block-parameter family** (`BlockParameterNode`,
`RestParameterNode`, `SplatNode`) is the broadest remaining
theme on DSL-shaped codebases; `KeywordHashNode` is the new
surprise top contender (Bundler — direct relevance to `rubund`).

Master moved the band up 1.9–2.6 pp from the PR #7 baseline
(79–82%) by landing a feature batch (`unless`/`until`, `**`, more
`Hash`/`String`/`Float` methods, `Kernel#p`, visibility,
op-assigns, `Range`/`Comparable`/`String#match`, inline `rescue`
modifier, `__method__`, `String#[]=`, Inspect round-out,
`&:method_name` symbol-to-proc, `case`/`when` with
`Range#===` / `Class#===`, **`Module` + `extend`**,
`Kernel#Integer`/`Float`/`String`). The two highest-impact landings
were `Module` (Jekyll −145, Rake −52, Liquid −74) and
`&:method_name` (Sinatra +0.81 pp because `BlockArgumentNode` was
its #1 blocker). Per-feature impact is now measurable via
`gapscan diff` — see the gap-reports README.

Caveat: the AST view *under*-states the gap — many runtime
features (`require`, `attr_accessor`, `include`, `private`) parse
as `CallNode` and so look Supported. See each report's "Top
bareword calls" section for the semantic-gap view.

Cross-scan observations and a deeper breakdown live in
[`docs/gap-reports/README.md`](gap-reports/README.md).
Candidate codebases for future scans are tracked in
[`docs/gap-reports/TARGETS.md`](gap-reports/TARGETS.md).

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

## Metaprogramming: known unknowns

Many real Ruby libraries lean heavily on metaprogramming —
`method_missing`, `define_method`, `instance_eval`, `send` with
non-literal symbols, `class_eval`, `Module.new { ... }`,
`ObjectSpace`. rubyrs supports **none** of these today, and
[SUBSET.md](SUBSET.md) currently lists most as "explicitly out of
scope". That language is too strong for our actual position — it's
"out of scope *for v1*", not "we will never look at this". This
section records what we do plan to do, eventually, and why we
haven't yet.

### Why rubyrs doesn't have it yet

- **Inline-cache hostile.** rubyrs's perf story (and roadmap item
  for monomorphic method dispatch IC) assumes class methods are
  fixed at compile time. `method_missing` and `define_method` add
  an unpredictable layer that defeats simple caches; doing it well
  needs the deopt machinery a JIT would have.
- **Embedding niche doesn't need it.** Brewfile, Gemfile, and most
  DSL files don't use it. The cost of implementing it would buy
  zero benefit for the current target use cases.
- **Cheap shims often suffice.** `attr_accessor` / `attr_reader` /
  `attr_writer` are the dominant real-world consumers of "define
  methods at class-load time"; implementing them as built-in
  macros (Near term #6) covers the common case without any
  general metaprogramming support.

### What we'd do, when the time comes

In rough order of effort vs. payoff. Each level subsumes the
previous — you'd implement them as a sequence, not in parallel.

1. **`attr_*` as built-in macros.** Already on Near term #6.
   Recognised at class-definition time, expand to compile-time
   method emission. Doesn't introduce any runtime metaprogramming
   surface. Closes a large fraction of the apparent gap surfaced
   by gapscan's bareword report (Jekyll: 32 `attr_reader`
   occurrences alone).
2. **`define_method :literal_name`.** When both receiver and name
   are static literals at compile time, lower to the same
   bytecode as `def`. No runtime support needed; the cache stays
   monomorphic.
3. **`send` / `public_send` with literal symbols.** Same shape as
   above: a sugar over direct dispatch when the symbol is a
   compile-time literal. Falls back to a trap otherwise.
4. **`Module.new { ... }` + `include`.** Once Near term #4
   (`Module` + `include`) lands, supporting anonymous modules is
   mostly already there.
5. **`method_missing` proper.** The hard one. Requires:
   (a) every method dispatch to check the class chain and fall to
   `method_missing` on miss; (b) the IC to handle the
   missing-then-found case via a deopt path; (c) `respond_to?`
   integration so libraries that probe for methods behave
   correctly. Probably the gating feature for "Run `mspec` inside
   rubyrs" in Long term, and the dividing line between "tiny
   embeddable runtime" and "general-purpose Ruby".
6. **`instance_eval` / `class_eval` with blocks.** Rebinds `self`
   inside a block scope; commonly used for DSLs (RSpec
   `describe`/`it`, Rake `task`). Builds on `Module.new` work.
   Plain string-eval (`eval "..."`) stays explicitly out of scope
   regardless — the WASM/embedding deployment story doesn't tolerate
   a runtime compiler.

### Decision gate

We'd start on (1) when an *embedding-niche* use case demands it
(several already do — see attr_* in the Jekyll bareword report).
We'd start on (5) only after a concrete user need — most likely
"running mspec natively" or "running a specific DSL library
that's worth the complexity budget". Until then, the position is
"document, don't build".

## Not on the roadmap (explicitly)

- Running Rails, Sinatra, or any meaningful gem
- A full JIT or AOT compiler
- `Fiber`, `Thread`, `Ractor`
- Reading `Gemfile`, `require`, gem ecosystem integration

These are good things; CRuby is good at them. rubyrs is good at something
else.
