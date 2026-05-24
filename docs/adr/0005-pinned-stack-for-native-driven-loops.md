# 0005: Pinned stack for native-driven loops

## Status

Accepted (2026-05).

## Context

Built-in iterators like `Array#map`, `Array#each`, `Hash#each` are
implemented in `Vm::collection_call_block`, not in Ruby bytecode. The
typical shape:

```rust
let snapshot = self.heap.array(*id).clone();   // Vec<Value>
let mut results = Vec::with_capacity(snapshot.len());
for v in snapshot {
    self.invoke_block(block, vec![v]);
    self.dispatch_until(pre_frames);            // ← block body runs here
    results.push(self.stack.pop().unwrap_or(Value::Nil));
}
self.heap.alloc(HeapObj::Array(results))
```

The block body can trigger any number of allocations (`Op::NewArray`,
`Op::NewHash`, `Class.new`). Each `Op::NewArray` calls `maybe_gc`. The
mark phase walks roots from `Vm.stack` and every `Frame`'s locals — but
**`snapshot`, `results`, and the source array `*id` are all in Rust
locals**, invisible to the GC.

Concrete reproduction: `nums.map { |x| [x, x * 2] }` for `nums.len() ==
2000`. After ~1024 inner-array allocations the heap threshold is crossed;
the previously-mapped inner arrays in `results` are unrooted, get marked
Dead, and their slots are recycled. After the loop, reading
`result[0][0]` panics with `use-after-free ObjId(N)`.

The happy path has never tripped this because:
- Our existing fixtures stay well under the heap threshold (initial
  `next_gc = 1024`).
- We have no stress-GC mode that would force a collection per allocation.

So this is a latent bug: correct for our existing tests, broken for
anything beyond toy size.

## Decision

Add a single GC root list, `Vm.pinned: Vec<Value>`. The mark phase walks
it alongside the operand stack and frame locals. Native code that holds
heap references across a potential GC point pushes the value onto
`pinned` before, pops after.

`Vm.maybe_gc` also gains an unconditional path: when `Vm.stress_gc` is
true (set from the `STRESS_GC=1` env var at construction), every
`maybe_gc` call triggers a full collection. This is wired into CI so
the GC root list cannot silently degrade.

`collection_call_block` is the only current pinned-stack user:

- `Array#each` / `Hash#each`: pin the source collection during iteration.
- `Array#map`: pin the source; pin the accumulating result array, which
  is allocated on the heap up front instead of accumulating in a Rust
  `Vec<Value>`.
- `Integer#times`: nothing to pin (integers and the counter aren't
  heap-managed).

## Consequences

Wins:

- The bug above is fixed; `tests/fixtures/gc_block.rb` covers the case
  end-to-end. Output is byte-identical to CRuby.
- `STRESS_GC=1 cargo test` passes on every fixture. CI runs both
  modes. Any future native-driven loop that forgets to pin will fail
  loudly under stress GC.
- The mechanism is dirt cheap: one extra `Vec::push` / `Vec::pop` per
  driver entry, and `mark_phase`'s root walk gets a few more values.

Costs:

- Native-code authors must remember to pin. We accept this as a
  contract spelled out in CONTRIBUTING.md and ARCHITECTURE.md. The
  stress-GC CI job is the safety net.
- `pinned` is a stack, not a generation. Long-lived host code that
  holds a `Value` across many GC cycles would keep its entry pinned
  forever; that's fine for our current uses (drivers push and pop in
  the same function) but is something to remember when designing the
  P1-C host embedding API.

## Why not other approaches we considered

- **Push into `Vm.stack` instead of a separate list.** Mixing accumulator
  state with operand-stack semantics breaks the assumption that ops
  pop a fixed number of values. Separate vector keeps the abstraction
  clean.
- **Make every native driver hold a heap `Array` directly, no
  `pinned`.** Works for `map`'s result, but not for `snapshot` (which is
  intentionally a clone, so block-body mutations to the source can't
  affect iteration).
- **Add a per-call `Guard` RAII wrapper.** Tempting, but requires
  borrowing `&mut Vm` through a guard whose `Drop` pops it — borrow
  checker friction we don't need right now. A future refactor might
  introduce this; the current explicit push/pop is honest about scope.
