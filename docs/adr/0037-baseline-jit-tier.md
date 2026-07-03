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
4. **Wave 4 — frame-lite entry (the (i) endgame) (SHIPPED, leaf tier — see
   "Wave 4 results" below).** For bodies whose prefix is call-free, enter
   native BEFORE materializing the frame and materialize lazily at the first
   call/raise — generalizing the zeroarg tier's measured −21..34%/call to
   the broad-admission population. Requires arg binding into native slots +
   a shadow-frame recipe for `caller`/raise; land per shape class (leaf
   predicates first).
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

### Wave 4 results (2026-07-02; FRAME-LITE — same box/method; a loaded
machine for part of the window, so every headline number is interleaved
best-of against the unmodified-HEAD baseline binary)

Design as shipped in `jit_tier2.rs` (wave 4 = "frame-lite entry", the
conservative leaf tier — option (a) of the wave plan):

- **A second, frameless entry per admitted proto.** `compile_tier2` emits a
  sibling function `t2lite(vm, self_w0, self_w1, n_pop) -> status` into the
  same module. The fixed-arity dispatch fast paths
  (`try_invoke_explicit_recv_cached`, `try_invoke_fixed_method_from_stack`
  — covering explicit-recv, self-recv, and toplevel calls; block-form and
  class-singleton sites keep the framed path) serve it INSTEAD of the
  bind+push+`t2_enter` sequence, gated on a dense `JFLAG_TIER2_LITE` byte:
  recv+args stay ON the operand stack (rooted, owned by their slots) for
  the whole run, self's raw words pass in registers as a borrowing view,
  and locals live in a native spill slot — the canonical local store while
  frameless (the wave-3 write-through discipline, retargeted). The wave-3
  virtual-stack codegen runs unchanged on top; `vm.stack` doubles as the
  GC-visible spill area for flushed temporaries and non-trivial values.
- **Admission (`t2_admit_lite`)**: method protos only, `creates_block ==
  false`, plain fixed argc ≤ 4, ≤ 48 ops, ≤ 12 locals, and every op in an
  enumerated frameless set — literals, `Locals` reads/writes/`IncLocal`,
  `LoadSelf`, `LoadIvar`/`StoreIvar` (lean register-passing lite helpers),
  `Dup`/`Pop`/`Swap`, inlineable Int binops, `CaseEqLit`, `Jump`/
  `JumpIfFalse`, `Return`, and the zero-arg `nil?` fusion. Every call form,
  constant read, block op, `$~`-writing op, and optional/kw prologue
  declines the whole body.
- **The shadow-record answer: there are no shadow records.** Any edge the
  lite mode can't finish — a failed tag/Int guard, a frozen store, a
  non-trivial ivar value, a fired back-edge poll gate, a generic op —
  MATERIALIZES the real frame (`t2_lite_materialize`: the deferred push,
  binding the CURRENT native locals into the arena, consuming recv+args off
  the stack, `base_sp`/`ip` exact) and returns BAIL; the serve site's
  caller continues the fresh frame like any interpreter push. Because
  guards run before their op's effects, this is the established mode-switch
  contract — never a replay. The soundness invariant making raise-fidelity
  trivial: **no foreign code can observe the VM while an activation is
  frameless** — the five `t2_lite_*` helpers touch only
  stack/heap/arena-append and never raise, never allocate a GC object, and
  never call Ruby (so no GC runs under native state, and `caller`/
  backtrace/binding/fiber machinery can only ever see materialized frames,
  which are canonical). Raises therefore always come from the interpreter
  re-running the op against a real frame — backtraces are byte-identical
  by construction (verified: raise-through-lite at depth 1..3, frozen
  store, ensure-in-caller-on-unwind, re-raise; the permanent
  `tier2_framelite_battery` fixture, green ×4 configs + STRESS_GC).
- **Ownership accounting at materialize** (the one subtle transfer): a
  non-trivially-tagged word in a native local slot is necessarily the
  UNTOUCHED borrow of that slot's caller-supplied arg (lite StoreLocal
  guards decline overwriting — or moving in — non-trivial values), so
  transmuting the slot words into the arena takes exactly the ownership the
  forgotten stack slot held; trivial slots carry no obligations; the recv
  slot transfers into `frame.self_val` (implicit-self entries clone through
  the borrowed words instead). Verified leak/double-free-clean under
  STRESS_GC with Str args crossing the transfer.
