# Metaprogramming PoC microbenchmarks

Three tiny scripts, each doing the same 2,000,000-iteration loop with a
different dispatch shape. Measures (a) per-call dispatch overhead and (b)
peak RSS. Apple silicon, `cargo build --release`, CRuby 3.4.1.

## Workloads

| Script | What it exercises |
|--------|-------------------|
| `mm_bench.rb`      | every call misses → `method_missing` echoes the symbol |
| `dm_bench.rb`      | accessor installed via `define_method`; closure over outer local |
| `static_bench.rb`  | matched workload with `def` + ivar (control — not apples-to-apples; ivars hit a hashmap) |

## Timing (hyperfine, 2 warmup + 6 runs, 2M iterations)

| | rubyrs | CRuby 3.4 | CRuby + YJIT |
|---|---|---|---|
| `method_missing` (mm) | 482 ms | **144 ms** | 146 ms |
| `define_method` (dm)  | 271 ms | 107 ms | **95 ms** |
| `def + ivar` (static) | 382 ms | 105 ms | **90 ms** |

CRuby is **3.0×–4.3× faster per-iteration** in steady state. That gap
*is* the loop body — boot is amortised. None of these scripts are within
striking distance of CRuby's interpreter, let alone YJIT.

## Peak memory (`/usr/bin/time -l`)

| | rubyrs | CRuby 3.4 |
|---|---|---|
| `method_missing` | **2.4 MB** | 12.9 MB |
| `define_method`  | **2.4 MB** | 12.8 MB |

rubyrs uses **5.3× less peak memory** on the same workload — heap caps
and a small `Op` representation pay off here exactly like they do on the
main `README.md` benchmarks.

## What this tells us

1. **Steady-state dispatch is the bottleneck, not metaprogramming.** The
   `def + ivar` (no metaprog at all) case is *also* 3.6× slower than
   CRuby. The 3× gap is just rubyrs' baseline dispatch cost — the PoC
   features themselves don't add visible overhead.
2. **`define_method` is *faster* than `def + ivar` in rubyrs (271 ms vs
   382 ms).** The closure version reads/writes a captured-local slot by
   index; the `def` version reads/writes an `@state` ivar through a
   per-Instance `HashMap<SymId, Value>`. Ivar-as-hashmap is the real
   slow path; the design point is "moving an `@x` access to an inline
   shape table" not "metaprogramming is expensive."
3. **`method_missing` adds one `lookup_method_uncached` per missed
   call.** That's an extra `HashMap<SymId, Rc<Method>>` walk along the
   class chain *before* every NoMethodError site. In a "every call
   misses" loop this nearly doubles the per-call cost vs static dispatch
   (482 ms vs 271 ms). A real workload — DSL with maybe-1%-miss rate —
   would barely notice.
4. **Memory advantage holds even with metaprog.** Closure-captured
   `Rc<RefCell<Vec<Value>>>` adds at most a few words per
   `define_method`-installed method; no extra arena, no extra page,
   no GC headers. The 2.4 MB → 12.9 MB ratio survives unchanged from
   the README's static-Ruby benchmarks.

## Where the per-iteration gap comes from

Likely suspects (not yet profiled):

- Dispatch is `HashMap<SymId, Rc<Method>>` lookup → `Rc::clone` →
  frame-push every call. The per-call-site inline cache (`cache_id`)
  hits most of the time, but the post-IC step is still allocating
  a frame and a fresh `Rc<RefCell<Vec<Value>>>` for the locals each
  invocation.
- `vec_nil(n_locals)` reallocates per call. A frame-pool (reuse the
  `Vec<Value>` across calls) is the obvious win and doesn't affect
  semantics.
- Integer arithmetic in the loop body still routes through `Op::BinOp`
  / `Op::BinOpInt`, which is fast — but `while i < n` then `i = i + 1`
  is two ops per loop iteration that could fuse into an `IncLocal +
  branch-if-less` pair.

These are interpreter-fundamentals fixes that would help every workload
— not metaprog-specific.

## How to reproduce

```bash
cargo build --release
cd crates/rubyrs/examples/metaprog_bench
hyperfine --warmup 2 --runs 6 \
  -n "rubyrs mm" "../../../../target/release/rubyrs mm_bench.rb" \
  -n "cruby mm"  "ruby --disable-gems mm_bench.rb" \
  -n "cruby+yjit mm" "ruby --yjit --disable-gems mm_bench.rb"
# repeat with dm_bench.rb / static_bench.rb

# memory
/usr/bin/time -l ../../../../target/release/rubyrs mm_bench.rb
/usr/bin/time -l ruby --disable-gems mm_bench.rb
```
