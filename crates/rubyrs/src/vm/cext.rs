//! C-extension dispatch helpers. Mirrors the bridging layer
//! between rubyrs values and `rubyrs_cext::Value` opaque handles
//! used by dlopen'd C extensions. CRuby's analogue is the
//! `internal/value.h` / `gc.c` handle-translation plus the
//! `vm_eval.c` callback installation; we keep the same shape but
//! in much smaller form.
//!
//! Contents:
//!   - `CEXT_TRANSLATE_MAX_DEPTH` recursion cap for self-referential
//!     C-built Arrays / Hashes.
//!   - `cext_handle_to_value` / `cext_handle_to_value_d` — handle →
//!     `Value` translation with depth tracking.
//!   - `cext_value_to_cvalue` / `cext_value_to_cvalue_d` — Value →
//!     `CValue` (intermediate type used by `cext_dispatch`).
//!   - `cext_dispatch` — installs the rb_funcallv callback for the
//!     duration of a C-ext call, invokes the C body, translates
//!     the return handle back to a Value.
//!   - `cext_funcall_to_vm` — the actual rb_funcallv callback
//!     body, bridges back into `Vm::cext_invoke_method`.

#![cfg(not(target_os = "wasi"))]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{Class, Value};

use super::{PinGuard, Vm};

// `rubyrs_jmp_raise` from c/setjmp_shim.c. Used by the
// rb_check_typeddata callback to convert a slot-type mismatch
// (review finding #1 on PR #27) into a Ruby-catchable
// rb_eTypeError instead of panicking the process. The
// `noreturn` lifetime contract matches the C side: it longjmps
// to the topmost setjmp installed by `rubyrs_jmp_invoke`.
unsafe extern "C" {
    fn rubyrs_jmp_raise(class_id: u64, msg: *const std::ffi::c_char) -> !;
}

fn current_vm_ptr() -> *mut Vm {
    CURRENT_VM_PTR.with(|c| c.get())
}


/// RAII guard around `rubyrs_cext::enter()` / `leave()`. Normal path
/// calls [`Self::into_state`] to consume the guard and receive the
/// drained `CExtState`. Panic path runs `Drop`, which discards the
/// state but always pops the stack — so a panic between `enter()`
/// and the matching pop doesn't leave a leaked CExtState on the
/// thread-local stack to corrupt subsequent cext calls.
#[cfg(not(target_os = "wasi"))]
struct CExtStateGuard {
    /// True until `into_state` consumes the guard. Tracks whether
    /// `Drop` should still pop (only on the panic path).
    active: bool,
}

#[cfg(not(target_os = "wasi"))]
impl CExtStateGuard {
    fn enter() -> Self {
        rubyrs_cext::enter();
        Self { active: true }
    }

    /// Consume the guard on the normal path, returning the drained
    /// `CExtState` for handle translation. Suppresses the `Drop`
    /// pop because the caller has already taken responsibility.
    fn into_state(mut self) -> rubyrs_cext::CExtState {
        self.active = false;
        rubyrs_cext::leave()
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for CExtStateGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = rubyrs_cext::leave();
        }
    }
}

/// RAII guard around `push_funcall_callback` / `pop_funcall_callback`.
/// Always pops on `Drop`, whether normal scope exit or panic unwinding.
/// Without this guard, a panic after the callback push but before the
/// matching pop would leak the callback into the next cext call.
#[cfg(not(target_os = "wasi"))]
struct FuncallCallbackGuard;

