# C-extension FFI safety contracts

The `rubyrs-cext` crate exposes ~40 `unsafe extern "C"` entry points
that a dlopen'd C extension calls during its `Init_<stem>` and
subsequent host-fn invocations. Each entry point has a `# Safety`
section spelling out its specific contract; this doc lifts those
sections into three structural buckets so the trust model is
visible in one place rather than scattered across the FFI surface.

The companion `vm/cext.rs` (in `crates/rubyrs`) is the host side
of the bridge — it installs callbacks, translates handles back
into `Value`s, and re-enters the Vm via `CURRENT_VM_PTR`. Its
safety story is in this doc's [§ Host bridge](#host-bridge) section.

> rubyrs is a **hardening layer**, not a sandbox. C extensions
> run with full host process privilege. See `docs/SECURITY.md` for
> the trust model — this doc is about what FFI fns will and won't
> do when given bad input, not about whether to load untrusted C
> code (you shouldn't).

## The trusted-loader contract

Every `rb_*` FFI entry point assumes its caller is a dlopen'd C
extension that the embedding host explicitly chose to load. C is
inherently trusted at the ABI boundary; we don't add input
validation that would slow the happy path.

What this means concretely:

- We **trust** that pointer arguments are well-aligned, point at
  valid Rust-readable memory for the duration of the call, and
  outlive the call.
- We **don't trust** opaque `Value` handles — those are 64-bit
  ints the C ext could forge. But forgery is bounded:
  `with_state` does range-checked `.get()` on the handle table,
  so a forged handle resolves to `Qnil` or a "wrong but defined"
  result. **No path here dereferences a raw Rust reference
  derived from a `Value` handle**, so bad handles yield wrong
  results, never undefined behaviour.

## Three contract classes

### Class A: handle-only (most entry points)

Functions taking only `Value` handles or primitive scalars
(`c_long`, `c_int`):

- String length / pointer accessors that take Value: `RSTRING_PTR`,
  `RSTRING_LEN`, `rb_str_new_frozen`.
- Number marshalling: `rb_long2num`, `rb_num2long`, `rb_num2ulong`,
  `rb_int2num`, `rb_num2int`.
- Array primitives: `rb_ary_new`, `rb_ary_new_capa`, `rb_ary_push`,
  `rb_ary_entry`, `RARRAY_LEN`.
- Hash primitives: `rb_hash_new`, `rb_hash_aset`, `rb_hash_aref`.

**Safety contract**: handle integrity is the dlopen'd C ext's
responsibility. Forged handles either resolve to a no-op or return
`Qnil` (range-checked inside `with_state`); no path here
dereferences a raw Rust reference derived from a handle, so a bad
value yields wrong results but never UB.

**What can go wrong if abused**:
- `rb_ary_push` on a non-Array handle panics with a clear ICE
  message instead of corrupting state.
- `rb_num2long` on a non-Int handle returns 0 (silent truncation —
  matches CRuby's `NUM2LONG` on the fast Fixnum path; spike
  doesn't yet trap on type mismatch).

### Class B: `*mut Value` (in/out parameters)

Two functions take a raw pointer to a `Value` slot:

- `rb_string_value_cstr(v: *mut Value)` — CRuby's
  `StringValueCStr(v)` macro counterpart; meant to coerce `*v` to
  a String and return a C string pointer.
- `rb_string_value_ptr(v: *mut Value)` — same, for
  `StringValuePtr(v)`.

**Safety contract**: in addition to the handle contract,
`v` must point at an aligned, valid, writable `Value` for the
duration of the call. CRuby's macros pass `&local_value`, which
satisfies this trivially.

**What can go wrong**: if a C ext passes a null or misaligned
pointer, the read of `*v` is UB. We `assert!(!v.is_null())` to
turn the most common bug into a clean abort instead of a
silent miscompile, but alignment is on the caller.

### Class C: `*const rb_data_type_t` (TypedData type tag)

One function takes a TypedData type tag pointer:

- `rb_check_typeddata(obj: Value, type_ptr: *const rb_data_type_t)`
  — CRuby's `TypedData_Get_Struct` wraps this; the C ext passes
  the address of a file-scope `rb_data_type_t` static and the
  object handle, gets back a `*mut c_void` to its concrete
  struct.

**Safety contract**: `type_ptr` must be either null or a valid
pointer to an `rb_data_type_t` whose lifetime covers the call.
CRuby's `TypedData_Get_Struct` macro always passes a file-scope
static, satisfying the lifetime requirement trivially. The check
is identity-based (`==` on the pointer), so the rb_data_type_t's
*contents* are never dereferenced — only its address.

## Host bridge (`vm/cext.rs`)

The host side of the bridge installs callbacks that the C ext
calls back into via `rb_funcallv`. Three threads of safety here:

### 1. `CURRENT_VM_PTR` thread-local pointer

A raw `*mut Vm` set by `do_call` (via `with_vm_ptr_set`) before
invoking a host fn, cleared after. Read by `cext_dispatch` when
installing the rb_funcallv callback so re-entrant C-to-Ruby calls
dispatch on the right Vm.

**Why it exists**: when `do_call` invokes a host fn, `&mut self`
is held for the duration. If the host fn re-enters the Vm via
`rb_funcallv`, the callback dereferences this raw pointer to
obtain a fresh `&mut Vm`, aliasing the outer borrow.

**Stacked Borrows considers this UB**; Tree Borrows is more
permissive. In practice the two `&mut`s are time-disjoint (only
one is used at any instant). Future work could move `Vm` into an
`UnsafeCell`-flavoured container for stricter Miri compliance;
not blocking until a real concurrent execution model lands.

**Panic safety**: `VmPtrGuard` restores the previous pointer on
**every** scope exit, including panic unwinding. Without this,
a panic between pointer-set and the matching restore would leak
a stale `*mut Vm` into the next host-fn call.

### 2. RAII guards around cext state stack

Three guards keep the rubyrs_cext callback stacks balanced
even on the panic path:

- `CExtStateGuard` (around `enter()` / `leave()` on `STATE`).
- `FuncallCallbackGuard` (around `push_funcall_callback` /
  `pop_funcall_callback`).
- `TypedDataCallbackGuard` (around the wrap + check callback
  stacks).

Each implements `Drop` to pop on scope exit. The state guard has
a normal-path `into_state()` consumer for the success case
(suppresses the Drop pop because the caller takes responsibility
for the drained `CExtState`).

**Why panic-safe**: a panic between push and the matching pop
would leak the callback into the next cext call. With the
guards, panic unwinding pops the stacks in reverse order
unconditionally.

### 3. Bounded handle translation

`cext_handle_to_value` and `cext_value_to_cvalue` recurse for
Array / Hash structures built by the C ext. A C extension can
construct a self-referential `CValue::Array(_)` (e.g. `a.push(a)`
from C); without a depth limit the recursion would stack-
overflow.

**Defence**: `CEXT_TRANSLATE_MAX_DEPTH = 256` is generous for
realistic JSON-shape inputs and well below the host stack limit.
Hitting the cap surfaces as a clean `ArgumentError` Trap rather
than a host-process abort.

`cvalue_eq` in `rubyrs-cext` has the same shape: a separate
`CVALUE_EQ_MAX_DEPTH = 256` guard against self-referential
keys during Hash lookup.

## What this doc doesn't cover

- **The `Init_<stem>` symbol contract** (what entry point a C
  ext must export). That's in `crates/rubyrs/src/vm/cext.rs`'s
  `cext_require` doc-comment.
- **The host's trust decision** ("should I load `bcrypt.so`?").
  That's a `docs/SECURITY.md` concern — the answer is "only
  extensions you'd trust as part of your host's TCB anyway".
- **Stacked Borrows / Miri status**. The `CURRENT_VM_PTR` aliasing
  pattern is a known wart. Documented in
  `crates/rubyrs/src/vm/cext.rs` source comments; a future ADR
  will retire it.

## See also

- [`docs/SECURITY.md`](SECURITY.md) — the broader trust model.
- [`docs/VM_MODULE_MAP.md`](VM_MODULE_MAP.md) — where to find
  `vm/cext.rs` and adjacent modules in the source tree.
- The per-fn `# Safety` sections in
  `crates/rubyrs-cext/src/lib.rs` — the source of truth.
