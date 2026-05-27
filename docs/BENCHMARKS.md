# Benchmarks

All numbers from an Apple Silicon (M-series) machine, `--release` builds.
Reproduce with the commands at the bottom.

## End-to-end DSL hosting (Brewfile demo)

The product-niche benchmark. A small Brewfile-shaped DSL (~50 lines
of Ruby: `tap "x"`, `brew "y"`, `cask "z"`, plus a class def and a
`.each` loop) is run end-to-end on each runtime. See
[`examples/brewfile/`](../examples/brewfile/) for the script and
host code.

| Runtime | Time (incl. process start) |
|---------|---:|
| rubyrs (embedded via Runtime API) | **1.8 ms** |
| CRuby 3.4 (no YJIT) | 74.7 ms |
| CRuby 3.4 + YJIT | 75.5 ms |

**rubyrs is 42.5× faster end-to-end** for this shape of workload.
YJIT doesn't help because almost all of CRuby's wall-time goes to
process startup and the gem loader, not arithmetic.

This is what rubyrs is built for: a host Rust app embedding a Ruby
DSL where script execution time is dwarfed by per-invocation
overhead.

## Cold start

Trivial program: `puts 1 + 2`. Time to first output.

| Implementation | Wall time |
|----------------|-----------|
| rubyrs (native) | **1.5 ms** |
| rubyrs.wasm via wasmtime (raw, JIT each run) | 12.7 ms |
| rubyrs.cwasm via wasmtime (AOT, `--allow-precompiled`) | **~7 ms** |
| CRuby 3.4 (no YJIT) | 77 ms |
| CRuby 3.4 + YJIT | 78 ms |

rubyrs is ~50× faster cold-start than CRuby. The two wasm rows
reflect different deployment shapes:
  * raw `.wasm` (12.7 ms) — what you ship to embedders, includes
    wasmtime's per-run JIT.
  * AOT `.cwasm` (~7 ms) — wasmtime compiles the module ahead of
    time (`wasmtime compile` then `wasmtime run
    --allow-precompiled`); the `.cwasm` is host-arch + wasmtime-
    version specific so it's generated per consumer, not shipped.
    `perf/wasm_check.sh` measures this shape.
This is the strongest signal for our target niche (CLI tools,
serverless / edge runtimes, embedded scripting).

## P2-A pivot: rubyrs.wasm vs ruby.wasm on a real DSL

The product-niche signal in WebAssembly shape. Both implementations
get the *same* Brewfile-style DSL workload
([`crates/rubyrs/tests/wasm/brewfile_dsl.rb`](../crates/rubyrs/tests/wasm/brewfile_dsl.rb)
— ~50 lines of declarations, a class def, two `.each` loops, a
conditional) and run under the *same* `wasmtime` engine. The
question P2-A asks is: **is the embedding niche big enough to be
worth building toward?**

Reproduce locally via `bash perf/p2a_compare.sh` once ruby.wasm 3.4
is unpacked at the path the script documents.

Apple M-series, wasmtime 45.0.0, ruby.wasm 3.4 `wasi-minimal`,
rubyrs.wasm release + `wasm-opt -Oz`, MIN of 5 runs:

| Shape | rubyrs | ruby.wasm 3.4 minimal | Ratio |
|-------|-------:|----------------------:|------:|
| **Raw .wasm wall** (no AOT)   | 30 ms  | 180 ms | **6.0× faster** |
| **AOT .cwasm wall**           | 10 ms  |  80 ms | **8.0× faster** |
| **Raw .wasm size**            | 1.14 MB | 24.14 MB | **21.2× smaller** |
| **AOT .cwasm size**           | 4.27 MB | 33.12 MB | **7.8× smaller** |

The cross-runtime ratios are the P2-A decision signal: rubyrs is
~6-8× faster end-to-end and 7-21× smaller for the same DSL workload
under the same engine. Cold-start floor and binary-shipment cost
are both dominated by the wasm bundle itself — ruby.wasm pays for
the full MRI runtime + stdlib even in its minimal build, where
rubyrs ships just the Tier 1 interpreter + Prism parser.

This is the embedding-niche thesis empirically: if your host
shipping path includes the runtime (CLI tools, serverless workers,
edge functions, plug-in scripting), the size difference dominates
and the wall-time difference is gravy. If your host has Ruby
already installed and just hosts a long-running process, ruby.wasm
or native MRI wins on throughput (see "Throughput" below where
rubyrs still trails CRuby's interpreter ~1.76× on a 1M-iteration
loop). The two niches don't overlap.

## Browser engine variation (Safari vs Chrome)

The P2-A numbers above are under `wasmtime`. In-browser behaviour
matters too if you're shipping into Safari, Chrome, or an embedded
WebView. The harness used is checked in at
[`poc/safari-stack-test/`](../poc/safari-stack-test/) — a minimal
HTML page loading the wasm via `@bjorn3/browser_wasi_shim`, with a
virtual filesystem holding the Ruby script. Each browser runs the
same workload 3 times against a single compiled `Module`; the
reported number is MIN.