- **Breaker**: 32 CONSECUTIVE materialize-bails disable the proto's lite
  entry (chronic shape mismatch = wasted entry + materialize per call); a
  completed serve resets the streak. Env `RUBYRS_JIT_TIER2_NOLITE` for A/B;
  stats family `t2lite` + `tier2 lite serves/materialize_bails/kills`.
- **Found + fixed a latent wave-3 bug** (caught by the acid battery, then
  reproduced on the pristine baseline binary): `t2_ivar_set_v` raised
  FrozenError without stamping `frame.ip`, so the backtrace line pointed at
  the last ip-synced op (in practice the class line) instead of the store.
  The helper now stamps `ip` like every other raising helper.

| measurement | baseline (tier2) | wave-4 (tier2+lite) |
|---|---:|---:|
| setter (`@v = v`) explicit-recv, ns/call raw (incl. ~47ns driver) | 118.9 | **97.1 (net ≈ −30%/call)** |
| zeroarg-declined predicate (`@loc.nil?`) | 122.4 | **103.5 (net ≈ −25%/call)** |
| leaf/branchy/getter/1-arg-Int shapes | — | unchanged (already served frameless by the getter/zeroarg/int/value tiers; lite never fires) |
| walkonly big1 ×20 (interleaved pairs, three sessions) | 255–294ms band | **neutral** — pairs mixed within the ±4% noise band, best-of 255.1 vs 259.1; the −5..−15ms target NOT met |
| f1.rb e2e (adaptive) | 1.91–1.97s | 1.93–1.95s (neutral) |
| big1.rb e2e | 2.52–2.65s | 2.55–2.60s (neutral) |
| tier-2 compile bill, f1 e2e | 43.3ms / 71 protos | 46.7ms / 71 (+3.4ms for the lite sibling functions) |
| fib canary (default / jit-native / tier2) | 0.31s / 0.01s / 0.01s | 0.31s / 0.01s / 0.01s (identical) |

Serve/decline census (walkonly big1 ×10, adaptive threshold): 253 protos
reach tier-2 compile; **20 admit to frame-lite**; decline histogram of the
rest: `CallNoRecv` 75, block proto 41, `Call` 27, `creates_block` 22,
`LoadLocalCall` 19, `LoadConstChain` 19, non-plain params 12, tail others
23. Of the 20 admitted, **3 actually serve** through the hooked sites —
`cop_rule?` (10.5k/walk) + two generated `send_type?` variants (8.2k/walk)
— ≈20.5k serves/walk, **0 materialize-bails, 0 breaker kills**. f1 e2e
serves 117k lite calls (config/registry-phase leaves), also bail-free.

**Finding (the precedence lesson).** The wave plan projected the zeroarg
tier's −21..34%/call onto "the walk's hot leaf population (2-op getters,
`*_type?` predicates)" — but that population was ALREADY frameless before
wave 4: the getter fast paths serve `getter_ivar` protos on both
explicit- and self-recv, and the zeroarg/int/value NativeProto families
take the predicates and 1-arg leaves (serving precedence: specialized
frameless → frame-lite → framed tier-2). Frame-lite's DIFFERENTIAL
population is the leaves those tiers decline — StoreIvar setters,
non-Int-shaped predicates (`x.nil?`, Sym compares the value tier won't
take) — where it wins −25..30%/call, but on the walk that residue is
~20.5k frames (≤2ms, under the noise floor). The walk's remaining frame
pool sits in CALL-BEARING small bodies (the 121 `Call*`-family declines)
and constant-reading bodies (`LoadConstChain` 19): reaching them frameless
means admitting calls — a lite `t2_call` whose callee dispatch runs
against a caller-link the backtrace/`caller` walkers can see, i.e. the
mission's option (b) shadow-activation design (or materialize-before-call,
which surrenders exactly the frames that matter). That — plus const-read
admission via the IC-hit path — is the named wave-4 follow-on; the
mechanism, gates, and breaker shipped here are its substrate.