#[cfg(not(target_os = "wasi"))]
impl FuncallCallbackGuard {
    fn install(cb: rubyrs_cext::FuncallCallback) -> Self {
        rubyrs_cext::push_funcall_callback(cb);
        Self
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for FuncallCallbackGuard {
    fn drop(&mut self) {
        rubyrs_cext::pop_funcall_callback();
    }
}

/// L3-B RAII guard around the TypedData wrap + check callbacks.
/// Always pops both on `Drop` (panic-safe, mirrors
/// [`FuncallCallbackGuard`]).
#[cfg(not(target_os = "wasi"))]
struct TypedDataCallbackGuard;

#[cfg(not(target_os = "wasi"))]
impl TypedDataCallbackGuard {
    fn install(
        wrap: rubyrs_cext::TypedDataWrapCallback,
        check: rubyrs_cext::TypedDataCheckCallback,
    ) -> Self {
        rubyrs_cext::push_typed_data_wrap_callback(wrap);
        rubyrs_cext::push_typed_data_check_callback(check);
        Self
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for TypedDataCallbackGuard {
    fn drop(&mut self) {
        // Pop in reverse install order to keep the stacks balanced.
        rubyrs_cext::pop_typed_data_check_callback();
        rubyrs_cext::pop_typed_data_wrap_callback();
    }
}


// Thread-local raw pointer to the currently-active Vm during a
// host-fn call. Set by `do_call` (via `with_vm_ptr_set`) before
// invoking entries from `host_fns` / `cext_class_methods`, cleared
// after. Read by `cext_dispatch` when installing the `rb_funcallv`
// callback so re-entrant C-to-Ruby calls dispatch on the right Vm.
//
// SAFETY / BORROW ALIASING NOTE — this deliberately routes around
// Rust's borrow checker. When `do_call` invokes a host fn, `&mut
// self` is held for the duration of that call. If the host fn
// re-enters the Vm via `rb_funcallv`, the callback dereferences
// this raw pointer to obtain a fresh `&mut Vm`, aliasing the outer
// borrow. Stacked Borrows considers this UB; Tree Borrows is more
// permissive. In practice the two `&mut`s are time-disjoint (only
// one is used at any instant). Documented here so a future
// contributor doesn't "fix" it by sprinkling `&mut self` borrows
// that violate the invariant. See ADR (forthcoming) for the
// safer-but-bigger refactor that would move Vm into an
// `UnsafeCell`-flavoured container.
//
// Wasi-gated for the same reason `cext_dispatch` is: the cext path
// is unreachable when there's no dynamic loader.
thread_local! {
    pub(crate) static CURRENT_VM_PTR: Cell<*mut Vm> = const { Cell::new(std::ptr::null_mut()) };
}

/// RAII guard that restores [`CURRENT_VM_PTR`] to its previous value
/// when dropped — runs the restore on **every** scope exit, including
/// panic unwinding. Without this guard, a panic inside the host fn
/// (e.g. from arg interning before `cext_dispatch` installs its
/// `with_caught_unwind` boundary) would leave a stale Vm pointer in
/// `CURRENT_VM_PTR`; a subsequent host-fn call would then dereference
/// it as a fresh `*mut Vm`, hitting use-after-free or worse.
struct VmPtrGuard {
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



/// Translate a C-side opaque handle back into a `Value`. Currently
/// covers exactly the `CValue` variants the spike supports.
///
/// Gated off `target_os = "wasi"` because the only caller chain
/// (`cext_dispatch` invoked from closures registered in
/// `Vm::cext_require`) is itself wasi-stubbed. Without the gate the
/// `-D dead-code` warning fires on the wasi build.
/// Bounded recursion depth for translating C-built Array/Hash
/// structures back into rubyrs `Value`. A C extension can construct
/// a self-referential `CValue::Array(_)` (e.g. `a.push(a)` from C);
/// without a depth limit the recursion would stack-overflow during
/// `cext_handle_to_value`. 256 is generous for realistic
/// JSON-shape inputs and well below the host stack limit.
#[cfg(not(target_os = "wasi"))]
const CEXT_TRANSLATE_MAX_DEPTH: usize = 256;

#[cfg(not(target_os = "wasi"))]
fn cext_handle_to_value(
    vm: &mut Vm,
    h: rubyrs_cext::Value,
) -> Result<Value, Trap> {
    cext_handle_to_value_d(vm, h, 0)
}

#[cfg(not(target_os = "wasi"))]
fn cext_handle_to_value_d(
    vm: &mut Vm,
    h: rubyrs_cext::Value,
    depth: usize,
) -> Result<Value, Trap> {
    if depth >= CEXT_TRANSLATE_MAX_DEPTH {
        // Pathological input — cycle or implausibly deep nesting in
        // the C-built Array/Hash. Surface as an ArgumentError Trap
        // (review #24 follow-up): the previous silent-Nil shape was
        // hard to debug for a C ext author. The Trap unwinds through
        // the cext call chain back into Ruby with a clear message.
        return Err(Trap::new(RubyError::ArgumentError {
            msg: format!(
                "C ext result: max translation depth {} exceeded \
                 (cycle or implausibly deep Array/Hash nesting)",
                CEXT_TRANSLATE_MAX_DEPTH
            ),
        }));
    }
    // PR #60 review #5: read STATE via with_state and clone the
    // CValue at this level so recursive calls can re-enter
    // with_state without colliding on the RefCell borrow. Each
    // level clones one CValue (cheap for Nil/Int/HeapRef;
    // proportional to payload for Str/Array/Hash). Total cost:
    // O(result tree size) — way cheaper than the previous
    // O(entire-table-size) per-leave clone.
    let cv = rubyrs_cext::with_state(|st| st.resolve(h).clone());
    Ok(match cv {
        rubyrs_cext::CValue::Nil => Value::Nil,
        rubyrs_cext::CValue::True => Value::Bool(true),
        rubyrs_cext::CValue::False => Value::Bool(false),
        // CValue::Str stores bytes + sentinel NUL; the logical
        // string is `.len() - 1` bytes. L3-G: rubyrs's Value::Str
        // now holds `Vec<u8>` directly, so we copy bytes through
        // without the previous lossy UTF-8 round-trip — msgpack
        // pack output (and any other binary-protocol cext) now
        // survives the cext → Vm crossing intact.
        rubyrs_cext::CValue::Str(bytes) => {
            let logical_len = bytes.len().saturating_sub(1);
            let mut v = bytes;
            v.truncate(logical_len);
            Value::new_str_bytes(v)
        }
        rubyrs_cext::CValue::Int(n) => Value::Int(n),
        rubyrs_cext::CValue::Float(f) => Value::Float(f),
        // L3-C: a CValue::Class handle resolves to the actual Vm
        // Class object via vm.classes lookup. Used when a cext does
        // `obj.class` mid-call via rb_funcall — the returned handle
        // stores the class name, and the next rb_funcall on it
        // (e.g. `.name`) needs to find the real Class. Falling
        // back to Nil here (the old L0 behaviour) made any
        // subsequent method call on the class handle segfault.
        rubyrs_cext::CValue::Symbol(name) => Value::Sym(vm.interner.intern(&name)),
        rubyrs_cext::CValue::BlockRef(n) => Value::Block(crate::value::ObjId(n)),
        rubyrs_cext::CValue::Class(name) => {
            let sym = vm.interner.intern(&name);
            // Class handles are produced by rb_define_class_under
            // / rb_define_module which both register into
            // vm.classes at drain time. A handle pointing to an
            // unregistered class is an ICE — fall-back-to-Nil
            // (the L0 default) would just mask the bug and surface
            // later as a confusing NoMethodError far from the
            // root cause. Review #5 on PR #27.
            match vm.classes.get(&sym).cloned() {
                Some(c) => Value::Class(c),
                None => panic!(
                    "ICE: cext_handle_to_value: CValue::Class({:?}) \
                     not registered in vm.classes — should have been \
                     drained by cext_require",
                    name
                ),
            }
        }
        // Recursive translation: an Array/Hash CValue is a vector of
        // C-side handles; build a Vec<Value> by recursing on each,
        // then allocate on the Vm heap. PinGuard protects the
        // children from being collected mid-build when a child's
        // recursive allocation triggers `maybe_gc`.
        rubyrs_cext::CValue::Array(handles) => {
            let mut g = PinGuard::new(vm);
            let mut elements: Vec<Value> = Vec::with_capacity(handles.len());
            for child in handles {
                let v = cext_handle_to_value_d(g.vm, child, depth + 1)?;
                g.pin(v.clone());
                elements.push(v);
            }
            g.vm.maybe_gc();
            // Heap-cap exhaustion now propagates the original
            // ResourceExhausted Trap up to Ruby (review #26).
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::Array(elements));
            Value::Array(id)
        }
        rubyrs_cext::CValue::Hash(pairs) => {
            let mut g = PinGuard::new(vm);
            let mut entries: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
            for (kh, vh) in pairs {
                let k = cext_handle_to_value_d(g.vm, kh, depth + 1)?;
                g.pin(k.clone());
                let v = cext_handle_to_value_d(g.vm, vh, depth + 1)?;
                g.pin(v.clone());
                entries.push((k, v));
            }
            g.vm.maybe_gc();
            // Review #27 — same Trap propagation as Array arm above.
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::Hash(entries));
            Value::Hash(id)
        }
        // L3-B: an already-allocated Vm-heap Object. The wrap
        // callback inside cext_dispatch eagerly alloc's
        // HeapObj::TypedData on the Vm heap and stashes the ObjId
        // in this CValue, so the translator just turns it back
        // into Value::Object — no second alloc, no copy.
        rubyrs_cext::CValue::HeapRef(n) => Value::Object(crate::value::ObjId(n)),
    })
}

/// Translate a rubyrs [`Value`] into the corresponding [`rubyrs_cext::CValue`]
/// so it can be interned as a C-visible handle. Supported variants today:
/// Nil, Bool, Str (binary-safe via Vec<u8> + sentinel NUL), Int. Types
/// that cross only as runtime references (Sym ids, Class<Rc>, Object/
/// Array/Hash/Range/Block heap ids) trap with `ArgumentError` until the
/// matching ABI surface (`rb_sym_new`, `rb_class_new`, heap-handle
/// translation) lands.
#[cfg(not(target_os = "wasi"))]
fn cext_value_to_cvalue(
    vm: &Vm,
    st: &mut rubyrs_cext::CExtState,
    name: &str,
    idx: usize,
    v: &Value,
) -> Result<rubyrs_cext::CValue, Trap> {
    cext_value_to_cvalue_d(vm, st, name, idx, v, 0)
}

/// Bounded-depth helper for [`cext_value_to_cvalue`]. Mirrors the
/// `CEXT_TRANSLATE_MAX_DEPTH` discipline applied on the C → Ruby
/// direction (see [`cext_handle_to_value_d`]). A Ruby-side Array
/// or Hash can also be self-referential (`a = []; a << a`) and
/// without this guard the recursion would stack-overflow when
/// crossing into a C ext via `rb_funcall`'s arg translation or
/// when returning a result. Trap with ArgumentError instead so
/// the caller sees a clean Ruby-level error.
#[cfg(not(target_os = "wasi"))]
fn cext_value_to_cvalue_d(
    vm: &Vm,
    st: &mut rubyrs_cext::CExtState,
    name: &str,
    idx: usize,
    v: &Value,
    depth: usize,
) -> Result<rubyrs_cext::CValue, Trap> {
    if depth >= CEXT_TRANSLATE_MAX_DEPTH {
        return Err(Trap::new(RubyError::ArgumentError {
            msg: format!(
                "C ext `{}': arg {} exceeds max nesting depth {} (cycle or pathological input)",
                name, idx, CEXT_TRANSLATE_MAX_DEPTH
            ),
        }));
    }
    Ok(match v {
        Value::Nil => rubyrs_cext::CValue::Nil,
        Value::Bool(true) => rubyrs_cext::CValue::True,
        Value::Bool(false) => rubyrs_cext::CValue::False,
        Value::Str(s) => rubyrs_cext::CValue::str_from_bytes(&s.borrow()),
        Value::Int(n) => rubyrs_cext::CValue::Int(*n),
        Value::Float(f) => rubyrs_cext::CValue::Float(*f),
        // L3-B: a Value::Object handle crossing Ruby → C is
        // represented as a CValue::HeapRef carrying the raw ObjId.
        // The cext sees an opaque VALUE handle; rb_check_typeddata
        // resolves it back via the symmetric translator on the C
        // → Ruby side. Works for both script-defined Instances and
        // TypedData-wrapped C state — the C ext is expected to
        // know which type it expects (via the rb_data_type_t
        // pointer-identity check in rb_check_typeddata).
        Value::Object(id) => rubyrs_cext::CValue::HeapRef(id.0),
        // L3-C: Value::Class crossing Ruby → C as a CValue::Class
        // handle. Needed when a cext does `obj.class` from inside
        // an rb_funcall, then operates on the returned class
        // handle (typical pattern: `cls.name` to dispatch-by-type
        // — exactly what mini-json's generator does).
        Value::Class(c) => rubyrs_cext::CValue::Class(c.name.clone()),
        // L3-J: Symbol crossing Vm → cext as CValue::Symbol carrying
        // the symbol's name. msgpack's no-registration Symbol pack
        // path (`Packer#write(:foo)`) and any cext that takes a
        // Symbol arg (`register_type_internal`, kwarg lookups via
        // ID2SYM) end up here. The cext side intern-table will see
        // the name on `rb_sym2id` if it needs the canonical ID.
        Value::Sym(id) => rubyrs_cext::CValue::Symbol(vm.interner.resolve(*id).to_string()),
        // L3-K: Proc / Block crossing Vm → cext. The cext only ever
        // stashes the handle (in registries, callback fields, etc.)
        // and later round-trips it back via `rb_funcall(h, :call, …)`,
        // at which point the symmetric translator on this file's
        // other side rebuilds `Value::Block(ObjId)` and the dispatch's
        // Block.call arm runs the underlying proto.
        Value::Block(id) => rubyrs_cext::CValue::BlockRef(id.0),
        // Array/Hash crossing Ruby → C: build a CValue::Array/Hash
        // whose elements are FRESH handles interned into `st`.
        // Recurses on contained Values, interning each child into
        // the SAME state the caller will hand the result to. This
        // is the L2-3-review-fix #10: the previous impl used the
        // thread-local `with_state` accessor, which interned children
        // into whatever state was topmost at the time — wrong if the
        // outer caller had a state pushed but the inner caller hadn't
        // pushed yet (top-level cext call), and corrupting on
        // nesting.
        Value::Array(id) => {
            // Borrow the backing Vec<Value> directly — no clone.
            // The recursive `cext_value_to_cvalue` takes `&Vm` (the
            // function's `vm` param), so the heap borrow + each
            // recursive call are both immutable borrows of `vm`;
            // multiple immutable borrows are allowed. Drops the
            // O(n) memcpy the previous `.clone()` paid on every
            // collection crossing.
            let elements = vm.heap.array(*id);
            let mut handles: Vec<rubyrs_cext::Value> = Vec::with_capacity(elements.len());
            for elem in elements {
                let cv = cext_value_to_cvalue_d(vm, st, name, idx, elem, depth + 1)?;
                handles.push(st.intern(cv));
            }
            rubyrs_cext::CValue::Array(handles)
        }
        Value::Hash(id) => {
            // Same borrow-no-clone treatment for Hash.
            let pairs = vm.heap.hash(*id);
            let mut pairs_out: Vec<(rubyrs_cext::Value, rubyrs_cext::Value)> =
                Vec::with_capacity(pairs.len());
            for (k, v) in pairs {
                let kc = cext_value_to_cvalue_d(vm, st, name, idx, k, depth + 1)?;
                let kh = st.intern(kc);
                let vc = cext_value_to_cvalue_d(vm, st, name, idx, v, depth + 1)?;
                let vh = st.intern(vc);
                pairs_out.push((kh, vh));
            }
            rubyrs_cext::CValue::Hash(pairs_out)
        }
        other => {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: format!(
                    "C ext `{}': arg {} has type {} which is not yet supported across the cext FFI",
                    name,
                    idx,
                    other.type_name()
                ),
            }));
        }
    })
}

