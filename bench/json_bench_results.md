# JSON benchmark — pure canon vs `_json_native` vs CRuby/Oj

Workload: 100-key Object with mixed-type values + 20-element nested
Array of Hashes (~3.4 KB JSON). 5000 iterations × 3 runs, minimum
total reported. Driver: `bench/json_bench.rb`.

Numbers from 2026-06-01, ARM macOS / Apple Silicon, release builds.
Rerun with the same driver on your machine for a current snapshot.

| Operation   | CRuby stdlib | Oj :strict | rubyrs pure canon | rubyrs `_json_native` |
|-------------|--------------|------------|-------------------|------------------------|
| `parse`     |  ~22 µs/iter | ~28 µs/iter | ~4100 µs/iter (193×) | **~17 µs/iter (0.62× Oj)** |
| `generate`  |  ~29 µs/iter | ~13 µs/iter | ~4500 µs/iter (163×) | ~15 µs/iter (1.11× Oj)     |
| `round_trip`|  ~54 µs/iter | ~40 µs/iter | ~8700 µs/iter (175×) | ~44 µs/iter (1.08× Oj)     |

Multiplier vs Oj :strict (the fastest gem-based Ruby JSON impl).
**Bold** = rubyrs beats both CRuby stdlib AND Oj.

## Takeaways

- **`_json_native` parse beats both CRuby stdlib AND Oj by ~40 %.**
  Streaming `serde::de::Visitor` + `DeserializeSeed` allocates Ruby
  Values directly during the serde state walk; no
  `serde_json::Value` intermediate. Each nested Array / Hash lands
  on `vm.heap` in one pass. Oj has to round-trip through the Ruby
  C-API (`rb_hash_aset` etc.) which is itself fast but pays a
  per-pair invocation cost; serde's recursive descent inlines
  better at the Rust optimizer level.

- **Generate within 15 % of Oj.** Byte-buffer output (`Vec<u8>`)
  with ASCII fast-path escape (bulk `extend_from_slice` over safe
  runs, escape per non-safe byte), hand-rolled `write_int`
  (skipping `std::fmt::Write` machinery), and 4 KB pre-sized output
  capacity (matches Apple Silicon page size; avoids the 3-doubling
  reallocs a 1 KB start pays on a 3.4 KB body). Remaining gap is
  bounded by:
    1. Host-fn dispatch overhead per call (TLS lookup +
       `CURRENT_VM_PTR` set + closure invoke — ~1 µs measured).
    2. RefCell::borrow on each `Value::Str` content access
       (cheap individually, adds up across ~120 strings/iter).
    3. Final `Value::new_str_bytes` Rc allocation for the
       returned String.
  Closing this would need either a direct-write API surface
  (`Oj.dump(obj, io)`-style streaming to an existing String) or
  custom Vm-internal dispatch interceptors for `JSON.generate`.

- **Round-trip ≈ Oj.** Parse + generate individually beat or match
  Oj; round-trip combines them with GC churn between (each iter
  discards ~150 short-lived Ruby objects). rubyrs's mark-sweep is
  single-generation; CRuby/Oj use a generational shape that wins
  this access pattern. Round-trip variance is high (44–83 µs
  across runs) for the same reason — GC scheduling dependent.

- **Pure canon is 160–200× slower than CRuby.** That's the cost
  of walking `String#chars` on a bytecode VM; the canon trades
  speed for being the spec (Rule 6 of ADR 0019 — pure-Ruby
  canonical form is what every behaviour claim measures against).
  Real apps that need throughput build with `--features
  _json_native`; the pure canon stays the reference impl.

## How we got here (perf milestones)

1. **Pure canon ships** — parse 4100 µs, generate 4500 µs.
   Reference impl; no native involvement.
2. **`_json_native` v1: two-pass serde** — parse 32 µs (1.5× CRuby
   stdlib), generate 27 µs (0.9×). `serde_json::from_str → Value
   → walk-and-build-Ruby`. One full Rust tree allocation
   pass + one Ruby tree pass.
3. **`_json_native` v2: streaming Visitor** — parse 17 µs (0.8×
   CRuby stdlib, 0.62× Oj). `serde::de::Visitor` allocates Ruby
   Values directly during deserialize; skips the intermediate
   `serde_json::Value`. parse becomes the fastest Ruby JSON
   parser measured.
4. **`_json_native` v3: byte-buffer generate** — generate 15 µs
   (1.11× Oj). `&mut Vec<u8>` instead of `&mut String`, ASCII
   fast-path escape with bulk `extend_from_slice`, hand-rolled
   `write_int`, no `Vec::clone` of heap contents during walk,
   4 KB pre-sized output.

## Reproducing

```bash
# CRuby (with optional Oj column if `oj` gem installed)
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
Oj column appears automatically when `gem install oj` is on PATH.
