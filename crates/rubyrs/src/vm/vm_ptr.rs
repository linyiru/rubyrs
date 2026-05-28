//! Thread-local Vm pointer machinery used by re-entrant
//! host-fn callers (cext bridge via `rb_funcallv`,
//! `_http_server` battery via per-request block invocation).
//!
//! Extracted from `vm/cext.rs` (which used to own this code
//! gated by `feature = "cext"`) so the `_http_server` battery
//! can use the same mechanism without depending on the cext
//! feature. The pre-`_http_server` comment at
//! `vm/dispatch.rs::invoke_host_fn` literally predicted this
//! move: "if a future non-cext V1 host needs TLS-Vm access,
//! this is the site to move `with_vm_ptr_set` out of `mod
//! cext` and lift the cfg gate."
//!
//! Available unconditionally — both `cext` and `_http_server`
//! features can import from here. ADR 0013 (CURRENT_VM_PTR
//! borrow-aliasing policy) is the safety contract.

use std::cell::Cell;

use crate::vm::Vm;

// Thread-local raw pointer to the currently-active Vm during a
// host-fn call. Set by `do_call` (via `with_vm_ptr_set`) before
// invoking entries from `host_fns` / `cext_class_methods`, cleared
// after. Read by `cext_dispatch` when installing the `rb_funcallv`
// callback so re-entrant C-to-Ruby calls dispatch on the right Vm.
// Also read by `_http_server` battery per-request handler to call
// back into the registered Ruby app block.
//
// SAFETY / BORROW ALIASING NOTE — this deliberately routes around
// Rust's borrow checker. When `do_call` invokes a host fn, `&mut
// self` is held for the duration of that call. If the host fn
// re-enters the Vm (via `rb_funcallv` for cext, or via the
// `_http_server` per-request callback), the callback dereferences
// this raw pointer to obtain a fresh `&mut Vm`, aliasing the outer
// borrow. Stacked Borrows considers this UB; Tree Borrows is more
// permissive. In practice the two `&mut`s are time-disjoint (only
// one is used at any instant). Documented here so a future
// contributor doesn't "fix" it by sprinkling `&mut self` borrows
// that violate the invariant. See ADR 0013 for the safety contract.
thread_local! {
    pub(crate) static CURRENT_VM_PTR: Cell<*mut Vm> = const { Cell::new(std::ptr::null_mut()) };
}

/// Read the currently-set Vm pointer. Returns null if no host
/// fn is in flight (i.e. called outside `with_vm_ptr_set`).
#[allow(dead_code)] // Used by cext bridge + _http_server battery, both feature-gated.
pub(crate) fn current_vm_ptr() -> *mut Vm {
    CURRENT_VM_PTR.with(|c| c.get())
}

/// RAII guard that restores [`CURRENT_VM_PTR`] to its previous value
/// when dropped — runs the restore on **every** scope exit, including
/// panic unwinding. Without this guard, a panic inside the host fn
/// (e.g. from arg interning before `cext_dispatch` installs its
/// `with_caught_unwind` boundary) would leave a stale Vm pointer in
/// `CURRENT_VM_PTR`; a subsequent host-fn call would then dereference
/// it as a fresh `*mut Vm`, hitting use-after-free or worse.
pub(crate) struct VmPtrGuard {
    prev: *mut Vm,
}

impl Drop for VmPtrGuard {
    fn drop(&mut self) {
        CURRENT_VM_PTR.with(|c| c.set(self.prev));
    }
}

/// Run `f` with [`CURRENT_VM_PTR`] set to `vm_ptr`, restoring the
/// previous value (likely null) on **all** exit paths — normal return
/// or panic unwinding — via [`VmPtrGuard`]. Save/restore lets nested
/// cext calls (rb_funcallv → another host fn) work without the inner
/// call clobbering the outer's pointer.
pub(crate) fn with_vm_ptr_set<R>(vm_ptr: *mut Vm, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_VM_PTR.with(|c| c.replace(vm_ptr));
    let _guard = VmPtrGuard { prev };
    f()
}