/// Invoke a registered C extension function: intern args into a fresh
/// per-call [`CExtState`], dispatch through the correct arity-specific
/// signature, translate the returned handle back into a rubyrs [`Value`].
///
/// Spike scope (Level 1): arities 0, 1, 2 are dispatched. The
/// `unsafe extern "C" fn()` stored in `CFn::func` is transmuted to the
/// arity-specific type — safe on x86_64 SysV and ARM64 AAPCS, where
/// `VALUE = u64` arg/return passes through scalar registers and unused
/// register args are simply ignored by the callee. Other arities trap
/// loudly at invocation rather than at register-time so the failure is
/// clearly attributable to the call site, not Init.
#[cfg(not(target_os = "wasi"))]
/// L3-C: data needed to dispatch a single cext-registered instance
/// method at call time. See `Vm::cext_instance_methods`.
#[derive(Clone)]
pub(crate) struct CextMethodReg {
    pub(crate) qualified_name: String,
    pub(crate) func: rubyrs_cext::OpaqueFn,
    pub(crate) arity: i32,
}

/// Self handle for a cext call. Distinguishes the three dispatch
/// shapes (L3-C broadened from the earlier `Option<&str>`):
///
///   - `Global`     — rb_define_global_function: `self` is Qnil.
///   - `Class(name)` — rb_define_singleton_method: `self` is the
///     class itself, interned as `CValue::Class`.
///   - `Object(v)`  — rb_define_method instance call: `self` is
///     the receiver, interned as `CValue::HeapRef` over its ObjId.
///     The C ext uses `TypedData_Get_Struct(self, ...)` to unwrap.
pub(crate) enum CextSelfHandle<'a> {
    Global,
    Class(&'a str),
    Object(Value),
}