Platform: **x86_64 macOS** (Intel, not Apple Silicon — these
absolute numbers will be lower on M-series, but the cross-engine
and cross-runtime ratios should hold). Safari 26.3 (JSC), Chrome
148 (V8). Both fresh tabs, foreground, no other heavy load.

### Throughput — fizzbuzz 1M (MIN of 3)

| Runtime | Safari/JSC | Chrome/V8 | Safari speedup |
|---------|-----------:|----------:|---------------:|
| **rubyrs.wasm** (1.5 MB) | **872 ms** | 1791 ms | **2.05× faster** |
| ruby.wasm 3.4 minimal (24 MB) | 881 ms | 957 ms | 1.09× faster |
| **rubyrs vs ruby.wasm on same engine** | **0.99× (tied)** | 1.87× slower | — |

The headline finding: **on Safari, rubyrs.wasm matches ruby.wasm on
throughput while shipping 16× less bytecode**. On Chrome, V8 happens
to handle CRuby's dispatch shape better than rubyrs's single
match-based loop, and rubyrs runs ~1.87× slower. Same binary, same
script, same harness — the gap is entirely engine-side.

JSC's wasm tier appears unusually friendly to rubyrs's
single-function `Op::*` match-based dispatch; V8's Liftoff
baseline is more conservative on long `br_table`s and at this
workload may not have tiered up to TurboFan. For ruby.wasm both
engines are within 9% of each other — CRuby's computed-goto split
across many wasm functions is handled comparably.

### Stack depth — `recurse.rb` (depth probe)

```ruby
def f(n); return 0 if n <= 0; 1 + f(n - 1); end
```

Maximum depth reached before either `SystemStackError` (rescuable)
or a wasm trap:

| Runtime | wasmtime 45 | Safari/JSC | Chrome/V8 |
|---------|------------:|-----------:|----------:|
| **rubyrs.wasm** | 1,000,000+ ✓ | **1,000,000+ ✓** | **1,000,000+ ✓** |
| ruby.wasm 3.4 minimal | ~16,000 (SystemStackError) | ~16,000 | ~16,000 |

rubyrs handles **60–125× deeper Ruby recursion** than ruby.wasm in
every environment tested. The structural reason: rubyrs stores Ruby
call frames in a heap-allocated `Vec<Frame>` ([`vm.rs:402`](../crates/rubyrs/src/vm.rs)),
so Ruby-level recursion is a `Vec::push`, not a wasm function call —
it doesn't consume the host wasm stack at all. CRuby's
interpreter walks the host C stack one frame per Ruby call, so the
maximum Ruby recursion depth in wasm is bounded by the engine's
wasm stack budget.

