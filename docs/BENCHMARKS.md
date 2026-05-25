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
| rubyrs.wasm via wasmtime | 12.7 ms |
| CRuby 3.4 (no YJIT) | 77 ms |
| CRuby 3.4 + YJIT | 78 ms |

rubyrs is ~50× faster cold-start than CRuby. This is the strongest signal
for our target niche (CLI tools, serverless / edge runtimes, embedded
scripting).

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

WASM:

```bash
# After WASI SDK setup (see DEVELOPMENT.md)
hyperfine 'wasmtime run --dir=. \
  target/wasm32-wasip1/release/rubyrs.wasm crates/rubyrs/benches/fizzbuzz_1m.rb'
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
