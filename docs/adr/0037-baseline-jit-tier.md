# ADR 0037 — The baseline-tier compiler: frame-keeping direct-threaded substrate, strengthened in waves toward frame elision

Date: 2026-07-02
Status: accepted (wave 1 shipped as a working prototype; decision settled by
measurement, not projection)

## Context

The RuboCop cop walk runs at ~274ms on rubyrs vs 60–63ms CRuby. The 2026-07-02
per-method self-time profile proved the remaining cost is method-BODY
execution spread FLAT across ~300–600 small methods: 80% of walk time needs
286 protos, 90% needs 603; 84% of the walk's ~877k method frames are
fixed-arity 0–2 bodies; blocks are 25% of the walk (83k invocations); frame +
dispatch machinery is an estimated 45–70% of the interpreter gap. No
template/hot-list tier can win this — the baseline tier must admit MOST
bodies and be profitable on them.

Three candidate architectures were on the table:

- **(i) no-deopt helper-call Cranelift tier** — native locals, entry-time
  admission, helpers implement full interpreter semantics per op, no mid-body
  deopt ever; needs a lazily-materializable frame for raise/backtrace/block
  capture.
- **(ii) frame-keeping direct-threaded tier** — keep the real interpreter
  frame + operand stack; compile the op SEQUENCE (branch targets native,
  operands immediate, per-op logic via the interpreter's own arms).
- **(iii) extend the specialized `compile()`** — the incumbent; its decline
  histogram on the walk (value-shape 2161, `CallNoRecv/1` 1318, arity 791,
  `CreateBlock` 528, …) says how far it is.

This ADR settles the decision **with a working prototype of (ii) run against
the real RuboCop walk**, because (ii) was the only candidate whose correctness
story allowed reaching end-to-end byte-identical RuboCop within the exercise —
and because its measured result cleanly quantifies what (i) must and must not
do.

## The prototype (shipped, env-gated `RUBYRS_JIT_TIER2=1`)

`crates/rubyrs/src/jit_tier2.rs` + a `t2_enter` serving hook after the
method-frame push at the seven dispatch fast-path sites. Design:

- The serving site pushes the REAL frame exactly as the interpreter would
  (args already bound — fixed, optionals, rest, blocks all reuse the existing
  binders), then runs the compiled body instead of returning to the loop.
- **Codegen**: one Cranelift function per proto, `(vm) -> status`. Branch
  targets are native blocks. `Jump` is a native jump; `JumpIfFalse` /
  `JumpIfArgGiven` / `JumpIfKwArgGiven` call 3-line condition helpers and
  branch natively. Twelve hot, call-free, trap-free ops (`LoadLocal`,
  `StoreLocal`, `LoadIvar`, literal pushes, `Dup`/`Pop`/`Swap`, `LoadSelf`)
  call per-op-kind helpers that mirror `step()`'s arms exactly, with operands
  baked as immediates. **Every other admitted op** runs through one generic
  helper: set `frame.ip = i+1`, execute the interpreter's own
  `step(op, pidx)` (op fetched through a baked `*const Op` into the proto's
  stable code buffer), and if the op pushed a callee frame, drive it to
  completion with `dispatch_until` — the same nested-driver pattern the Rust
  iterator primitives use.
- **THE invariant (why there is no deopt discipline at all)**: at every op
  boundary the machine state IS the interpreter state — real frame, real
  operand stack, `ip` current. A mid-body exit (control signal, fiber yield,
  non-local walk) simply returns to the master loop, which CONTINUES the
  frame at `ip`. A bail is a mode switch, never a re-execution; side effects
  never replay. This also makes every future strengthening step (inline an
  op, elide a store, cache a local natively) individually landable: the bail
  path is always one op boundary away.
- **Traps**: a raising op or callee stores its `Trap` in `Vm::t2_trap`
  (status 3); the serving site re-`Err`s it and the OUTER dispatch loop runs
  the exact rescue/unwind machinery it would have run interpreted.
  `AlreadyCaught` flows through unchanged. Backtraces are byte-identical
  (verified explicitly: `caller`, `e.backtrace`, rescue-and-continue).
- **Admission** (`t2_admit`): decline ONLY ops that can retarget `frame.ip`
  into this frame behind the native code (rescue/ensure installation +
  `Raise`/`EndEnsure`) and the non-local-exit ops owned by the master loop
  (`ReturnMethod` — blocks only in practice — and `Break`). Everything else
  admits: all call forms, `CreateBlock`/`CallBlock`/`Yield`, massign splats,
  globals, constants, `Def*`, optional/kwarg prologues.
- **GC**: zero surface. No Value ever lives in native code; everything stays
  in `vm.stack`/frame locals (real roots). STRESS_GC is clean by
  construction (and verified).
- **Plumbing**: compile on the 8th frame entry; verdicts in the dense
  `jit_flags` byte (`JFLAG_NO_TIER2`/`JFLAG_TIER2_HAS`); serves via a dense
  fn-ptr table (no hash probe); native nesting capped at 96 (deeper Ruby
  recursion falls back to the flat loop, which has no Rust-stack cost);
  `RUBYRS_JIT_TIER2_ONLY=names` allowlist for controlled A/B;
  `RUBYRS_JIT_STATS=1` reports serves/bails/compile time (family `tier2`).

### A latent bug class the tier exposed (fixed)

**Push-then-mutate**: three `new` arms (Class.new, Class.new-with-block,
Module-subclass new) set `swap_return` on `frames.last_mut()` AFTER
`invoke_method` — sound only while "push frame" and "run frame" are separated
by returning to the loop. With the frame completed inside the push, the stamp
hit the CALLER's frame (RuboCop load corrupted via `Set#-`). Fixed by checking
frame growth and post-hoc replacing the pushed return value — the discipline
`Op::CallAset` already had. Audit found no other post-push mutation sites
(`instance_eval_definee` targets a block frame, never tier-2-served).

## Measurements (Apple Silicon dev box; best-of-3 interleaved; RuboCop 1.88 walk on big1.rb)

### Coverage on the real walk (the headline the tier had to hit)

| metric | value |
|---|---|
| protos attempted → compiled | 3579 → **3534 (98.8%)** |
| native serves per walk | **~900k ≈ ALL method frames** (the 877k figure) |
| bail rate | **0.15%** (mode switches, not re-runs) |
| compile cost | 932ms total, ~264µs/proto (one-time) |
| representative methods | all 7 admitted (node_parts, loc?, loc_is?, arguments, each_child_node incl. its block-taking rest-arg shape, space_after?, valid_name?) |

### Per-method ns/call (interpreter vs tier-2, real rubocop-ast nodes)

| method (ops) | interp | tier-2 | Δ |
|---|---:|---:|---:|
| IfNode#node_parts (45) | 3025 | 2992 | −1.1% |
| Node#loc? (16) | 697 | 707 | +1.5% |
| Node#loc_is? (15) | 1417 | 1449 | +2.2% |
| ParameterizedNode#arguments (9) | 892 | 902 | +1.2% |
| Descendence#each_child_node (19, block-taking) | 1965 | 2052 | +4.4% |
| Token#space_after? (7) | 2198 | 2338 | +6.4% |

### Controlled shape decomposition (quiet machine)

| shape | interp | tier-2 | Δ |
|---|---:|---:|---:|
| leaf predicate, no calls (`@t == :send`) | 252 | 243 | −3% |
| branchy leaf, no calls | 286 | 259 | −9% |
| one self-call | 349 | 327 | −6% |
| four self-calls | 1027 | 828 | **−19%** |
| fib(30) whole program (interp 0.33s) | 0.33s | 0.26s | **−21%** |

### Whole-walk A/B (walkonly, 30 iters, warm past compile threshold)

| config | best | verdict |
|---|---:|---|
| tier-2 off | 273.1ms | baseline |
| tier-2 on, broad (~100% frames served) | 274.9ms | **+0.7% ≈ neutral** |
| tier-2 on, ONLY the 5 representative methods (83k serves/walk) | within noise (±2%) | neutral |

### Gates (all green)

- diff_cruby **1057/0** under default, `RUBYRS_JIT_NATIVE=1`, AND
  `RUBYRS_JIT_TIER2=1` (broad admission — the whole suite runs through the
  tier).
- Byte-identical RuboCop stdout with the tier on: f1.rb, big1.rb, and the
  20-file prism batch vs the CRuby-produced expectation.
- STRESS_GC=1 clean on walk fixtures with the tier on (tier verified firing).
- fib canary: default and `RUBYRS_JIT_NATIVE=1` identical to the parent
  commit's binary (interleaved A/B); default-config walk unchanged.

## The finding that settles the architecture

**Eliminating the fetch/decode/dispatch loop with full-semantics helpers is
worth ~zero on the walk.** Serving essentially every method frame natively
moved the walk +0.7% (noise). The interpreter's op loop — the thing
direct-threading removes — costs only a few ns/op, and the per-op helper-call
layer gives most of it back. Where the calls are self-sends whose callees are
also tier-2 (recursion, call chains), the tier wins −6..−21%; where bodies are
dominated by builtin/getter sends and explicit-recv dispatch (the RuboCop
shapes), it is neutral. The walk's cost therefore lives EXACTLY where the
profile said: the do_call dispatch machinery per call op, frame
setup/teardown, and the value work inside op arms — none of which (ii) as
such removes. Corroborating datum: the existing frameless zeroarg tier's
serves measure −21..34% per call on the same walk's predicates — frame
elision is where the money is.

### Judgement of the three candidates on the evidence

- **(iii) extend specialized `compile()`** — REJECTED as the baseline tier.
  Its decline tail is arbitrary op soup (the histogram's value-shape 2161 +
  CreateBlock 528 entries need general codegen, not more shapes); every new
  shape adds bespoke codegen + deopt discipline; admission is all-or-nothing
  per body. It stays what it is: the loop/leaf specialist that already beats
  YJIT on its shapes, serving BEFORE tier-2 in precedence (frameless serves
  first; tier-2 sees only what they declined).
- **(ii) naive direct threading** — as an endpoint, REJECTED: measured
  neutral on the target workload. As a SUBSTRATE, ACCEPTED and shipped: 98.8%
  admission incl. block-taking/optional/rest bodies, zero-deopt correctness
  by construction, zero GC surface, all-gates-green in one wave, and the
  bail-anywhere property that makes incremental strengthening safe.
- **(i) no-deopt helper-call tier with native locals** — ACCEPTED as the
  DESTINATION, rejected as a from-scratch build. The prototype's central
  negative result applies to (i)'s naive form too: helper-per-op cannot pay
  for itself, so (i) only wins where ops INLINE and frames are elided — and
  building that all-at-once means solving raise/backtrace shadow frames,
  block capture, and GC rooting of native-held Values in one cliff, exactly
  the "complete or worthless" shape ADR 0034 warns against. Tier-2's
  op-boundary-exact state lets every (i) ingredient land one op at a time.

## Decision

**Adopt the tier-2 substrate as THE baseline tier and strengthen it in waves
toward (i)-grade code, keeping the bail-anywhere invariant at every step.**
Serving precedence stays: frameless specialized protos (int/value/objparam/
zeroarg/getter) → tier-2 → interpreter.

### Wave plan (each wave ships behind the same gates: byte-identical rubocop
f1/big1/20-file, diff_cruby 3-config green, STRESS_GC, fib canary)

