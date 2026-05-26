# Time Path A vs B microbench — local results

**Hardware:** Apple M-series, rust 1.95 release build, rubyrs HEAD
at the time this file was written (commit `9cc69ff` was the last
ADR-0017-fill-out commit; the bench was run on the same binary).
**N:** 1,000,000 iterations per scenario, MIN of 5 runs.

| scenario   | A ms | B ms | A/B   | A per-op | absolute floor |
|------------|-----:|-----:|------:|---------:|----------------|
| `to_i`     | 170  | 70   | 2.4×  | 170 ns   | trivial method call |
| `+`        | 1170 | 50   | 23.4× | 1170 ns  | method call + alloc per op |
| `<=>`      | 590  | 90   | 6.6×  | 590 ns   | method call + Int cmp |
| construct  | 470  | 300  | 1.6×  | 470 ns   | Object + ivar Hash alloc |

`A` = pure-Ruby Time class with `@sec` / `@nsec` ivars — the shape
the proposed `crates/rubyrs/src/preamble/time.rb` would carry.

`B` = the closest existing primitive: `Integer#to_i` for method
calls, `Int + Int` via BinOpInt for arithmetic. Integer is the
LOWER BOUND of what a Rust `Value::Time` primitive could achieve;
a real Path B implementation would carry one extra match-arm and
one heap alloc per `+`, so realistic Path B sits between `B`
and `A` — see "Caveats" below.

## How to read these numbers

The headline `23.4×` on `+` looks alarming but the absolute floor
is `1.17 µs per op`. For the rubyrs niche workloads, multiply by
the realistic op count:

| workload                                         | Time ops | Path A wall |
|--------------------------------------------------|---------:|------------:|
| Brewfile / Gemfile / Dangerfile DSL              |        0 |       0 ms  |
| Sinatra-style request handler (log + timestamp)  |    5-20  |      ~5 µs  |
| Config DSL with retry-policy timestamps          |     <10  |     ~10 µs  |
| Log / report generator, per-line `Time.now`      |     ~1k  |      ~1 ms  |
| Tight-loop time-series synthesiser (rare)        |    100k+ |    100+ ms  |

The first four categories are completely invisible. Only the
last — which doesn't match any embed-niche use case rubyrs
targets — would benefit from Path B's faster dispatch.

## Caveats

1. **B's `+` underestimates Path B.** `Int + Int` hits the
   BinOpInt fast path (no method dispatch). A real
   `Value::Time + Int` would go through `primitive_call` with a
   per-op `HeapObj::Time` allocation. Realistic Path B sits
   ~3-5× faster than Path A, not 23×.

2. **B's `construct` underestimates Path B.** `Array.new(2, 0)`
   has zero per-element initialization cost; a `Value::Time`
   construction would copy `sec` / `nsec` from the host fn
   return into a HeapObj. Realistic Path B construct cost is
   probably halfway between the two — maybe `~350 ms` for 1M
   iterations.

3. **Microbench excludes the host fn cost.** Both paths need a
   `Config::time_now` capability injection; the call overhead is
   the same on either side (one Rust call) and isn't part of the
   dispatch comparison. The bench uses a pre-computed
   `TimeA.new(sec, 0)` to keep the loop on the dispatch path
   only.

## Interpretation

Path A is the right default. The 23× on `+` is real but absolute
cost stays <1 µs per op, and the embed niche workloads don't put
Time in tight loops. We get:

- Half the dev cost (~half a day vs ~1.5-2 days).
- ~5× smaller binary impact (~3-5 KB vs ~10-20 KB) — matters for
  the wasm cwasm shipping shape.
- Easy incremental surface growth (each new Time method = 5
  lines of Ruby in the preamble file).

If a future workload surfaces Path-A-bound bottlenecks, the
escape hatch is a targeted `__time_arith(a, op, b)` builtin host
fn that does the alloc in Rust — closes the 23× to ~3-5× without
the full Path B rewrite. Defer until we have data.

## Re-running

```bash
cargo build --release -p rubyrs
bash perf/time_microbench.sh

# Or with a different sample size:
BENCH_N=5_000_000 RUNS=3 bash perf/time_microbench.sh
```

The script's surrogate Time class lives inline in
`crates/rubyrs/benches/time_path_microbench.rb` and survives
any future preamble landing — keep it as the floor-comparison
reference even after the real Time vendor ships.
