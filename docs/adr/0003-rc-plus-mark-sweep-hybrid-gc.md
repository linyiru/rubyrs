# 0003: Hybrid Rc + mark-sweep GC

## Status

Accepted (2026-05).

## Context

Garbage collection is non-trivial in Rust because of the borrow checker.
We considered:

1. **All `Rc`.** Simple, but cycles leak. Demonstrated: 200k `a.link(b);
   b.link(a)` ate 117 MB of RAM (CRuby: 10 MB).
2. **All on a managed heap.** Solves cycles, but every value access goes
   through an arena handle. Lots of borrow-checker friction with mutable
   ivars during method dispatch.
3. **`gc-arena` crate.** Rooted-access model. Requires substantial
   refactor: every operation has to be wrapped in a `mutation_context`.
4. **Hybrid: `Rc` for immutable types, mark-sweep for mutable ones.**
   The observation: only mutable heap objects can form cycles. Strings
   literals, class definitions, and method tables are immutable in our
   subset — they cannot be part of a cycle.

## Decision

**Hybrid GC strategy:**

- `Rc<T>` for: `Class`, `Method`, `String` literal (`Value::Str(Rc<String>)`),
  `Symbol`, `BlockHandle`.
- Mark-sweep `Heap` for: `Instance`, `Array`, `Hash`.

GC roots: the operand stack and every `Frame`'s `locals`, `self_val`,
`swap_return`, and any attached `block_arg.captured`.

Triggered on allocation when `heap.live_count >= heap.next_gc`. After
collection, `next_gc` resets to `2 × live_count` (min 1024).

## Consequences

Wins:

- Cycle test: 117 MB → 2.4 MB. GC works.
- The immutable fast path stays as cheap as `Rc::clone` (one atomic add).
  Method dispatch, string creation etc. don't touch the heap.
- Mark-sweep only needs to walk three node types: `Instance.ivars`,
  `Array` elements, `Hash` k/v pairs. Mark closure is small and obvious.
- Borrow checker stays happy: `Rc<T>` and `Heap::get(id) -> &T` both
  return shared references with explicit lifetimes.

Costs:

- The `Value` enum has two flavours of heap reference: `Class(Rc<Class>)`
  vs `Object(ObjId)`. Readers must know which is which. We codify this
  in [ARCHITECTURE.md](../ARCHITECTURE.md).
- `Heap::get` panics on use-after-free. We trust the marker to prevent
  this; if a bug slipped through, the panic is the indicator. Acceptable
  for a single-threaded interpreter with explicit roots.
- Adding a new mutable, cycle-capable type means three things: a
  `Value::Foo(ObjId)` variant, a `HeapObj::Foo` variant, and a mark-walk
  case. Slightly more boilerplate than "just clone an Rc".

If we ever add mutable strings (e.g. `String#replace`) we'll need to
move `Str` to the heap. That's a follow-up ADR.
