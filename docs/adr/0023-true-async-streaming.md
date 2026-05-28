# 0023: True async streaming for `_http_server` — architecture analysis

## Status

Proposed (2026-05-28). **v2**, follow-up to
[ADR 0022 v6](0022-http-server-battery.md) which deferred
"A3β true async streaming" to this ADR. No code change in
this ADR — implementation lands in a follow-up after the
architecture decision is accepted.

**v1 → v2 changes** (three parallel reviewer rounds —
architecture / Rust safety / Ruby+Rack — surfaced 22
actionable items, deduplicated into 7 groups):

- Expanded §"Mechanics" with the **full FiberSnapshot
  field list** (was: just frames). Yielding inside `break`,
  `return`, `rescue`, or a class body now has specified
  behavior.
- Rewrote §"Correctness" CURRENT_VM_PTR claim — v1 said
  "stays valid across polls" which a literal reader could
  use to justify keeping the static set across `.await`
  (= UB). v2 says "Vm address stable; CURRENT_VM_PTR is
  set per-poll, never across `.await`."
- Added §"Fiber-scoped Vm state" table mirroring
  [ADR 0022 v6](0022-http-server-battery.md)'s
  `reset_between_requests` field-by-field discipline.
- Added §"Frame-stack swap invariants" + Miri acceptance
  test (Phase 1 item).
- **Detection order flipped** to Array → `each` → `call` →
  `to_a` (was: Array → call → each → to_a). Rack 3 SPEC
  requires `each` to win when both are present; v1 order
  would have mis-routed Rails ActionDispatch::Response.
- Stream contract expanded: `write`, `<<`, `flush`,
  `close`, `close_write`, `closed?` (v1 listed only 3).
- `body.close` invocation explicit in the poll_frame loop.
- cext re-entrancy: `Fiber.yield` traps with FiberError
  when `vm.cext_depth > 0` (was: doc-only punt).
- Phase 0 verification step added (confirm user-defined
  `def each; yield; end` composes with externally-
  supplied block on current Tier 1, before Phase 2
  starts).
- Phase 1 estimate bumped from 7-10 commits to 12-15.
- New §"Deferred Fiber surface" listing transfer / #raise
  / Scheduler / blocking-fiber distinction (was: silently
  absent — reader assumed CRuby parity).
- New risks: `ensure`-blocks don't run on client-disconnect
  in v1 (footgun); `rack.hijack` + `to_path` named as
  deferred (was: implied covered by callable-body).
- Test categories expanded: close-idempotent,
  write-after-close, empty body, headers-before-chunk
  ordering, flush no-op, fast-path no-Fiber assertion.
- Resource caps: `Config::max_live_fibers` (cap concurrent
  Fibers) + `Config::max_fiber_frame_depth` (cap per-Fiber
  stack growth) — neither is covered by the existing
  `max_live` heap-objects cap.
- Drop contract for `ResponseBody`: `Vm`-free (just lets
  the ObjId go; GC lazy reap) — otherwise a future
  eager-finalizer would trigger `&mut Vm` access during an
  await with no `VmBorrow` (UB).
- Softened the backpressure claim: hyper's poll_frame
  cooperates with bounded internal buffering (chunk-encoder
  + tokio BufWriter); not strictly socket-coupled per-frame.

## Context

[ADR 0022 v6](0022-http-server-battery.md) §"A3α — iterable
body via to_a" shipped buffered-only body handling:
`marshal_rack_response` accepts Array or any object responding
to `to_a`, but it collects every chunk via `to_a` into a single
`Bytes` before any wire byte goes out. The Rack 3 streaming
body shape `[status, headers, ->(stream) { stream.write(...);
stream.close }]` does not work today.

This ADR addresses the workloads that A3α can't:

- **Server-Sent Events (SSE)**: the client expects bytes to
  arrive as soon as the handler `puts "data: ..."` — buffered
  is unusable.
- **Large file downloads**: streaming a 1 GB file via
  `File.foreach { |line| stream << line }` should not require
  the handler to load the full file into RAM first.
- **Long-poll / Comet**: response stays open while application
  awaits an event; buffered semantics force the connection to
  close before the event arrives.

The constraint that makes this hard, from ADR 0022 v6:

> Architecture limit: Ruby is synchronous; Vm `!Send + !Sync`;
> the VmBorrow contract requires no `.await` while the Vm is
> borrowed. Current-thread tokio + a synchronous Vm means
> tokio cannot make progress while Ruby is running.

True streaming overlap requires breaking ONE of these
constraints. This ADR surveys three candidates and recommends
one.

