# 0004: Block locals share parent's Rc

## Status

Accepted (2026-05).

## Context

Ruby blocks see outer-scope variables:

```ruby
sum = 0
[1, 2, 3].each { |x| sum = sum + x }  # mutates outer sum
```

Standard implementations:

1. **Per-name upvalue resolution.** At compile time, identify each block
   reference: is it a block-local or an upvalue? Generate
   `LoadUpvalue(idx)` / `StoreUpvalue(idx)`. At runtime, the block frame
   has an upvalue table pointing at the enclosing frame.
2. **Implicit display / chain frames.** Block frame's parent pointer is
   walked dynamically on each name resolution. Simple but slow.
3. **Shared local slot table.** Block's compiled bytecode addresses the
   same slot indices as the parent's. At runtime, the block frame's
   `locals` is the same `Rc<RefCell<Vec<Value>>>` as the parent frame.

## Decision

**Option 3: shared local slot table via `Rc<RefCell<Vec<Value>>>`.**

`Frame.locals` is `Rc<RefCell<Vec<Value>>>` for every frame, not just
block frames. When compiling a block, `ProtoBuilder` inherits the parent
`ProtoBuilder`'s `locals: HashMap<String, u16>` and `n_locals`. Names
that already exist in the parent reuse the parent's slot. Block params
and new block-local names allocate slots beyond `parent.n_locals`.

At runtime, when invoking a block, the block's frame uses the parent's
locals `Rc` directly. The parent's `Vec` is grown to fit the block's
extra slots on first invocation.

## Consequences

Wins:

- Generates one new local for each new name, never an upvalue indirection.
- `LoadLocal` / `StoreLocal` semantics are identical regardless of whether
  we're in a method or a block — same op, same dispatch.
- Compile-time complexity is minimal: one extra `parent: &ProtoBuilder`
  parameter to `compile_block`.

Costs:

- **Escaping blocks don't work.** A block that outlives its capturing
  frame (e.g. returned as a `Proc` and called later) would observe a
  stale `Vec`. We don't support `Proc.new` / `lambda` yet, so this isn't
  observed today. When we add them, we'll need to **detach** — clone
  the frame's locals into a fresh `Rc` at frame return time, so any
  surviving Block reference points to a snapshot.
- A bit more bookkeeping for GC: a `Block` value visits its captured
  `Vec<Value>` directly during mark.
- `Frame.locals.borrow_mut()` everywhere there used to be `Vec` direct
  access. RefCell costs a tiny runtime check on every read/write.

Reversal: low. If we ever need true upvalue semantics, the path is
clear: introduce `LoadUpvalue(depth, slot)` ops, slot-resolution at
compile time. The shared-Rc design becomes a special case of "depth 0".

This decision specifically does **not** make rubyrs incorrect for
escaping closures; it just doesn't support them at all yet. The escape
hatch will be `Proc.new` + frame-detach when that's implemented.