pub(crate) fn cext_dispatch(
    name: &str,
    func: rubyrs_cext::OpaqueFn,
    arity: i32,
    args: &[Value],
    self_handle: CextSelfHandle<'_>,
) -> Result<Value, Trap> {
    // L3-H: arity -1 means variadic (`func(int argc, VALUE *argv,
    // VALUE self)`); the setjmp shim handles it as a separate case.
    // Any args.len() is permitted for -1. Fixed arity 0..=5 still
    // requires exact match.
    match arity {
        -1 => {}  // variadic — no args.len() check
        0..=5 => {
            let expected_argc = arity as usize;
            if args.len() != expected_argc {
                return Err(Trap::new(RubyError::ArgumentError {
                    msg: format!(
                        "C ext `{}': expected {} args, got {}",
                        name, expected_argc, args.len()
                    ),
                }));
            }
        }
        _ => {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: format!(
                    "C ext `{}': spike dispatches arity -1 and 0..=5 (got arity {})",
                    name, arity
                ),
            }));
        }
    }

    // SAFETY: `current_vm_ptr()` returns the same Vm pointer that
    // `do_call` stashed before invoking us; it stays valid until
    // `do_call` returns. The closure captures the pointer by value
    // so subsequent host_fn invocations don't have to re-stash it
    // (they will anyway, with the same value).
    //
    // Check the invariant BEFORE pushing any cext state on the
    // thread-local stacks — if this assert ever fires, no STATE or
    // callback gets leaked to corrupt the next cext call. Moved out
    // of the unsafe block so it sequences before arg translation
    // (which now needs `&Vm` for Array/Hash heap reads).
    let vm_ptr = current_vm_ptr();
    assert!(
        !vm_ptr.is_null(),
        "ICE: cext_dispatch reached with null CURRENT_VM_PTR; \
         host did not set it before calling host fn"
    );

    // SAFETY: we transmute `OpaqueFn` (zero-arg) to an arity-specific
    // signature with VALUE-shaped args. The original function was
    // registered with that exact signature by the C ext; we just
    // recovered it through the `ANYARGS` convention.
    unsafe {
        // From here on, every push has a matching RAII guard. A panic
        // (or any future early-return) will unwind through these and
        // pop both stacks in LIFO order, leaving thread-local state
        // exactly as we found it.
        let state_guard = CExtStateGuard::enter();
        let _cb_guard = FuncallCallbackGuard::install(Box::new(
            move |recv_h, method_name, arg_hs| {
                cext_funcall_to_vm(vm_ptr, recv_h, method_name, arg_hs)
            },
        ));
        // L3-B: install the TypedData wrap + check callbacks for
        // the duration of this dispatch. The closures capture
        // `vm_ptr` and do raw heap allocation / lookup on it.
        //
        // Wrap callback: resolve the klass handle from the topmost
        // CExtState (the cext defined it via rb_define_class_under
        // earlier in Init_/, or in the same dispatch), allocate a
        // HeapObj::TypedData on the Vm heap, intern a HeapRef
        // sentinel back into the state so the returned handle
        // resolves to Value::Object(typed_data_id) at cext-return
        // time AND while still inside this dispatch (for nested
        // rb_funcall passes).
        let _td_guard = TypedDataCallbackGuard::install(
            Box::new(move |klass_h, data_ptr, type_ptr, dfree| {
                // SAFETY: vm_ptr is the one the outer dispatch's
                // unsafe block holds — valid for the dispatch's
                // lifetime. The closure is defined under that
                // unsafe block so the deref doesn't need its own.
                let vm: &mut Vm = &mut *vm_ptr;
                // Resolve the class name from the klass handle.
                // Lookup the rubyrs Class by joined name; if the
                // cext registered it via rb_define_class_under,
                // it's already in vm.classes.
                let class_name = rubyrs_cext::with_state(|st| {
                    match st.resolve(klass_h) {
                        rubyrs_cext::CValue::Class(n) => n.clone(),
                        other => panic!(
                            "ICE: rb_data_typed_object_wrap: klass arg \
                             is not a Class handle: {:?}",
                            other
                        ),
                    }
                });
                let class_id_sym = vm.interner.intern(&class_name);
                let class = vm.classes.get(&class_id_sym).cloned()
                    .unwrap_or_else(|| panic!(
                        "ICE: rb_data_typed_object_wrap: class {:?} \
                         not registered (rb_define_class_under not called?)",
                        class_name
                    ));
                vm.maybe_gc();
                // Respect heap.max_live via the same maybe_gc +
                // check_alloc pattern as every other allocator
                // (review #3). On exhaustion this currently
                // panics; surfacing as a rb_raise(rb_eNoMemError)
                // would be L3-B.1 follow-up once rb_eNoMemError
                // lands in the sentinel set.
                vm.check_alloc()
                    .expect("L3-B spike: heap cap exhausted during TypedData wrap");
                let id = vm.heap.alloc(crate::heap::HeapObj::TypedData(
                    crate::heap::TypedDataObj { class, data_ptr, type_ptr, dfree }
                ));
                rubyrs_cext::with_state(|st| {
                    st.intern(rubyrs_cext::CValue::HeapRef(id.0))
                })
            }),
            std::rc::Rc::new(move |obj_h, expected_type| {
                // SAFETY: same vm_ptr as above; immutable read here.
                let vm: &Vm = &*vm_ptr;
                //
                // Three failure shapes, all surfaced as a Ruby-catchable
                // TypeError via rb_raise → longjmp (closes review #1
                // on PR #27 — the previous panicking shape aborted the
                // process when user Ruby did `Counter.new.bump`, i.e.
                // got a non-TypedData receiver to a method that
                // expected one):
                //
                //   1. Handle doesn't refer to an Object at all (e.g.
                //      a Nil / Int / Str crossed the boundary as the
                //      receiver — should be impossible from dispatch.rs
                //      but the FFI surface accepts arbitrary u64s).
                //   2. ObjId resolves to a non-TypedData slot — this is
                //      the `Counter.new.bump` case: generic .new
                //      allocates HeapObj::Instance; user expected
                //      HeapObj::TypedData.
                //   3. ObjId resolves to TypedData but the descriptor
                //      pointer doesn't match — wrong cext type passed
                //      to TypedData_Get_Struct.
                let cvalue = rubyrs_cext::with_state(|st| st.resolve(obj_h).clone());
                let id = match cvalue {
                    rubyrs_cext::CValue::HeapRef(n) => crate::value::ObjId(n),
                    _ => {
                        // Static C string (no allocation needed); raise
                        // via the longjmp shim. The msg matches CRuby's
                        // wording for the same condition. The enclosing
                        // closure is defined inside cext_dispatch's
                        // outer `unsafe` block, so the deref doesn't
                        // need its own (Rust rule for closure-local
                        // unsafe inheritance).
                        rubyrs_jmp_raise(
                            rubyrs_cext::raise::rb_eTypeError,
                            c"wrong argument type (expected wrapped object)".as_ptr(),
                        );
                    }
                };
                let td = match vm.heap.try_typed_data(id) {
                    Some(td) => td,
                    None => {
                        rubyrs_jmp_raise(
                            rubyrs_cext::raise::rb_eTypeError,
                            c"wrong argument type (object is not wrapped TypedData)".as_ptr(),
                        );
                    }
                };
                if td.type_ptr != expected_type {
                    panic!(
                        "ICE: rb_check_typeddata: type descriptor mismatch \
                         (expected {:p}, got {:p}) — L3-B.1 raise wiring TBD",
                        expected_type, td.type_ptr
                    );
                }
                td.data_ptr
            }),
        );

        // Translate args INTO the now-active state, interning each
        // (and each child for Array/Hash) directly via the same `st`
        // we're about to hand to the C ext. Trap-propagating via `?`;
        // RAII guards above drop on the early-return path.
        //
        // Previously the translation ran BEFORE `enter()` and used
        // `with_state` for child interning, which silently interned
        // Array/Hash children into the OUTER state (or panicked on
        // empty STATE for top-level calls). Fix for PR #6 review #10.
        let arg_handles: Vec<rubyrs_cext::Value> = {
            let vm_ref: &Vm = &*vm_ptr;
            rubyrs_cext::with_state(|st| {
                args.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let cv = cext_value_to_cvalue(vm_ref, st, name, i, v)?;
                        Ok::<_, Trap>(st.intern(cv))
                    })
                    .collect::<Result<Vec<_>, Trap>>()
            })?
        };

        // L3-A: build the self handle + args array in Rust, then
        // hand off to `invoke_with_raise` which does the setjmp +
        // C-side arity dispatch + cext call ENTIRELY in C frames.
        // There are NO Rust frames between setjmp and the cext fn,
        // so a longjmp from `rb_raise` never has to unwind a Rust
        // RAII Drop (closes Copilot reviews #7 / #8 on PR #14 —
        // the earlier Rust trampoline + FnOnce design WAS letting
        // longjmp skip Rust frames, which is at-best implementation-
        // defined).
        //
        // The earlier `with_caught_unwind` wrapper is gone: it
        // can't catch panics from inside the cext fn either (they
        // cross the same C-ABI boundary and abort regardless), and
        // the previous overclaim about it covering trampoline
        // panics was already flagged by review #1.
        //
        // **Known limitation** (L3-A spike): a `rb_raise` from a
        // deeply-nested rb_funcall chain longjmps PAST any
        // intermediate Rust frames inside `cext_funcall_to_vm`.
        // Their `PinGuard`s' `Drop` never runs → vm.pinned grows.
        // Harmless for non-pathological loads; cleanup protocol is
        // the next spike step.
        let self_handle = match self_handle {
            CextSelfHandle::Global => rubyrs_cext::Qnil,
            CextSelfHandle::Class(cname) => rubyrs_cext::with_state(|st| {
                st.intern(rubyrs_cext::CValue::Class(cname.to_string()))
            }),
            // L3-C: instance method dispatch — pass the receiver as
            // self via HeapRef. The cext typically extracts its
            // backing data with TypedData_Get_Struct(self, ...).
            CextSelfHandle::Object(Value::Object(id)) => rubyrs_cext::with_state(|st| {
                st.intern(rubyrs_cext::CValue::HeapRef(id.0))
            }),
            CextSelfHandle::Object(other) => {
                return Err(Trap::new(RubyError::TypeError {
                    msg: format!(
                        "C ext `{}': instance method dispatch with non-Object receiver {}",
                        name,
                        other.type_name()
                    ),
                }));
            }
        };
        // C helper expects [self, arg0, arg1, ...]; pre-allocate
        // with capacity to keep the hot path branch-free.
        let mut invoke_args: Vec<rubyrs_cext::Value> =
            Vec::with_capacity(arg_handles.len() + 1);
        invoke_args.push(self_handle);
        invoke_args.extend_from_slice(&arg_handles);
        let raised = rubyrs_cext::raise::invoke_with_raise(
            func, arity, &invoke_args,
        );
        let ret_handle = match raised {
            rubyrs_cext::raise::Raised::Returned(v) => v,
            rubyrs_cext::raise::Raised::Raised { class, msg } => {
                // Map sentinel → typed RubyError variant when we
                // recognise it so script-level `rescue
                // ArgumentError` / `rescue TypeError` etc. behaves
                // exactly like a same-named Ruby-side raise. Unknown
                // sentinels fall through to RuntimeError with the
                // class name prefixed onto the message (wedge
                // behaviour; per-class mapping is mechanical
                // follow-up — add a RubyError variant or extend
                // class_name() to cover the rest).
                let class_name = rubyrs_cext::raise::exception_class_name_for_sentinel(class);
                let err = match class_name {
                    "ArgumentError"     => RubyError::ArgumentError { msg },
                    "RuntimeError"      => RubyError::RuntimeError { msg },
                    "TypeError"         => RubyError::TypeError { msg },
                    "NameError"         => RubyError::NameError { msg },
                    "ZeroDivisionError" => RubyError::ZeroDivisionError { msg },
                    other => RubyError::RuntimeError {
                        msg: format!("{}: {}", other, msg),
                    },
                };
                // state_guard / _cb_guard drop normally on this
                // early return — Rust unwinding still works because
                // the longjmp landed in C frames BELOW us (inside
                // rubyrs_jmp_call) and returned into Rust here. No
                // RAII is skipped at this level.
                return Err(Trap::new(err));
            }
        };
        // PR #60 review #5: translate the result handle BEFORE
        // leaving so we can read the live STATE via `with_state`
        // (no full-table clone). cext_handle_to_value now accesses
        // STATE through with_state internally instead of taking
        // &CExtState by reference.
        //
        // Re-deref vm_ptr for the result translation (Array/Hash
        // returns need `&mut Vm` to allocate on the heap). Time-
        // disjoint from any earlier &Vm uses in this function.
        let vm: &mut Vm = &mut *vm_ptr;
        let translated = cext_handle_to_value(vm, ret_handle);
        // Now drain registered_* — the state_guard's into_state
        // calls leave() which moves them out. `values` is no longer
        // cloned (PR #60 review #5).
        let _ = state_guard.into_state();
        translated
    }
}

