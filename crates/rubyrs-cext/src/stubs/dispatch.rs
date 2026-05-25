//! Class / method / exception dispatch stubs (L3-D bulk-stub batch).
//!
//! See `stubs/mod.rs`. Most return Qnil/Qfalse conservatively; a
//! few forward to existing lib.rs impls (rb_define_private_method →
//! rb_define_method, etc.).

use std::ffi::{c_char, c_int, c_long};

use crate::{with_state, CValue, OpaqueFn, Qnil, Value, ID};

// Class of `v` — forward to rb_basic_class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_class(v: Value) -> Value {
    unsafe { crate::rb_basic_class(v) }
}

// Class name. PR #42 review #3 fix: header declares return as
// `VALUE` (a Ruby String, which callers pass to RSTRING_PTR), not
// `*const c_char`. Previously returned null ptr → UB on any caller
// that read through the returned VALUE as a String. Now: return
// the class's interned name as a real Ruby String VALUE.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_class_name(klass: Value) -> Value {
    crate::with_state(|st| match st.resolve(klass) {
        CValue::Class(name) => {
            let bytes = name.clone().into_bytes();
            st.intern(CValue::str_from_bytes(&bytes))
        }
        _ => Qnil,
    })
}

// Instantiate `klass` with argv — spike returns Qnil (allocator path not modelled).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_class_new_instance(
    _argc: c_int,
    _argv: *const Value,
    _klass: Value,
) -> Value {
    Qnil
}

// respond_to? check — conservatively 0 (no); callers fall back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_respond_to(_obj: Value, _id: ID) -> c_int {
    0
}

// kind_of? check. PR #42 review #4 fix: header declares return as
// `int`, NOT `VALUE`. Previously returned Qfalse (=2 as Value),
// which in C bool context evaluates *truthy*, inverting the
// intended semantics. Return 0 (false) — also ABI-correct vs the
// header's `int` declaration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_is_kind_of(_obj: Value, _klass: Value) -> c_int {
    0
}

// Alias method — no-op at spike scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_alias(
    _klass: Value,
    _new: *const c_char,
    _old: *const c_char,
) {
}

// rb_define_alloc_func — moved to main lib.rs as a real (non-stub)
// registration entry point as of L3-F.

// Private method — forward to rb_define_method; private vs public identical at spike scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_private_method(
    klass: Value,
    name: *const c_char,
    func: OpaqueFn,
    arity: c_int,
) {
    unsafe { crate::rb_define_method(klass, name, func, arity) }
}

// Nested module — forward to rb_define_class_under with Qnil super.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_module_under(parent: Value, name: *const c_char) -> Value {
    unsafe { crate::rb_define_class_under(parent, name, Qnil) }
}

// Ruby-side constant lookup — spike has no const table; return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_const_get(_klass: Value, _id: ID) -> Value {
    Qnil
}

// Instance variable set — ivars not modelled; return `val` per CRuby convention.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ivar_set(_obj: Value, _id: ID, val: Value) -> Value {
    val
}

// Instance variable get — ivars not modelled; return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ivar_get(_obj: Value, _id: ID) -> Value {
    Qnil
}

// Super call — not modelled at the cext boundary; return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_call_super(_argc: c_int, _argv: *const Value) -> Value {
    Qnil
}

// Exception construction — return klass as a stand-in handle (known gap).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_exc_new_str(klass: Value, _str: Value) -> Value {
    klass
}

// Raise pre-built exception — no longjmp wiring yet; panic gives a clear trace.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_exc_raise(_exc: Value) -> ! {
    panic!("rb_exc_raise: not implemented at spike scope")
}

// rescue wrapper — invoke body without rescue (no Rust-side rescue mechanism yet).
//
// PR #42 review #5 fix: header declares the rescue parameter as a
// function pointer `VALUE (*rescue)(VALUE, VALUE)`, NOT a data
// pointer. C callers (flori/json's generator) pass a real fn ptr;
// receiving as *const c_void was an ABI mismatch and would be UB
// once the rescue branch was ever taken. Now matches header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_rescue(
    body: extern "C" fn(Value) -> Value,
    body_arg: Value,
    _rescue: extern "C" fn(Value, Value) -> Value,
    _rescue_arg: Value,
) -> Value {
    body(body_arg)
}

// rb_scan_args — spike returns argc; caller handles args manually.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_scan_args(
    argc: c_int,
    _argv: *const Value,
    _fmt: *const c_char,
) -> c_int {
    argc
}