Gates: diff_cruby **1069/0** ×4 configs (default / `RUBYRS_JIT_NATIVE=1` /
tier2 / tier2+`THRESHOLD=1`; includes the newly REGISTERED wave-3
`tier2_writeback_battery` + `tier2_own_capture_rebind` — present on disk
since wave 3 but never wired into the suite — and the new
`tier2_framelite_battery`); STRESS_GC=1 green on the `jit_*_walk`
fixtures, both wave-3 batteries, the block battery, the closure fixtures,
and the frame-lite battery with the tier forced on (`THRESHOLD=1`);
rubocop f1/big1 byte-identical tier-on/off AND == a fresh CRuby oracle;
20-file prism batch == CRuby run in the same environment (the stored
`/tmp/poca_cruby20.txt` no longer matches CRUBY ITSELF on this box — every
cop errors under the direct `Team.mobilize` harness on both engines, an
environment drift predating this wave; the full-CLI runs above carry the
offense-level parity gate); 4-file `--parallel` == `--no-parallel` (p311
harness, cache root under `/private/tmp`); fib canary identical.

### Wave-4 follow-on results (2026-07-02; LITE t2_call — call-bearing
frameless bodies; same box/method, interleaved best-of against a pristine
aac6b8ad baseline binary)

Design as shipped in `jit_tier2.rs` (the wave-4 finding's named follow-on:
admit the `Call*`-family + `LoadConstChain` declines — the frame pool's
last block — into frame-lite):

- **Call ops in lite bodies.** `t2_admit_lite` now admits plain fixed-argc
  `Call`/`CallNoRecv` (argc ≤ 4), the `LoadLocalCall` fusion, and
  `LoadConstChain`. Each lowers to a `t2_lite_call_*` helper that resolves
  through the SAME site IC as the framed `t2_call` family (boundary gates
  mirrored: `bypass_visibility_once`/`force_primitive_dispatch`/refined
  names/host-fn precedence/singleton flags), then either SERVES the callee
  frameless or MATERIALIZES the caller (`ip` at the call op) and returns —
  the native body exits `T2_BAIL` and the interpreter re-runs the call
  against the real frame. Frameless-servable families (each provably
  frame-free, raise-free, and GC-free — jit_native code never runs
  `maybe_gc`, so the wave-4 "no GC under native state" invariant is
  untouched): the getter fast path, the zeroarg/int/value/objparam/fparam
  NativeProto families (cache-hit-only; compilation stays on the
  interpreted paths), the rest-predicate body-shape serve, the cascade's
  own fast-prim/fast-index arms for non-Object receivers, IC-hit
  const-chain reads — and lite→lite native chains.
- **Cascading materialization (the soundness core).** Before invoking a
  lite callee, the caller registers a `T2LitePending` record (spill-slot
  address, stack shape, self words, `resume_ip = call op + 1`, its
  `defining_class`). Any deeper materialize drains the pending stack
  OUTERMOST-FIRST — each drained frame's `trunc` adjusted for the
  recv+args slots removed below it — so `vm.frames` ends up ordered
  exactly as the interpreter would have built it, and each suspended
  caller resumes interpreted AFTER its call op with the callee's return
  value landing at the interpreter's exact stack position. On a completed
  chain the record pops unused. Depth/capacity: chains share
  `T2_MAX_NATIVE_DEPTH` (96) and re-check frame-cap headroom
  (`frames + pending + 2 ≤ 10000`; embedder `max_frames` declines
  wholesale) before deferring another frame.
- **`defining_class` hand-off.** Materialized lite frames previously
  stamped `defining_class: None` (sound while calls were declined). With
  bare calls admitted, `do_call`'s Nil-self gates can read it — so every
  lite serve entry stashes the resolving `Method`'s upgraded
  `defining_class` (`Vm::t2_lite_dc`; chain hand-offs save/restore through
  the pending record) and the deferred push stamps exactly what the framed
  push would have.
- **Fuel exactness (better than the framed tier).** `t2_poll_flags != 0`
  (fuel or deadline active) declines every lite call up front — the
  interpreted re-run charges exactly what `step()` would, instead of the
  framed tier's documented slightly-fewer-ops divergence.
- **Breaker attribution.** The consecutive-bail streak is charged to the
  proto that materialized ITSELF (in `lite_materialize_core`), not to the
  suspended callers a cascade drains — one deep-recursion event no longer
  burns a whole kill streak, and deep chains keep serving (their DONEs
  reset the streak). Kills count once (idempotent).