/// Bridge a `rubyrs_cext::FuncallCallback` invocation to
/// [`Vm::cext_invoke_method`]. Used as the body of the closure
/// installed by [`cext_dispatch`].
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a [`Vm`] for the duration of
/// this call. The caller (`cext_dispatch`) guarantees this by only
/// installing the callback while the corresponding `do_call` frame
/// is on the host's Rust stack — see [`CURRENT_VM_PTR`] for the
/// borrow-aliasing discussion.
#[cfg(not(target_os = "wasi"))]
fn cext_funcall_to_vm(
    vm_ptr: *mut Vm,
    recv: rubyrs_cext::Value,
    method: &str,
    arg_handles: &[rubyrs_cext::Value],
) -> rubyrs_cext::Value {
    // SAFETY: see CURRENT_VM_PTR doc — vm_ptr is valid for the life
    // of the surrounding cext_dispatch call. We deref the same
    // pointer twice in this function: first as `&mut Vm` (inner
    // block) for the recv/arg handle → Value translation and the
    // cext_invoke_method call; then, AFTER the inner block exits
    // and the &mut goes out of scope, as `&Vm` for the result
    // Value → handle translation. The two derefs are split into
    // separate scopes so no &mut + & alias exists at any moment —
    // the previous `let (result, vm_for_result) = unsafe { ... }`
    // pattern returned `&*vm_ptr` while `&mut *vm_ptr` was still
    // alive in the same block, which Stacked Borrows flags as UB.
    let result = unsafe {
        let vm = &mut *vm_ptr;
        // PinGuard the translated `recv_v` and each arg Value as
        // they are produced: `cext_handle_to_value` recursively
        // allocates Vm-heap Arrays/Hashes for nested C-built
        // structures, and each alloc can trigger `maybe_gc`. A
        // previously-translated recv or earlier arg sitting only
        // in a Rust local has no GC root, so STRESS_GC would sweep
        // it before `cext_invoke_method` saw it (slot-reuse → ICE
        // "use-after-free" inside dispatch). The guard is alive
        // across `cext_invoke_method` itself — which is intentional:
        // dispatch may also `maybe_gc` (e.g. compiling a string→sym,
        // alloc'ing intermediate Arrays), and we want recv/args
        // protected the whole way until they're consumed onto the
        // operand stack. The guard drops at the end of the unsafe
        // block, after the call has returned and the result Value
        // is bound.
        let mut g = PinGuard::new(vm);
        // `cext_handle_to_value` now returns Result (L2.5 Trap
        // propagation). On a translation Trap here — e.g. a cycle
        // in C-built recv/args, or heap-cap exhaustion mid-build —
        // we can't unwind into Ruby (this IS a C-ABI callback
        // entry point), so we collapse to Nil and let the inner
        // dispatch handle the degenerate input. Surfacing the
        // Trap via the rb_funcall return value requires `rb_raise`
        // / longjmp (Level 3).
        let recv_v = cext_handle_to_value(g.vm, recv).unwrap_or(Value::Nil);
        g.pin(recv_v.clone());
        let arg_vs: Vec<Value> = arg_handles
            .iter()
            .map(|h| {
                let v = cext_handle_to_value(g.vm, *h).unwrap_or(Value::Nil);
                g.pin(v.clone());
                v
            })
            .collect();
        match g.vm.cext_invoke_method(recv_v, method, arg_vs) {
            Ok(v) => v,
            // Spike: propagating Trap back through the C-ABI boundary
            // needs `rb_raise` / longjmp coordination (Level 3+).
            // For now collapse to Nil so the C side gets a defined
            // return without aborting.
            Err(_trap) => Value::Nil,
        }
        // `vm: &mut Vm` drops here.
    };

    // Now safe to take a fresh `&Vm` from the same pointer — the
    // previous `&mut` is out of scope.
    let vm_for_result: &Vm = unsafe { &*vm_ptr };

    // Translate result back to a handle in the topmost CExtState.
    // `cext_value_to_cvalue` now takes the same `st` it'll be interned
    // into, so Array/Hash result children land in the correct state
    // — the topmost, which is the C ext's current state.
    rubyrs_cext::with_state(|st| {
        match cext_value_to_cvalue(vm_for_result, st, "rb_funcallv:result", 0, &result) {
            Ok(cv) => st.intern(cv),
            Err(_) => rubyrs_cext::Qnil,
        }
    })
}