// Array from C buffer — push each value into a fresh array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ary_new_from_values(n: c_long, vals: *const Value) -> Value {
    let arr = unsafe { crate::rb_ary_new() };
    if vals.is_null() || n <= 0 {
        return arr;
    }
    for i in 0..n as isize {
        let v = unsafe { *vals.offset(i) };
        unsafe { crate::rb_ary_push(arr, v) };
    }
    arr
}

// Hash iteration — snapshot pairs first since callbacks may re-enter cext code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_foreach(
    hash: Value,
    callback: extern "C" fn(Value, Value, Value) -> c_int,
    arg: Value,
) {
    let pairs: Vec<(Value, Value)> = with_state(|st| match st.resolve(hash) {
        CValue::Hash(p) => p.clone(),
        _ => Vec::new(),
    });
    for (k, v) in pairs {
        if callback(k, v, arg) != 0 {
            break;
        }
    }
}

// Hash with capacity hint — forward to rb_hash_new (hint is advisory).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_new_capa(_capa: c_long) -> Value {
    unsafe { crate::rb_hash_new() }
}

// ===== msgpack-ruby additions =====

// Define a constant on a class — spike doesn't model class constants,
// so no-op (subsequent rb_const_get returns Qnil regardless).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_const(_klass: Value, _name: *const c_char, _val: Value) {}

// Class name as C string. CRuby's contract: returned pointer is
// stable for the program's lifetime (cexts cache it in static
// globals). PR #46 review #3: original impl leaked a fresh CString
// per call via `into_raw()`. Now cache CStrings in a thread-local
// table keyed by class name — each distinct name allocates once
// and the pointer stays stable. Bounded by the number of distinct
// class names ever seen across the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_classname(v: Value) -> *const c_char {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::CString;

    thread_local! {
        static CACHE: RefCell<HashMap<String, &'static CString>> =
            RefCell::new(HashMap::new());
    }

    let name = with_state(|st| match st.resolve(v) {
        CValue::Class(n) => n.clone(),
        _ => String::new(),
    });
    if name.is_empty() {
        return c"".as_ptr();
    }
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        if let Some(cs) = m.get(&name) {
            return cs.as_ptr();
        }
        // Box::leak gives a 'static reference, exactly what the
        // CRuby contract promises. Bounded by distinct class
        // names — the same names appear repeatedly in real
        // workloads so the cache hits immediately after first use.
        let cs: &'static CString = Box::leak(Box::new(
            CString::new(name.clone()).unwrap_or_default(),
        ));
        let ptr = cs.as_ptr();
        m.insert(name, cs);
        ptr
    })
}

// Freeze / frozen?: spike doesn't track frozenness.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_freeze(v: Value) -> Value { v }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_frozen_p(_v: Value) -> c_int { 0 }

// Array from variadic args. Variadic forwarding requires nightly
// c_variadic; spike returns an empty array and drops the args
// (cdecl ABI cleans up). msgpack uses rb_ary_new3 in non-hot paths
// (error msgs, registry init); empty array is wrong-but-defined.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ary_new3(_n: c_long) -> Value {
    unsafe { crate::rb_ary_new() }
}

// Class ancestry — rubyrs has no inheritance modeling. Return Qtrue
// conservatively (caller treats any non-Qfalse as "yes").
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_class_inherited_p(_child: Value, _parent: Value) -> Value {
    crate::Qtrue
}

// Hash mutators.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_clear(h: Value) -> Value {
    with_state(|st| {
        if let CValue::Hash(pairs) = st.resolve_mut(h) {
            pairs.clear();
        }
    });
    h
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_dup(h: Value) -> Value {
    let pairs: Vec<(Value, Value)> = with_state(|st| match st.resolve(h) {
        CValue::Hash(p) => p.clone(),
        _ => Vec::new(),
    });
    with_state(|st| st.intern(CValue::Hash(pairs)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_freeze(h: Value) -> Value { h }

// Mixin: spike doesn't model module composition.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_include_module(_klass: Value, _mod: Value) {}

// Opposite of rb_define_alloc_func: spike has no allocator table to undo.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_undef_alloc_func(_klass: Value) {}

// Struct: rubyrs has no Struct. Return the class handle directly;
// the (variadic) member-name args are dropped by the ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_struct_define(name: *const c_char) -> Value {
    if name.is_null() {
        return Qnil;
    }
    let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
    with_state(|st| st.intern(CValue::Class(n)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_struct_new(_klass: Value) -> Value { Qnil }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_struct_aref(_s: Value, _i: c_long) -> Value { Qnil }
