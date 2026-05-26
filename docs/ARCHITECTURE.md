# Architecture

A ~11k-line interpreter, organised as a Cargo workspace
(`crates/rubyrs` core + `crates/rubund` runner + `crates/rubyrs-cext`
opaque-handle bridge + `crates/rubyrs-gapscan` tooling). Inside the
core crate the VM is split CRuby-style across 20 `vm/*.rs` submodules,
each mirroring a CRuby compilation unit. The pipeline:

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

### Top-level (parsing → compilation → public API)

| File | Lines (~) | Role |
|------|-----------|------|
| `src/ast.rs` | 1100 | `Expr` enum, `Spanned<T>`, `tr()`: walk Prism `Node<'pr>`, drop the parser lifetime, attach byte-offset spans |
| `src/value.rs` | 155 | `Value`, `Class`, `Instance`, `Method`, `BlockHandle`, `ObjId` |
| `src/intern.rs` | 50 | `SymId(u32)` + `Interner`: dedup of method names, ivar names, class names, string literals |
| `src/heap.rs` | 450 | `Heap`, `HeapObj`, `Slot`, mark-sweep; `impl Value` for display / equality (needs `&Heap` and `&Interner`) |
| `src/bytecode.rs` | 200 | `Op` enum (Copy), `BinOpKind`, `Proto` (with `op_spans` and `filename`) |
| `src/compiler.rs` | 930 | `ProtoBuilder`, `compile_expr`, `compile_proto`, `compile_block`; threads `&mut Interner` through |
| `src/error.rs` | 175 | `Span`, `RubyError`, `Trap`, `TrapFrame`, `line_col`; `RubyError::is(class_name)` helper |
| `src/lib.rs` | 645 | Public embedding API: `Runtime`, `Config`, `format_trap`, re-exports of `Value`/`Trap`/`RubyError` etc. |
| `src/main.rs` | 30 | CLI entry: argv + env vars → `Config` → `Runtime::eval_file` |

### `vm/` submodules (CRuby-mirrored layout)

The VM itself is split across 20 files. Each mirrors a CRuby
compilation unit so the question "where would CRuby put this?"
maps to the same intuition here. `vm.rs` holds only the `Vm`
struct, `Frame`, `PinGuard`, `RescueHandler`, and the cext
re-entrance thread-local; everything else lives in `vm/*.rs`.
Line counts below are approximate snapshots and drift as the
code grows — see `wc -l crates/rubyrs/src/vm.rs crates/rubyrs/src/vm/*.rs`
for the live numbers.