impl Vm {

    /// Load a C extension shared library, run its `Init_<stem>` symbol,
    /// and register every function it declared via
    /// `rb_define_global_function` into `self.host_fns`.
    ///
    /// Level 0 caveats:
    /// - Only literal paths (with optional auto-extension) are resolved;
    ///   `$LOAD_PATH` and gem lookup are deferred.
    /// - Loaded libraries are leaked (never unloaded). A real impl
    ///   tracks them on the Vm and unloads on drop.
    /// - Only arity 0 callbacks dispatch correctly; other arities
    ///   register, then trap on invocation with an ArgumentError.
    ///
    /// wasm32-wasi has no `dlopen` — a separate
    /// `#[cfg(target_os = "wasi")]` stub below returns a clear Trap
    /// instead of the dlopen path.
    pub(crate) fn cext_require(&mut self, path_str: &str) -> Result<Value, Trap> {
        use libloading::{Library, Symbol};
        use std::path::Path;

        // Auto-extension: `require "foo"` resolves "foo.dylib" / "foo.so"
        // / "foo.bundle" depending on host. Matches CRuby's behaviour for
        // the literal-path case.
        let exts: &[&str] = if cfg!(target_os = "macos") {
            &["dylib", "bundle"]
        } else if cfg!(windows) {
            &["dll"]
        } else {
            &["so"]
        };
        let p = Path::new(path_str);
        let so_path = if p.exists() {
            p.to_path_buf()
        } else {
            let mut found = None;
            for ext in exts {
                let with = p.with_extension(ext);
                if with.exists() {
                    found = Some(with);
                    break;
                }
            }
            found.ok_or_else(|| {
                self.trap(RubyError::RuntimeError {
                    msg: format!("cannot find C ext: {}", path_str),
                })
            })?
        };

        let stem = so_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                self.trap(RubyError::RuntimeError {
                    msg: format!("invalid C ext filename: {}", so_path.display()),
                })
            })?
            .to_string();
        let init_sym = format!("Init_{}", stem);

        // SAFETY: dlopen is intrinsically unsafe; the C ext can do
        // anything. We trust extensions we explicitly load — sandboxing
        // is for the Ruby-language layer, not the FFI layer.
        unsafe {
            rubyrs_cext::enter();
            let lib = match Library::new(&so_path) {
                Ok(l) => l,
                Err(e) => {
                    let _ = rubyrs_cext::leave();
                    return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("dlopen {}: {}", so_path.display(), e),
                    }));
                }
            };
            let init: Symbol<unsafe extern "C" fn()> = match lib.get(init_sym.as_bytes()) {
                Ok(s) => s,
                Err(e) => {
                    let _ = rubyrs_cext::leave();
                    return Err(self.trap(RubyError::RuntimeError {
                        msg: format!(
                            "symbol {} not found in {}: {}",
                            init_sym,
                            so_path.display(),
                            e
                        ),
                    }));
                }
            };
            init();
            let state = rubyrs_cext::leave();

            for cfn in state.registered_fns {
                let sym = self.interner.intern(&cfn.name);
                let func = cfn.func;
                let arity = cfn.arity;
                let cfn_name = cfn.name.clone();
                self.host_fns.insert(
                    sym,
                    crate::vm::HostFnSlot::V1(Rc::new(move |args: &[Value]| {
                        // Top-level functions get Qnil as `self`,
                        // matching CRuby's `rb_define_global_function`
                        // convention.
                        cext_dispatch(&cfn_name, func, arity, args, CextSelfHandle::Global)
                    })),
                );
            }

            // Materialise every class/module the C ext declared, so
            // `LoadConst("BCrypt::Engine")` finds them.
            for cls in state.registered_classes {
                let name_sym = self.interner.intern(&cls.joined_name);
                let new_class = Rc::new(Class {
                    name: cls.joined_name.clone(),
                    methods: RefCell::new(HashMap::new()),
                    singleton_methods: RefCell::new(HashMap::new()),
                    superclass: RefCell::new(None),
                    includes: RefCell::new(Vec::new()),
                    cext_alloc_func: std::cell::Cell::new(None),
                });
                self.classes.insert(name_sym, new_class);
            }

            // L3-C: instance methods → per-class dispatch table
            // consulted from `do_call`'s Value::Object arm. Stored
            // as plain registration data (qualified name + OpaqueFn
            // + arity) rather than a HostFn closure because the
            // receiver isn't known at registration time and HostFn
            // has no room for a self-Value param without widening
            // every call site.
            for im in state.registered_methods {
                let method_sym = self.interner.intern(&im.method_name);
                let qualified = format!("{}#{}", im.class_joined_name, im.method_name);
                self.cext_instance_methods
                    .entry(im.class_joined_name)
                    .or_default()
                    .insert(
                        method_sym,
                        CextMethodReg {
                            qualified_name: qualified,
                            func: im.func,
                            arity: im.arity,
                        },
                    );
            }

            // Register every singleton method into the per-class
            // dispatch table consulted by `do_call`.
            for sm in state.registered_singletons {
                let method_sym = self.interner.intern(&sm.method_name);
                let func = sm.func;
                let arity = sm.arity;
                let class_name = sm.class_joined_name.clone();
                let qualified = format!("{}.{}", class_name, sm.method_name);
                self.cext_class_methods
                    .entry(sm.class_joined_name)
                    .or_default()
                    .insert(
                        method_sym,
                        Rc::new(move |args: &[Value]| {
                            // Singleton methods get the class itself
                            // as `self`, matching CRuby's
                            // `rb_define_singleton_method` contract.
                            cext_dispatch(&qualified, func, arity, args, CextSelfHandle::Class(&class_name))
                        }),
                    );
            }

            // L3-F: install per-class custom allocators. Looks up the
            // target class by joined name; if found, stash the cext
            // allocator fn in its `cext_alloc_func` slot so
            // `Klass.new(args)` routes through the cext-side
            // allocator before calling `initialize`.
            //
            // PR #50 review #4: registered_classes is drained BEFORE
            // alloc_funcs (a few blocks up), so by this point every
            // class the cext declared is in self.classes. An alloc_func
            // pointing at an unknown class means the cext called
            // rb_define_alloc_func without a prior rb_define_class_under
            // — a contract violation that would silently produce bare
            // Instance receivers at .new time, leading to confusing
            // TypeError far from the cause. Panic instead so cext
            // authors find the bug immediately.
            for af in state.registered_alloc_funcs {
                let name_sym = self.interner.intern(&af.class_joined_name);
                let cls = self.classes.get(&name_sym).unwrap_or_else(|| {
                    panic!(
                        "ICE: rb_define_alloc_func for unregistered class {:?} \
                         — cext called rb_define_alloc_func before (or instead of) \
                         rb_define_class_under",
                        af.class_joined_name
                    )
                });
                cls.cext_alloc_func.set(Some(af.func));
            }

            // Level 0: keep the library mapped for the lifetime of the
            // process. Registered function pointers point into its
            // text segment; unmapping would dangle them. A real impl
            // stores `lib` on the Vm so it drops with the Vm.
            std::mem::forget(lib);
        }

        Ok(Value::Nil)
    }

}