| measurement | baseline (tier2) | +LITE t2_call |
|---|---:|---:|
| one-self-call body (ns/call, net of driver) | 153.5 | **121.8 (−21%)** |
| four-self-calls body | 288.0 | **222.5 (−23%)** |
| getter-chain (`o.leafv + o.leafv`, explicit recv) | 177.1 | **150.6 (−15%)** |
| depth-2 lite→lite→lite chain | 223.3 | **176.7 (−21%)** |
| `LoadLocalCall`-fused getter chain | 143.0 | 146.2 (noise) |
| fib(30) whole program (tier2 config) | 0.17–0.22s | **0.11–0.13s (−35..40%)** — toplevel lite→lite recursion |
| fib (default / jit-native) | 0.36–0.42s / 0.048s | identical band / 0.046s |
| walkonly big1 ×20 (interleaved, 5 rounds) | best 250.5ms, band 250–269 | best 250.9ms, band 251–265 — **flat** (NOLITE hatch also flat: best 248.9) |
| f1.rb e2e (adaptive) | 1.68–1.71s | 1.69–1.70s (neutral) |
| big1.rb e2e | 2.30–2.34s | 2.32–2.34s (neutral) |
| tier-2 compile bill, f1 e2e | 33.7ms / 68 protos | 39.0ms / 68 (+5.3ms for the call-bearing lite siblings; e2e neutral → the threshold economics hold) |

Serve/decline census (walkonly big1 ×10, adaptive, stats build, both
binaries measured under identical conditions): tier-2 compiles 499 protos;
lite admits **64 → 253** (the 251 `Call`/`CallNoRecv`/`LoadLocalCall`/
`LoadConstChain` declines are gone); remaining decline histogram:
block proto 70, `creates_block` 58, `LoadConstStr` 45, non-plain params
14, `JumpIfArgGiven` 10, tail 49. Per walk: root lite serves **50.1k →
147.6k** (+ ~2k lite→lite chain serves), plus 109.6k in-place frameless
call serves and 3.3k IC-hit const serves INSIDE lite bodies;
materialize-bails 1.2k/walk, 128 breaker kills (chronic
interpreted-callee shapes settling back to the framed tier, by design).
Framed-path traffic moved accordingly: `t2_call` ic_fast 724k → 611k/walk
and native→native 500k → 427k/walk.

**Finding (the wave-4 lesson, one tier deeper).** Coverage tripled and the
call ops the census named are all admitted — and the WALK still did not
move. The calls that now complete frameless were already served by the
framed tier's IC-fast path at nearly the same per-call cost (the callee
was frameless there too; only the CALLER's frame is new, worth tens of ns
on bodies the walk enters ~150k times ≈ noise-floor ms), and the walk's
residual pool is where wave 5 left it: the `do_call` fallback
re-resolution (~30% of in-body calls) and the block-invocation
frame-build. Where the frame pool IS the workload — call-chain bodies
(−15..23%/call) and whole-program recursion (fib −35..40%) — the design's
value is real and measured. The walk's next levers remain the fallback
shapes and `LoadConstStr`/block-proto admission, not deeper frame elision.

Gates: diff_cruby **1073/0** ×4 configs, release profile (default /
`RUBYRS_JIT_NATIVE=1` / tier2 / tier2+`THRESHOLD=1`; includes the new
permanent `tier2_litecall_battery` — interpreted-callee raise backtraces
through a lite caller, `caller` from such a callee, deep lite→lite
recursion past the depth cap, cascade frame-order via mid-chain raise,
redefinition-after-warm re-resolve, ensure-in-caller across lite
activations, const-cache invalidation, toplevel-main and genuinely-nil
bare-call routing). The debug profile reproduces 4 PRE-EXISTING t2t1
recursion-fixture failures on the pristine baseline binary
(`stack_depth_guard`/`json_roundtrip`/`thread_coop_*` — debug Rust frames
overflow the stack before the depth guards fire; unrelated to this
change). STRESS_GC=1 + `THRESHOLD=1` green on the litecall battery, the
frame-lite/writeback/own-capture/block/call-family batteries, the closure
fixtures, and the six `jit_*_walk` fixtures; rubocop f1/big1
byte-identical tier-on/off == fresh CRuby oracles; 20-file prism batch ==
`/tmp/cruby_prism20_fresh.txt` (regenerated and re-verified against CRuby
first); fib canary above.

### Fallback-census wave results (2026-07-03; census-first — measure the
residual `do_call` fallback pool, then absorb only what the numbers rank)

