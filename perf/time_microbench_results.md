# Time Path A vs B microbench — local results (v2)

**Hardware:** Apple M-series, rust 1.95 release build, rubyrs HEAD
at the time this file was written. **N:** 1,000,000 iterations
per scenario, MIN of 5 runs. Two consecutive bench invocations
gave the same numbers (within ±20 ms on the noisiest row).

## What changed since v1

v1 reported only an `A vs B-floor` comparison where the B-floor
used `Int + 1` via the **BinOpInt fused fast path** — a path a
real `Value::Time + Int` could NOT take because BinOpInt is
hardcoded to (Int, Int) operands. This produced a 23.4× ratio
that overstated the realistic gap.

v2 adds:
- **`b_*_send`** — `n.send(:op, x)` shapes that force the call
  through `do_call`'s `primitive_call` path (skipping BinOpInt
  fuse). EXACTLY the dispatch shape a Rust `Value::Time`
  primitive arm would carry.
- **`b_*_range`** — `Range#begin` / `(begin..end)` shapes
  against a heap-backed `Value::Range` with two inner Value
  slots. Closest existing primitive that mirrors how a
  `HeapObj::Time { sec, nsec }` would be laid out.
- **`*_workload`** — a realistic Time-shaped mix per
  iteration: construct + read inner + compute delta + compare.
  The most honest end-to-end number for a request handler /
  log-line generator workload.
- **Peak RSS column** — captured from `/usr/bin/time -l`
  (Darwin) / `-v` (Linux). Answers "does Path A's per-op
  Object allocation push the high-water mark?".

## Numbers

### Path A — pure-Ruby Time class (user-method dispatch)

| scenario          | wall ms | peak RSS KB |
|-------------------|--------:|------------:|
| `a_to_i`          |     160 |        4256 |
| `a_plus`          |    1120 |        4624 |
| `a_cmp`           |     580 |        4320 |
| `a_construct`     |     460 |        4608 |
| `a_workload`      |    1400 |        4640 |

### Path B floor — bare primitive (BinOpInt-fused where applicable)

| scenario          | wall ms | peak RSS KB |
|-------------------|--------:|------------:|
| `b_to_i`          |      70 |        4272 |
| `b_plus`          |      50 |        4256 |
| `b_cmp`           |      90 |        4288 |
| `b_construct`     |     290 |        4448 |

### Path B realistic — send-dispatched + Range-shaped

| scenario               | wall ms | peak RSS KB |
|------------------------|--------:|------------:|
| `b_to_i_send`          |     100 |        4256 |
| `b_to_i_range`         |     150 |        4240 |
| `b_plus_send`          |     120 |        4304 |
| `b_cmp_send`           |     130 |        4288 |
| `b_construct_range`    |      60 |        4384 |
| `b_workload`           |     710 |        4464 |

### A / B ratios

| pair                | A/B wall | A/B RSS |
|---------------------|---------:|--------:|
| to_i (floor)        |     2.3× |    1.0× |
| to_i (send)         |     1.6× |    1.0× |
| to_i (range)        |     1.1× |    1.0× |
| plus (floor)        |    22.6× |    1.1× |
| plus (send)         |     9.4× |    1.1× |
| cmp  (floor)        |     6.4× |    1.0× |
| cmp  (send)         |     4.5× |    1.0× |
| construct (floor)   |     1.6× |    1.0× |
| construct (range)   |     7.7× |    1.0× |
| **workload (mix)**  | **2.0×** |    1.0× |

## What v2 changes about the analysis

1. **The 23× plus ratio was misleading.** Against the realistic
   `b_plus_send` surrogate the gap shrinks to **9.4×** — still
   significant per-op (1.12 µs vs 0.12 µs) but a third the size
   of v1's headline.

2. **The end-to-end workload row is 2.0×.** This is the most
   honest number: per-iteration mix of construct + field read +
   arithmetic + compare. Path A pays 1.4 s per million
   iterations vs realistic Path B's 0.71 s.