#[cfg(test)]
mod miri_tests {
    //! Synthetic tests for the `CURRENT_VM_PTR` re-entrance
    //! pattern documented in ADR 0013. Driver code mirrors
    //! what `cext_funcall_to_vm` does in production —
    //! `with_vm_ptr_set` installs the thread-local, the body
    //! reads it back and reborrows as `&mut Vm` to mutate the
    //! VM state — but WITHOUT going through `dlopen`.
    //!
    //! We deliberately do NOT call `cext_invoke_method` here:
    //! it pushes a frame and assumes an outer dispatch loop
    //! will execute the bytecode. The frame-machinery sets up
    //! its own `&mut Vm` aliasing inside the dispatcher
    //! that's orthogonal to ADR 0013's concern. What ADR 0013
    //! actually worries about is the raw-pointer reborrow
    //! happening on top of an existing outer `&mut` — and
    //! THAT shape we can exercise directly here.
    //!
    //! If Miri (Stacked or Tree Borrows) flags any of this as
    //! UB, the time-disjoint argument behind the current shape
    //! collapses and ADR 0013's deferred `UnsafeCell<VmState>`
    //! refactor (J4) gets a forcing function.
    use super::*;
    use crate::bytecode::Proto;
    use crate::heap::HeapObj;
    use crate::intern::Interner;

