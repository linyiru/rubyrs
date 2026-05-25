# 0009: C-ext crate panic policy

## Status

Accepted (2026-05) as the spike-level Level-1.5 policy. Revisit when
`rb_raise` lands and the cext layer can convert contract violations
into Ruby exceptions instead of process abort.

## Context

`rubyrs-cext` exposes a CRuby-shape C ABI surface (`Qnil`, `Qtrue`,
`Qfalse`, `rb_str_new_cstr`, `RSTRING_PTR`, `rb_define_module`, etc.).
Every exported function is `#[unsafe(no_mangle)] pub unsafe extern "C"`
so dlopen'd C extensions can resolve and call them.

Today those functions react to contract violations from the C caller
— null pointer where a non-null is required, stale `VALUE` handle
that outlived its `CExtState`, wrong-type handle passed where the
ABI expects a class — with `assert!` / `expect!` / `panic!`. Concrete
examples:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_new_cstr(s: *const c_char) -> Value {
    assert!(!s.is_null(), "rb_str_new_cstr: null pointer");
    ...
}
```

```rust
pub fn resolve(&self, h: Value) -> &CValue {
    self.values
        .get(h as usize)
        .expect("ICE: cext handle out of range; C ext leaked a stale VALUE")
}
```

A Copilot review on [PR #2](https://github.com/linyiru/rubyrs/pull/2)
flagged this as undefined behaviour: Rust panics unwinding across
`extern "C"` frames is UB by the C ABI definition.

The premise is correct in general; the specific call-out for our case
is not, because of how Rust handles panics in `extern "C"` since the
2018 edition.

## What actually happens on panic

Rust 2018+ marks every `extern "C"` function as `nounwind` by default.
A panic that propagates to the boundary of such a function is
intercepted by the panic runtime and the **process aborts**. It does
not unwind into C frames; it does not invoke `catch_unwind` from the
caller; it does not invoke any drop handlers past the function
boundary. Behaviour is **defined** — abort is the contract.

The Rust reference is unambiguous:
[https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding](https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding).
The `extern "C-unwind"` ABI exists specifically to opt in to
cross-FFI unwinding; we don't use it, so we get the safer abort
semantics.

This means the cext exports are **not unsound**. They are *less
ergonomic than they could be*, because every contract violation
takes the host process down instead of becoming a Ruby exception.

## Decision

At spike Level 1.5, **keep the contract-violation panics**.

Rationale:

1. **Contract violations are programmer errors, not runtime states.**
   `rb_str_new_cstr(NULL)`, stale handles, wrong-type `rb_*` calls
   — these are all "the C extension is buggy" cases. Loud abort
   with a precise message is the right failure mode at the spike
   level. Silent error sentinels make C-ext bugs harder to find.

2. **There is no idiomatic "error pending" mechanism yet.** CRuby
   converts most C-ext contract violations into Ruby exceptions by
   calling `rb_raise` from the failing API function. We don't have
   `rb_raise` integration yet (it requires `longjmp`-style unwinding
   through C frames coordinated with our `Trap` machinery — non-
   trivial). Until that lands, the choice is "abort loudly" vs
   "abort silently"; loud wins.

3. **The host can still recover.** The rubyrs `Runtime` API is
   designed so that an aborted process is the host's worst case.
   For untrusted scripts the same defensive boundary (out-of-
   process sandbox, wasmtime, etc.) that protects against runaway
   loops also catches a contract-violation abort. The cext layer
   does not weaken that perimeter.

4. **`catch_unwind` at the Rust dispatch layer covers our own
   intermediate panics.** `Vm::cext_require` wraps the C call in
   `with_caught_unwind`, which catches panics from our argument
   interning / state ops (e.g. a stale handle from a previous
   pinned value, an out-of-range `Vec::get`). Those become
   `RuntimeError` Traps. The catch boundary specifically does NOT
   try to intercept the C side's invocation of our `rb_*` ABI
   functions — that's the abort branch.

## What this is NOT

This ADR does **not** sanction:

- Silent error returns from cext exports. If a future call site
  swallows a contract violation by returning `Qnil` instead of
  aborting, that's a regression in observability.
- Using `extern "C-unwind"` to actually allow unwinding through C
  frames. That requires every linked C extension to also opt in,
  which they don't, and would reintroduce real UB.
- Removing `with_caught_unwind` around the dispatch loop. That layer
  catches *our* panics, not the C side's, and it's load-bearing for
  Trap-style recovery from host-side bugs.

## Forward path

When `rb_raise` integration lands (probably Level 2 or 3, when we
need it for `json`'s type-coercion errors or `sqlite3`'s open-failure
paths):

1. Convert the cext exports from `assert!` / `expect!` / `panic!` to
   `rb_raise(rb_eArgError, ...)` / `rb_raise(rb_eTypeError, ...)`.
2. Each contract violation becomes a catchable Ruby exception
   instead of a process abort.
3. This ADR moves to *Superseded* status pointing at the new ADR
   that documents `rb_raise` integration.

Until then: contract violations abort, intentionally, and that's
the documented behaviour.

## References

- PR #2, review comment #2 (the catalyst).
- [Rustonomicon: FFI and unwinding](https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding)
- [RFC 2945 (`C-unwind` ABI)](https://rust-lang.github.io/rfcs/2945-c-unwind-abi.html)
- [`Vm::cext_require`](../../crates/rubyrs/src/vm.rs) — host-side catch boundary.