1. **Wave 1 — substrate (SHIPPED, this ADR).** Serving hooks, admission,
   statuses, stats, allowlist, the push-then-mutate fix. Walk-neutral;
   recursion/self-call chains −6..−21%.
2. **Wave 2 — pay for the calls (SHIPPED — see "Wave 2 results" below).**
   The walk is call-op-dominated, so attack the per-call-op cost inside
   compiled bodies: the IC-fast `t2_call` helper family (`Op::Call` /
   `Op::CallNoRecv` argc 0–2 + the `LoadLocalCall` fusion — the census's
   84%), the `t2_return` frame-pop shortcut, direct native→native dispatch
   via the serves' trailing `t2_enter`, and the adaptive compile threshold
   (item 6). Exit criterion was "a measurable walk win (≥3%) or a documented
   negative result": measured −4.6..−6.1% vs the wave-1 tier, −1.3% vs the
   interpreter on the walk; call-chain microshapes −13% and fib −13%, both
   on top of wave-1's wins.
3. **Wave 3 — inline the ops.** Direct Cranelift lowering of the hot simple
   ops against the pinned `Value` layout (ADR 0035): operand-stack push/pop
   of immediates without helper calls, locals cached in native slots BETWEEN
   effectful ops with write-back at bail/call boundaries. This is where the
   leaf/branchy −3..−9% grows.
