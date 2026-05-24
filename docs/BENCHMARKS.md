# Benchmarks

All numbers from an Apple Silicon (M-series) machine, `--release` builds.
Reproduce with the commands at the bottom.

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
| rubyrs (bytecode VM + BinOp + SymId interner) | **0.41 s** | 2.1 MB |
| rubyrs.wasm via wasmtime | ~0.86 s | ~16.7 MB |
| CRuby 3.4 (no YJIT) | 0.19 s | 18.4 MB |
| CRuby 3.4 + YJIT | 0.15 s | 19.1 MB |

We're 2.3× slower than CRuby's interpreter, ~3× slower than YJIT.
The remaining gap is method-dispatch overhead (every call routes through
a `HashMap<String, _>` lookup; method inline caching is on the roadmap).

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

The "1M fizzbuzz" benchmark:

```ruby
# /tmp/bench.rb
def fizzbuzz(n)
  if    n % 15 == 0 then "FizzBuzz"
  elsif n % 3  == 0 then "Fizz"
  elsif n % 5  == 0 then "Buzz"
  else n.to_s end
end

i = 1; acc = 0
while i <= 1000000
  acc = acc + fizzbuzz(i).length
  i = i + 1
end
puts acc
```

Run:

```bash
cargo build --release
hyperfine --warmup 2 \
  './target/release/rubyrs /tmp/bench.rb' \
  'ruby --disable=yjit /tmp/bench.rb' \
  'ruby --yjit /tmp/bench.rb'
```

Memory:

```bash
/usr/bin/time -lp ./target/release/rubyrs /tmp/bench.rb
```

WASM:

```bash
# After WASI SDK setup (see DEVELOPMENT.md)
hyperfine 'wasmtime run --dir=/tmp \
  target/wasm32-wasip1/release/rubyrs.wasm /tmp/bench.rb'
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
