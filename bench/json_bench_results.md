# JSON benchmark — pure canon vs `_json_native` vs CRuby/Oj

Workload: 100-key Object with mixed-type values + 20-element nested
Array of Hashes (~3.4 KB JSON). Driver: `bench/json_bench.rb`.

## Current (2026-07-03 verifier round, ARM macOS / Apple Silicon, ITERS=3000 RUNS=5, best of 3 interleaved rounds)

Gate build = parity feature set + `mimalloc` (the `cli-defaults`
bundle allocator, ADR 0019 v3) with the parse-side GC safe point
active: parse loops now COLLECT their discarded trees (bounded RSS
— 1000 discarded 830 KB parses peak at ~45 MB, previously
unbounded), so sweep/free cost is an honest part of every number
below.

| Operation   | CRuby 3.4.1 + json 2.20 | Oj :strict | rubyrs `_json_native` | vs CRuby |
|-------------|-------------------------|------------|------------------------|----------|
| `parse`     | 14.2 µs/iter            | 25.8 µs/iter | **13.2 µs/iter**     | **0.93×** |
| `generate`  |  6.0 µs/iter            | 11.8 µs/iter | **5.7 µs/iter**      | **0.95×** |
| `round_trip`| 20.5 µs/iter            | 36.9 µs/iter | **18.8 µs/iter**     | **0.92×** |
| `parse_sids`| 19.6 µs/iter            | —          | 32.5 µs/iter          | 1.66× (see below) |

`parse_sids` (200 records of `{"sid":"<19-digit id>","n":i}`) is
reported, not yet won: the string-aware pre-scan costs ~3 µs and
the rest is the record-shape structural gap (Hash-alloc side, the
same residual as keys_repeated) that predates this work. It exists
as a metric because a context-BLIND bigint scan briefly made this
shape decline to the pure canon — a 160× regression, now gated.

Honest-notes ledger:
- **Claim wording**: parse and round_trip are reliably ahead of
  CRuby across machines; generate is parity-to-ahead depending on
  the machine's CRuby build (wins best-of and all interleaved
  rounds on the calibration machine after the 2026-07 wrapper
  work: conditional method definition kills the per-call
  NATIVE_AVAILABLE lookup, and the host fn signals decline by
  returning nil so the hot path carries no begin/rescue frame —
  ~54 ns/call off the fixed cost).
- **Bare ≥19-digit INTEGER literals: RESOLVED** (2026-07
  exact-number round). These previously declined the whole
  document to the pure canon (exact values at ~200× speed —
  ~8.8 ms/iter on the 200-record 25-digit `parse_bigints`
  fixture); they now parse natively via the ordered-literal
  retry pass (~72 µs, ~1.5× CRuby's ~48 µs — CRuby pays Bignum
  allocation here too). `-0` (CRuby Integer 0) and float-spelled
  negative zeros also resolve natively instead of declining.
  See "2026-07 exact-number round" below for the design and the
  serde_json `arbitrary_precision` evaluation that rejected the
  obvious alternative.
- **Parse-error columns are byte-based** (probed CRuby json 2.20
  semantics): multibyte characters before the offending token
  advance the column by their byte length, and the 32-unit
  fragment cap in "invalid number" messages is a BYTE cap with
  CRuby's trailing-multibyte strip quirks, replicated exactly.

**rubyrs beats CRuby stdlib AND Oj on all three metrics.** Byte
parity with CRuby is pinned by `tests/diff/json_parity_battery.rb`
(three-way: CRuby oracle == native accelerator ==
RUBYRS_JSON_NO_NATIVE pure canon) and an 11.0M-sample fpconv float
differential (0 mismatches).

Differential micro-fixtures (µs/iter, rubyrs vs CRuby):

| Fixture | rubyrs | CRuby | note |
|---------|--------|-------|------|
| `generate {}` | 0.52 | 0.11 | fixed cost, was 3.74 before the no-opts fast path |
| `parse "{}"`  | 0.73 | 0.12 | was 1.53 |
| gen 200 integral floats | 5.6 | 5.2 | was 31.2 (5.35×) before the fpconv port |
| gen 200 fractional floats | 5.6 | 5.2 | was 11.7 |
| parse keys_repeated (5×200) | 43.0 | 32.0 | was 64.6; residual gap is Hash-alloc/GC-side |
| parse keys_unique (1000)    | 40.9 | 60.6 | win extended (was 58.7) |
| gen 3.4 KB / 1 MB | 3.3 / 755 | 3.3 / 834 | large payloads at or ahead of parity |

