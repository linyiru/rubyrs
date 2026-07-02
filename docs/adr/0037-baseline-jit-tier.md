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
2. **Wave 2 — pay for the calls.** The walk is call-op-dominated, so attack
   the per-call-op cost inside compiled bodies: an IC-fast `t2_call` helper
   (monomorphic explicit-recv hit → resolved-method invoke, skipping the
   do_call cascade re-entry; miss → full `do_call`), fused
   LoadLocal/LoadIvar+Call receiver forms mirroring `LoadLocalCall`, and
   direct native→native dispatch when the callee is itself tier-2 (the ABI
   already permits it: callee = `(vm) -> status` after a frame push whose
   binder can be specialized per fixed arity). Exit criterion: a measurable
   walk win (≥3%) or a documented negative result.
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
5. **Wave 5 — blocks (25% of the walk).** Compile block protos on the same
   substrate, served from `invoke_block` (the frame model is identical); then
   `each`-driver → native-block direct calls.
6. **Compile-cost control (parallel).** 932ms for the walk's hot set is fine
   for daemon/batch, too much for one-shot CLI: raise the threshold under a
   process-lifetime heuristic, or move Cranelift compilation off-thread.

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
  generic ops still count); fuel-capped runs count slightly fewer ops with
  the tier on. Default runs unaffected.
- SIGINT safe-points inside a native body are deferred to the next
  call/generic op (bounded by body length; same property as the existing
  loop templates).
- Rust-stack nesting: each native serve inside a callee chain adds a Rust
  stack segment; capped at 96 native levels, deeper recursion interprets.
- One flaky default-config diff_cruby failure was observed once under heavy
  machine load and did not reproduce (green on immediate re-run, twice).
