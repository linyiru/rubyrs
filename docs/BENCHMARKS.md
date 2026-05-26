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

## Edge runtimes: cross-host portability

Validates the "one wasm artifact, many edge runtimes" thesis. Same
`src/rubyrs_worker.wasm` (1.68 MB after Wizer pre-init, no
wasm-opt — see [PoC details](#wasm-opt-vs-wizer-notes) below)
runs unchanged under three different V8-based runtimes and one
non-V8 baseline. Spike branch:
[`spike/cf-worker-poc`](../poc/cf-worker/).

`puts 1+1` workload, n=5 each, Apple M-series:

| Runtime | Engine | Self-host? | Cold-start | Warm tiny | Warm smoke.rb | 1M `each` |
|---------|:------:|:---------:|:----------:|:---------:|:-------------:|:---------:|
| **Deno** 2.8 + browser_wasi_shim | V8 14.9 | ✅ | 25 ms | **1.5 ms** | **1.7 ms** | 124 ms |
| **workerd** 2026-05-26 + workers-wasi | V8 | ✅ | **18 ms** | 2.5 ms | 4.0 ms | 135 ms |
| **CF Workers edge** (managed) | V8 (= workerd) | ❌ | ~149 ms wall | 7 ms cpu | 7 ms cpu | 173 ms cpu |
| wasmtime 45 (CLI, no HTTP) | wasmtime | ✅ | 12.7 ms (raw) / ~7 ms (AOT) | — | — | — |

Notes:

- **CF edge numbers are CPU time from `wrangler tail`** bucketed
  by per-isolate invocation count (a header `x-rubyrs-invocation`
  emitted by [worker.js](../poc/cf-worker/src/worker.js)). Cold
  isolate (invocation == 1) wall is 149 ms / cpu ~80 ms; warm
  (invocation > 1) settles to 7 ms cpu p50, p90 12 ms, max 13 ms.
  The earlier-reading "wizer regresses edge perf" turned out to
  be deploy-then-immediately-measure pool-warming noise, not a
  real regression — [Pyodide-on-Workers' published 1027 ms
  mean](https://blog.cloudflare.com/python-workers-advancements/)
  is similarly a pool-hit + pool-miss blend.
- **Deno beats workerd on warm by ~40 %** (1.5 vs 2.5 ms tiny)
  despite trailing on cold (25 vs 18 ms). Plausible reasons: (1)
  `browser_wasi_shim`'s stdin/stdout is a pure-JS callback on a
  single `Uint8Array`, vs `workers-wasi`'s extra `memfs.wasm`
  proxy step; (2) `Deno.serve` is hyper-based Rust HTTP cutting
  out workerd's JS-shim ↔ kj layer. Heavy compute converges to
  within ~10 % because V8's wasm engine dominates that regime.
- **wasmtime cold-start (7-13 ms)** beats every V8 host on
  cold but provides no HTTP layer of its own — listed for
  baseline only; HTTP-serving wasmtime would require either
  wasi-http (component model, not Preview 1) or a custom Rust
  HTTP loop. Not part of the V8-host comparison.

#### wasm-opt vs Wizer notes

Counter-intuitive PoC finding: **`wasm-opt` is consistently
net-negative on V8 cold-start at every optimisation level**, even
when its size reductions are large. Smaller wasm doesn't translate
into faster instantiate; the V8 wasm parser appears to bottleneck
on IR construction / module setup rather than byte count. Wizer
pre-init is the win, n=5 each on workerd local:

| Build pipeline | Wasm size | Cold-start (median) |
|----------------|----------:|--------------------:|
| baseline (raw cargo output) | 1.54 MB | 57 ms |
| wasm-opt -Oz only | 1.22 MB (−21 %) | 53 ms (−7 %) |
| wasm-opt -Oz + Wizer | 1.37 MB | 27 ms (−53 %) |
| wasm-opt -O2 + Wizer | 1.42 MB | 23 ms (−60 %) |
| **Wizer only** (no wasm-opt) | **1.68 MB** | **18 ms (−69 %)** |

The Wizer win matches what
[`workerd/src/pyodide/make_snapshots.py`](https://github.com/cloudflare/workerd/tree/main/src/pyodide)
does for Python Workers — snapshot the post-init linear memory
so cold-start skips re-running the interpreter's bootstrap. We
cannot match CF's *baseline-preloaded-in-isolate-pool* trick
(that requires the runtime to be linked into workerd itself),
but the per-Worker snapshot equivalent is exactly what the PoC's
`build.sh` produces.

#### Cold-start floor — two negative experiments

Above ~18 ms (workerd local) the marginal cost of further wasm
shrinkage is zero. Two independent attempts confirmed:

1. **Lazy-loading the Tier 1 stdlib preambles** (Random + SecureRandom,
   `src/lib.rs::load_preamble` calls these unconditionally today).
   Cuts ~40 KB from the Wizer'd wasm. Cold-start n=5: 17.7, 19.5,
   22.1, 22.6, 19.3 ms — **median 19.5 ms, marginally SLOWER than
   the 18.2 ms baseline**, well within variance.

2. **`opt-level = "z"` + LTO=fat + codegen-units=1**. Repo's own
   `[profile.release-min]` history note records this combination as
   **3–19 % SLOWER at cold start despite producing a 56 %-smaller
   binary**, measured on three hosts (macOS arm64, Linux arm64,
   Linux x86_64). The reason is the same one wasm-opt -Oz hits:
   aggressive size shrinkage suppresses inlining and substitutes
   shorter call sequences, which V8's wasm tier-up engine takes
   longer to fix up than it would have to compile the original.

Two independent angles, same negative result — the 18 ms floor
is V8's wasm parser + module-instantiate fixed cost, NOT a
function of our byte count. To reduce further the project would
have to either (a) reduce the function count (1798 today) so V8
has fewer IRs to build, requiring rubyrs-internal refactoring; (b)
move to component model + AOT (wasmtime serve), bypassing V8's
parse path entirely; or (c) get CF to expose a generic
`--save-wasm-snapshot` user-wasm equivalent of their privileged
Python preload (currently not on offer). The 18 ms cold-start
is best treated as the public-API floor for this build shape and
the PoC is now operating at that floor.

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

| Target | Stripped size |
|--------|--------------|
| Native (aarch64-apple-darwin) | 997 KB |
| `wasm32-wasip1` | 644 KB |

Includes the vendored Prism parser. There is no separate runtime to ship.

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
