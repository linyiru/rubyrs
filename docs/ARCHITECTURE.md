# Architecture

A ~1900-line interpreter across nine focused modules, plus a thin CLI and
a public `lib.rs`. The pipeline:

```
.rb source bytes
  │
  ▼  ruby_prism::parse  (FFI to vendored Prism C library)
ruby_prism::Node<'pr>   — borrowed from the parse result
  │
  ▼  ast::tr(node)       — single-pass, drops the 'pr lifetime
Spanned<Expr>           — owned, Clone, every node carries a Span
  │
  ▼  compiler::compile_proto / compile_expr
Vec<Proto> { code, op_spans, n_locals, filename } + global Interner
  │
  ▼  vm::Vm::run(entry)
stdout (Box<dyn Write>) / Result<Value, Trap>
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

## Modules

| File | Lines (~) | Role |
|------|-----------|------|
| `src/ast.rs` | 270 | `Expr` enum, `Spanned<T>`, `tr()`: walk Prism `Node<'pr>`, drop the parser lifetime, attach byte-offset spans |
| `src/value.rs` | 55 | `Value`, `Class`, `Instance`, `Method`, `BlockHandle`, `ObjId` |
| `src/intern.rs` | 40 | `SymId(u32)` + `Interner`: dedup of method names, ivar names, class names, string literals |
| `src/heap.rs` | 200 | `Heap`, `HeapObj`, `Slot`, mark-sweep; `impl Value` for display / equality (needs `&Heap` and `&Interner`) |
| `src/bytecode.rs` | 90 | `Op` enum (Copy), `BinOpKind`, `Proto` (with `op_spans` and `filename`) |
| `src/compiler.rs` | 290 | `ProtoBuilder`, `compile_expr`, `compile_proto`, `compile_block`; threads `&mut Interner` through |
| `src/error.rs` | 90 | `Span`, `RubyError`, `Trap`, `TrapFrame`, `line_col` |
| `src/vm.rs` | 720 | `Vm` (incl. `stdout`, `host_fns`, `fuel`, `max_frames`, `pinned`), `Frame`, `RescueHandler`, `step()`, dispatch loop, `primitive_call`, `collection_call_block`, builtins |
| `src/lib.rs` | 130 | Public embedding API: `Runtime`, `Config`, re-exports of `Value`/`Trap`/`RubyError` etc. |
| `src/main.rs` | 30 | CLI entry: argv + env vars → `Config` → `Runtime::eval_file` |

Cross-module dependency is acyclic. `ast` and `bytecode` and `intern`
have no inter-module deps; `value` depends on `intern`; `heap` and
`error` depend on `value`/`intern`; `compiler` depends on `ast` +
`bytecode` + `intern`; `vm` depends on all of the above; `lib`
re-exports the public surface.

## The Value type

```rust
enum Value {
    Int(i64),          // unboxed
    Str(Rc<str>),      // immutable; literal strings share Rc with the interner
    Sym(SymId),        // u32 into Interner; equality is u32 == u32
    Bool(bool), Nil,
    Class(Rc<Class>),  // Class is an immutable methods table; no cycles
    Object(ObjId),     // on Heap, GC-managed
    Array(ObjId),      // on Heap
    Hash(ObjId),       // on Heap
    Block(Rc<BlockHandle>),  // can capture cycles via captured locals (GC visits)
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

Every method call passes through `Vm::do_call(name_id: SymId, argc, no_recv)`:

1. Drain `argc` args off the stack.
2. If `no_recv`:
   1. Attempt builtin (`puts`, `print`, `raise`).
   2. Attempt host-fn registered via `Runtime::register_fn`.
   3. Attempt implicit-self method on the current frame's `self`.
   4. Attempt toplevel method.
   5. Otherwise `NoMethodError`.
3. With a receiver: `primitive_call(recv, name, args)` for the fast path
   (Int + Int arithmetic via `Op::BinOp` doesn't even enter `do_call`).
4. `Sym#to_s` / `to_sym` go through `Vm::sym_primitive` (needs interner
   access).
5. `Class.new` allocates an `Instance`, runs `initialize` if present,
   and `swap_return`s the result so the caller sees the new object.
6. Otherwise: `HashMap<SymId, Rc<Method>>` lookup on the receiver's
   class, push a frame.

Method dispatch hashes on `SymId` (a `u32`) instead of bytes since
[ADR 0006](adr/0006-global-string-intern.md). A per-call-site inline
cache is on the roadmap; the `BinOp` fast path was the bigger near-term
win for arithmetic-heavy workloads.

### Resource caps

`do_call` and `step()` both run `Vm::check_*` helpers when `Config`
asked for limits:

- `check_fuel()` decrements `Vm.fuel` at the top of every `step()`.
  Placed there so both `dispatch` and `dispatch_until` (block-driver
  inner loop) route through it — see ADR 0008 on why this matters.
- `check_alloc()` runs after `maybe_gc` and before each `heap.alloc`.
- `check_frames()` runs before each `frames.push`.

Any hit returns `Err(Trap { err: ResourceExhausted { ... }, .. })`.

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

## Public embedding API

`src/lib.rs` exposes a `Runtime` wrapper around `Vm`:

- `Runtime::new()` / `Runtime::with_config(Config)` builds a fresh
  interpreter.
- `Runtime::eval(source, filename)` and `eval_file(path)` parse,
  compile, and run. Both are **incremental**: classes/methods/host fns
  defined in one call persist for the next.
- `Runtime::register_fn(name, |args| ...)` makes a Rust closure
  callable from Ruby as `name(args)`.
- `Runtime::set_stdout(Box<dyn Write>)` redirects `puts`/`print` to an
  arbitrary sink (default is `io::stdout()`).
- `Runtime::format_trap(&trap)` formats a `Trap` CRuby-style using the
  source(s) cached during `eval`.

`Config` exposes the resource caps from ADR 0008: `fuel`,
`max_heap_objects`, `max_frames`, plus `stress_gc` for the
collection-on-every-alloc debug mode.

See [`examples/embed.rs`](../examples/embed.rs) for a worked example
and [`tests/embed.rs`](../tests/embed.rs) for the pinned API surface.

## Why split now

We were a single file for the first ~1600 lines. We split at the seam
between P0 (correctness) and P1 (structure) milestones for three reasons:

1. **PR conflict reduction** — every change touched `src/main.rs`; any
   two parallel branches conflicted on the same file.
2. **Embedding API runway** (P1-C) — exposing a `lib.rs` requires
   visible module boundaries anyway.
3. **Readability had a ceiling** — beyond ~2000 lines, scrolling
   become its own friction.

The split was a move-only refactor: stdout was bit-identical to the
pre-split binary across all fixtures. No logic moved between sections.