| File | Lines (~) | CRuby analogue | Role |
|------|-----------|----------------|------|
| `src/vm.rs` | 745 | (struct definitions in `vm_core.h` / `vm_eval.c`) | `Vm`, `Frame`, `PinGuard`, `RescueHandler`, `HostFn` |
| `vm/dispatch.rs` | 4280 | `vm_eval.c` / `vm_insnhelper.c` | `do_call`, `do_call_block`, `invoke_method`, `invoke_method_with_block`, `invoke_block`, `cext_invoke_method`, `try_method_missing` |
| `vm/iter.rs` | 2160 | `enum.c` | block-form Enumerable filter/aggregation family (`iter_*_filter`, `collection_call_block`) |
| `vm/step.rs` | 1640 | `vm_exec.c` | `dispatch` / `dispatch_until` outer drivers + per-opcode `step` |
| `vm/string.rs` | 1570 | `string.c` | String primitives + Regex shims |
| `vm/kernel.rs` | 1250 | `object.c` (Kernel) | `puts` / `p` / `Integer()` / `Float()` / … |
| `vm/cext.rs` | 1240 | `internal/value.h` + `vm_eval.c` | rb_funcallv callback installation, handle ↔ Value translation, `cext_dispatch`, `cext_require`, `CURRENT_VM_PTR` |
| `vm/bignum.rs` | 1170 | `bignum.c` | `try_bigint_binop`, `try_bigint_pow`, `try_bigint_unary`, `try_bigint_pow_method`, `try_integer_digits`, `bigint_primitive`, `bigint_to_value`, `as_bigint{,_ref}`, `bigint_arith` |
| `vm/lookup.rs` | 1040 | `vm_method.c` + `class.c` | `CallCache`, `lookup_method_cached/uncached`, `responds_to`, `class_of`, `class_is_a`, `sym_primitive` |
| `vm/array.rs` | 880 | `array.c` | no-block Array methods |
| `vm/numeric.rs` | 595 | `numeric.c` | Int/Float primitives, `apply_int_promote` (i64-overflow promotion for reduce-style accumulators) |
| `vm/raise.rs` | 490 | `eval.c` / `eval_error.c` | `normalize_exception`, `trap_to_exception`, `unwind_with_exception` |
| `vm/hash.rs` | 390 | `hash.c` | Hash primitives |
| `vm/range.rs` | 375 | `range.c` | Range primitives |
| `vm/gc.rs` | 335 | `gc.c` + `thread.c` + `vm.c` | `Vm::run`, `check_fuel`/`alloc`/`frames`, `trap`, `maybe_gc` |
| `vm/sprintf.rs` | 265 | `sprintf.c` | `ruby_sprintf` + width/prec parser |
| `vm/fileops.rs` | 175 | `file.c` | `File.read` / `File.exist?` … |
| `vm/primitive.rs` | 130 | (per-class C function tables) | `primitive_call` — typed fast-path dispatch for Int/Float/String/Symbol/Bool/Nil |
| `vm/util.rs` | 80 | (cross-cutting) | `value_cmp_v`, `vec_nil`, `visibility_from_name` |
| `vm/match_data.rs` | 45 | `re.c` | `materialize_match_data` — shared MatchData ivar wiring (regex feature only) |
| `vm/cext_wasi.rs` | 25 | (target-specific shim) | wasm32-wasi alt for `cext_require` (traps; WASI has no dynamic loader) |

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
    Float(f64),        // unboxed (since P2 numeric)
    Object(ObjId),     // user instances, on Heap
    Array(ObjId), Hash(ObjId), Range(ObjId), Block(ObjId),
                       // all heap-managed; Block moved off Rc in P2-13
                       // because captured-cycle blocks could form Rc
                       // cycles the mark-sweep collector couldn't break.
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
cache (P1-B upgrade) lives in `Vm.call_caches: Vec<CallCache>`; every
`Op::Call*` carries a `u16` slot id assigned at compile time, and
`Vm::lookup_method_cached` (in `vm/lookup.rs`) skips the
`HashMap<SymId, _>::get` on monomorphic sites. Invalidation: any
`Op::DefMethod` / `Op::DefClass` bumps `Vm.method_gen`.

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

If unwind reaches an empty frame stack, the exception is wrapped as
`RubyError::Uncaught { class_name, message }` and propagated through
the normal `Trap` path — the host (CLI or embedder) decides whether
to print, retry, or carry on. Earlier revisions called
`std::process::exit(1)` directly; that was fatal for embedded hosts
and was retired as part of the P2-15 hardening pass (see
[docs/SECURITY.md](SECURITY.md)). Class-body frames pop their
`class_stack` entry on the way down.

The rescue / unwind machinery itself lives in `vm/raise.rs`
(`normalize_exception`, `trap_to_exception`, `unwind_with_exception`).
Rescue-by-class filtering walks the superclass chain via `class_is_a`
in `vm/lookup.rs`.

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

### Second split: `vm.rs` → `vm/*.rs` (CRuby-mirrored layout)

The same ceiling argument repeated once `vm.rs` reached ~6.6k lines.
A second move-only refactor pass split it into 17 `vm/*.rs`
submodules, each named after the CRuby file with the same role
(`string.c` → `vm/string.rs`, `array.c` → `vm/array.rs`, …). The
mapping is documented in the [Modules](#modules) table above.
Reasoning:

1. **Locating code** — "where would CRuby put this?" now answers
   "where rubyrs puts this", which is the lowest-friction
   navigation rule for anyone reading both codebases.
2. **PR seam isolation** — features that touch only one type
   (Array extras, Hash extras, the `String#sub`/`gsub`/`tr` batch,
   etc.) land in a single submodule instead of patching a 6k-line
   match.
3. **Behaviour preservation** — each extraction was its own
   atomic commit, gated on the 79-test `diff_cruby` suite staying
   byte-identical to CRuby.

`vm.rs` is currently ~745 lines, holding the `Vm` struct, the
per-frame and per-rescue records, and the pin-stack RAII guard.
(The original ~440-line target reflected the immediate-post-split
state; subsequent work landed a handful of helpers back in `vm.rs`
that we've since pulled out — wasi `cext_require` →
`vm/cext_wasi.rs`, MatchData materialization →
`vm/match_data.rs`, `apply_int_promote` → `vm/numeric.rs`, and
the entire BigInt cluster → `vm/bignum.rs`. The remaining gap is
mostly utility helpers like `dig_step` / `user_cmp` that any
`impl Vm` site can reach; further trimming is cosmetic rather than
load-bearing.) The thread-local `CURRENT_VM_PTR` the cext
re-entrance machinery needs lives in `vm/cext.rs`.