Instrumentation (permanent, env-gated `RUBYRS_T2_FALLBACK_STATS=1`, the
`RUBYRS_CASCADE_STATS` debug-knob shape): (a) every `t2_call`-family
fallback edge classified post-hoc via the UNCACHED lookup (reason ×
name × receiver shape × argc — gate/settled/host-fn/lookup-miss/
non-public/closure/builtin/arity/nfa-kw-rest-blk-opt/eligible/prim-recv/
class-recv), (b) a one-shot `t2_fb_from` marker giving exact first-level
"reached the slow cascade" attribution, (c) a `t2_op` census of every
generic-helper op execution, (d) per-reason lite materialize-bail
attribution. Dumped as `t2fb-stats` / `t2op-stats` CLI rows.

**The census (walkonly big1 ×10, adaptive tier2), per walk:** 295K
t2_call fallbacks (32.6% of t2_call attempts) = **63% primitive-receiver
+ 35% universal lookup-miss shapes that `do_call`'s own mid-cascade
buckets re-serve** — the fallback tax was the `do_call` preamble, not
the serve; 94K/walk also fell through to the slow cascade (`Array#drop`
23.7K — the single hottest shape, `Object#class` 6.4K, bare
`block_given?` 6.4K, `Hash#fetch` 3.7K, `Array#freeze` 3.2K …); 57K/walk
block-form calls ALL fell past the block IC (`each` on Array 23.8K,
NFA-shaped `visit_descendants` 16.3K); generic-helper op residue:
`LoadConst`/`LoadConstChain` 86K, `CreateBlock` 38K, `ApplyCall` family
38K (`type?` 14.9K), argc-3/4 plain calls 15-20K. The wave-guess ledger:
kwargs shapes measured TINY at the t2 edge (nfa-kw 383/walk — the NFA
plan already absorbs rest/opt; kw stays interpreter-bound by design),
`LoadConstStr` 28.7K but each is one cheap helper round-trip (~0.4ms
pool), `Super` 2.4K.