3. **Peak RSS is identical across A and B.** Path A's
   per-iteration Object + ivar Hash allocations DO push more
   pressure into the GC (visible as the higher wall on `a_plus`),
   but the high-water-mark stays at the same ~4.3-4.6 MB for
   both paths. The mark-sweep GC is keeping up.

4. **`to_i` realistic ratio is 1.1×.** When the operation is a
   bare field read (the dominant Time API shape — `to_i`,
   `year`, `month`, `sec`, `min`, `nsec`, etc.), Path A is
   essentially indistinguishable from Path B.

## Translation to realistic workloads

Multiply the `workload (mix)` per-op cost back into realistic
op counts:

|         workload                                | Time ops | A wall | B wall | Δ      |
|--------------------------------------------------|---------:|-------:|-------:|-------:|
| Brewfile / Gemfile / Dangerfile DSL              |        0 |   0 ms |   0 ms |   0 ms |
| Sinatra-style request handler (timestamp + log)  |     5-20 |  ~7 µs |  ~3 µs |  ~4 µs |
| Config DSL with retry-policy timestamps          |     <10  |  ~5 µs |  ~3 µs |  ~2 µs |
| Log / report generator, per-line `Time.now`      |    ~1k   |  1.4 ms|  0.7 ms| 0.7 ms |
| Tight-loop time-series synthesiser (rare)        |    100k+ |  140 ms|  71 ms |  69 ms |

The first four categories stay sub-millisecond on either path.
Only the synth case crosses ~100 ms of difference — and that
workload is well outside the rubyrs embed niche (Brewfile-shape
DSLs, Sinatra handlers, config files).

## Decision

**Path A remains the right default.** Even the v2 realistic
ratios show:

- End-to-end `workload` ratio is 2×.
- The break-even crosses ~10k Time ops per script run.
- Embed-niche scripts make 0-1k Time ops per run.
- No RSS penalty on Path A — the per-iter alloc churn is
  absorbed by the GC without pushing peak memory.

Path A's other advantages from the v1 writeup still hold:
- Half the dev cost (~half a day vs ~1.5-2 days).
- ~5× smaller binary impact for the wasm cwasm story.
- Easy incremental surface growth (strftime directives, parse
  shapes) as Ruby edits to the preamble file.

**Escape hatches if a future workload needs more speed:**
1. **`__time_arith(a, op, b)` builtin** — fold `+ / - / <=>`
   into one Rust call so the 9× plus ratio drops to ~2× (one
   primitive arm + alloc). Pure additive change to the Ruby
   preamble.
2. **Path B proper** — if the embed niche shifts to high-
   throughput Time workloads, the full primitive port is
   worthwhile. The microbench scaffold survives the
   transition so the speedup is measurable on the same
   axis.

Defer both until a real workload forces them.

## Caveats still in place

1. **`b_*_send` is still SLIGHTLY optimistic for `+`** — a
   real `Value::Time + Int` allocs a new HeapObj per call;
   `n.send(:+, 1)` returns an Int (no heap alloc). Realistic
   Path B `+` is `b_plus_send` (120 ms) plus alloc overhead,
   probably ~200-250 ms. So the realistic A/`+` ratio is
   ~5-6×, not 9.4×. Doesn't change the decision.

2. **`b_construct_range`'s 60 ms is OPTIMISTIC** — Range
   construction is a single op (`NewRange`) while a real
   `Time.new(sec, nsec)` would go through Class.new + alloc +
   initialize dispatch. Realistic Path B construct is closer
   to ~150-200 ms; Path A's 460 ms is then 2-3× heavier, not
   7.7×.

3. **`a_workload` includes block-arg + iter index passthrough
   that real DSLs don't pay.** The number is realistic shape
   but a clean Sinatra handler would be ~5-10% faster than
   measured.

These caveats all close the gap further in Path A's favour.

## Re-running

```bash
cargo build --release -p rubyrs
bash perf/time_microbench.sh

# Or with a different sample size:
BENCH_N=5_000_000 RUNS=3 bash perf/time_microbench.sh
```

Scenarios: see the `case scenario when` arms in
`crates/rubyrs/benches/time_path_microbench.rb`. The microbench
scaffold survives any future preamble landing — keep it as the
floor-comparison reference.
