# Metaprogramming PoC microbenchmarks

Three tiny scripts, each doing the same 2,000,000-iteration loop with a
different dispatch shape. Measures (a) per-call dispatch overhead and (b)
peak RSS. Re-measured 2026-07-06: Apple M2 Max, rubyrs built with the
standard measurement feature set (see docs/BENCHMARKS.md —
`stdlib,jit-native,_fiber,_json_native,mimalloc`), CRuby 3.4.8 via rbenv
(direct binary, `--disable-gems`).

## Workloads

| Script | What it exercises |
|--------|-------------------|
| `mm_bench.rb`      | every call misses → `method_missing` echoes the symbol |
| `dm_bench.rb`      | accessor installed via `define_method`; closure over outer local |
| `static_bench.rb`  | matched workload with `def` + ivar (control — no metaprogramming) |

## Timing (hyperfine, 2 warmup + 10 runs, repeated in reverse order, 2M iterations)

| | rubyrs | CRuby 3.4.8 | CRuby + YJIT |
|---|---|---|---|
| `method_missing` (mm) | 530 ms | 88 ms | **82 ms** |
| `define_method` (dm)  | 268 ms | 74 ms | **51 ms** |
| `def + ivar` (static) | 275 ms | 73 ms | **51 ms** |

CRuby's interpreter is **3.6×–6.0× faster per-iteration** in steady
state (YJIT 5.3×–6.5×). That gap *is* the loop body — boot is
amortised. None of these scripts are within striking distance of
CRuby's interpreter; the opt-in `jit-native` tier doesn't cover
these plain `while`-loop + method-call shapes (measured: the
jit-native binary lands within ±5% of a no-JIT build on all three).

## Peak memory (`/usr/bin/time -l`, maximum resident set size)

| | rubyrs | CRuby 3.4.8 `--disable-gems` |
|---|---|---|
| `method_missing` | **9.7 MB** | 12.5 MB |
| `define_method`  | **9.7 MB** | 12.5 MB |

rubyrs runs ~1.3× lighter here (~1.8× against stock CRuby's 17.2 MB).
Honesty note: the 2026-06-era table said 2.4 MB vs 12.9 MB ("5.3×
less") — rubyrs's base RSS has since grown to ~9.7 MB because it is
dominated by the binary's own resident `.text`, which grew through
the gem-compat + JIT campaigns (see docs/BENCHMARKS.md "Memory").

## What this tells us (updated 2026-07-06)

1. **Steady-state dispatch is the bottleneck, not metaprogramming.**
   The `def + ivar` (no metaprog at all) case is *also* 3.8× slower
   than CRuby. The gap is rubyrs' baseline dispatch cost — the PoC
   features themselves don't add order-of-magnitude overhead.
2. **`define_method` dispatch is now at `def` parity.** The 2026-06
   arc saw this flip twice: dm first won (271 ms vs 382 ms) because
   `@state` went through a per-Instance `HashMap`, then lost
   (409 ms vs 269 ms) once the ivar path gained fast-paths while
   closure-backed methods still fell through the ENTIRE `do_call`
   slow cascade. Dispatch-campaign P1 (2026-07) extended the
   explicit-recv/self-recv monomorphic IC serves to simple
   fixed-arity closure methods (`try_invoke_closure_method_from_stack`),
   so dm now binds stack-direct like `def` does: 268 ms vs 275 ms
   (−35% wall, −37% instructions on dm; the shared-cell arg bind is
   marginally cheaper than the arena+ivar path).
3. **`method_missing` adds one failed lookup along the class chain
   before every dispatch.** In an "every call misses" loop this
   roughly doubles the per-call cost vs `def` dispatch (530 ms vs
   275 ms). A real workload — DSL with maybe-1%-miss rate — would
   barely notice.
4. **The memory advantage on this shape is real but modest** (see
   the honesty note above): the heap stays tiny either way; what
   differs is each runtime's fixed footprint.

## Where the per-iteration gap comes from

The 2026-06 suspects listed here (per-call `vec_nil` locals, unfused
`Op::BinOp`) are dated — the dispatch cost has since been profiled
properly: ADR 0031 measures `do_call`'s slow cascade at ~72% of
dispatch time and lands incremental fast-paths, and the structural
conclusion (interpreter dispatch is ~2.5–5× CRuby even when
fast-pathed; the lever is the opt-in native JIT + PIC, not more
interpreter tweaks) is written up in ADR 0034. These loops are
plain `while` + method-call shapes the JIT doesn't yet cover, so
they still pay full interpreter dispatch.

## How to reproduce

```bash
# standard measurement feature set (docs/BENCHMARKS.md) — verify
# the binary with perf/alloc_fingerprint.sh before timing
cargo build --release -p rubyrs \
  --features stdlib,jit-native,_fiber,_json_native,mimalloc
cd crates/rubyrs/examples/metaprog_bench
hyperfine --warmup 2 --runs 10 \
  -n "rubyrs mm" "../../../../target/release/rubyrs mm_bench.rb" \
  -n "cruby mm"  "ruby --disable-gems mm_bench.rb" \
  -n "cruby+yjit mm" "ruby --yjit --disable-gems mm_bench.rb"
# repeat with dm_bench.rb / static_bench.rb; if `ruby` is an rbenv
# shim, time the real binary (~/.rbenv/versions/…/bin/ruby) — the
# shim adds ~38 ms of its own

# memory
/usr/bin/time -l ../../../../target/release/rubyrs mm_bench.rb
/usr/bin/time -l ruby --disable-gems mm_bench.rb
```