4. **Wave 4 — frame-lite entry (the (i) endgame).** For bodies whose prefix
   is call-free, enter native BEFORE materializing the frame and materialize
   lazily at the first call/raise — generalizing the zeroarg tier's measured
   −21..34%/call to the broad-admission population. Requires arg binding into
   native slots + a shadow-frame recipe for `caller`/raise; land per shape
   class (leaf predicates first).
5. **Wave 5 — blocks (25% of the walk) (SHIPPED — see "Wave 5 results"
   below).** Compile block protos on the same substrate, served from
   `invoke_block` (the frame model is identical); then `each`-driver →
   native-block direct calls (NOT yet shipped — named by the wave-5
   measurement as where the remaining block pool lives).
6. **Compile-cost control (SHIPPED with wave 2).** 932ms for the walk's hot
   set is fine for daemon/batch, too much for one-shot CLI. Shipped as an
   adaptive entry threshold `1024 + 16 × body_ops` (compile cost is ~linear
   in ops, ~8µs/op; per-entry savings are tens-to-hundreds of ns, so payback
   needs O(1000) entries). Env overrides: `RUBYRS_JIT_TIER2_THRESHOLD`
   (absolute), `_BASE`, `_PEROP`; `RUBYRS_JIT_TIER2_NOCALL` reproduces the
   wave-1 tier for A/B. Snapshot-caching compiled code was assessed and
   deferred: Cranelift modules bake absolute helper/`code`-buffer addresses,
   so a cross-run cache means relocation machinery — off-thread compilation
   is the cheaper next step if the residual matters.

