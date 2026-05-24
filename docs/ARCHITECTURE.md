# Architecture

A single-file interpreter in ~1800 lines of Rust (`src/main.rs`). The pipeline:

```
.rb source bytes
  │
  ▼  ruby_prism::parse  (FFI to vendored Prism C library)
ruby_prism::Node<'pr>   — borrowed from the parse result
  │
  ▼  tr(node)            — single-pass, drops the 'pr lifetime
Expr                    — owned, Clone, no lifetimes
  │
  ▼  compile_proto / compile_expr
Vec<Proto> { code: Vec<Op>, strings: Vec<String>, n_locals }
  │
  ▼  Vm::run(entry)
stdout / exit code
```

Three reasons this structure is the way it is:

1. **`'pr` lifetime stops at `tr()`.** Prism nodes borrow from the parser; if
   we walked them all the way to runtime, every type below would carry a
   lifetime parameter. Translating once to an owned `Expr` is a 50-line
   investment that pays off everywhere downstream.
2. **Bytecode > tree-walking.** A tree-walker was the v0; switching to a
   bytecode VM was a 2.2× speedup with no language changes. See
   [ADR 0002](adr/0002-bytecode-vm-not-jit.md).
3. **No JIT.** rubyrs' niche is fast cold start and tiny memory; a JIT
   directly conflicts with both. See [ADR 0002](adr/0002-bytecode-vm-not-jit.md).

## Modules in `src/main.rs`

The file is sectioned by `// ---------- HEADING ----------` markers:

| Section | Lines (~) | Role |
|---------|-----------|------|
| IR | 1–70 | `Expr` enum: owned, mirror of Prism nodes we care about |
| Translate prism AST to Expr | 70–230 | `tr()`: walk `Node<'pr>`, return `Expr` |
| Values | 230–320 | `Value`, `Class`, `Instance`, `Method`, `BlockHandle` |
| GC Heap | 320–470 | `Heap`, `HeapObj`, mark-sweep |
| Bytecode | 470–560 | `Op` enum, `BinOpKind`, `Proto` |
| Compiler | 560–780 | `ProtoBuilder`, `compile_expr`, `compile_proto`, `compile_block` |
| VM | 780–end | `Vm`, `Frame`, `step()`, dispatch loop, builtins |

## The Value type

```rust
enum Value {
    Int(i64),          // unboxed
    Str(Rc<String>),   // immutable, refcounted, no cycle possible
    Sym(Rc<String>),   // interned via Rc identity for fast eq
    Bool(bool), Nil,
    Class(Rc<Class>),  // Class itself is immutable methods table; no cycles
    Object(ObjId),     // on Heap, GC-managed
    Array(ObjId),      // on Heap
    Hash(ObjId),       // on Heap
    Block(Rc<BlockHandle>),  // can capture cycles via captured locals (handled in GC visit)
}
```

The split between `Rc<T>` (immutable, can't cycle) and `Heap`-managed
(mutable, can cycle) is deliberate — see
[ADR 0003](adr/0003-rc-plus-mark-sweep-hybrid-gc.md).

## The GC

Stop-the-world mark-sweep, triggered when `live_count >= next_gc` (initially
1024; grows to 2× the survivor count after each cycle).

Roots:
- Every value on the operand stack
- Every `Frame`: `self_val`, all `locals`, `swap_return`, and any captured
  locals of an attached `block_arg`

Mark walks transitively through:
- `Instance.ivars`
- `Array` elements
- `Hash` key+value pairs
- `Block.captured` (the shared locals Vec)

Sweep zeros out unmarked `Slot::Live(_)` entries, pushes the index onto a
free list.

The class table (`Vm.classes`) and toplevel methods are not GC-managed; they
live in `HashMap`s of `Rc<T>` and outlive the heap.

## Blocks and closures

A block is a `Proto` whose locals layout **inherits** the parent proto's. When
the block body refers to an outer-scope variable, the compiler reuses the
parent's slot index — no upvalue indirection. At runtime:

- `Frame.locals` is `Rc<RefCell<Vec<Value>>>` (not `Vec<Value>`)
- When a block is invoked, its frame's `locals` is the **same `Rc` as the
  capturing frame's**. Reads/writes go to the same slots.
- The block's own params live in slots *after* the parent's `n_locals`. The
  parent's `Vec` is resized on demand to fit them.

This is simpler than per-name upvalue resolution and works for the common
`each` / `times` pattern. The cost is that escaping blocks (returning a
`Proc` from a method) would observe a stale frame; we don't support that yet.
See [ADR 0004](adr/0004-block-locals-share-parent-rc.md).

## Method dispatch

Every method call passes through `do_call(name, argc, no_recv)`:

1. Drain `argc` args off the stack.
2. If `no_recv`, attempt builtin (`puts`, `print`, `raise` for kernel-style
   calls); otherwise implicit-self; otherwise toplevel method.
3. With a receiver: `primitive_call(recv, name, args)` for the fast path
   (Int+Int arithmetic via `BinOp` op handles 90% of arithmetic-heavy code
   without entering `do_call` at all).
4. `Class.new` short-circuits to allocate and `swap_return` the result.
5. Otherwise: lookup `Method` on the receiver's class, push a frame.

No inline cache yet — `cls.methods.borrow().get(&name)` HashMap each call.
Closing this gap is on the roadmap. The `BinOp` fast path was the
higher-ROI win for our current benchmarks.

## Exceptions

`raise X` compiles to `<eval X>; Op::Raise`. `Op::Raise` pops the value and
walks `frames` looking for a `RescueHandler`. Each `begin/rescue` block emits:

```
PushRescue handler_off, bind_slot, bind_flag
<body>
PopRescue
Jump end
handler:
<rescue body>
end:
```

`PushRescue` records the handler IP, current stack depth, and the local slot
to bind the exception value to (if `rescue => e`). On unwind we truncate the
stack to that depth and jump.

If unwind reaches an empty frame stack, we print `uncaught exception: ...`
and `exit(1)`. Class-body frames pop their `class_stack` entry on the way.

## Why a single file

Modules would normally split this up. Single-file is on purpose for now:

- The whole runtime fits in one `cargo build`; no inter-module borrow puzzles.
- Cross-cutting concerns (the dispatch loop touches Value, Heap, Frame,
  every Op handler) are easier to read top-to-bottom.
- ~1800 lines is still mentally tractable.

We'll split when something else demands it (e.g. exposing a public API for
embedding, multiple binaries, or `tools/spec_extract` reusing the parser
layer).
