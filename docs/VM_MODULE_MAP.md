# `vm/` module map

A navigation guide to the 17 submodules under `crates/rubyrs/src/vm/`,
each named after the CRuby compilation unit it mirrors. Use this
when looking for "where does X live" — start from the CRuby file
you'd open in MRI, find the row, follow the path.

`vm.rs` itself is the entry point: it holds only the `Vm` struct,
`Frame`, `PinGuard`, `RescueHandler`, the `HostFn` type alias, and
the `mod` declarations + cross-module re-exports.

## Index by responsibility

| If you want to change… | Open |
|---|---|
| how `Op::Call` resolves a target | [`vm/dispatch.rs`](#vmdispatchrs-vm_evalc--vm_insnhelperc) |
| how a specific opcode executes | [`vm/step.rs`](#vmsteprs-vm_execc) |
| how primitives like `5 + 3` dispatch without a class lookup | [`vm/primitive.rs`](#vmprimitivers-per-class-c-function-tables) |
| what built-in methods Array/Hash/Range expose | [`vm/array.rs`](#vmarrayrs-arrayc) / [`vm/hash.rs`](#vmhashrs-hashc) / [`vm/range.rs`](#vmrangers-rangec) |
| how `each` / `map` / `select` etc. work | [`vm/iter.rs`](#vmiterrs-enumc) |
| how `puts` / `p` / `Integer()` work | [`vm/kernel.rs`](#vmkernelrs-objectc-kernel-arms) |
| C extension loading + `rb_funcallv` dispatch | [`vm/cext.rs`](#vmcextrs-internalvalueh--vm_evalc-handle-bridge) |
| `raise` / `rescue` / `ensure` plumbing | [`vm/raise.rs`](#vmraisers-evalc--eval_errorc) |
| method-entry inline cache + class-ancestor walks | [`vm/lookup.rs`](#vmlookuprs-vm_methodc--classc) |
| resource caps (fuel/heap/deadline) + GC trigger | [`vm/gc.rs`](#vmgcrs-gcc--threadc--vmc) |
| `sprintf` / `%`-format on strings | [`vm/sprintf.rs`](#vmsprintfrs-sprintfc) |
| Int / Float methods | [`vm/numeric.rs`](#vmnumericrs-numericc) |
| String methods + Regex match shims | [`vm/string.rs`](#vmstringrs-stringc) |
| `File.read` / `File.exist?` etc. | [`vm/fileops.rs`](#vmfileopsrs-filec) |
| shared cross-cutting helpers | [`vm/util.rs`](#vmutilrs-cross-cutting) |

## Per-module entries

Each entry: CRuby analogue, role, the public surface (what other
`vm/` modules call), and notable internal landmarks. Line counts
are approximate; see `wc -l crates/rubyrs/src/vm/` for current.

### `vm/dispatch.rs` (`vm_eval.c` + `vm_insnhelper.c`)

The call-handling layer. Owns the path from "Op::Call fired" to
"target Method located + frame pushed + args bound". ~960 lines.

Public surface (used from `step.rs`):
- `Vm::do_call(name_id, argc, no_recv, cache_id)` — entry point
  for `Op::Call` / `Op::CallNoRecv`.
- `Vm::do_call_block(name_id, argc, no_recv, cache_id)` — same,
  for the `*Block` variants that take an attached block.
- `Vm::invoke_method` / `invoke_method_with_block` — frame-setup
  layer once the target `Method` is resolved.
- `Vm::invoke_block(block_id, args)` — re-enter a captured block.
- `Vm::cext_invoke_method` — bridge for C-ext re-entering Ruby
  via `rb_funcallv`.
- `Vm::try_method_missing` — fallback path on name miss.

Landmarks:
- The 459-line `do_call` body is the big switch over receiver
  kind. Walks: builtins (puts/p/raise) → host_fns → primitive_call
  → collection_call → toplevel_methods → class chain via
  `lookup_method_cached`.
- `invoke_method_with_block` is where default args / rest /
  keyword args / block binding all converge.

### `vm/step.rs` (`vm_exec.c`)

The opcode interpreter. ~750 lines, dominated by `step` (one big
match over `Op` variants).

Public surface:
- `Vm::run(entry)` — kick off the entry frame (actually lives in
  `gc.rs` for historical reasons; calls `dispatch`).
- `Vm::dispatch()` — the top-level run loop.
- `Vm::dispatch_until(until_depth)` — re-entrant loop used by
  `invoke_block` / `do_call_block` to interpret nested frames
  without unwinding through the host stack.
- `Vm::step(op, proto_idx)` — execute one opcode.

Each opcode arm in `step` should be straightforward; the
complexity lives in `dispatch.rs` (for Call/CallBlock) and
`raise.rs` (for Raise / unwind).

### `vm/cext.rs` (`internal/value.h` + `vm_eval.c` handle bridge)

C-extension dispatch and handle translation. ~915 lines. Gated
`#![cfg(not(target_os = "wasi"))]` — wasi has no dynamic loader.

Public surface:
- `Vm::cext_require(path)` — dlopen, run `Init_<stem>`, register
  every fn / class / singleton-method the C ext declared.
- `cext_dispatch` (free fn) — wraps a single host-fn call: enters
  the cext state, installs the rb_funcallv callback, translates
  return handle back to a Value.
- `with_vm_ptr_set` + `CURRENT_VM_PTR` (re-exported via `vm.rs`)
  — thread-local raw pointer for cext re-entrance into the Vm.

Landmarks:
- `cext_handle_to_value` / `cext_value_to_cvalue` recursive pairs
  with `CEXT_TRANSLATE_MAX_DEPTH` cap (defends against
  C-built self-referential Array/Hash).
- `CExtStateGuard` / `FuncallCallbackGuard` / `TypedDataCallbackGuard`
  — RAII pop-on-Drop guards keep cext callbacks balanced across
  panic unwinds.

### `vm/iter.rs` (`enum.c`)

Block-form `Enumerable`. ~1220 lines, the biggest submodule.

Public surface:
- `Vm::collection_call_block(recv, name, args, block_id)` — the
  branch `dispatch.rs` takes when the receiver supports
  Enumerable iteration AND a block is attached.

Internal: per-receiver-type iterator drivers
(`iter_array_filter`, `iter_hash_filter`, `iter_range_filter`)
parametrised by an `IterMode` enum so the GC-pinning /
break-propagation / short-circuit logic only lives in one place
per collection.

### `vm/string.rs` (`string.c`)

String primitives + Regex match shims. ~690 lines.

Public surface (consumed by `primitive.rs`):
- `string_call(recv, name, args, max_value_bytes)` — fast-path
  dispatch over String + Regex receivers.
- `Vm::string_collection_call` — heap-aware path for
  `Str` instance methods that need the Vm context (e.g. for
  `gsub` block forms).

### `vm/array.rs` (`array.c`)

No-block Array methods (everything that isn't an iterator
driver). ~550 lines.

Public surface:
- `Vm::array_collection_call(id, name, args)` — non-block Array
  primitives (`push`, `<<`, `[]`, `[]=`, `length`, `reverse`,
  `uniq`, `flatten`, `+`, `-`, `concat`, etc.).

### `vm/hash.rs` (`hash.c`)

Hash primitives. ~230 lines. Mirror of `array.rs`.

### `vm/range.rs` (`range.c`)

Range primitives. ~150 lines.

### `vm/numeric.rs` (`numeric.c`)

Int + Float primitives (`abs`, `succ`, predicates, conversions).
~200 lines. Consumed by `primitive.rs`.

### `vm/kernel.rs` (`object.c` Kernel arms)

Built-in kernel functions called without a receiver:
`puts` / `print` / `p` / `Integer()` / `Float()` / `gets` /
`raise` (the no-class form). ~265 lines.

Public surface:
- `Vm::builtin_call(name, args)` — `dispatch.rs` calls this first
  on no-recv paths.

### `vm/fileops.rs` (`file.c`)

`File.read` / `File.exist?` / `File.write` host-fn shims. ~110
lines.

Public surface:
- `Vm::file_class_dispatch(name, args)` — called from `do_call`
  when the receiver is the `File` class.

### `vm/raise.rs` (`eval.c` + `eval_error.c`)

Exception machinery. ~200 lines.

Public surface:
- `Vm::normalize_exception(v)` — converts a raise arg (String /
  Class / Instance) into an Exception instance.
- `Vm::trap_to_exception(trap)` — promotes a host-side Trap
  into a Ruby-level Exception so the script can `rescue` it.
- `Vm::unwind_with_exception(exc)` — frame-stack walk looking
  for a matching `rescue` handler; the heart of unwind.

### `vm/lookup.rs` (`vm_method.c` + `class.c`)

Method-entry resolution + class-ancestor walks. ~270 lines.

Public surface:
- `CallCache` struct + `Vm::ensure_call_caches(n)` /
  `lookup_method_cached(cls, name_id, cache_id)` /
  `lookup_method_uncached(cls, name_id)` — per-call-site inline
  method cache.
- `Vm::responds_to(recv, name_id)` — `Object#respond_to?`
  backend.
- `Vm::class_of(recv)` — `Object#class` backend.
- `Vm::sym_primitive(recv, name, args)` — Symbol primitives
  (`<=>`, `to_proc`, etc.) that need interner access.
- `class_is_a(child, ancestor)` (free fn) — superclass-chain
  walk; used by `unwind_with_exception` for rescue-by-class
  filtering.

### `vm/gc.rs` (`gc.c` + `thread.c` + `vm.c`)

Resource caps + GC trigger + the Vm runtime entry point. ~175
lines.

Public surface:
- `Vm::run(entry)` — push the entry frame and call dispatch.
- `Vm::check_fuel()` — decrement per-op fuel, also runs the
  every-1024-ops deadline check.
- `Vm::check_alloc()` — heap object count cap.
- `Vm::check_frames()` — frame stack depth cap.
- `Vm::trap(err)` — build a `Trap` with the current backtrace.
- `Vm::maybe_gc()` — heap-pressure or stress-GC trigger.

### `vm/primitive.rs` (per-class C function tables)

The typed fast-path dispatch table. ~100 lines.

Public surface:
- `primitive_call(recv, name, args, max_value_bytes)` (free fn)
  — `dispatch.rs` calls this before any Object lookup. On
  `Ok(None)` the call falls through to the user-method path.

Mirrors CRuby's per-class C function tables (numeric.c,
string.c, etc.) but as a single Rust match so the type checks
short-circuit before any HashMap work.

### `vm/sprintf.rs` (`sprintf.c`)

`ruby_sprintf` implementation + width / precision parser. ~240
lines. Re-exported from `vm.rs` as `pub(crate) ruby_sprintf` so
both `string.rs` (`String#%`) and `kernel.rs` (`Kernel#sprintf`)
can consume it.

### `vm/util.rs` (cross-cutting)

Small shared helpers too small to deserve their own module but
not belonging with any per-type module. ~45 lines.

- `value_cmp_v(a, b, interner)` — total ordering for Int/Str/Sym;
  consumed by `iter.rs` aggregation methods + `array.rs` sort.
- `vec_nil(n)` — fresh `Vec<Value>` of `n` `Nil`s; used everywhere
  fresh local-slot vectors are needed.
- `visibility_from_name(name)` — parse `private` / `protected` /
  `public` into a `Visibility` enum.

## Cross-module call graph (informal)

```
                     ┌──── Vm::run (gc.rs) ─── dispatch (step.rs)
                     │                              │
                     │                              ▼
                     │                          step (step.rs)
                     │                              │
                     │                              ├─→ do_call (dispatch.rs)
                     │                              │       │
                     │                              │       ├─→ builtin_call (kernel.rs)
                     │                              │       ├─→ primitive_call (primitive.rs)
                     │                              │       │       │
                     │                              │       │       ├─→ numeric_call (numeric.rs)
                     │                              │       │       └─→ string_call (string.rs)
                     │                              │       │
                     │                              │       ├─→ collection_call (array/hash/range.rs)
                     │                              │       ├─→ collection_call_block (iter.rs)
                     │                              │       ├─→ file_class_dispatch (fileops.rs)
                     │                              │       └─→ lookup_method_cached (lookup.rs)
                     │                              │               + invoke_method (dispatch.rs)
                     │                              │
                     │                              └─→ Raise → unwind_with_exception (raise.rs)
                     │
                     ├─→ maybe_gc / check_fuel etc. (gc.rs)
                     └─→ cext_require / cext_dispatch (cext.rs)
```

## Adding a new built-in method: which module?

| The method is on… | Edit |
|---|---|
| Integer / Float (no block) | `vm/numeric.rs` |
| String (no block) | `vm/string.rs` |
| Symbol | `vm/lookup.rs` (`sym_primitive`) |
| Array (no block) | `vm/array.rs` |
| Hash (no block) | `vm/hash.rs` |
| Range (no block) | `vm/range.rs` |
| any collection, with block (each/map/select/…) | `vm/iter.rs` |
| Kernel (`puts`, `Integer()`, …) | `vm/kernel.rs` |
| File class methods | `vm/fileops.rs` |
| Universal (`nil?`, `<=>`, `==` cross-type, `class`, `respond_to?`) | `vm/primitive.rs` or `vm/lookup.rs` |

For anything else, ask: what would CRuby's file path be? That's
your `vm/<name>.rs`.
