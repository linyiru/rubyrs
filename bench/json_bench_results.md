# JSON benchmark — pure canon vs `_json_native` vs CRuby

Workload: 100-key Object with mixed-type values + 20-element nested
Array of Hashes (~3.4 KB JSON). 5000 iterations × 3 runs, minimum
total reported. Driver: `bench/json_bench.rb`.

Numbers from 2026-06-01, ARM macOS / Apple Silicon, release builds.
Rerun with the same driver on your machine for a current snapshot.

| Operation   | CRuby 3.4 stdlib | rubyrs (pure canon)  | rubyrs (`_json_native`) |
|-------------|------------------|----------------------|--------------------------|
| `parse`     |   21.4 µs/iter   |   4 115 µs/iter (193×) |  **17.0 µs/iter (0.80×)** |
| `generate`  |   27.3 µs/iter   |   4 464 µs/iter (163×) |   27.6 µs/iter (1.01×)   |
| `round_trip`|   49.6 µs/iter   |   8 676 µs/iter (175×) |   83.6 µs/iter (1.69×)   |

Multiplier vs CRuby stdlib (lower = faster relative to CRuby).
**Bold** = rubyrs beats CRuby.

## Takeaways

- **`_json_native` parse beats CRuby by 20 %.** The streaming-visitor
  shape (`serde::de::Visitor` + `DeserializeSeed` that allocates Ruby
  `Value`s directly during the serde state walk) skips the
  `serde_json::Value` intermediate tree that an earlier two-pass form
  paid for. Per 3.4 KB payload that's ~one tree-allocation pass
  saved — 32 µs → 17 µs, ~47 % reduction over the two-pass shape.
- **Generate is tied with CRuby.** serde_json's emit performance was
  already competitive with `ext/json/ext/generator.c`; no further
  juice without going below `vm.heap.alloc` for the result String.
- **Round-trip stays at 1.7× CRuby** even with parse + generate
  individually faster. Plausible explanation: each round-trip iter
  discards its parsed tree (Hash + Array + Strings), so rubyrs's
  GC sweeps thousands of objects per iter. CRuby's mark-sweep
  generational collector handles short-lived-object churn cheaper;
  rubyrs's mark-sweep doesn't (yet) have a generational shape. The
  fix is GC work, not JSON work — out of scope for the menu item.
- **Pure canon is ~160–200× slower than CRuby.** That's the cost of
  walking `String#chars` one position at a time on a bytecode VM;
  the canon trades speed for being the spec (Rule 6 of ADR 0019 —
  pure-Ruby canonical form is what every behaviour claim measures
  against).

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