### Wave 2 results (2026-07-02; same box/method as wave 1, best-of-3
interleaved rounds)

Design as shipped in `jit_tier2.rs`: `t2_call`/`t2_call_norecv`/
`t2_call_local` execute the plain fixed-argc call ops by front-loading the
EXACT serve the `do_call` cascade would reach for each receiver shape —
gates (`bypass_visibility_once`/`force_primitive_dispatch`/refined name) →
explicit recv: `try_fast_primitive`+`try_fast_index` (guarded on the
Str/Array/Block/Hash singleton flags being clear and name ≠ `call`, which
makes the skipped cascade prefix provably inert) → the explicit-recv IC
path → the class-singleton IC path; implicit self: `host_fns` precedence →
toplevel-IC serve for main/Nil self (the fib shape) → the self-recv IC
path. ANY decline falls back to the interpreter's own arm (full `do_call`
with `trailing_hash_positional`), so misses re-resolve identically —
method_gen redefinition, megamorphic sites, method_missing, visibility
errors, arity errors all take the canonical path. `t2_return` mirrors the
step arm's direct pop (`$~`/`$!` restore, `swap_return`, the
recycle_frame_aux discipline); ensure/class-body shapes route through
`step` itself. Native→native = a serve's trailing `t2_enter` running the
callee's compiled body inside the caller's native frame — no new ABI, so
every existing serve family (getter, zeroarg, int/value/objparam,
rest-pred, NFA-plan) composes unchanged.

| measurement | off | wave-1 tier | wave-2 |
|---|---:|---:|---:|
| walkonly big1 ×30 (broad, threshold 8) | 255.2ms | 268.5ms (+5.2%) | 256.1ms (−4.6% vs w1) |
| walkonly big1 ×30 (adaptive threshold) | 255.2ms | — | **252.0ms (−1.3% vs off, −6.1% vs w1)** |
| four-self-calls shape (ns/call) | 889 | 745 | **645 (−27% vs off)** |
| one-self-call shape | 179 | 155 | 141 |
| branchy leaf | 125 | 107 | 103 |
| fib(30) | 0.319s | 0.251s | **0.218s (−32% vs off)** |
| f1.rb e2e (adaptive) | 1.58–1.61s | ~1.95s (w1, threshold 8) | **1.56–1.57s (no regression)** |
| big1.rb e2e (adaptive) | 2.28–2.31s | ~2.9s (w1, threshold 8) | **2.24–2.28s** |
| 20-file prism batch e2e | 8.82–8.98s | — | 8.69–8.88s (≈neutral) |
| tier-2 compile bill, f1 e2e | — | 268ms / 848 protos | **20.7ms / 97 protos** |
| tier-2 compile bill, big1 e2e | — | 636ms / 2446 protos | **35ms / 233 protos** |

Counters (walkonly ×10, adaptive): IC-fast serves 8.42M, fallbacks 2.64M
(24%), native→native entries 4.94M, compile 111ms/686 protos. At threshold
8 (full coverage): IC-fast 11.98M, fallbacks 4.16M, native→native 7.80M.

Findings that shape wave 3:

- **The call path pays for the substrate, not (yet) for the interpreter.**
  Wave-2 calls recover the wave-1 tier's +5% per-op-helper overhead and land
  the broad tier at −1.3% vs the interpreter on the walk. The reason the
  walk win is small where microshapes win −27%: the serving-arms work
  already made the cascade's Object-receiver prefix cheap, so skipping it
  saves ~10-25ns/call, while the frame push + arg bind + `t2_enter` — which
  wave 2 deliberately kept — still dominates per-call cost. The money
  remains where the wave-1 report put it: frame elision (wave 4) and inline
  ops with native-cached locals (wave 3).
- **Adaptive threshold beats full coverage ON TOP of killing the compile
  bill** (252.0 vs 256.1ms): compiling only genuinely hot bodies avoids
  paying the per-op helper layer on cold/tail bodies — coverage is not the
  metric, per-frame profit is (wave 1's lesson, now load-bearing in the
  default config).
- **Remaining fallbacks (24%)** are block-passing sites (`CallBlock` —
  wave 5), kw/splat forms, non-fixed-arity misses, and receiver shapes
  outside the mirrored serves; each fallback costs the full cascade plus
  one wasted IC probe. Wave 3's inline-op work should also lower
  `t2_call`'s own entry cost (helper call + gates ≈ the same magnitude as
  the savings on already-fast serves).

### Wave 3 results (2026-07-02; same box/method — interleaved best-of-3
rounds; the walk table re-measured on a quiet machine)

Design as shipped in `jit_tier2.rs` (wave 3 = "inline the ops"):

