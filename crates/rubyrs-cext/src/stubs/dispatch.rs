//! Class / method / exception dispatch stubs (L3-D bulk-stub batch).
//!
//! See `stubs/mod.rs`. Most return Qnil/Qfalse conservatively; a
//! few forward to existing lib.rs impls (rb_define_private_method →
//! rb_define_method, etc.).

use std::ffi::{c_char, c_int, c_long, c_void};
use std::ptr;

use crate::{with_state, CValue, OpaqueFn, Qfalse, Qnil, Value, ID};

// Class of `v` — forward to rb_basic_class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_class(v: Value) -> Value {
    unsafe { crate::rb_basic_class(v) }
}

// Class name as C string — spike returns null (callers use this for diagnostics).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_class_name(_klass: Value) -> *const c_char {
    ptr::null()
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

// kind_of? check — conservatively false; callers fall back to general dispatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_obj_is_kind_of(_obj: Value, _klass: Value) -> Value {
    Qfalse
}

// respond_to? check — conservatively 0 (no); callers fall back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_respond_to(_obj: Value, _id: ID) -> c_int {
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

// Custom allocator — no-op (rubyrs doesn't model allocator dispatch separately).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_alloc_func(
    _klass: Value,
    _func: extern "C" fn(Value) -> Value,
) {
}

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_rescue(
    body: extern "C" fn(Value) -> Value,
    body_arg: Value,
    _rescue: *const c_void,
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