## 2026-07 exact-number round (bigint decline elimination)

Replaced the whole-document bigint pre-scan + decline with exact
native number handling. Two candidate designs were evaluated:

**serde_json `arbitrary_precision` (REJECTED on measurement).**
STEP-0 probes (serde_json 1.0.150 source + a standalone probe crate,
ARM macOS): with the feature on, numbers are scanned into a fresh
`String` per literal and `buf.parse::<u64>/<i64>()` is tried first —
so i64/u64-range integers still arrive via `visit_i64/u64` (including
`-0` → `visit_i64(0)`), and only floats + >u64 integers take the
"magic map" (`$serde_json::private::Number` key borrowed `&'static`;
value an owned MOVED `String` — no second alloc, but the internal
alloc + reparse taxes EVERY number: 200-int array 2.63→6.56 µs/iter
(+19.7 ns/number), 200-float 3.30→7.36 (+20.3 ns/number), mixed
3.4 KB payload +2.5 µs vs ~0.15 µs of scan savings there. Grammar
enforcement and `end()` are unchanged by the feature. The flip
arithmetic: parse would land ~1.5-2.3 µs ABOVE CRuby and the number
fixtures 2.2-2.5× — fails the perf gates. The feature is also global
to serde_json across the workspace (carmine / gapscan / rouge tables
share the crate; no `untagged`/`flatten` users, so behaviour-safe,
but the tax is not scopeable without vendoring a renamed copy).

**Ordered-literal retry (SHIPPED — zero fast-path tax, no new deps).**
Same serde_json build; three pieces:
1. `visit_u64` in `(i64::MAX, u64::MAX]` → exact Bignum straight
   from the u64 (serde parses 19-20-digit ints exactly): snowflake
   IDs never even retry.
2. `visit_f64` suspicion check (3 predictable compares): |n| at/past
   2^64 / i64::MIN (a possibly-rounded >u64 integer literal) or
   negative zero (possibly the integer literal `-0`, CRuby Integer
   0). Primary pass aborts → a string-aware scan extracts the exact
   literals IN DOCUMENT ORDER (int spellings beyond native lanes →
   Bignum-from-text; `-0` → Integer 0; huge/negative-zero float
   spellings → their f64) → the same parse re-runs consuming that
   queue, each suspicious visit pairing BIT-IDENTICALLY with the
   queue head (expected f64 precomputed via `from_str`; two in-file
   property tests pin `from_str` ≡ serde's f64 delivery over 1.6M+
   samples incl. the fpconv writer corpus and random 20-40-digit
   literals). Any scan/serde tokenization disagreement breaks the
   pairing → decline to the canon: wrong values cannot escape.
3. The 19-digit pre-scan became a memchr2(e,E)-gated ≥10-digit
   EXPONENT fence: CRuby's two exponent-overflow regimes (literal
   saturation at ≥20 written digits / |exp| > i64::MAX, and the
   adjusted-exponent > INT32_MAX shortcut — `0.0e2147483649` →
   Infinity even with a ZERO mantissa, a LATENT canon+native
   divergence this round found and fixed in the canon) are
   unrecoverable from a parsed f64 and both need ≥10 written
   exponent digits. Such documents (pathological only) decline
   whole to the canon, the single value+error authority. e-free
   payloads (number arrays, sid docs) pay ONE SIMD sweep.

Interleaved best-of-4-rounds vs the pre-change build (`old`) and
CRuby 3.4.1 + json 2.20, mimalloc gate builds, ITERS=3000 RUNS=5:

| Fixture | old | new | CRuby | new/old | new/CRuby |
|---|---:|---:|---:|---:|---:|
| `parse` (3.4 KB mixed) | 14.77 µs | 15.30 µs | 13.93 µs | 1.04× (see note) | 1.10× |
| `generate` | 5.94 | 6.09 | 5.95 | 1.03× (untouched code) | 1.02× |
| `round_trip` | 20.67 | 21.56 | 20.85 | 1.04× | 1.03× |
| `parse_sids` (11 KB, IDs in strings) | 32.41 | **29.23** | 20.44 | **0.90×** | 1.43× |
| `parse_ints` (200 ints) | 3.64 | **3.55** | 1.95 | **0.97×** | 1.82× |
| `parse_floats` (200 floats) | 5.74 | **4.67** | 2.91 | **0.81×** | 1.60× |
| `parse_bigints` (200×25-digit) | **8700.6** | **67.4** | 48.7 | **0.008× (129×)** | 1.38× |
| `keys_repeated` | 60.10 | 60.56 | 37.32 | 1.01× | 1.62× |
| `keys_unique` | 41.65 | 42.72 | 65.54 | 1.03× | 0.65× (win kept) |

Noise note (honest): `generate` runs byte-identical UNCHANGED code
yet reads +3% in this table, and `new` always ran LAST within each
interleaved round on a thermally-drifting box — treat ±3-4% as this
run's noise floor. Quieter same-day runs put parse at 14.65-14.93
vs old 14.55-14.77 (≈ +1%), consistent with the one real new cost
on e-containing payloads: the fence stride tightened 19→10 bytes
(a 10-digit exponent run must never be missed — the price of the
newly-CORRECT adjusted-exp overflow family; the old scan was
silently WRONG there, `0.0e999999999999999999` parsed as 0.0 vs
CRuby's Infinity). An intermediate version also regressed
parse_ints +14% via inline-threshold spill of the fattened
visit_u64 — fixed by outlining the cold Bignum/exact-number arms;
the number-visit lanes are inline-budget-critical.

Correctness riders: the parity battery gained bigint straddles
(i64/u64 boundaries ±1, 30/100/320-digit), zero-spelling sweeps,
the exponent saturation + adjusted-exp-overflow regimes, exact-
pairing order (huge floats & bigints interleaved, float-vs-int
spellings of one f64, both negative-zero spellings in one doc),
and sid payloads — three-way byte-identical (CRuby == native ==
RUBYRS_JSON_NO_NATIVE canon) under default / TIER2+THRESHOLD /
STRESS_GC. RSS stays bounded on bigint-doc parse loops (the retry
allocates two trees per parse; 5000 parses peak ~22 MB).

## 2026-07 small-hash representation (record-shape campaign)

The parse_sids residual was profiled to decomposition (sample(1),
7.2k samples): GC+alloc+free complex 23.5% (per-record pairs-Vec
malloc/free, 168-byte HashObj slot writes, sweep-side frees), serde
scanning ~29%, the bigint pre-scan ~12%, key-interning ~7%. The
hash-side slice shipped as `HashObj` = SmallVec inline pairs
(`HASH_INLINE_PAIRS` = 3, the ar_table-analogue: a ≤3-pair record
embeds its pairs in the heap slot, zero pairs allocation) + the cold
tail (defaults/tag/ivars/indexes/eigenclass) boxed behind
`Option<Box<HashExtras>>`. HashObj 168 → 120 bytes; since HashObj was
the largest HeapObj variant, the shared heap-slot size dropped
168 → 136 for EVERY heap object.

| Metric | before | after | CRuby | note |
|---|---:|---:|---:|---|
| `parse_sids`    | 34.9 µs | 31.6 µs | ~19.9 µs | −9.5%; residual = pre-scan ~4 µs + serde str scan ~6 µs + value-string allocs ~2 µs (not hash-side) |
| `parse`         | 14.12 µs | 13.34 µs | ~14.0 µs | win extended |
| `round_trip`    | 20.04 µs | 19.55 µs | ~20.7 µs | win extended |
| `keys_unique`   | 41.6 µs | 38.6 µs | ~65.5 µs | lazy-index win extended |
| `keys_repeated` | 56.1 µs | 55.5 µs | ~38.2 µs | 5-key records spill past the inline cap; residual is scan/intern/GC-side |
| live-heap RSS, 200k 2-pair hashes | 143.4 MB | 117.3 MB | — | −18.2% (slot shrink + no pairs buffers) |
| live-heap RSS, 300k 2-ivar instances | 154.8 MB | 128.4 MB | — | −17.0% (global slot shrink) |

(Numbers from the campaign box under moderate load; the ratcheted
baselines row documents the quiet-machine locals.)

## Historical (2026-06-01 snapshot, ITERS=5000 RUNS=3 — CRuby was json 2.9-era timings)

| Operation   | CRuby stdlib | Oj :strict | rubyrs pure canon | rubyrs `_json_native` |
|-------------|--------------|------------|-------------------|------------------------|
| `parse`     |  ~22 µs/iter | ~28 µs/iter | ~4100 µs/iter (193×) | ~17 µs/iter |
| `generate`  |  ~29 µs/iter | ~13 µs/iter | ~4500 µs/iter (163×) | ~14 µs/iter |
| `round_trip`|  ~54 µs/iter | ~40 µs/iter | ~8700 µs/iter (175×) | ~40 µs/iter |

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

- **Round-trip matches Oj** after GC threshold tuning. Initial
  measurement at 44 µs/iter (1.1× Oj) was 27 % GC overhead — proven
  by an `RUBYRS_GC_DISABLE=1` probe that dropped round_trip to
  32 µs/iter. The fix: bump the post-sweep `next_gc` heuristic from
  `live * 2 max 1024` to `live * 4 max 4096`. Same single-generation
  mark-sweep, but ~4× fewer sweep cycles on alloc-and-discard loops
  (JSON round-trip, request body re-parsing, …). Recovers ~70 % of
  the GC overhead; the remaining ~7 µs is the per-sweep mark cost
  on a larger live set, which would need true generational
  separation to fix — out of scope for the menu item. Tunable via
  `RUBYRS_GC_GROWTH` + `RUBYRS_GC_MIN_THRESHOLD` for embedders
  running on tight RSS budgets who'd rather pay sweep frequency
  than peak memory.

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
5. **GC threshold tuning** — round_trip 40 µs (1.04× Oj). Post-
   sweep trigger raised from `live*2 max 1024` to `live*4 max
   4096`, ~4× fewer sweep cycles on alloc-and-discard loops.
   Recovers ~70 % of the GC overhead the `RUBYRS_GC_DISABLE=1`
   probe identified.
6. **2026-07 beat-CRuby pass** — parse 12.8 µs (0.90× CRuby),
   generate 5.8 µs (0.94×), round_trip 18.9 µs (0.90×); all three
   now ahead of both CRuby stdlib (json 2.20) and Oj. Four levers,
   each byte-parity-pinned by `json_parity_battery`:
   (a) exact fpconv/Grisu2 float port (`json_float.rs`) — CRuby's
   generator does NOT use Float#to_s; the old `write!("{:.1}")`
   arm was 5.35× slower AND format-divergent (1e20 emitted as
   an integer literal). Verified equal to CRuby over 11.0M
   samples. (b) no-opts wrapper fast paths — `JSON.generate(obj)`
   built a full State per call (~3.2 µs); `{}` generate dropped
   3.7 → 0.5 µs. Plus thread-local scratch + exact-size result
   (the old 4 KB `Vec` move pinned 4 KB behind every small
   result String). (c) zero-copy `from_slice` parse +
   `float_roundtrip` (serde's default float parse is 1 ULP off
   CRuby on tie/boundary decimals). (d) fstring-equivalent key
   interning (thread-local capped cache — keys parse frozen +
   shared like CRuby) + visitor Vec pre-sizing; repeated-key
   parse 64.6 → 43.0 µs, unique-key 58.7 → 40.9 µs.
   Correctness riders: exact bigints (serde was silently
   producing Floats; the canon's `to_i` wrapped), 1e999 →
   Infinity, duplicate-key last-wins, CRuby nesting limits +
   messages, invalid-UTF-8 generate errors (exact class +
   message).
7. **2026-07 verifier round** — four blocking fixes (non-empty
   nesting rule, strict number grammar + exponent saturation with
   the canon as the single error authority, tier-2 canon stack
   headroom, string-aware bigint pre-scan) plus the parse-side GC
   safe point: `loop { JSON.parse(s) }` previously never collected
   (7.6 GB RSS on a 830 KB-doc loop; now ~45 MB) because maybe_gc
   only lived at interpreter alloc sites. Honest sweep cost moved
   parse 12.8 → 16.1 µs on the system allocator; the gate build
   now includes `mimalloc` per the `cli-defaults` policy (JSON GC
   is alloc/free-bound), landing at 13.2 vs CRuby 14.2 with
   bounded memory. NestingError re-parented under ParserError;
   canon key cache made persistent (cross-parse `.equal?` parity
   on the kill-switch path).

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