- **Inline lowering against the pinned `Value` layout (ADR 0035).** The
  codegen views a `Value` as two raw i64 words (tag byte at 0, bool at 1,
  u32 payloads at 4, i64/f64/Rc at 8) and lowers the hot op set with NO
  helper call on the fast path: literals (Int/Float/Sym/Bool/Nil),
  `Locals::Stack` `LoadLocal`/`StoreLocal`/`IncLocal*`, `LoadSelf`,
  small-Int `+`/`-`/`*` (native overflow flags), Int comparisons, same-tag
  Int/Sym `==`/`!=`, truthiness + fused compare-and-branch,
  `Dup`/`Pop`/`Swap`, and `x.nil?` on a virtual receiver (same
  `prim_reopen_mask` gate as `try_fast_primitive`'s universal arm).
  `LoadIvar` (Object-self guard via the baked oid) and `StoreIvar` /
  `CaseEqLit` (literal + gates mirrored) run through LEAN register-passing
  helpers that skip the operand-stack round trip. The op set was chosen
  from the measured dynamic op mix of the RuboCop walk (walkonly big1,
  interpreter): LoadLocal 16.7%, JumpIfFalse 13.4%, Call 8.9%, Dup 6.2%,
  Return 5.9%, StoreLocal 5.1%, Pop 5.0%, CaseEqLit 4.8%, LoadIvar 3.7%,
  LoadConstInt 3.6%, BinOp family 4.2%, LoadSymbol 1.0% — the inline set
  covers ~87% of executed ops.
- **The virtual stack + boundary discipline.** Operand-stack values live in
  SSA registers between ops (a compile-time "virtual stack" of raw value
  words, all trivially-tagged — no `Rc` payloads, so copy=clone and
  discard=free); every point foreign code can observe the VM (any helper
  call, every branch edge, every bail) MATERIALIZES the virtual stack
  first. `vm.stack`'s ptr/len/cap are read through empirically-probed Vec
  field offsets (probe failure disables the inline lowering entirely —
  helper emission is the universal fallback, never a miscompile).
- **Local read cache, write-through stores.** `Locals::Stack` slots (method
  protos with `creates_block == false` only — block protos and
  closure-carrying bodies keep capture-routed helpers, which is what makes
  the shared-binding closure model (4f6ef741) correct by construction) are
  cached in SSA within a basic block. STORES ARE WRITE-THROUGH: the
  canonical slot is updated at the store op itself (with an inline
  drop-guard on the old value's tag), so the frame is canonical at every
  instruction and the cache is a pure READ cache — the "write-back before
  boundary" matrix degenerates to "nothing to write back", eliminating the
  GC-rooting and Binding-observability hazard classes outright. What
  remains of the matrix: (a) reads of the frame's locals by foreign code —
  `Kernel#binding` / string-`class_eval` snapshots (inside call helpers;
  slots canonical ✓), backtraces (ip-only), the GC root walk (slots
  canonical ✓, and cached ObjIds are copies of rooted slot values —
  mark-sweep never moves objects); (b) writes INTO the frame's locals by
  foreign code — PROVEN IMPOSSIBLE: callees cannot capture a
  `Locals::Stack` frame (no `CreateBlock` in the body, by construction),
  rubyrs `Kernel#binding` snapshots into `Vm::binding_locals` and nothing
  ever writes a Binding back into a frame (`extract_binding_ctx` seeds a
  fresh eval frame; `Binding#local_variable_set` does not exist), so the
  cache survives call boundaries (the fib win) and is invalidated only by
  the op's own slow edges and by generic-`t2_op` ops (conservative).
- **Slow edges = straight-line resume, never re-execution.** Guards
  (unexpected tag, Int overflow, gated fast-flags) run BEFORE any effect of
  their op; a failed guard materializes the virtual stack and hands the
  REST of the segment — through the segment-ending branch — to `t2_resume`
  (per-op `step()`, the interpreter's own semantics), which reports the
  landing ip so native code re-enters at the right block. Bail/trap
  contracts are unchanged from waves 1–2.
- **Backward-branch poll.** Loop back-edges gate on three inline byte loads
  (`control_signals`, the new `t2_poll_flags` fuel/deadline mirror —
  recomputed per serve — and the baked `interrupt_pending` address); when
  any fires, a helper charges `check_fuel` and BAILs for signals/interrupts
  (delivery stays owned by the dispatch loop heads). Fuel-capped runs now
  charge per back-edge instead of per op inside compiled bodies — an
  extension of the wave-1 "specialized ops don't charge fuel" note.
- **`t2_call` entry cost (item 3): per-site settled-verdict byte.**
  `Vm::t2_site_verdict` (dense by cache_id) counts consecutive fast-probe
  declines per call site; at 16 the probe is skipped (straight to the
  cascade) with a ~1/1024 periodic retry keyed off `op_counter`. On f1 e2e
  this cut fallback probes 719k → 482k (−33%); ~426k further calls left
  the family entirely via the `nil?` fusion.

| measurement | off | wave-2 | wave-3 |
|---|---:|---:|---:|
| leaf predicate (`@t == :send`), ns/call | 251.3 | 241.7 | **207.2 (−18% vs off, −14% vs w2)** |
| branchy leaf | 281.9 | 258.0 | **220.3 (−22% vs off, −15% vs w2)** |
| one self-call | 355.3 | 315.9 | 294.2 (−17%) |
| four self-calls | 1033.4 | 764.1 | **636.2 (−38% vs off, −17% vs w2)** |
| fib(30) whole program | 0.33s | 0.25s | **0.16s (−52% vs off, −36% vs w2)** |
| walkonly big1 ×30 (adaptive) | 267–284ms | 259–287ms | 267–309ms — **indistinguishable** (box noise ±4% > any delta; the −5% target NOT met) |
| f1.rb e2e (adaptive, tuned threshold) | 1.68–1.73s | 1.69–1.78s | 1.70–1.78s (**neutral**; interleaved pairs −0.01..+0.05s) |
| big1.rb e2e (adaptive, tuned threshold) | 2.35–2.54s | 2.36–2.49s | 2.38–2.52s (neutral) |
| tier-2 compile bill, f1 e2e | — | 27.4ms / 97 protos | 65.7ms/97 at the wave-2 threshold → **30.5ms / 50 protos** at the wave-3 default |
| tier-2 compile bill, big1 e2e | — | 35ms / 233 protos | 53.5ms / 116 protos |

(The microshape harness's ns/call includes the un-tiered driver block +
its yield dispatch — a fixed ~150ns constant — so the per-METHOD delta is
substantially larger than the headline percentage; the leaf/branchy
targets read against that floor. Wave 5's block compilation removes the
constant.)

Findings that shape wave 4:

- **Inline ops pay exactly where the profile said**: shapes dominated by
  local/ivar/compare/branch work (leaf −18%, branchy −21%, fib −52%)
  collapse, and the call-chain shape stacks the savings of every frame in
  the chain (four-calls −38%). The remaining per-call cost is frame push +
  arg bind + `t2_enter` — wave 4's frame-lite entry remains the money.
- **The write-through simplification held**: zero write-back machinery, no
  deopt, no GC surface, and the maximal-exposure gate (every method
  compiled, `THRESHOLD=1`) runs the full 1065-fixture diff suite green,
  including the new write-back battery (binding hostages, mid-body raises,
  ensure, deep recursion past the native cap, method redefinition
  mid-loop, Int overflow promotion, Str locals/ivars) and the own-region
  capture-rebind fixtures.
- **The walk did NOT move** — the honest negative result of this wave. The
  walk's in-body time sits in ops whose cost is a hash lookup or a full
  dispatch either way (`LoadIvar`/`CaseEqLit`'s FxHashMap probes, the call
  family), so inlining the surrounding locals/branches shaves only a few
  ns per frame while the bigger native bodies add icache pressure and the
  2.4× compile bill; net: within the noise band of wave-2/off. This
  CONFIRMS the wave-1/2 diagnosis with a stronger instrument: the walk's
  remaining pool is frame push + arg bind + `t2_enter` per call (wave 4,
  frame-lite) and flat-ivar object layout (ADR 0035 phases 4/5) — not
  per-op execution.
- **Compile bill grew ~2.4×/proto** (bigger IR: guards + slow-edge
  blocks), so the adaptive threshold was re-tuned to keep the wave-2
  payback rule "entries ∝ compile cost": defaults moved 1024+16/op →
  **2048+64/op**, which is one-shot-e2e NEUTRAL (f1: 30.5ms bill, 50
  protos, 629k IC-fast serves; at the old threshold wave-3 f1 e2e
  regressed +2.6%). Hot workloads still compile within their first few
  thousand entries (fib crosses its ~3k-entry threshold in the first
  0.2% of its 1.6M calls). Off-thread compilation remains the next lever
  if the one-shot bill ever matters again.
- **The backward-edge poll fixed a wave-2 gap**: SIGINT now terminates an
  all-native loop (`while true; i += 1; end` compiled at threshold 1 hung
  under the wave-2 tier; wave 3 exits with the proper Interrupt), and
  fuel-capped runs charge once per loop back-edge inside compiled bodies
  (previously per generic-helper op; both are documented divergences from
  per-op interpreter fuel counts).

### Wave 5 results (2026-07-02; blocks — same box/method, interleaved
best-of rounds against the unmodified-HEAD baseline binary)

Design as shipped: block protos compile through the SAME `compile_tier2`
entry (identical admission — a block body containing `break`
(`Op::Break`) or `return` (`Op::ReturnMethod`) declines and stays
interpreted; `next` compiles as the block's `Op::Return` → the native
`t2_return` pop; `redo` is an intra-proto backward jump, admitted and
native). Serving = `t2_enter_block`, a twin of `t2_enter` called RIGHT
AFTER the interpreter's own block binders pushed the frame — param
binding (autosplat, `|*a|` whole-capture, kw/kw-rest with CRuby's
missing/unknown error order, `&b` block-params, lambda strict arity,
numbered params) is `invoke_block`/`invoke_block1`/`invoke_block2`'s
by construction, so there are NO binding-shape declines at all. Hooked
sites: the `Op::Yield` arm (extracted verbatim to `Vm::do_yield`, still
the single source of truth for the break/fiber/non-local postlude), the
`step_block`/`step_block1`/`step_block2` drivers (covers every Rust
iterator: `Array#each`/`map`/…, `Hash#each`, `Integer#times`), and the
three `proc.call` arms. NOT hooked (cold, or post-push frame mutation):
fiber body first-resume, at_exit/fork-child/signal-trap/host-API
invocations, `invoke_block_with_self` (instance_eval/class_eval — its
`instance_eval_definee` post-push stamp is exactly the wave-1 bug
class). In compiled METHOD bodies, `Op::Yield`/`Op::ApplyYield` lower
to `t2_yield` (ip advance + fuel + `do_yield`) and
`Op::CallBlock`/`Op::CallNoRecvBlock` to `t2_call_block` — the arm's
`do_call_block` already front-loads the block-form IC + trailing
`t2_enter`, so a compiled caller → compiled callee → compiled block is
a full native chain, with yield case (a) breaks landing as
frames-below-entry → DONE (the break value placed as the method's
return) and case (b)/non-local-return/fiber as BAIL. Shipped alongside:
the yield BINDER fast path — `do_yield` routes argc 1/2 through
`invoke_block1`/`invoke_block2` (no per-yield args-Vec allocation, no
general-binder pass), an interpreter-side win too. Env:
`RUBYRS_JIT_TIER2_NOBLOCK` disables block serving for A/B;
`RUBYRS_JIT_STATS=1` adds `tier2 blocks invocations/native_serves/
native_yield_serves`.

| measurement | baseline (tier2) | wave-5 (tier2) |
|---|---:|---:|
| walkonly big1 ×30 (adaptive; quiet-phase interleaved, best) | 266.1ms | **263.3ms (−1.1%)**; 14/15 interleaved pairs across three sessions favour wave-5 (typical pair delta −3..−8.5ms) |
| walkonly, `NOBLOCK=1` on the wave-5 binary | — | 268.9ms best ≈ baseline (hook plumbing is free) |
| `each`-driver block loop (ns/elem) | 104–107 | **79.5–81.8 (−22%)** |
| nested `each` (ns/elem) | 114–115 | 97.5–99.5 (−13%) |
| visit_descendants-shaped yield/proc recursion (ns/node) | 948–951 (interp: 917) | 872–911 (−6..8%, now BELOW interp — the wave-1 tier had been a loss on this shape) |
| fib canary (default / jit-native / tier2) | 0.312s / 0.008s / 0.240s | 0.306s / 0.008s / 0.234s |
| f1.rb e2e (adaptive) | 1.66–1.72s | 1.68–1.69s (band) |
| big1.rb e2e | 2.34–2.35s | 2.30–2.34s |
| 20-file prism batch e2e | 9.27–9.44s | 9.13–9.23s |
| tier-2 compile bill, f1 e2e | 25.6ms / 97 protos | **28.5ms / 130 protos** (+2.9ms for 33 block protos) |

Counters (walkonly ×10, adaptive, stats build): block invocations at
the hooked sites 1.82M, served natively **1.44M (79%)**, of which 267k
native-yield serves; in-body t2_call ic_fast 9.41M / fallback 4.05M
(30%) / native→native 6.64M (wave-2: 8.42M / 2.64M / 4.94M — compiled
block bodies route ~1.4M more call ops through the IC-fast family at a
somewhat higher miss rate).

**Finding (the wave-1 lesson holds for blocks).** 79% of block
invocations served natively moves the walk only −1%: block-BODY
fetch/decode was never the money — the walk's flat profile with wave 5
on shows the residue in `do_call` fallback re-resolution (30% of
in-body calls; `step` + `do_call` + `lookup_method_cached` +
`walk_module` dominate self-time) and in the per-invocation
frame-build machinery wave 5 deliberately kept (`invoke_block1`'s
handle snapshot + locals cell + 24-field frame push; the reentrancy
scan). Where the block body is a tight loop over IC-served calls the
tier now wins −13..−22% (and the yield-recursion shape went from a
regression to a win), which is exactly the microshape family RuboCop's
blocks are NOT. The wave-5 walk target (−15ms) is therefore NOT met by
body compilation alone — the remaining block pool belongs to (a) the
`each`-driver → native-block DIRECT call (bind-once/frame-reuse, the
follow-on this ADR already names), (b) wave-3 inline ops lowering
`t2_call`'s own entry cost, and (c) absorbing the fallback shapes.

Gates: diff_cruby **1066/0** ×4 configs (default / `RUBYRS_JIT_NATIVE=1`
/ `RUBYRS_JIT_TIER2=1` / tier2+`THRESHOLD=1`); the
`tier2_block_family` battery (break-through-compiled-yielder incl.
two-level, next-with-value, nested/splat/multi-arg yield,
captured-local rebinding, copy-path isolation, $~ transparency,
ensure-on-break, `&block` forwarding, proc-vs-lambda arity, kw/block
params, numbered params, escaped-proc rebinding, redo,
Enumerator/StopIteration) byte-identical under all 4 configs AND
STRESS_GC; `closure_capture_nested` under tier2 `THRESHOLD=1` (+
STRESS_GC) identical; rubocop f1/big1 tier-on/off byte-identical + a
fresh CRuby oracle; 20-file prism batch == the CRuby expectation;
4-file `--parallel` == `--no-parallel`; STRESS_GC on the
`jit_*_walk` fixtures with the tier forced on.

### Amdahl honesty

At 84–100% frame coverage the measured wave-1 win is 0 — coverage without
per-frame savings is worth nothing, which is the point of settling this by
prototype. The projection that matters: frame+dispatch is 45–70% of the
~211ms gap (≈95–148ms/walk); waves 2–4 target that pool with mechanisms whose
per-call savings are already measured in this codebase (IC-hit direct invoke,
frame elision at −21..34%/call on leaf shapes). Capturing half the pool is
~50–75ms — walk ~200–225ms — and full native→native chains (wave 2's ceiling)
are required to approach CRuby's 60ms; nothing in this ADR assumes that
arrives in one step.

## Correctness discipline (the contract every wave must keep)

1. At every op boundary reachable by bail, VM state = interpreter state
   (frame, stack, `ip`). Native-only state (cached locals, elided frames)
   must have an explicit materialization recipe at every bail/trap/call
   boundary.
2. No re-execution, ever: bail continues, never replays (side effects are
   already visible). This SUBSUMES "deopt can only change speed" — there is
   no deopt.
3. The frame's `rescues` must stay empty (admission), so no unwind can
   retarget `ip` into a natively-running body.
4. Helpers hold no Rust references across VM re-entry; fn pointers/entries
   are copied out of growable tables before running (`NpEntry` discipline).
5. Values live only in rooted VM structures until a wave introduces
   native-held Values, which must then follow the loop-template rooting
   rules (heap.rs's alloc-never-collects-mid-hostfn does NOT hold across
   helpers that re-enter the VM).
6. Post-push frame mutation is forbidden pattern-wide; any arm that pushes a
   frame and then touches `frames.last_mut()` must check frame growth first
   (the three `new` arms are the template).

## Known gaps / notes

- `check_fuel` is not run for the 12 specialized simple ops (calls and all
  generic ops still count — the wave-2 `t2_call`/`t2_return` helpers charge
  the same fuel tick `step()` would, before any stack effect); fuel-capped
  runs count slightly fewer ops with the tier on. Default runs unaffected.
- SIGINT safe-points inside a native body are deferred to the next
  interpreted segment (bounded by body length; same property as the existing
  loop templates and unchanged by wave 2 — tier-2 call ops never ran the
  dispatch loop's per-op interrupt check in wave 1 either; interpreted
  callee frames driven by `dispatch_until` still check per op).
- Rust-stack nesting: each native serve inside a callee chain adds a Rust
  stack segment; capped at 96 native levels, deeper recursion interprets
  (verified by the wave-2 fixture's depth-3000 native→native recursion).
- A `t2_call` decline costs one wasted IC probe before the full `do_call`
  re-resolves (~24% of in-body calls on the walk); acceptable today,
  shrinks as waves 3/5 absorb more shapes.
- Wave 2 exposed a PRE-EXISTING jit-native (not tier-2) refinement gap: a
  compiled body's baked obj-dispatch cross-call bypasses the
  `refined_method_names` detour (see `JIT_KNOWN_DIVERGENCES` in
  `diff_cruby.rs`, fixture `tier2_call_refined`; reproduces on the pristine
  pre-wave-2 binary under `RUBYRS_JIT_NATIVE=1`). Fix belongs in
  `jit_native.rs`'s PIC fill/lookup.
- One flaky default-config diff_cruby failure was observed once under heavy
  machine load and did not reproduce (green on immediate re-run, twice).