Absorbed (three pieces, each gated + battery'd):

- **A. `Vm::try_walk_fast_buckets`** — the mid-cascade bucket zone
  (`===`, the walk-attributed universal/collection buckets, send-family
  #1-#3) extracted verbatim from `do_call` and probed by `t2_call_impl`
  at the cascade's exact position (after the class-singleton sibling),
  gated per receiver KIND on the str/heap/hash singleton flags.
- **B. `T2_CALL_MAX_ARGC` 2 → 8** — the framed argc cap was wave-2
  conservatism, not an ABI limit; argc 3-8 `Call`/`CallNoRecv` now take
  the IC-fast helpers.
- **C. Census-ranked new buckets** (serving both `do_call` and the t2
  probe): `Array#drop`/`freeze`/`dup`, `Hash#fetch` (1-arg hit-only +
  2-arg, blockless), `String#dup`, `Object#class`, bare `block_given?` —
  canonical arms mirrored byte-for-byte behind new method_gen-
  revalidated chain-clean flags / IC-miss gates; everything uncertain
  declines.

| measurement | baseline (5ec68cd8) | census wave |
|---|---:|---:|
| t2_call fallbacks /walk | 295K (ic_fast 611K) | **39.5K (−87%; ic_fast 879K)** |
| slow-cascade sends from compiled bodies /walk | 94K | **47K** |
| walkonly big1 ×20 (tier2, interleaved best-of, quiet box) | 252.4ms | **250.6ms** (4/5 pairs −1.5..−2.5ms) |
| walkonly big1 ×20 (tier OFF — the buckets serve interpreted dispatch too) | 256.7ms | **254.5ms** (3/3 pairs) |
| f1 e2e | 1.59–1.61s | 1.57–1.59s |
| fib canary (default / jit-native / tier2) | 0.32/0.008/0.078s | identical |

**Finding (the wave-4 lesson at the dispatch layer).** Absorbing 87% of
the fallback edge moved the wall only ~2-3ms: the re-served fallbacks
cost ~10ns of `do_call` preamble each, not the ~40ns projected — the
cascade's early arms are cheap and branch-predicted; the real per-call
money was only in the ~47K/walk that reached the slow cascade (halved
here). The walk's remaining measured pool: the 57K/walk block-form
calls falling past the block IC (the block-machinery track owns the
frame-build side; the DISPATCH side — an NFA-shaped block-form serve —
is open), the `[]=` argc-3 slice-assign (4.8K/walk, ~1.2ms, declined
here for write-semantics risk), `method_defined?` (2.7K, ~0.7ms —
below the 1ms bar), and the LoadConst/Chain helper round-trips (~0.9ms
pool). Stop rule honoured: every remaining single shape projects <1.2ms.

Gates: diff_cruby **1077/0** ×4 configs (default / JIT_NATIVE / tier2 /
tier2+THRESHOLD=1; includes the new `t2_walk_buckets_battery` —
redefinition-after-warm across all five bucket names, per-instance-
singleton flip after warm, frozen/defaulted-hash/KeyError semantics,
argc-3/4 private + wrong-arity + method_missing + `__send__` shapes);
battery three-way vs CRuby byte-identical under plain / tier2+T1 /
STRESS_GC(both) / JIT_NATIVE; rubocop f1 + big1 + 20-file prism batch
byte-identical vs FRESH CRuby oracles, tier on and off. Surfaced
pre-existing (not fixed): `instance_variable_set/get` on Array/Hash
values raises a misleading FrozenError via the reflection catch-all —
the heap ivar tables exist, only the reflection arms are unwired.

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

### Block-frame residue follow-on (2026-07-03; the binder fast arms +
LITE BLOCKS — Apple Silicon dev box, interleaved best-of vs a pristine
5ec68cd8 baseline binary, canonical feature set)

Blocks are ~25% of the RuboCop walk and their frames cost ~3.5× a
method frame. Wave 5 compiled block BODIES; this follow-on attacked the
frame BUILD itself, measure-first.

**Item 1 — the prologue breakdown.** New env-gated profiler
(`RUBYRS_BLOCK_PROF`, cntvct-tick phase counters in the
`invoke_block` family; ~0 cost when unset). Walkonly big1 ×10 + warm
(2.95M block invocations), pre-change:

| phase | total (10-walk set) | ns/inv avg |
|---|---:|---:|
| handle snapshot | 30.3 ms | 10.3 |
| gates | 3.2 ms | 1.1 |
| argprep (general binder) | 97.6 ms | 33.1 |
| locals (share/copy decision) | 55.0 ms | 18.7 |
| — of which reentrancy scan | 34.4 ms | 11.7 (19.2 frames examined/scan) |
| param bind | 21.7 ms | 7.3 |
| frame push | 16.9 ms | 5.7 |
| **prologue sum** | **224.6 ms** | **76.1** (~9.3% of walk time) |

The decisive census: 882k of the 933k general-binder runs were
`invoke_block1` FALLBACKS paying a double handle-snapshot + args-Vec +
the full kw/rest/autosplat preamble — and the population was exactly
two shapes: rest-only `|*a|` (501,201, all `n_params == 0`) and
single-Array auto-splat into `|a, b, …|` (380,913, all Array args).

**Item 2 — cheapened pushes (a117a481).** (a) ib1 fast arms for both
census shapes (bind in place; the rest arm pins only when a GC is
actually due — GC can only run inside `maybe_gc`, never `heap.alloc`);
(b) `block_is_reentrant` walks TOP-DOWN with an owner early-stop
(cells are per-invocation and never pool-recycled while a BlockHandle
holds them → exactly one owning non-block frame can be live, every
same-cell block frame sits above it; dm_share frames and share-direct
sibling blocks alias without owning and don't stop the walk). Post:
fallbacks 882,154 → 40; argprep 97.6 → 76.6 ms (the residue is almost
purely the rest-Array alloc CRuby semantics require); reent scan
34.4 → 6.3 ms; prologue sum 224.6 → 169.3 ms/set. Micro: each `|*a|`
221.0 → 170.9 ns/inv (−23%), each-pairs `|a,b|` 167.1 → 116.1 (−31%).

**Item 3 — LITE BLOCKS (cd0644fa).** The frameless tier extended to
block protos; design as shipped:

- `Proto::block_shape` (compile_block-stamped `param_start`/`n_params`/
  rest/kw-rest) gives admission/codegen the slot classification without
  a handle. Admission: plain interface (np ≤ 2, no rest/kw/&param/
  optionals, `block_body_local_start == ps + np`), `creates_block`
  declines, own region ≤ 12 slots, the wave-4 op envelope + calls.
- Entry `(vm, self_w0, self_w1, block_id)`: args pushed by the serve
  site (rooted; `n_pop = np` baked), own region in a native spill,
  self borrowed from the handle. CAPTURED-OUTER slots (< param_start)
  route through the canonical cells (`captured` / `chain_owner_cell`)
  via three raise-free/GC-free helpers — push-clone read, pop-store
  write, and an effect-free register read for fused `BinOpLocalLocal`
  operands (guards must run with no stack effect so a guard-fail can
  materialize at the op boundary); outer slots are never SSA-cached.
- Materialize pushes a real BLOCK frame through the interpreter's own
  `block_frame_locals` (share/copy + routing + writeback identity),
  own region bound from the spill under the wave-4 ownership
  accounting. `T2LitePending` records carry `blk`/`ps`, so a lite
  block suspended behind a lite→lite call chain drains as a block
  frame in exact interpreter order.
- `next` = the block's `Return` (frameless); `break`/`return`-from-
  block decline `t2_admit` wholesale and stay interpreted; `$~` needs
  nothing (blocks share the method's match data and no admitted op
  writes it). Serving lives INSIDE `invoke_block1/2` (covers the
  step_block drivers + yield argc 1–2); on a serve the sites skip
  `t2_enter_block` — the framed entry always starts at op 0. The 1-arg
  site requires `np ≤ 1`: a lone Array arg into a 2-param entry is the
  AUTO-SPLAT shape and must keep the general binder (the
  tier2_block_family battery caught exactly this).

| measurement (tier2 on) | baseline | this work |
|---|---:|---:|
| each-empty 1-param block (ns/inv, whole driver loop) | 48.6 | **24.2 (−50%)** |
| each-accumulate `t += x` (outer-cell write per elem) | 79.1 | **33.2 (−58%)** |
| hash-each `\|k, v\|` (ns/pair) | 67.1 | **36.6 (−45%)** |
| yield-driven 1-param | 120.4 | **101.8 (−15%)** |
| copy-path / re-entrant visit shapes | 78.2 / 859 | unchanged (decline correctly) |
| walkonly big1 ×15 (interleaved, 3 rounds) | 243.7–248.4 | **240.3–243.0 (−1..2%, all pairs favour)** |
| walkonly, tier OFF (binder arms + scan only) | 253.1–253.9 | 249.1–250.4 |
| f1 e2e (tier2) | 1.618–1.639 s | 1.599–1.603 s |
| fib canary (default / jit-native / tier2) | 0.126/0.004/0.030 s | 0.123/0.003/0.031 s |

Serve census (walkonly big1 ×10, stats build): 34 block protos admit
(54 decline: 24 non-plain params — the rest-blocks — 12 creates_block,
tail op-set); **126k frameless block serves**/set (124.3k DONE, 1.7k
materialize-bails), lite→lite chains 96k → 145k (block bodies chaining
into callees natively). The walk-level lesson repeats wave 4: the
mechanism wins big exactly where it fires (−45..58% on the served
micro-shapes) but the walk's block pool is dominated by shapes outside
the envelope — rest-blocks (whose Array alloc is semantic), splat
binds, and op-set declines. The named follow-ons: a rest-arm lite
entry (needs a frameless heap alloc — sound since `heap.alloc` never
collects, but weakens STRESS_GC's every-alloc discipline; decide
deliberately), `IncLocal` on outer cells, and absorbing the
`LoadConstStr` declines.

Gates: diff_cruby **1078/0** ×4 configs (default / RUBYRS_JIT_NATIVE=1
/ tier2 / tier2+THRESHOLD=1; +2 new fixtures: block_binder_fast_arms,
tier2_liteblock_battery — the latter covering capture-write visibility
from frameless blocks to sibling blocks + the defining scope,
value-carrying break through framed yielders, next-with-value, `$~`
scoping, deep re-entrant recursion, redo, non-local return, escaped
procs, mixed-tag bails, and the 2-param/auto-splat mix); every
block/closure/litecall/framelite battery green under tier2+THRESHOLD=1
and STRESS_GC; rubocop f1 + big1 byte-identical ×3 configs and the
20-file prism batch vs FRESH CRuby oracles; thread_coop_* stdout
parity (blocks are thread bodies); lib tests 234/0.