    fn mk_vm() -> Vm {
        Vm::new(Vec::<Proto>::new(), Interner::new())
    }

    /// Helper: simulate the outer-frame entry point. Real
    /// dispatch holds `&mut self` on its callstack; inside it
    /// reborrows `self` as `*mut Vm` for the host-fn call. We
    /// model that with a `&mut Vm` method that takes the
    /// pointer, hands it to `with_vm_ptr_set`, and lets the
    /// closure reborrow.
    impl Vm {
        fn miri_drive_cext_pattern(&mut self) -> usize {
            // `let vm_ptr: *mut Vm = self;` — the production
            // pattern (dispatch.rs:152). The outer `&mut self`
            // is "parked" while the closure runs.
            let vm_ptr: *mut Vm = self;
            with_vm_ptr_set(vm_ptr, || {
                let ptr = CURRENT_VM_PTR.with(|c| c.get());
                assert!(!ptr.is_null());
                // SAFETY: matches the cext_funcall_to_vm
                // contract documented at cext.rs:780+. The
                // outer `&mut self` is time-disjoint with this
                // reborrow because no method on `*self` runs
                // between the `let vm_ptr = self;` line and
                // here — we already entered the closure.
                let inner_vm = unsafe { &mut *ptr };
                // Mutate every Vm field the cext path touches
                // in production: heap, classes, interner, stack,
                // pinned. If any of these triggers Stacked
                // Borrows UB it'll surface under Miri here.
                let id = inner_vm.heap.alloc(HeapObj::Array(vec![Value::Int(1)]));
                let name = inner_vm.interner.intern("synthetic");
                inner_vm.stack.push(Value::Array(id));
                inner_vm.pinned.push(Value::Sym(name));
                let popped = inner_vm.stack.pop().expect("just pushed");
                inner_vm.pinned.pop();
                let _ = popped;
                inner_vm.heap.live_count
            })
        }
    }

    #[test]
    fn cext_reentrance_pattern_is_aliasing_clean() {
        // Drive the synthetic pattern. If Miri (Stacked or
        // Tree Borrows) finds an aliasing violation here, it
        // would find one in the real cext path too. The real
        // path is gated by dlopen which Miri can't run; this
        // synthetic version covers the same reborrow shape
        // with only types Miri CAN run.
        let mut vm = mk_vm();
        let pre_live = vm.heap.live_count;
        let live = vm.miri_drive_cext_pattern();
        assert_eq!(live, pre_live + 1, "alloc should bump live_count");
        // Post-invariant: CURRENT_VM_PTR restored to null.
        let ptr_after = CURRENT_VM_PTR.with(|c| c.get());
        assert!(ptr_after.is_null());
    }

    #[test]
    fn cext_reentrance_can_re_enter_multiple_times() {
        // Two separate `miri_drive_cext_pattern` invocations
        // from the same outer scope. Verifies the
        // pointer-stash + restore cycles cleanly across
        // independent host-fn calls.
        let mut vm = mk_vm();
        let _ = vm.miri_drive_cext_pattern();
        let _ = vm.miri_drive_cext_pattern();
        assert_eq!(vm.heap.live_count, 2);
    }

    #[test]
    fn cext_reentrance_nested_save_restore() {
        // Two nested `with_vm_ptr_set` calls should save and
        // restore the TLS exactly. Matches the real-world
        // "host_fn invokes rb_funcallv which invokes another
        // host_fn" chain.
        let mut vm = mk_vm();
        let vm_ptr: *mut Vm = &mut vm;

        let outer = with_vm_ptr_set(vm_ptr, || {
            let outer_ptr = CURRENT_VM_PTR.with(|c| c.get());
            assert!(!outer_ptr.is_null());

            // Inner: same pointer (could in principle be a
            // different vm in a multi-Vm host, but our model
            // is single-Vm per thread).
            let inner = with_vm_ptr_set(vm_ptr, || {
                CURRENT_VM_PTR.with(|c| c.get())
            });
            assert_eq!(outer_ptr, inner);

            // After inner exits, outer pointer is restored.
            CURRENT_VM_PTR.with(|c| c.get())
        });

        assert_eq!(outer, vm_ptr);
        // After outer exits, TLS is null again.
        assert!(CURRENT_VM_PTR.with(|c| c.get()).is_null());
    }
}