This is a "won by design" property of the Rust rewrite, not a
tunable knob. The 16k floor for ruby.wasm is consistent across
wasmtime, Safari, and Chrome, suggesting it's CRuby's own
`stack_chk` triggering a clean SystemStackError before any engine
trap. Bug reports of harder Safari crashes
([ruby/ruby.wasm#532](https://github.com/ruby/ruby.wasm/issues/532))
likely need a different reproducer than depth-only recursion;
this section only documents the depth ceiling we measured.

### Cold start

The browser harness reports `compile` time separately — i.e. how
long `WebAssembly.compile(bytes)` takes once the module is fetched.
Per a single fresh tab (numbers vary ±20 ms run-to-run):

| Runtime | Safari/JSC compile | Chrome/V8 compile |
|---------|-------------------:|------------------:|
| rubyrs.wasm | ~10 ms | ~14 ms |
| ruby.wasm 3.4 minimal | ~120 ms | ~100 ms |

A 10× difference in compile time, driven entirely by binary size.
The implication: rubyrs's size advantage shows up not just in
shipping cost but in tab-load latency. For interactive workloads
(IRB-style demos, in-browser scripting), rubyrs is ready ~100 ms
sooner per page load.

### Honesty notes

- Sample size is 3 per cell. Spreads were tight (≤±5% for fizzbuzz,
  ≤±2% for stack depth — see [`poc/safari-stack-test/results.jsonl`](../poc/safari-stack-test/results.jsonl)).
- Both browsers were given the script via the same virtual-FS shim
  to keep instantiation overhead apples-to-apples.
- Compile times are wall-clock from JS, not engine-internal
  metrics. Background tier-up could continue past the reported
  number; the throughput measurements that follow include any
  tier-up effects amortised across 3 runs of the same Module.
- We did **not** measure JS interop, DOM access, or `JS::Object#await`
  — rubyrs ships zero JS-host integration today. These results
  cover script execution only.

## Throughput

1M iteration loop computing fizzbuzz string lengths.

| Implementation | Time | Peak RSS |
|----------------|------|---------|
| rubyrs (current: bytecode + IC + Inc*+ BinOpInt + stmt-pos elision) | **0.33 s** | 2.1 MB |
| rubyrs.wasm via wasmtime | ~0.86 s | ~16.7 MB |
| CRuby 3.4 (no YJIT) | 0.19 s | 18.4 MB |
| CRuby 3.4 + YJIT | 0.15 s | 19.1 MB |

rubyrs is **1.76× of CRuby's interpreter** on fizzbuzz, ~2.2× of CRuby+YJIT.

Method-dispatch-heavy workloads close the gap further. `Counter.inc × 1M`
(every iteration is a method call into `@count = @count + 1`):

| Implementation | Time |
|----------------|------|
| rubyrs | 0.15 s |
| CRuby 3.4 (no YJIT) | 0.11 s |

rubyrs is **1.43× of CRuby** there — the single-slot inline method cache
(ADR 0007 / Tier1-1) plus `Op::IncIvar` (Tier1-3) collectively close the
gap for this shape of workload.

### Tier 1 progression (5 small commits)

Starting from `dd7826c` (the P1-B interner landing, before any Tier 1
work), each commit verified against the 10-fixture CRuby diff harness:

| Commit | Change | fizzbuzz | vs CRuby |
|--------|--------|---------:|---------:|
| baseline | post P1-B interner | 408 ms | 2.17× |
| Tier1-1  | single-slot method cache | 386 ms | 2.05× |
| Tier1-2  | `Op::IncLocal` (`i = i + 1`) | 369 ms | 1.96× |
| Tier1-3  | `Op::IncIvar` (`@x = @x + 1`) | 364 ms | 1.94× |
| Tier1-4  | `Op::BinOpInt` (fuse LoadConstInt + BinOp) | 332 ms | 1.78× |
| Tier1-5  | stmt-position omits Dup/Pop | **327 ms** | **1.76×** |

20% wall-clock improvement; gap to CRuby's interpreter closed from
2.17× to 1.76×.

## Memory

| Workload | rubyrs RSS | CRuby RSS |
|----------|-----------|-----------|
| Trivial `puts 1+2` | 2.1 MB | 18.4 MB |
| 1M fizzbuzz | 2.1 MB | 18.4 MB |
| 200k cycle-allocations (with our mark-sweep GC) | 2.4 MB | 18.3 MB |
| 2M cycle-allocations | 2.4 MB | n/a |

GC works: heap stays flat even when the Ruby program allocates millions of
short-lived objects with cycles. See [ADR 0003](adr/0003-rc-plus-mark-sweep-hybrid-gc.md).

## Binary size

| Target | Size |
|--------|-----:|
| Native (aarch64-apple-darwin, stripped) | 3.5 MB |
| `wasm32-wasip1` (stripped) | 1.3 MB |
| `wasm32-wasip1` (stripped + `wasm-opt -Oz`) | 1.2 MB |

Includes the vendored Prism parser. There is no separate runtime to ship.
For comparison, ruby.wasm 3.4 `wasi-minimal` is 24.1 MB raw — see the
"P2-A pivot" section above for the same-shape comparison.

## Reproducing

The "1M fizzbuzz" benchmark is checked in at
`crates/rubyrs/benches/fizzbuzz_1m.rb`. Run:

```bash
cargo build --release
hyperfine --warmup 2 \
  './target/release/rubyrs crates/rubyrs/benches/fizzbuzz_1m.rb' \
  'ruby --disable=yjit crates/rubyrs/benches/fizzbuzz_1m.rb' \
  'ruby --yjit crates/rubyrs/benches/fizzbuzz_1m.rb'
```

Note: the release profile pins `lto = "thin"` (see `Cargo.toml`).
This recovers ~7% on this microbench by re-enabling cross-module
inlining the single-file `vm.rs` got for free before the
CRuby-mirrored split.

Memory:

```bash
/usr/bin/time -lp ./target/release/rubyrs crates/rubyrs/benches/fizzbuzz_1m.rb
```

WASM (raw `.wasm`, JIT each run — the 12.7 ms column above):

```bash
# After WASI SDK setup (see DEVELOPMENT.md)
hyperfine 'wasmtime run --dir=. \
  target/wasm32-wasip1/release/rubyrs.wasm crates/rubyrs/benches/fizzbuzz_1m.rb'
```

WASM (AOT `.cwasm` — the ~7 ms column above; matches the
`perf/wasm_check.sh` measurement shape):

```bash
# After WASI SDK setup (see DEVELOPMENT.md). Needs binaryen (wasm-opt)
# and wasmtime on PATH; the perf gate script automates this pipeline.
wasm-opt -Oz target/wasm32-wasip1/release/rubyrs.wasm -o /tmp/rubyrs.opt.wasm
wasmtime compile /tmp/rubyrs.opt.wasm -o /tmp/rubyrs.cwasm
hyperfine 'wasmtime run --allow-precompiled --dir=. /tmp/rubyrs.cwasm \
  crates/rubyrs/benches/fizzbuzz_1m.rb'
```

## Methodology notes

- All runs are `--release` with default optimisation. We don't override
  `RUSTFLAGS` or use LTO; those are easy follow-up wins.
- `hyperfine --warmup 2` discards the first two runs to avoid cold-disk
  bias on the binary.
- CRuby version is whatever is on `PATH`; check with `ruby -v`. Numbers
  above are from CRuby 3.4.1 with Prism enabled.
- Bench programs are deliberately small. Microbenchmarks lie; real-world
  numbers will differ. Use these as **directional**, not absolute.