## Decision

**Adopt Option A: Fiber-based cooperative scheduling**.
Ruby's response-producing code runs inside a Fiber; each
`stream.write(chunk)` (or equivalent) suspends the Fiber.
Rust drives the response body's `poll_frame` by resuming the
Fiber per frame, pulling the next chunk, and yielding back to
tokio between chunks. Same thread; same Vm; cooperative.

Why Option A over the alternatives (full comparison below):

1. **Fits the existing Vm shape.** rubyrs already has
   `dispatch_until(target_frame_depth)` (vm/step.rs:307) +
   `step_block` (vm/iter.rs:86) that drive bytecode to a
   specific stop point. A Fiber is `step_until_yield` — same
   pattern, different stop condition.
2. **No unsafe Send.** The Vm stays `!Send + !Sync`. Fiber
   suspension is local to the Vm — no cross-thread anything.
3. **CRuby parity.** CRuby's Fiber semantics are the canonical
   Ruby way to do cooperative scheduling. Embedders who already
   use Fiber for in-process async in their Ruby code get a
   natural mapping.
4. **Composable.** A Fiber-driven streaming body composes with
   other Fiber-aware code (queue-fed enumerators, generators,
   etc.) without further bridges.

Implementation cost: ~3-4 weeks (v2 revised from v1's
2-3 after the architecture reviewer flagged GC integration
and dispatch_until's `until_depth` rebase as larger than
estimated). The Fiber primitive itself is ~700-1100 lines
(allocate FiberSnapshot per the table below, suspend /
resume state machine with FiberStashGuard, GC mark
extensions, cext_depth counter); the `_http_server` wiring
is another ~300-400 lines (stream-contract host fns +
detection-order rewrite + body.close invocation).

## Architecture survey

### Option A — Fiber cooperative scheduling (recommended)

```
[hyper poll_frame] → resume Fiber → Ruby runs to next yield →
suspend, return chunk → tokio writes to socket → loop
```

Mechanics:

- New `Value::Fiber(ObjId)` heap variant. The FiberObject
  carries the **full FiberSnapshot** (see §"Fiber-scoped
  Vm state" below for the field-by-field table) plus
  bytecode IP + proto `Rc<Proto>` clone, last-yielded
  value, and state enum
  `Created | Running | Suspended | Returned`.
- `Fiber.new { |stream| ... }` allocates; returns
  `Value::Fiber(id)`. The block body is pinned via
  PinGuard for the Fiber's lifetime so GC doesn't sweep
  the closure mid-suspend.
- `fiber.resume` (driven from Rust) swaps the current Vm
  state with the Fiber's snapshot via a Drop-guarded
  `FiberStashGuard` (mirrors ADR 0013's `VmPtrGuard`):
  the stash is restored on panic mid-swap so panic
  safety holds. Then runs `dispatch_until` with stop
  condition `Suspended` (extends step.rs:307's existing
  frame-depth-based stop), and returns the yielded value
  when the Fiber hits `Fiber.yield(v)`.
- `Fiber.yield(v)` is a Vm primitive op: stashes `v` on
  the Fiber's "last yielded" slot, sets state Suspended,
  returns control via dispatch_until's normal exit.
- Resume restores the full FiberSnapshot. Bytecode picks
  up right after the `Fiber.yield` call site.
- **cext re-entrancy guard**: `Fiber.yield` traps with
  `FiberError("can't yield from cext")` when
  `vm.cext_depth > 0`. New per-Vm counter incremented at
  cext entry / decremented on exit. Without this,
  yielding inside a cext frame would unwind through
  C code that doesn't expect Ruby control flow — UB.

v1 surface (additions surfaced by Ruby/Rack reviewer):

- `Fiber.current` — returns the currently-running Fiber
  (sentinel "root" Fiber when outside any Fiber body).
  Required by the `stream.write` host fn to route writes
  to the correct Fiber.
- `Fiber#alive?` — boolean; `false` once state is
  `Returned`. Required by category-3 unit tests.

See §"Deferred Fiber surface" for what's explicitly NOT
in v1.

`_http_server` integration:

- New Rack-3 body shapes recognised: see §"Detection
  order" below — `each`-shape body wraps in a Fiber; the
  Fiber body IS the `invoke_method(body, :each, &chunk_yielder)`
  call (NOT once-per-resume; once-total with yields
  suspending). For callable-shape body, the Fiber body
  IS `invoke_method(body, :call, [stream])`.
- Hyper's `BoxBody` `poll_frame` is wired to:
  1. Inside a `VmBorrow`, resume the Fiber.
  2. If Fiber yielded a chunk → wrap as `Frame::data(bytes)`,
     return `Poll::Ready(Some(Ok(frame)))`. Drop the VmBorrow.
  3. If Fiber returned (finished) → invoke `body.close`
     (also under VmBorrow + Fiber-wrapped if it can yield;
     Rack 3 SPEC: "servers MUST call close"). Then return
     `Poll::Ready(None)`. **Idempotency**: a second close
     call must not raise (Rack SPEC; BodyProxy double-
     close pattern).
  4. If Fiber raised → return `Poll::Ready(Some(Err(...)))`,
     hyper drops the connection. **User `ensure`-blocks DO
     run on the raise path** because the Fiber's body
     unwinds Ruby-side. They do NOT run on client-
     disconnect mid-stream (see Risks §1).
- Between poll_frame calls, tokio is free to write the
  yielded chunk to the socket. The Vm is NOT borrowed
  between polls — the FiberObject holds the suspended
  state; the live Vm's state is whatever was there
  pre-resume (the FiberStashGuard restores on exit).
- The `VmBorrow` contract holds because each `poll_frame`
  is a complete synchronous Vm reborrow that finishes
  (yields or returns) before the `.await` resumes.

Correctness:

- Each poll_frame is a fresh, time-disjoint Vm reborrow
  under ADR 0013. The **Vm's address is stable**;
  `CURRENT_VM_PTR` is set per-poll inside the synchronous
  `VmBorrow::with(...)` and cleared on scope exit —
  **NEVER held set across `.await`**. v1 of this ADR
  wrote "CURRENT_VM_PTR stays valid across polls" which
  a literal reader could turn into UB; v2 clarifies that
  only the Vm's memory location persists, not the
  reborrow proof.
- The Fiber's frame stack lives in the heap when
  suspended (in `FiberObject.snapshot.frames`) and gets
  GC roots via the `Vm::gc_mark` callback walking
  **both** `vm.frames` AND every alive
  `FiberObject.snapshot.{frames,stack,pinned}`
  unconditionally — the union is safe, and removes the
  hand-off window where a frame stack is "live in either
  location but not both" mid-swap.
- A Fiber that's mid-execution when its connection
  closes: hyper drops the `ResponseBody`, which drops
  the `Value::Fiber(ObjId)`, which the GC eventually
  reaps. `Drop for ResponseBody` is **`Vm`-free** — it
  just releases the ObjId so the next GC cycle reaps the
  FiberObject. No `&mut Vm` access during the drop,
  because drop can fire on the tokio task between polls
  with no `VmBorrow` proof.
- **No two FiberObjects' snapshots are simultaneously
  swapped into `vm.*`** — invariant enforced by the
  fact that only one `FiberStashGuard` can be alive at
  a time per Vm (compile-time via `&mut Vm`).
- **No `&Frame`/`&mut Frame` cache held across resume.**
  step.rs's `dispatch_until` re-fetches `last_mut()` per
  op, so this holds today. Inline cache state in step.rs
  is per-frame so the swap clears it implicitly. Phase 1
  must add an audit checkbox.

Backpressure: hyper's HTTP/1 `SendResponse` cooperates
with bounded internal buffering (chunk-encoder + tokio
`BufWriter`); not strictly socket-coupled per-frame.
Net: backpressure exists but a slow client may take 1-2
extra chunks before Fiber resumption pauses. Acceptable
for streaming workloads; documented for embedders.

### Option B — Cross-thread Vm + channel

```
[Vm thread] runs Ruby, writes chunks to channel
[tokio thread] reads from channel, writes to socket
```

Mechanics:

- Move the Vm to a dedicated OS thread on every request (or
  to a thread pool of dedicated Vms).
- An `unsafe impl Send for VmHandle {}` wrapper marks a
  newtype as Send, with the invariant "only the dedicated
  thread ever touches the inner Vm".
- Tokio request handler sends a request to the Vm thread via
  a synchronous channel, receives chunks back via a bounded
  channel.

Why rejected:

- **!Send violation requires unsafe**. The `unsafe Send` is
  load-bearing for the design. Any future code path that
  accidentally accesses the Vm from the wrong thread is UB —
  same Stacked Borrows class of footgun as ADR 0013's
  CURRENT_VM_PTR but worse because we lose the time-disjoint
  guarantee.
- **Thread-per-request is expensive**. Spawning an OS thread
  per request defeats the point of tokio. A Vm thread pool
  with a queue would work but adds queue + scheduling
  complexity comparable to implementing Fiber, with worse
  safety properties.
- **Doesn't compose with other Vm-bound code**. The user's
  host fn closures, cext code, etc. all run on the dedicated
  thread; any time a user closure wants to interact with
  tokio (logging, metrics, etc.) it has to channel-hop.
  Option A keeps everything on one thread.

The single concrete benefit (faster build — ~1 week vs
2-3 weeks for Fiber) is not worth the safety + ergonomics
regression.

### Option C — Buffered "callable body" (API-shape only)

```
body.call(stream)  # stream.write queues to Vec<Bytes>
                   # When body.call returns, drain to wire
```

Mechanics:

- Accept Rack 3 callable body shape `body.call(stream)`.
- Provide `stream` as a host fn-backed Ruby object with
  `write`, `close` methods.
- `write` appends to a `Vec<Bytes>` buffer.
- After `body.call` returns, drain buffer to hyper.

Why rejected:

- **Not actually streaming**. Wire emission happens after
  Ruby finishes — identical client-observable behavior to
  A3α. Users with SSE / long-poll use cases get no benefit.
- **API parity is shallow**. We'd advertise Rack 3 streaming
  body support but fail any test that checks for incremental
  delivery (which most real Rack 3 tests do).

The only justification would be "do the easy API now, real
streaming later" — but that risks freezing the wrong contract
(e.g., users might assume their writes are flushed when
written, and design their code accordingly).

## API surface

The chosen design adds **two** body shapes to A3α's Array +
to_a path:

1. **Rack-3 enumerable body** (chunked-streaming case): an
   object responding to `each` that yields `String` chunks
   one at a time. Wrapped in a Fiber by the marshal layer;
   each `yield` becomes a Fiber suspension → hyper frame.

2. **Rack-3 callable body** (full streaming case): an object
   responding to `call(stream)`. The stream is a host-fn-
   backed object implementing the **full Rack 3 stream
   contract**:

   | Method | Behavior |
   |--------|----------|
   | `write(chunk)` | Synchronously yield the chunk through the Fiber → hyper frame |
   | `<<(chunk)` | Alias for `write` |
   | `flush` | No-op (chunks already flushed per-write by the Fiber suspension); MUST NOT raise |
   | `close` | Set internal state to closed; trigger Fiber return on next pump |
   | `close_write` | Synonym for `close` in v1 (full-duplex distinction deferred) |
   | `closed?` | Boolean reflecting close state |

   After `body.call` returns OR `stream.close` is invoked,
   the Fiber returns and hyper sees EOF.

   Real Rack 3 implementations (Puma's `Puma::NullIO`,
   Falcon's `Async::HTTP::Body::Writable`) cover the same
   surface; v1 matches the minimum compatible subset.

Detection order in marshal_rack_response:

```rust
match body {
    Value::Array(_)                       => array_path(),         // A3α
    v if responds_to(v, :each)            => each_fiber_path(),    // new (Rack 3 preferred)
    v if responds_to(v, :call)            => callable_fiber_path(),// new
    v if responds_to(v, :to_a)            => to_a_array_path(),    // A3α
    _ => Err("Rack body must be Array or respond to each/call/to_a"),
}
```

**Order rationale** (v1 of this ADR had `call` before
`each` — wrong): Rack 3 SPEC requires `each` to win when
both are present. Rails `ActionDispatch::Response`,
`Rack::BodyProxy`, `Sinatra::Response`, and
`Enumerator::Lazy` all respond to both `each` and
(sometimes) `call`. The Rack convention is "`each` is the
preferred shape; `call` is for the new streaming case
only." v2 matches.

Single-element Array `[String]` keeps the fast non-Fiber
path: no Fiber allocation for `[200, headers, ["hello"]]`-
shape bodies (95% of hello-world tests). Phase 2 must add
a perf-regression test asserting NO Fiber is allocated for
this shape.

**Block + Fiber interaction**: inside a `Fiber.new { |x| ... }`
block, bare `yield` raises (no enclosing method body), and
`block_given?` returns false. This matches CRuby. Users
who write `Fiber.new { yield chunk }` thinking it suspends
the Fiber get a clear error pointing at `Fiber.yield`.

**`body.close` invocation**: Rack 3 SPEC §"Body" requires
the server to call `body.close` after iteration completes
(normal path) OR after a raise propagates out (cleanup
path). v1 wires both:
- Normal completion → Fiber returns → server calls
  `body.close` (also Fiber-wrapped if it may yield).
- Raised exception → server calls `body.close` THEN
  surfaces the exception to hyper.
- `body.close` raising → ignored (Rack convention: cleanup
  errors don't override the original raise reason).

## Fiber-scoped Vm state

Yielding inside a method call, a `rescue`, a `break` /
`return` target, or a class body must NOT corrupt the
resumer's control-flow state. The FiberSnapshot stashes
the following Vm fields on `Fiber.yield` and restores on
`fiber.resume`. This table mirrors ADR 0022 v6's
`reset_between_requests` discipline.

**Must stash + restore (12 fields)**:

| Vm field | Why |
|----------|-----|
| `frames: Vec<Frame>` | The active call stack. Each Frame carries locals, IP, return target. |
| `stack: Vec<Value>` | Operand stack. The current expression's partial values. |
| `pinned: Vec<Value>` | GC pins. Fiber-scoped pins must follow the Fiber. |
| `class_stack` | Open class context. Yielding inside `class Foo; ...; end` must leave the resumer's class context unchanged. |
| `class_visibility_stack` | Tracks `private`/`public`/`protected`. Same reasoning. |
| `method_return: Option<Value>` | `return` from a method-body Fiber must NOT unwind the resumer's Rust frame. |
| `break_signaled: bool` | Same shape as method_return for `break`. |
| `pending_loop_transfer` | `next`/`redo` flow markers. |
| `suppress_call_result_push: bool` | Op-sequencing flag from step.rs. |
| `bypass_visibility_once: bool` | `send` private-dispatch flag. |
| `last_match: Option<...>` (regex feature) | `$~` is Fiber-local per CRuby. |
| `last_read_line: Option<Value>` | `$_` is Fiber-local per CRuby. |

**Must stash (pending exception)**:

| State | Notes |
|-------|-------|
| In-progress unwind exception | If yield happens inside `rescue`, the in-progress exception object must be Fiber-local. The active rescue frame is part of `frames`; the exception itself needs a separate stash slot. |

**DO NOT stash (process-wide)**:

| Vm field | Why |
|----------|-----|
| `heap` | Object identity is process-wide; ObjIds in the snapshot stay valid. |
| `interner` | Symbols are process-wide. |
| `classes`, `constants` | Class definitions don't fork. |
| `globals` | `$foo` is global per CRuby (only `$~` and `$_` are Fiber-local). |
| `host_fns`, `cext_*` | Registration is process-wide. |
| `cext_depth` | Counter is the Vm's "am I in cext?" view; if a Fiber resumes inside cext, that fact about the resumer remains true. |

**Phase 1 acceptance criterion**: a unit test for each
field in the "Must stash" list that proves the resumer's
state is unaffected by yielding inside the corresponding
context. E.g.: `yield_inside_break_does_not_propagate_break_to_resumer`.

## Frame-stack swap invariants

Three properties the implementation must maintain:

1. **At most one FiberObject's snapshot is installed in
   the Vm at any time.** Enforced compile-time via the
   `FiberStashGuard<'a>` borrowing `&'a mut Vm` — only
   one can exist per Vm.
2. **Swap is panic-safe.** `FiberStashGuard` holds the
   stashed state in its own struct; `Drop` restores on
   panic mid-swap. No transient `vm.frames = Vec::new()`
   window observable from a panic handler.
3. **GC roots cover both locations.** `Vm::gc_mark`
   walks `vm.frames` (the currently-installed snapshot
   if any) AND every alive `FiberObject.snapshot.*`
   (all suspended Fibers'). Union over locations =
   safe; no hand-off window where roots are lost.

**Miri acceptance test** (Phase 1 item): synthetic test
extending `vm::cext::miri_tests` (ADR 0013) that exercises
`mem::swap(&mut vm.frames, &mut fiber.snapshot.frames)`
under both Stacked Borrows and Tree Borrows, ensuring the
swap pattern preserves the SharedReadWrite tag on
subsequent reborrows.

## Deferred Fiber surface

The following CRuby Fiber API is **NOT** in v1; reader
should not assume parity. Add to a future ADR as
embedders' use cases surface.

| API | v1 status | Reason |
|-----|-----------|--------|
| `Fiber.transfer` (symmetric transfer) | Deferred | Significantly complicates the stash/restore semantics (transfer doesn't return to the resumer; it transfers to a third party). |
| `Fiber#raise` (external interruption) | Deferred | Needed for clean cancellation on client disconnect; see Risks §1. |
| Fiber Scheduler (Ruby 3.0+ `Fiber.set_scheduler`) | Deferred | Out of scope; rubyrs doesn't have an event-loop abstraction to plug a scheduler into. |
| Blocking vs non-blocking fiber distinction | Deferred | Tied to Fiber Scheduler; not meaningful without it. |
| Fiber-local variables (`Fiber[]` Ruby 3.2+) | Deferred | Easy to add but no v1 consumer. |

## Test strategy

The hard part of streaming tests is proving CHUNKS ARRIVED
INCREMENTALLY, not just that the final body matches. Two
test categories:

### Category 1 — chunked wire format

Client reads `Transfer-Encoding: chunked` framing and asserts
each chunk's bytes arrive WITHIN a bounded time of the
preceding chunk. Pattern:

```rust
// Server-side: handler emits chunks at 200ms intervals
let body = ChunkedBody.new(["a", "b", "c"], delay_ms: 200)
[200, {"Content-Type" => "text/event-stream"}, body]

// Client-side: read with 100ms read_timeout per chunk;
// successful read of "a" then "b" then "c" proves
// chunks arrived incrementally rather than the server
// having buffered all 3 + sent them in one syscall after
// 600ms total.
```

### Category 2 — Vm progress under backpressure

A slow client (set TCP recv buffer small, don't `read` for
seconds) should NOT cause the server's accept loop to stall.
The Vm should be available to handle other connections'
requests during the slow upload's write.

Subprocess test: 2 parallel clients, one slow, one fast.
Fast client's request must complete within seconds even
while slow client is mid-transfer.

### Category 3 — Fiber primitives (unit)

Independent of `_http_server`:

- `Fiber.new { ... }.resume` returns the block's value when
  no yield happens.
- `Fiber.yield(v)` followed by `.resume` returns `v` on the
  resume side.
- Resume-after-return raises FiberError.
- Resume from inside a cext callback raises FiberError
  ("can't yield from cext"); the `cext_depth` guard fires.
- Fiber GC: a Fiber object with no remaining references
  gets collected; running fiber bodies that hold
  references stay pinned.
- `Fiber.current` returns the active Fiber inside a body;
  returns a sentinel "root" Fiber at top level.
- `Fiber#alive?` is true before resume, true between
  yields, false after the body returns.
- One unit test per "Must stash" Vm field (see
  §"Fiber-scoped Vm state"): yield inside `break`,
  `return`, `rescue`, class body, regex `$~` context,
  etc. — assert resumer state is unchanged.
- Exception propagation: a raise inside the Fiber body
  re-raises in the resumer's frame (via direct `.resume`).

### Category 4 — Rack 3 stream contract (unit + subprocess)

- **Idempotent close**: `body.close` called twice does NOT
  raise (Rack SPEC requirement; Rails BodyProxy double-
  closes routinely).
- **Write-after-close**: `stream.write(chunk)` after
  `stream.close` raises IOError.
- **Empty stream**: `body.call(stream)` that calls
  `stream.close` without any `write` produces zero data
  frames and clean EOF — no hang.
- **Headers-before-chunk ordering**: a streaming body that
  delays its first `write` by 500ms produces visible
  status + headers on the client BEFORE the chunk
  arrives. Asserts hyper isn't buffering headers waiting
  for the first body byte.
- **`flush` no-op**: `stream.flush` MUST NOT raise even
  though it's a no-op.
- **`close_write` synonym**: `stream.close_write` is
  equivalent to `stream.close` in v1.
- **Single-element Array no-Fiber**: `[200, h, ["hi"]]`
  body produces a 200 OK without allocating a Fiber. Use
  a host fn probe to count Fiber allocations across the
  request; assert zero.
- **`body.close` invoked on normal completion**: a body
  with a side-effecting `close` (e.g. setting a flag)
  proves the marshal layer called it.
- **`ensure` runs on body's natural return**: handler with
  `def each; yield "a"; ensure; @closed = true; end` —
  the ensure DOES run.
- **`ensure` does NOT run on client disconnect mid-stream**
  (documented footgun; see Risks §1). Test asserts the
  ensure-set flag stays unset when the client drops mid-
  stream. The negative test guards against accidental
  semantic changes; the footgun stays known.

## Migration

A3α's Array + to_a path stays. The two new shapes (`call` /
`each`) are opt-in by virtue of detection-order — apps that
return Array continue to use the fast non-Fiber path.

Documentation:

- ADR 0022 v6's "A3α" section gets a follow-up note pointing
  here.
- README "HTTP server battery" gains a new sub-section
  "Streaming responses" with SSE + large-file examples.
- `examples/sse_server.rb` ships alongside `prefork_server.rb`.

Backwards compatibility: NONE BROKEN. The Array path is
unchanged; new shapes are additive.

## Implementation plan

**Phase 0 — Tier 1 verification** (~1 commit):

0. Confirm user-defined `def each; yield "a"; end` composes
   with an externally-supplied block under current rubyrs
   semantics (SUBSET.md line 95 says method-body `yield`
   works; this commit pins it with a regression test
   targeting the each-block-yield path A3β depends on).
   If a gap surfaces, lift it BEFORE Phase 2 starts.

**Phase 1 — Fiber primitive** (~12-15 commits, revised
upward from v1's optimistic 7-10):

1. `FiberSnapshot` struct enumerating all "Must stash"
   Vm fields (see §"Fiber-scoped Vm state"). One commit
   to define the type + ensure new Vm fields get a
   compile-time prompt to declare their snapshot disposition.
2. `Value::Fiber(ObjId)` + `FiberObject { snapshot,
   proto, ip, last_yielded, state }` in heap.rs.
3. `Fiber.new { |...| ... }` allocator.
4. `FiberStashGuard<'a>`: Drop-guarded swap helper.
   Panic-safe restore on Drop.
5. Frame-stack swap + dispatch_until's new
   `until: SuspendOrDepth(...)` stop condition.
6. `Fiber#resume` + `Fiber.yield` host fns / bytecode ops.
7. `Fiber.current` + `Fiber#alive?`.
8. cext_depth counter on Vm; Fiber.yield trap when nonzero.
9. Fiber GC: `gc_mark` walks both `vm.frames` AND every
   `FiberObject.snapshot.{frames,stack,pinned}`.
10. `Drop for ResponseBody` is Vm-free contract — pin a
    test that proves drop doesn't touch `&mut Vm`.
11. `Config::max_live_fibers` + `Config::max_fiber_frame_depth`;
    enforce caps at Fiber alloc + frame-grow boundaries.
12. **Miri acceptance test**: synthetic test extending
    `vm::cext::miri_tests` for the frame-stack swap
    pattern (Stacked Borrows + Tree Borrows).
13. Unit tests covering Category 3 above.
14-15. Slack for review iterations / unforeseen
    interactions with the existing dispatch / step.rs
    inline-cache code.

**Phase 2 — `_http_server` integration** (~5-7 commits,
revised upward to account for `body.close` invocation +
stream contract surface):

16. Detect `responds_to?(:each)` and `:call` in
    `marshal_rack_response` per the fixed v2 detection
    order (each → call → to_a).
17. Stream writer object with the full 6-method contract:
    `write`, `<<`, `flush`, `close`, `close_write`,
    `closed?`.
18. Build a hyper `BoxBody` whose `poll_frame` resumes a
    request-scoped Fiber.
19. Wire `body.close` invocation on normal completion +
    raise propagation paths (Rack 3 SPEC).
20. Fast-path assertion: `[String]` body NEVER allocates
    a Fiber (perf-regression guard).
21. Subprocess tests for Category 1 (chunked wire
    timing) + Category 2 (Vm progress under backpressure).
22. Unit + subprocess tests for Category 4 (Rack 3
    stream contract).

**Phase 3 — Docs + example** (~2 commits):

23. README "Streaming responses" subsection + SSE example
    in body. Updated `_http_server` platform matrix if any
    platform-specific Fiber behavior surfaces.
24. `examples/sse_server.rb` + manual verification +
    a follow-up to ADR 0022 v6's A3α note pointing here.

**Total**: ~19-25 commits over 3-4 weeks. Each commit
atomic with tests per the existing process.

Total: ~12-17 commits over 2-3 weeks of focused work. Each
commit atomic with tests per the existing process.

## Risks + open questions

1. **`ensure` blocks DON'T run on client disconnect mid-
   stream** (footgun). When a client drops the connection
   while the Fiber is suspended, hyper drops the
   `ResponseBody`, which releases the Fiber's ObjId. GC
   eventually reaps the FiberObject, but **the Ruby-side
   bytecode never resumes** — `ensure` blocks attached to
   in-progress methods don't fire. Embedders who write
   `db.transaction { stream_results(stream); }` will see
   transactions leak open on client disconnect.

   v1 mitigation: document loudly in README "Streaming
   responses" section + the SSE example's comments.
   Suggest pattern: use `Connection#on_close` style
   callbacks (deferred — not in v1) OR don't put critical
   cleanup in `ensure` for streaming handlers.

   v2 of A3β: add `Fiber#raise` so the server can inject
   an exception on disconnect that propagates through
   `ensure` cleanly. Deferred because `Fiber#raise`
   semantics interact with suspended Fiber state in
   subtle ways.

2. **Fiber memory cost**: each in-flight streaming response
   carries a FiberSnapshot. For 1000 concurrent SSE
   connections, that's 1000 snapshots pinned. The new
   `Config::max_live_fibers` cap bounds this (default:
   tied to `max_concurrent_requests`). `Config::max_fiber_frame_depth`
   bounds per-Fiber stack growth — without it a malicious
   script could deepen one Fiber's frame stack to OOM
   while staying under the heap object cap.

3. **Fiber + cext interaction**: covered by the
   `cext_depth` counter (see §"Mechanics"). v1 traps
   `Fiber.yield` inside cext frames with FiberError; v2
   may relax this if a use case surfaces, but the
   default conservative behavior holds.

4. **`rack.hijack` (full + partial hijack)**: Rack 3 SPEC
   §"Hijacking" defines `env['rack.hijack'].call → io` for
   full hijack and `response[1]['rack.hijack'] = ->(io) {}`
   for partial hijack. Both deferred to a future ADR.
   WebSocket gems (faye-websocket etc.) require full
   hijack; embedders using those should know A3β does
   NOT cover this.

5. **`to_path` body** (sendfile optimization): Rack SPEC
   defines `body.to_path` as a hint that the server may
   sendfile(2) the path instead of iterating chunks.
   Useful for large static-file responses. Deferred.

6. **Trailer support**: HTTP/1.1 chunked trailers (key-
   value pairs after the final chunk) would need explicit
   API surface. Not in v1; deferred.

7. **HTTP/2 + HTTP/3 cross-task resume**: per ADR 0022
   v6's `_http_server_h2` (not yet implemented), h2's
   `poll_frame` may run on a different task than the
   response future. Cross-task Fiber resume re-introduces
   the `!Send` problem A3β avoids on HTTP/1's
   current-thread LocalSet. **The h2 wiring is NOT
   guaranteed to transfer unchanged** — needs a separate
   design pass when h2 lands. v1 of this ADR only commits
   to HTTP/1.

8. **Phase 0 prerequisite gap risk**: the Phase 0
   verification commit confirms user-defined `def each;
   yield; end` composes with externally-supplied block.
   If a Tier-1 gap surfaces (e.g. nested `each`-block
   coalescing semantics), Phase 2 must wait. Mitigation:
   Phase 0 is the first commit, so the prerequisite is
   checked before any A3β-specific code is written.

## Alternatives considered (summary)

| Option | Cost | Risk | Recommendation |
|--------|------|------|----------------|
| A: Fiber cooperative scheduling | 3-4 weeks (revised from v1's 2-3 — see Phase 1's 12-15 commit estimate) | LOW (fits existing Vm) | **Adopt** |
| B: Cross-thread Vm + channel | ~1 week | HIGH (unsafe Send, thread cost) | Reject |
| C: Buffered callable body | ~1 day | NEG (no real streaming, freezes wrong contract) | Reject |

## Revision log

- **2026-05-28 — v2 (this revision).** Tightening pass
  after three parallel reviewer rounds (architecture /
  Rust safety / Ruby+Rack). 22 actionable items
  surfaced; deduplicated into 7 groups. See "v1 → v2
  changes" at the top for the full diff. No accept-
  blockers; recommendation stays Option A. Major
  changes: full FiberSnapshot field table,
  CURRENT_VM_PTR per-poll wording, detection order
  flipped to each-before-call, stream contract
  expanded to Rack 3's 6-method surface, cext_depth
  guard for Fiber.yield, ensure-on-disconnect named
  as footgun, rack.hijack + to_path explicitly
  deferred, Phase 1 estimate revised 7-10 → 12-15.
- **2026-05-28 — v1.** Initial analysis + recommendation.
  Identified Option A (Fiber) over B (cross-thread Vm)
  and C (buffered callable). Phase 1-3 implementation
  plan sketched.

## Related

- [ADR 0022](0022-http-server-battery.md) v6 §"A3α" deferred
  this work to this ADR.
- [ADR 0013](0013-cext-vm-aliasing.md) — VmBorrow contract
  + CURRENT_VM_PTR; the Fiber design must hold this contract
  per-poll.
- [ADR 0019](0019-tier2-tier3-boundary.md) v3 Rule 7 — battery
  ADRs must specify their own VmBorrow semantics; A3β
  inherits 0022 v6's contract with the per-poll qualification
  added here.
