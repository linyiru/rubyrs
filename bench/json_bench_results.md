# JSON benchmark — pure canon vs `_json_native` vs CRuby

Workload: 100-key Object with mixed-type values + 20-element nested
Array of Hashes (~3.4 KB JSON). 5000 iterations × 3 runs, minimum
total reported. Driver: `bench/json_bench.rb`.

Numbers from 2026-06-01, ARM macOS / Apple Silicon, release builds.
Rerun with the same driver on your machine for a current snapshot.

| Operation   | CRuby 3.4 stdlib | rubyrs (pure canon)  | rubyrs (`_json_native`) |
|-------------|------------------|----------------------|--------------------------|
| `parse`     |   22.4 µs/iter   |   4 326 µs/iter (193×) |   32.6 µs/iter (1.46×)   |
| `generate`  |   29.1 µs/iter   |   4 602 µs/iter (158×) |   26.7 µs/iter (0.92×)   |
| `round_trip`|   53.2 µs/iter   |   8 675 µs/iter (163×) |   75.9 µs/iter (1.43×)   |

Multiplier vs CRuby stdlib (lower = faster relative to CRuby).

## Takeaways

- The pure-Ruby canon is ~160–200× slower than CRuby's C parser.
  That's the expected cost of walking `String#chars` one position
  at a time on a bytecode VM; the canon trades speed for being the
  spec (Rule 6 of ADR 0019 — pure-Ruby canonical form is what every
  behaviour-claim measures against).
- The `_json_native` accelerator closes the gap to ~1.5× CRuby on
  parse and **beats CRuby on generate** (serde_json's emit is
  faster than CRuby's `ext/json/ext/generator`). Round-trip stays
  ~1.4× CRuby because the round-trip pays the parse-side Ruby-
  Value-tree construction overhead (each `Value::Hash` allocation
  goes through `vm.heap.alloc`, whereas CRuby's C parser builds
  hashes via direct `rb_hash_aset` calls and skips one Ruby-level
  allocation layer per pair).
- Bytecode dispatch overhead dominates the pure canon: serde_json's
  Rust parser would itself be ≥ CRuby speed if not for the Ruby
  Value reconstruction. The accelerator path proves the VM isn't
  the bottleneck on JSON-heavy workloads when the heavy lifting is
  pushed to a Rust battery — same architecture ADR 0019 v3
  prescribes for the menu's data-layer items.

## Reproducing

```bash
# CRuby
ruby bench/json_bench.rb

# rubyrs pure canon (stdlib feature only)
cargo build --release --features default,stdlib -p rubyrs
target/release/rubyrs bench/json_bench.rb

# rubyrs + native accelerator
cargo build --release --features default,_json_native,stdlib -p rubyrs
target/release/rubyrs bench/json_bench.rb
```

Environment knobs: `ITERS=5000 RUNS=3` (defaults). Increase
ITERS for noisier hosts; RUNS=3 already absorbs warm-up + GC.
