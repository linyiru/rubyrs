//! rubyrs — a tiny Ruby-subset runtime, embeddable in Rust hosts.
//!
//! # Quick start
//!
//! ```no_run
//! use rubyrs::{Runtime, Value};
//!
//! let mut rt = Runtime::new();
//! // Per ADR 0017, `Runtime::new()` defaults its stdout sink to
//! // `std::io::sink()`. Wire it up to wherever the host wants
//! // script output to land before evaluating anything — the CLI
//! // binary uses process stdout; library embedders typically
//! // capture into a buffer.
//! rt.set_stdout(Box::new(std::io::stdout()));
//! rt.eval(r#"puts "hello, world""#, "inline").unwrap();
//!
//! // Register a host function callable from Ruby:
//! rt.register_fn("host_pid", |_args| Ok(Value::Int(std::process::id() as i64)));
//! rt.eval(r#"puts "pid is #{host_pid}""#, "inline").unwrap();
//! ```
//!
//! See [`docs/SUBSET.md`](https://github.com/linyiru/rubyrs/blob/master/docs/SUBSET.md)
//! for the Ruby semantics this runtime does and does not support.

mod ast;
mod bytecode;
mod compiler;
mod error;
mod heap;
mod intern;
#[cfg(feature = "stdlib")]
mod stdlib_vendor;
mod value;
mod vm;

use std::io::Write;
use std::path::Path;
use std::rc::Rc;

pub use error::{RubyError, Span, Trap, TrapFrame};
pub use value::Value;
pub use intern::SymId;

// C-ext compat (spike Level 0): the `rb_*` functions from `rubyrs-cext`
// are only ever called from dlopen'd C extensions — there is no Rust
// call site for the linker to see. Without this `#[used]` static
// holding raw function pointers, dead-code elimination drops them from
// the rubyrs binary and `dlsym` from the bundle returns NULL.
//
// `Qnil` / `Qtrue` / `Qfalse` get their own `#[used]` on the statics
// themselves over in `rubyrs-cext`; functions need this indirection
// because `#[used]` only applies to statics, not function definitions.
// Function pointers are `Sync`; using strongly-typed statics avoids
// the `fn as usize` cast which isn't allowed in const context.
//
// All of this is wrapped in a single `cfg(feature = "cext")` module
// so the entire block disappears (deps, symbols, link-keep-alive
// trick) when cext is opt-out. The 162 statics + the L3-D bulk
// exports module move inside `_cext_link_keep_alive` unchanged;
// `#[used]` still applies through the module boundary because the
// linker sees mangled symbols flat, not Rust modules. See ADR 0015
// (concentric architecture) for why `cext` is the first tier
// boundary made opt-in.
#[cfg(feature = "cext")]
mod _cext_link_keep_alive {
#[used]
static _RB_STR_NEW_CSTR: unsafe extern "C" fn(*const std::ffi::c_char) -> rubyrs_cext::Value =
    rubyrs_cext::rb_str_new_cstr;
#[used]
static _RB_STR_NEW: unsafe extern "C" fn(*const std::ffi::c_char, std::ffi::c_long) -> rubyrs_cext::Value =
    rubyrs_cext::rb_str_new;
#[used]
static _RSTRING_PTR: unsafe extern "C" fn(rubyrs_cext::Value) -> *const std::ffi::c_char =
    rubyrs_cext::RSTRING_PTR;
#[used]
static _RSTRING_LEN: unsafe extern "C" fn(rubyrs_cext::Value) -> std::ffi::c_long =
    rubyrs_cext::RSTRING_LEN;
#[used]
static _RB_DEFINE_GLOBAL_FUNCTION: unsafe extern "C" fn(
    *const std::ffi::c_char,
    rubyrs_cext::OpaqueFn,
    std::ffi::c_int,
) = rubyrs_cext::rb_define_global_function;
#[used]
static _RB_STR_NEW_FROZEN: unsafe extern "C" fn(rubyrs_cext::Value) -> rubyrs_cext::Value =
    rubyrs_cext::rb_str_new_frozen;
#[used]
static _RB_STRING_VALUE_CSTR: unsafe extern "C" fn(*mut rubyrs_cext::Value) -> *const std::ffi::c_char =
    rubyrs_cext::rb_string_value_cstr;
#[used]
static _RB_STRING_VALUE_PTR: unsafe extern "C" fn(*mut rubyrs_cext::Value) -> *const std::ffi::c_char =
    rubyrs_cext::rb_string_value_ptr;
#[used]
static _RB_INT2NUM: unsafe extern "C" fn(std::ffi::c_int) -> rubyrs_cext::Value =
    rubyrs_cext::rb_int2num;
#[used]
static _RB_LONG2NUM: unsafe extern "C" fn(std::ffi::c_long) -> rubyrs_cext::Value =
    rubyrs_cext::rb_long2num;
#[used]
static _RB_NUM2INT: unsafe extern "C" fn(rubyrs_cext::Value) -> std::ffi::c_int =
    rubyrs_cext::rb_num2int;
#[used]
static _RB_NUM2LONG: unsafe extern "C" fn(rubyrs_cext::Value) -> std::ffi::c_long =
    rubyrs_cext::rb_num2long;
#[used]
static _RB_NUM2ULONG: unsafe extern "C" fn(rubyrs_cext::Value) -> std::ffi::c_ulong =
    rubyrs_cext::rb_num2ulong;
#[used]
static _RB_DEFINE_MODULE: unsafe extern "C" fn(*const std::ffi::c_char) -> rubyrs_cext::Value =
    rubyrs_cext::rb_define_module;
#[used]
static _RB_DEFINE_CLASS_UNDER: unsafe extern "C" fn(
    rubyrs_cext::Value,
    *const std::ffi::c_char,
    rubyrs_cext::Value,
) -> rubyrs_cext::Value = rubyrs_cext::rb_define_class_under;
#[used]
static _RB_DEFINE_SINGLETON_METHOD: unsafe extern "C" fn(
    rubyrs_cext::Value,
    *const std::ffi::c_char,
    rubyrs_cext::OpaqueFn,
    std::ffi::c_int,
) = rubyrs_cext::rb_define_singleton_method;
// L3-C: instance-method registration. Same signature as singleton.
#[used]
static _RB_DEFINE_METHOD: unsafe extern "C" fn(
    rubyrs_cext::Value,
    *const std::ffi::c_char,
    rubyrs_cext::OpaqueFn,
    std::ffi::c_int,
) = rubyrs_cext::rb_define_method;
#[used]
static _RB_INTERN: unsafe extern "C" fn(*const std::ffi::c_char) -> rubyrs_cext::ID =
    rubyrs_cext::rb_intern;
#[used]
static _RB_FUNCALLV: unsafe extern "C" fn(
    rubyrs_cext::Value,
    rubyrs_cext::ID,
    std::ffi::c_int,
    *const rubyrs_cext::Value,
) -> rubyrs_cext::Value = rubyrs_cext::rb_funcallv;
#[used]
static _RB_ARY_NEW: unsafe extern "C" fn() -> rubyrs_cext::Value = rubyrs_cext::rb_ary_new;
#[used]
static _RB_ARY_NEW_CAPA: unsafe extern "C" fn(std::ffi::c_long) -> rubyrs_cext::Value =
    rubyrs_cext::rb_ary_new_capa;
#[used]
static _RB_ARY_PUSH: unsafe extern "C" fn(rubyrs_cext::Value, rubyrs_cext::Value) -> rubyrs_cext::Value =
    rubyrs_cext::rb_ary_push;
#[used]
static _RB_ARY_ENTRY: unsafe extern "C" fn(rubyrs_cext::Value, std::ffi::c_long) -> rubyrs_cext::Value =
    rubyrs_cext::rb_ary_entry;
#[used]
static _RARRAY_LEN: unsafe extern "C" fn(rubyrs_cext::Value) -> std::ffi::c_long =
    rubyrs_cext::RARRAY_LEN;
#[used]
static _RB_HASH_NEW: unsafe extern "C" fn() -> rubyrs_cext::Value = rubyrs_cext::rb_hash_new;
#[used]
static _RB_HASH_ASET: unsafe extern "C" fn(rubyrs_cext::Value, rubyrs_cext::Value, rubyrs_cext::Value) -> rubyrs_cext::Value =
    rubyrs_cext::rb_hash_aset;
#[used]
static _RB_HASH_AREF: unsafe extern "C" fn(rubyrs_cext::Value, rubyrs_cext::Value) -> rubyrs_cext::Value =
    rubyrs_cext::rb_hash_aref;

// === L3-A: rb_raise + rb_e* class sentinels ===
//
// `rb_raise` itself is in C (rubyrs-cext/c/raise.c) — the variadic
// shim uses vsnprintf. The Rust extern declaration here + the
// `#[used]` static pulls the C symbol into the host binary's
// export table.
//
// Each `rb_e*` sentinel is a `pub static Value` defined in
// rubyrs_cext::raise; the `#[used]` references below keep them in
// the host's symbol table for dlopen'd C extensions to resolve
// (matches the existing pattern for Qnil/Qtrue/Qfalse).
#[cfg(not(target_os = "wasi"))]
mod cext_raise_exports {
    unsafe extern "C" {
        pub fn rb_raise(exc_class: u64, fmt: *const std::ffi::c_char, ...) -> !;
    }

    #[used]
    static _RB_RAISE: unsafe extern "C" fn(u64, *const std::ffi::c_char, ...) -> ! = rb_raise;

    #[used]
    static _RB_E_RUNTIME:    &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eRuntimeError;
    #[used]
    static _RB_E_ARGUMENT:   &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eArgumentError;
    #[used]
    static _RB_E_TYPE:       &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eTypeError;
    #[used]
    static _RB_E_RANGE:      &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eRangeError;
    #[used]
    static _RB_E_STANDARD:   &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eStandardError;
    #[used]
    static _RB_E_NO_METHOD:  &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eNoMethodError;
    #[used]
    static _RB_E_IO:         &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eIOError;
    #[used]
    static _RB_E_NAME:       &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eNameError;
    #[used]
    static _RB_E_ZERO_DIV:   &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eZeroDivError;
    #[used]
    static _RB_E_NOT_IMP:    &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eNotImpError;
    #[used]
    static _RB_E_EOF:        &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eEOFError;
    #[used]
    static _RB_E_FROZEN:     &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eFrozenError;
    #[used]
    static _RB_E_ENC_COMPAT: &rubyrs_cext::Value = &rubyrs_cext::raise::rb_eEncCompatError;
}

// === L3-B: TypedData ABI ===
#[used]
static _RB_DATA_TYPED_OBJECT_WRAP: unsafe extern "C" fn(
    rubyrs_cext::Value,
    *mut std::ffi::c_void,
    *const rubyrs_cext::rb_data_type_t,
) -> rubyrs_cext::Value = rubyrs_cext::rb_data_typed_object_wrap;
#[used]
static _RB_CHECK_TYPEDDATA: unsafe extern "C" fn(
    rubyrs_cext::Value,
    *const rubyrs_cext::rb_data_type_t,
) -> *mut std::ffi::c_void = rubyrs_cext::rb_check_typeddata;

// === L3-D: bulk #[used] force-exports for stubs/*.rs ===
//
// rubyrs-cext is consumed as an rlib. The linker drops object
// files whose symbols aren't referenced from the consuming crate
// (rubyrs binary). `#[unsafe(no_mangle)]` on a fn alone is not
// enough — `#[used]` only applies to statics in stable Rust, so
// each cext fn needs a static reference here to survive DCE.
//
// The list below is exhaustive for what flori/json's parser.c
// and generator.c touch. Compile-checked by type inference: if
// a signature drifts between header and impl, the static line
// fails to type-check (catches ABI breakage early).
#[allow(non_upper_case_globals)]
mod _cext_l3d_exports {
    use rubyrs_cext::{ID, Value};
    use std::ffi::{c_char, c_double, c_int, c_long, c_longlong, c_void};

    // Tricky fns (live in main lib.rs, not stubs/).
    #[used] static F1: unsafe extern "C" fn(Value) -> Value = rubyrs_cext::rb_basic_class;
    #[used] static F2: unsafe extern "C" fn(Value) -> *mut *mut c_void = rubyrs_cext::rb_typeddata_data_slot;
    #[used] static F3: unsafe extern "C" fn(Value) -> Value = rubyrs_cext::rb_path_to_class;
    #[used] static F4: unsafe extern "C" fn(*const c_char) -> Value = rubyrs_cext::rb_path2class;

    // stubs/types.rs
    use rubyrs_cext::stubs::types::*;
    #[used] static T1: unsafe extern "C" fn(Value) -> c_int = rb_value_type;
    #[used] static T2: unsafe extern "C" fn(Value) -> c_int = rb_value_is_fixnum;
    #[used] static T3: unsafe extern "C" fn(Value) -> c_int = rb_value_is_flonum;
    #[used] static T4: unsafe extern "C" fn(Value) -> c_int = rb_value_is_special_const;
    #[used] static T5: unsafe extern "C" fn(c_longlong) -> Value = rb_ll2num;
    #[used] static T6: unsafe extern "C" fn(c_double) -> Value = rb_dbl2num;
    #[used] static T7: unsafe extern "C" fn(*const c_char, c_int) -> Value = rb_cstr2inum;
    #[used] static T8: unsafe extern "C" fn(*const c_char, c_int) -> c_double = rb_cstr_to_dbl;
    #[used] static T9: unsafe extern "C" fn(Value) -> c_double = rb_float_value;
    #[used] static T10: unsafe extern "C" fn(ID) -> Value = rb_id2sym;
    #[used] static T11: unsafe extern "C" fn(Value) -> ID = rb_sym2id;
    #[used] static T12: unsafe extern "C" fn(Value) -> Value = rb_sym2str;
    #[used] static T13: unsafe extern "C" fn(Value, c_int) = rb_check_type;
    #[used] static T14: unsafe extern "C" fn(c_int, c_int, c_int) = rb_check_arity;
    #[used] static T15: unsafe extern "C" fn(Value, c_int, *const c_char, *const c_char) -> Value = rb_convert_type;
    #[used] static T16: unsafe extern "C" fn(Value) -> c_long = RHASH_SIZE;
    // msgpack additions in types.rs
    #[used] static T17: unsafe extern "C" fn(c_longlong) -> Value = rb_ll2inum;
    #[used] static T18: unsafe extern "C" fn(u64) -> Value = rb_ull2inum;
    #[used] static T19: unsafe extern "C" fn(Value) -> c_double = rb_num2dbl;
    #[used] static T20: unsafe extern "C" fn(c_double) -> Value = rb_float_new;
    #[used] static T21: unsafe extern "C" fn(Value, *mut c_int) -> usize = rb_absint_size;
    #[used] static T22: unsafe extern "C" fn(Value) -> u64 = rb_big2ull;
    #[used] static T23: unsafe extern "C" fn(Value) -> c_longlong = rb_big2ll;
    #[used] static T24: unsafe extern "C" fn(Value) -> c_int = rb_bignum_positive_p;

    // stubs/strings.rs
    use rubyrs_cext::stubs::strings::*;
    #[used] static S1: unsafe extern "C" fn(c_long) -> Value = rb_str_buf_new;
    #[used] static S2: unsafe extern "C" fn(Value) -> Value = rb_str_dup;
    #[used] static S3: unsafe extern "C" fn(Value) -> Value = rb_str_freeze;
    #[used] static S4: unsafe extern "C" fn(Value) -> Value = rb_str_intern;
    #[used] static S5: unsafe extern "C" fn(Value, c_long) = rb_str_set_len;
    #[used] static S6: unsafe extern "C" fn(Value, c_long, c_long) -> Value = rb_str_substr;
    #[used] static S7: unsafe extern "C" fn(*mut Value) -> Value = rb_string_value;
    #[used] static S8: unsafe extern "C" fn(*const c_char, c_long) -> Value = rb_utf8_str_new;
    #[used] static S9: unsafe extern "C" fn(*const c_char) -> Value = rb_utf8_str_new_cstr;
    #[used] static S10: unsafe extern "C" fn(*const c_char, c_long) -> Value = rb_usascii_str_new;
    #[used] static S11: unsafe extern "C" fn(*const c_char, c_long, *const c_void) -> Value = rb_enc_str_new;
    #[used] static S12: unsafe extern "C" fn(*const c_char, c_long, *const c_void) -> Value = rb_enc_interned_str;
    #[used] static S13: unsafe extern "C" fn() -> *mut c_void = rb_utf8_encoding;
    #[used] static S14: unsafe extern "C" fn() -> *mut c_void = rb_ascii8bit_encoding;
    #[used] static S15: unsafe extern "C" fn() -> *mut c_void = rb_usascii_encoding;
    #[used] static S16: unsafe extern "C" fn() -> c_int = rb_utf8_encindex;
    #[used] static S17: unsafe extern "C" fn() -> c_int = rb_usascii_encindex;
    #[used] static S18: unsafe extern "C" fn() -> c_int = rb_ascii8bit_encindex;
    #[used] static S19: unsafe extern "C" fn(Value, c_int) -> Value = rb_enc_associate_index;
    #[used] static S20: unsafe extern "C" fn(Value) -> c_int = rb_enc_get_index;
    #[used] static S21: unsafe extern "C" fn(Value) -> c_int = rb_enc_str_coderange;
    #[used] static S22: unsafe extern "C-unwind" fn(*mut c_void, Value, *const c_char) -> ! = rb_enc_raise;
    // msgpack additions in strings.rs
    #[used] static S23: unsafe extern "C" fn(Value, *const c_char, c_long) -> Value = rb_str_buf_cat;
    #[used] static S24: unsafe extern "C" fn(Value, Value) -> Value = rb_str_replace;
    #[used] static S25: unsafe extern "C" fn(Value, c_long) -> Value = rb_str_resize;
    #[used] static S26: unsafe extern "C" fn(Value, Value, c_int, Value) -> Value = rb_str_encode;
    #[used] static S27: unsafe extern "C" fn(Value) -> Value = rb_check_string_type;
    #[used] static S28: unsafe extern "C" fn(Value) -> Value = rb_String;
    #[used] static S29: unsafe extern "C" fn(*mut c_void) -> c_int = rb_enc_asciicompat;
    #[used] static S30: unsafe extern "C" fn(c_int) -> *mut c_void = rb_enc_from_index;
    #[used] static S31: unsafe extern "C" fn(*mut c_void) -> Value = rb_enc_from_encoding;

    // stubs/gc.rs
    use rubyrs_cext::stubs::gc::*;
    #[used] static G1: unsafe extern "C" fn(Value) = rb_gc_mark;
    #[used] static G2: unsafe extern "C" fn(Value) = rb_gc_mark_movable;
    #[used] static G3: unsafe extern "C" fn(Value) -> Value = rb_gc_location;
    #[used] static G4: unsafe extern "C" fn(Value) = rb_gc_register_mark_object;
    #[used] static G5: unsafe extern "C" fn(*mut Value) = rb_global_variable;
    #[used] static G6: unsafe extern "C" fn(c_int) = rb_ext_ractor_safe;
    #[used] static G7: unsafe extern "C" fn(*const c_char) = rb_warn;
    #[used] static G8: unsafe extern "C" fn(*const c_char, *const c_char) = rb_category_warn;
    #[used] static G9: unsafe extern "C" fn(*const c_char, *mut c_void) -> Value = rb_vsprintf;
    #[used] static G10: unsafe extern "C" fn(Value) -> Value = rb_io_flush;
    #[used] static G11: unsafe extern "C" fn(Value, Value) -> Value = rb_io_write;
    #[used] static G12: unsafe extern "C" fn(*const c_char) -> Value = rb_require;
    // msgpack additions in gc.rs
    #[used] static G13: unsafe extern "C" fn(*const c_char) -> ! = rb_bug;
    #[used] static G14: unsafe extern "C" fn() -> Value = rb_errinfo;
    #[used] static G15: unsafe extern "C" fn(c_int) -> ! = rb_jump_tag;
    #[used] static G16: unsafe extern "C" fn(extern "C" fn(Value) -> Value, Value, *mut c_int) -> Value = rb_protect;
    #[used] static G17: unsafe extern "C" fn(extern "C" fn(Value) -> Value, Value, extern "C" fn(Value, Value) -> Value, Value) -> Value = rb_rescue2;
    #[used] static G18: unsafe extern "C" fn(Value) -> Value = rb_yield;
    #[used] static G19: unsafe extern "C" fn(*const c_char, c_long, *mut c_void) -> u64 = rb_intern3;

    // stubs/dispatch.rs
    use rubyrs_cext::stubs::dispatch::*;
    #[used] static D1: unsafe extern "C" fn(Value) -> Value = rb_obj_class;
    #[used] static D2: unsafe extern "C" fn(Value) -> Value = rb_class_name;
    #[used] static D3: unsafe extern "C" fn(c_int, *const Value, Value) -> Value = rb_class_new_instance;
    #[used] static D4: unsafe extern "C" fn(Value, Value) -> c_int = rb_obj_is_kind_of;
    #[used] static D5: unsafe extern "C" fn(Value, ID) -> c_int = rb_respond_to;
    #[used] static D6: unsafe extern "C" fn(Value, *const c_char, *const c_char) = rb_define_alias;
    // L3-F: rb_define_alloc_func now lives in main lib.rs (was stub) —
    // signature uses OpaqueFn since the cext side stores it that way.
    #[used] static D7: unsafe extern "C" fn(Value, rubyrs_cext::OpaqueFn) = rubyrs_cext::rb_define_alloc_func;
    #[used] static D8: unsafe extern "C" fn(Value, *const c_char, rubyrs_cext::OpaqueFn, c_int) = rb_define_private_method;
    #[used] static D9: unsafe extern "C" fn(Value, *const c_char) -> Value = rb_define_module_under;
    #[used] static D10: unsafe extern "C" fn(Value, ID) -> Value = rb_const_get;
    #[used] static D11: unsafe extern "C" fn(Value, ID, Value) -> Value = rb_ivar_set;
    #[used] static D12: unsafe extern "C" fn(Value, ID) -> Value = rb_ivar_get;
    #[used] static D13: unsafe extern "C" fn(c_int, *const Value) -> Value = rb_call_super;
    #[used] static D14: unsafe extern "C" fn(Value, Value) -> Value = rb_exc_new_str;
    #[used] static D15: unsafe extern "C" fn(Value) -> ! = rb_exc_raise;
    #[used] static D16: unsafe extern "C" fn(extern "C" fn(Value) -> Value, Value, extern "C" fn(Value, Value) -> Value, Value) -> Value = rb_rescue;
    #[used] static D17: unsafe extern "C" fn(c_int, *const Value, *const c_char) -> c_int = rb_scan_args;
    #[used] static D18: unsafe extern "C" fn(c_long, *const Value) -> Value = rb_ary_new_from_values;
    #[used] static D19: unsafe extern "C" fn(Value, extern "C" fn(Value, Value, Value) -> c_int, Value) = rb_hash_foreach;
    #[used] static D20: unsafe extern "C" fn(c_long) -> Value = rb_hash_new_capa;
    // msgpack additions in dispatch.rs
    #[used] static D21: unsafe extern "C" fn(Value, *const c_char, Value) = rb_define_const;
    #[used] static D22: unsafe extern "C" fn(Value) -> *const c_char = rb_obj_classname;
    #[used] static D23: unsafe extern "C" fn(Value) -> Value = rb_obj_freeze;
    #[used] static D24: unsafe extern "C" fn(Value) -> c_int = rb_obj_frozen_p;
    #[used] static D25: unsafe extern "C" fn(c_long) -> Value = rb_ary_new3;
    // Arity-specialised rb_ary_new3 dispatch targets (the header
    // macro routes call sites to these by counting __VA_ARGS__).
    #[used] static D25A: unsafe extern "C" fn(Value) -> Value = rubyrs_ary_new3_1;
    #[used] static D25B: unsafe extern "C" fn(Value, Value) -> Value = rubyrs_ary_new3_2;
    #[used] static D25C: unsafe extern "C" fn(Value, Value, Value) -> Value = rubyrs_ary_new3_3;
    // L3-K: Proc dispatch (msgpack's protected_proc_call_safe).
    #[used] static D25D: unsafe extern "C" fn(Value, c_int, *const Value, Value) -> Value =
        rb_proc_call_with_block;
    #[used] static D26: unsafe extern "C" fn(Value, Value) -> Value = rb_class_inherited_p;
    #[used] static D27: unsafe extern "C" fn(Value) -> Value = rb_hash_clear;
    #[used] static D28: unsafe extern "C" fn(Value) -> Value = rb_hash_dup;
    #[used] static D29: unsafe extern "C" fn(Value) -> Value = rb_hash_freeze;
    #[used] static D30: unsafe extern "C" fn(Value, Value) = rb_include_module;
    #[used] static D31: unsafe extern "C" fn(Value) = rb_undef_alloc_func;
    #[used] static D32: unsafe extern "C" fn(*const c_char) -> Value = rb_struct_define;
    #[used] static D33: unsafe extern "C" fn(Value) -> Value = rb_struct_new;
    #[used] static D34: unsafe extern "C" fn(Value, c_long) -> Value = rb_struct_aref;
}
} // mod _cext_link_keep_alive

// Public Prism node-class manifests. Generated by build.rs from
// `data/supported_prism_nodes.txt` and `data/rides_along_prism_nodes.txt`,
// and validated against `src/ast.rs` — see build.rs. Consumed by
// `rubyrs-gapscan` as the single source of truth for the supported subset.
include!(concat!(env!("OUT_DIR"), "/prism_node_sets.rs"));

/// Configuration for a [`Runtime`]. Defaults are unlimited; tighten for
/// untrusted scripts.
///
/// **Construction**: use `Config::default()` with field-update syntax:
/// ```no_run
/// let _cfg = rubyrs::Config { fuel: Some(1_000_000), ..Default::default() };
/// ```
///
/// Adding new fields is still source-breaking for downstream
/// embedders using full struct literals. The fix is a dedicated
/// builder API (`Config::builder().fuel(n).env(map).build()`)
/// rather than `#[non_exhaustive]`, which forbids struct
/// expressions cross-crate entirely and would have a much larger
/// migration footprint here. Tracked as follow-up; see PR #88
/// thread for the analysis.
pub struct Config {
    /// When true, every potential GC point triggers a full collection.
    /// Useful for catching root-set bugs in host code; rough on
    /// performance. Equivalent to `STRESS_GC=1` env var.
    pub stress_gc: bool,
    /// If `Some(n)`, dispatching more than `n` ops returns a
    /// `ResourceExhausted` trap. Includes ops inside blocks via
    /// `dispatch_until`, so a runaway `[1].each { while true ... }`
    /// cannot bypass the limit.
    pub fuel: Option<u64>,
    /// If `Some(n)`, allocating past `n` simultaneously-live heap
    /// objects (Instance / Array / Hash) returns a `ResourceExhausted`
    /// trap. Checked after `maybe_gc`, so only steady-state allocation
    /// counts.
    pub max_heap_objects: Option<usize>,
    /// If `Some(n)`, pushing past `n` simultaneously-live frames
    /// returns a `ResourceExhausted` trap before the host's Rust stack
    /// can overflow.
    pub max_frames: Option<usize>,
    /// If `Some(n)`, runtime `String#to_sym` (and any other future
    /// script-driven intern path) traps when interning would push
    /// the total beyond `n` distinct symbols. Compile-time intern
    /// (method names, ivar names, string literals in source) is
    /// not capped — it's bounded by source size, which the host
    /// already controls by how big a script it feeds `eval`.
    /// Defends against `arr.map { |x| x.to_s.to_sym }`-style loops
    /// that fuel can't usefully bound.
    pub max_symbols: Option<usize>,
    /// If `Some(n)`, individual `String` / `Array` / `Hash` values
    /// can't grow past `n` bytes of content. String size is the
    /// byte length; Array/Hash size is `len * size_of::<Value>()`
    /// (or `* size_of::<(Value, Value)>()` for Hash). Checked at
    /// mutation sites: `String#+` / `String#*` / `Array#push` /
    /// `Array#<<` / `Array#[]=` / `Hash#[]=`. Heap-cap caps
    /// *number* of live objects (good against shallow alloc
    /// storms); this caps *individual* object size (good against
    /// `"a" * 10_000_000`, which is one heap object that grabs 10 MB).
    pub max_value_bytes: Option<usize>,
    /// If `Some(d)`, an `eval` call that runs longer than `d`
    /// wall-clock time returns a `ResourceExhausted` trap. Checked
    /// every 1024 ops (cheap and precise enough for the host-side
    /// timeouts this is meant to enforce — request budgets,
    /// gemspec-evaluation guards, similar). The deadline is
    /// per-`eval`: each call re-anchors the clock, so a host can
    /// reuse a Runtime across many short evaluations without each
    /// one inheriting the previous timer.
    pub deadline: Option<std::time::Duration>,
    /// Host-injected ENV map. Closes the ADR 0017 deviation: with
    /// the previous default, `LoadConst("ENV")` populated a Hash
    /// from `std::env::vars()` of the host process — script-visible
    /// non-deterministic value and a capability leak from the host's
    /// environment into untrusted scripts.
    ///
    /// `None` (default) means the script sees an empty `ENV` Hash.
    /// `Some(map)` means script-visible `ENV[k]` resolves against
    /// `map` only. The host explicitly chooses what to expose; the
    /// script never reads the host process directly.
    ///
    /// The CLI binary `rubyrs` sets this from `std::env::vars()` so
    /// `rubyrs script.rb` behaves like CRuby; library/embed users
    /// must opt in explicitly.
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Host-injected PID for the `$$` global. Closes the ADR 0017
    /// deviation: with the previous default, `$$` returned the host
    /// process's PID via `std::process::id()` — script-visible
    /// non-deterministic value.
    ///
    /// `None` (default) means `$$` returns `0` as a sentinel
    /// (CRuby's `$$` is documented to always be a positive Integer;
    /// `0` is a sentinel that won't collide with any real PID).
    /// `Some(n)` means `$$` returns `n`.
    ///
    /// Typed as `Option<NonZeroU32>` to match `std::process::id()`'s
    /// return type and enforce the "positive PID" contract at the
    /// type level — `0` is reserved for the default sentinel and
    /// negatives are impossible by construction (Copilot review PR #88).
    ///
    /// The CLI binary `rubyrs` sets this from `std::process::id()`
    /// so `rubyrs script.rb` behaves like CRuby; embed users that
    /// want the host PID exposed must opt in.
    pub pid: Option<std::num::NonZeroU32>,
    /// Host-injected wall-clock source for `Time.now`. Closes the
    /// ADR 0017 deviation that would otherwise let `Time.now`
    /// reach for `std::time::SystemTime::now()` directly —
    /// script-visible non-determinism + a host-clock capability
    /// leak into untrusted scripts.
    ///
    /// `None` (default) means script-visible `Time.now` raises
    /// `RuntimeError` (Tier 1 stays deterministic by default).
    /// `Some(closure)` is called once per `Time.now` invocation;
    /// the closure returns `(epoch_seconds, nanoseconds)` and the
    /// preamble's `Time` class wraps that into a `Value::Object`
    /// instance carrying `@sec` / `@nsec` ivars.
    ///
    /// The closure is `Arc<dyn Fn>` so a single Runtime can be
    /// shared across calls without consuming the injected source.
    /// `Send + Sync` keeps the Runtime usable from multi-threaded
    /// host code that wraps it in an `Arc<Mutex<_>>`-style guard;
    /// rubyrs itself is single-threaded (one Runtime = one Vm at
    /// a time), but the bound matches what `register_fn` already
    /// requires of host functions.
    ///
    /// The CLI binary `rubyrs` injects `std::time::SystemTime::now()`
    /// so `rubyrs script.rb` behaves like CRuby; library / embed
    /// users that want the host wall-clock exposed must opt in
    /// explicitly. Deterministic-test hosts inject a fixed
    /// `|| (1_700_000_000, 0)` closure for reproducible output.
    pub time_now: Option<std::sync::Arc<dyn Fn() -> (i64, u32) + Send + Sync>>,
}

impl Default for Config {
    /// Default Config. `stress_gc` auto-enables from the `STRESS_GC`
    /// env var on non-wasi hosts so `STRESS_GC=1 cargo test` flips
    /// every `Runtime::new()`-using test into stress mode — the
    /// previous behavior, lost when `Vm::new` stopped reading the
    /// env var to satisfy wizer's no-imports rule. The wizer path
    /// does NOT go through `Config::default()` (it calls
    /// `Runtime::new_default_impl` directly), so this env read can't
    /// pollute the snapshot. The CLI binary explicitly reads
    /// `STRESS_GC` again from main.rs for the same flag — both reads
    /// agree, harmless.
    fn default() -> Self {
        #[cfg(not(target_os = "wasi"))]
        let stress_gc = std::env::var("STRESS_GC").is_ok();
        #[cfg(target_os = "wasi")]
        let stress_gc = false;
        Self {
            stress_gc,
            fuel: None,
            max_heap_objects: None,
            max_frames: None,
            max_symbols: None,
            max_value_bytes: None,
            deadline: None,
            env: None,
            pid: None,
            time_now: None,
        }
    }
}

/// Read-only handle into the runtime's heap, passed to closures
/// registered via [`Runtime::register_fn_v2`]. Lets the closure
/// unpack `Value::Array` / `Value::Hash` arguments without going
/// back through [`Runtime::resolve_array`] / [`Runtime::resolve_hash`]
/// (which clone).
///
/// Mutability is omitted by construction:
/// - `HostCtx` exposes no mutating method.
/// - The V2 dispatch path (see `Vm::invoke_host_fn`) does NOT
///   overwrite `CURRENT_VM_PTR`, and the TLS itself is `pub(crate)`,
///   so an external v2 closure has no language-level path to reach
///   the `*mut Vm` re-entry channel. The TLS may already be non-null
///   from an outer v1/cext frame — the guarantee is "unreachable
///   from external v2 code," not "TLS is null."
///
/// Together these mean the slices returned by `resolve_array` /
/// `resolve_hash` are valid for the entire closure body without
/// further caveat from outside the crate. Embed hosts MUST NOT leak
/// the returned slices past the closure return (the lifetime
/// already prevents this, but worth stating).
pub struct HostCtx<'a> {
    heap: &'a heap::Heap,
    interner: &'a intern::Interner,
}

impl<'a> HostCtx<'a> {
    /// Internal constructor — the dispatch site borrows `&Heap` and
    /// `&Interner` from the VM and hands them to the v2 closure via
    /// this ctx. Both borrows are immutable and time-bounded by the
    /// closure invocation (see `Vm::invoke_host_fn`).
    pub(crate) fn new(heap: &'a heap::Heap, interner: &'a intern::Interner) -> Self {
        Self { heap, interner }
    }

    /// Borrow the contents of a `Value::Array`. Returns `None` for
    /// any other shape — the host fn can decide whether to error or
    /// fall through.
    pub fn resolve_array(&self, val: &Value) -> Option<&[Value]> {
        if let Value::Array(id) = val {
            Some(self.heap.array(*id).as_slice())
        } else {
            None
        }
    }

    /// Borrow the contents of a `Value::Hash` as a flat slice of
    /// `(key, value)` pairs (preserving insertion order). Returns
    /// `None` for any other shape.
    pub fn resolve_hash(&self, val: &Value) -> Option<&[(Value, Value)]> {
        if let Value::Hash(id) = val {
            Some(self.heap.hash(*id).as_slice())
        } else {
            None
        }
    }

    /// Borrow the interned name of a `Value::Sym`. Returns `None` for
    /// any other shape. Useful for v2 host fns that receive a
    /// kwargs `Hash` with Symbol keys (`require:`, `platforms:`, …)
    /// — the host can branch on the borrowed `&str` directly without
    /// a Ruby-side `k.to_s` rebuild of the Hash.
    pub fn resolve_sym(&self, val: &Value) -> Option<&str> {
        if let Value::Sym(id) = val {
            Some(self.interner.resolve(*id))
        } else {
            None
        }
    }
}

/// A self-contained rubyrs runtime. State (class definitions, top-level
/// methods, registered host functions, GC heap) persists across calls to
/// [`Runtime::eval`].
pub struct Runtime {
    vm: vm::Vm,
    /// Per-`eval` wall-clock budget (P2-14a). Retained as a
    /// Duration; an absolute `Instant` is computed at the start of
    /// each `eval` call and stored on `Vm.deadline_at`. `None` means
    /// unlimited.
    deadline: Option<std::time::Duration>,
}

/// Per-process slot used by the wizer pre-initialize path. On
/// wasm32-wasip1, the binary exports `wizer.initialize` (below)
/// which constructs a default Runtime and stashes it here; Wizer
/// then snapshots linear memory so a later `wasmtime run` picks
/// up the prebuilt classes + preamble bytecode without redoing
/// that work. The CLI binary's `main()` calls `take_wizer_runtime()`
/// to consume the slot.
///
/// Single-threaded by design: wasm32-wasip1 has no threads, and
/// the CLI binary owns the process. Embedded hosts that link
/// rubyrs as a library don't touch this slot — they construct
/// their own Runtime via `Runtime::new()` / `Runtime::with_config()`.
#[cfg(target_os = "wasi")]
static mut WIZER_RUNTIME: Option<Runtime> = None;

/// Wizer entry point. Builds a default-Config Runtime (registers
/// builtin classes, runs the Ruby preamble) and stashes it in
/// `WIZER_RUNTIME` for the eventual `main()` to consume.
///
/// Wizer's rules forbid imported function calls during init, so
/// we deliberately split out `new_default_impl()` (no env reads,
/// no PID lookup, no stdout binding) as the wizer-able subset.
/// Config-driven settings — fuel caps, env override, deadline —
/// are applied later by `main()` via `apply_config()`, AFTER the
/// snapshot.
///
/// If Wizer is never invoked, the function is dead code; the
/// `#[export_name]` attribute keeps it linked anyway so the
/// build artifact is wizer-ready as-shipped.
#[cfg(target_os = "wasi")]
#[unsafe(export_name = "wizer.initialize")]
pub extern "C" fn wizer_initialize() {
    // SAFETY: wasm32-wasip1 is single-threaded; this static is
    // only mutated here (during the one wizer-init pass) and
    // consumed once by `take_wizer_runtime()` from main(). Use
    // `addr_of_mut!` + `(*p).replace(...)` to avoid the Rust
    // 2024 `static_mut_refs` lint AND ensure any prior occupant
    // is dropped (a bare `p.write(...)` would leak the previous
    // Runtime if `wizer.initialize` were somehow invoked more
    // than once during a debugging / embedding cycle).
    unsafe {
        let p = std::ptr::addr_of_mut!(WIZER_RUNTIME);
        let _prev = (*p).replace(Runtime::new_default_impl());
        // `_prev` (Option<Runtime>) drops here, freeing the
        // earlier Runtime's heap + Vm slots if one existed.
    }
}

/// Consume the wizer-built Runtime if one is present, returning
/// `None` when the binary wasn't put through wizer. The CLI
/// binary's `main()` calls this, then applies its own Config
/// (env vars, PID, fuel) on top of the rehydrated state.
#[cfg(target_os = "wasi")]
pub fn take_wizer_runtime() -> Option<Runtime> {
    // SAFETY: same single-threaded justification as above; this
    // is the unique consumer site. `addr_of_mut!` for the same
    // 2024-edition reason as `wizer_initialize`.
    unsafe {
        let p = std::ptr::addr_of_mut!(WIZER_RUNTIME);
        (*p).take()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    pub fn with_config(cfg: Config) -> Self {
        // Apply Config BEFORE loading the preamble so user-supplied
        // resource caps (fuel, max_frames, max_heap_objects, etc.)
        // are in effect during preamble eval — and `apply_config`'s
        // contract ("call before any eval()") is honored. The wizer
        // path can't do this: it has no host Config at snapshot time,
        // so its preamble runs under defaults — that's by design and
        // documented on `new_default_impl`.
        let mut rt = Self::build_skeleton();
        rt.apply_config(cfg);
        rt.load_preamble();
        rt
    }

    /// Construct a Runtime skeleton (fresh Vm, no preamble) shared
    /// by `with_config` and the wizer pre-initialize path. Doing it
    /// this way lets the non-wizer constructor apply Config before
    /// the preamble runs, while the wizer path runs the preamble
    /// under defaults so the snapshot is host-Config-independent.
    fn build_skeleton() -> Self {
        // PR #60 review #14: clear any leftover per-thread cext
        // STATE before constructing a fresh Vm. The persistent
        // CExtState (L3-H) lives in a thread_local; without this
        // reset, a previous Runtime's `values` table — which can
        // hold `CValue::HeapRef(ObjId)` referencing the OLD Vm's
        // heap — would dangle into the new Vm and resolve to
        // unrelated objects (or panic on out-of-range ObjId).
        #[cfg(all(feature = "cext", not(target_os = "wasi")))]
        rubyrs_cext::reset_state();

        let interner = intern::Interner::new();
        let vm = vm::Vm::new(vec![], interner);
        Runtime { vm, deadline: None }
    }

    /// Wizer-able default Runtime: skeleton + preamble, no host
    /// Config applied. The wizer path stashes this in
    /// `WIZER_RUNTIME`; `main()` later calls `apply_config()` on the
    /// rehydrated instance to layer host-driven settings (env, PID,
    /// fuel caps) on top. The preamble runs under default caps here
    /// — fuel/stress_gc/etc. do NOT apply to preamble execution in
    /// the wizer path (they can't: wizer's no-imports rule forbids
    /// reading host env at snapshot time).
    #[cfg(target_os = "wasi")]
    fn new_default_impl() -> Self {
        let mut rt = Self::build_skeleton();
        rt.load_preamble();
        rt
    }

    /// Apply a Config to an already-constructed Runtime. Used by the
    /// wizer-init path so the snapshotted Runtime (built with default
    /// Config) can pick up host-driven settings (env vars, PID, fuel
    /// caps) at runtime AFTER wizer rehydrates it.
    ///
    /// Contract: call this once, BEFORE any `eval()` or `ENV` access.
    /// Later calls fully overwrite resource caps, PID, deadline, and
    /// `stress_gc`. However, `cfg.env` is consumed lazily on the
    /// first `ENV` materialization inside the Vm (via
    /// `env_override.take()`); once `ENV` has been touched by Ruby
    /// code, a subsequent `apply_config` cannot retroactively change
    /// the script-visible env. The CLI binary always calls this
    /// exactly once at startup, so the caveat only matters for
    /// embedders.
    pub fn apply_config(&mut self, cfg: Config) {
        // Fully overwrite (not OR-merge) so a later call with
        // `stress_gc: false` actually clears a previously-set true.
        // Previously this was `if cfg.stress_gc { ... = true; }`,
        // which made the flag monotonic and broke the documented
        // overwrite semantics.
        self.vm.stress_gc = cfg.stress_gc;
        self.vm.fuel = cfg.fuel;
        self.vm.max_frames = cfg.max_frames;
        self.vm.heap.max_live = cfg.max_heap_objects;
        self.vm.max_symbols = cfg.max_symbols;
        self.vm.max_value_bytes = cfg.max_value_bytes;
        self.vm.env_override = cfg.env;
        self.vm.pid = cfg.pid.map(|n| n.get() as i64);
        self.vm.time_now = cfg.time_now;
        self.deadline = cfg.deadline;
    }

    /// Bootstrap the built-in Ruby class hierarchy (currently just
    /// exceptions) by `eval`-ing a small Ruby preamble. Done with the
    /// runtime's own machinery so the resulting classes look identical
    /// to user-defined ones (no special-cased C structs).
    fn load_preamble(&mut self) {
        // Exception hierarchy first — other preamble fragments below
        // (and any user code that raises during their load) need
        // `RuntimeError`/`StandardError`/etc. to be resolvable.
        // Lives in its own file per the random.rb / time.rb pattern:
        // larger, structurally distinct from the class-stub block
        // below, and meaty enough that editor support pays off.
        self.eval(
            include_str!("preamble/exceptions.rb"),
            "<rubyrs:preamble:exceptions>",
        )
            .expect("ICE: failed to load exception preamble");
        // Object — universal ancestor stub. Loaded between
        // exceptions and the remaining preamble (which contains
        // `class X < Object` shapes) so the constant resolves at
        // class-definition time.
        self.eval(
            include_str!("preamble/object.rb"),
            "<rubyrs:preamble:object>",
        )
            .expect("ICE: failed to load Object preamble");
        const PREAMBLE: &str = r#"
## Stub classes for built-in types. Without these, `5.class` and
## friends have nothing to return; the bodies stay empty because
## built-in method dispatch goes through `primitive_call` /
## `collection_call` before any class-table lookup. Re-opening
## these from user code does work (`class Integer; def foo; end`)
## but adding methods that way won't shadow the primitive arms —
## see docs/SUBSET.md.
## `class Object; end` is loaded from `preamble/object.rb` BEFORE
## this `PREAMBLE` eval, so subsequent `class Foo < Object` shapes
## here resolve without ordering hazards.
class Integer
end
class Float
end
class String
end
class Symbol
end
class Array
end
class Hash
end
class Range
end
class TrueClass
end
class FalseClass
end
class NilClass
end
class Proc
end
## Method — `Object#method(:foo)` returns a BoundMethod value
## whose class reports as Method. Stub class so `m.class.name`
## resolves to "Method".
class Method
end
## UnboundMethod — `Method#unbind` returns this; `bind(obj)`
## rehydrates it into a Method.
class UnboundMethod
end
## Module — empty preamble shell so `is_a?(Module)` /
## `class_of` reach a real class table entry. CRuby's
## hierarchy: `Class < Module < Object`; rubyrs mirrors
## the inheritance so `Class.is_a?(Module)` walks
## superclass → Module → true via the existing
## `class_is_a` helper. `module` keyword sets
## `is_module: true` on the Class shell (`Op::DefModule`).
module Module
end
class Class < Module
end
## File — class-method dispatch is wired host-side in
## `Vm::file_class_dispatch`. The class body is intentionally
## empty; methods are not defined here.
class File
end
## Mutex — rubyrs is single-threaded, so the entire lock surface
## degenerates to "run the block / no-op". Real codebases use
## `LOCK = Mutex.new` + `LOCK.synchronize { ... }` to wrap
## compilation caches (tilt, sinatra, dry-struct all do this);
## with one thread the critical section is already exclusive.
## `try_lock` returns true (the lock is always available);
## `locked?` returns false (we never actually hold one).
## Re-entrant `synchronize` "just works" because there's no
## real lock state to deadlock against.
class Mutex
  # CRuby's Mutex.new takes zero args; defining an explicit
  # 0-arity initialize delegates arity-checking to the existing
  # method-call machinery so `Mutex.new(1)` raises ArgumentError
  # instead of silently dropping the arg.
  def initialize
  end
  # `synchronize` requires a block — CRuby raises ThreadError on
  # bare call; we raise RuntimeError ("no block given (yield)")
  # via the bare yield. Different exception class, same fail-loud
  # semantics; no realistic code depends on the class name here.
  def synchronize
    yield
  end
  def lock
    self
  end
  def unlock
    self
  end
  def try_lock
    true
  end
  def locked?
    false
  end
  def owned?
    false
  end
end
## Kernel — sentinel class (CRuby's Kernel is a Module included in
## Object). We don't model Modules, but real codebases use
## `Kernel.instance_method(:class).bind(scope).call` for a stable
## handle to a method that gets invoked many times later (tilt
## does this at template.rb:238 to cache `:class` once at boot).
## `is_primitive_class_name` lists Kernel so
## `Class#instance_method` synthesises an UnboundMethod, and
## UnboundMethod#bind skips its is-a check when the captured class
## is Kernel — every value is_a Kernel in CRuby semantics, so the
## downstream `do_call` routes the method name to the receiver's
## normal method dispatch as if called directly.
##
## DIVERGENCE: in CRuby, an UnboundMethod captured from Kernel
## bypasses receiver-overridden methods (so `bind(liar).call`
## returns the real `#class` even if `liar` defines its own).
## We route through normal do_call — the override wins. Tilt's
## scope objects don't override `#class`, so the practical impact
## is metaprogramming-only; would need a "skip user methods"
## flag on Kernel-derived BoundMethods to fix.
class Kernel
end
## Encoding — minimal stub for codebases that use the encoding
## API (ERB's compiler does at lib/erb/compiler.rb:317 / :461
## to detect the source encoding from a magic comment). rubyrs
## stores raw bytes with no per-string encoding tag, so every
## String reports as UTF-8 and every encoding is treated as
## ASCII-compatible / non-dummy. `Encoding.find(name)` returns
## the predefined constant for the standard names (singleton
## identity stable across calls) and raises ArgumentError for
## unknown names. Identity guarantee: `s.encoding ==
## Encoding.find("UTF-8")` works because the find returns the
## same `Encoding::UTF_8` instance the dispatch.rs intercept
## reads.
##
## Predefined constants cover the names ERB / cgi/util / similar
## stdlib-shaped consumers reach for; add more as real targets
## need them. `Encoding::BINARY` is the canonical CRuby alias
## for `ASCII_8BIT`.
class Encoding
  ## `Encoding.find(name)` returns the singleton instance for each
  ## of the four predefined encoding names. Identity is stable for
  ## the standard names — `Encoding.find("UTF-8").equal?(Encoding::UTF_8)`
  ## — because we return the same constant on every call. There is
  ## NO Hash cache; a case-over-name dispatch hits the four named
  ## constants directly.
  ##
  ## Why no Hash cache: `Runtime::with_config` applies `Config`
  ## (including `max_value_bytes`) BEFORE `load_preamble` runs, so
  ## any Hash mutation inside the preamble would count against tiny
  ## caps used in resource-limit tests and fail preamble load
  ## entirely.
  ##
  ## Unknown names raise `ArgumentError`, matching CRuby's
  ## `Encoding.find("missing")` shape (and avoiding the equality-
  ## breaking trap of returning two different `.new` instances for
  ## the same name).
  def self.find(name)
    # Case-insensitive only — match CRuby's actual behaviour.
    # ERB and similar consumers feed values from magic-comment
    # regex captures ("utf-8", "UTF-8", ...); without
    # normalization, lowercase magic comments would surprise
    # users with ArgumentError. CRuby does NOT fold '_' → '-'
    # (it rejects "UTF_8") and does NOT accept "UTF8" (the
    # un-hyphenated form), but does fold "ASCII" → US-ASCII
    # and "BINARY" → ASCII-8BIT — verified empirically vs
    # CRuby 3.4.
    case name.to_s.upcase
    when "UTF-8" then UTF_8
    when "US-ASCII", "ASCII" then US_ASCII
    when "ASCII-8BIT", "BINARY" then ASCII_8BIT
    else raise ArgumentError, "unknown encoding name - " + name.to_s
    end
  end

  def initialize(name)
    @name = name
  end

  def name
    @name
  end

  def to_s
    @name
  end

  def inspect
    # Built with concat — using the quote-then-hash sequence
    # inline would close the outer raw-string delimiter at
    # Rust parse time.
    '#<Encoding:' + @name + '>'
  end

  ## Always false in our subset — we don't model dummy encodings
  ## (the ones CRuby uses for, e.g., UTF-16 where you must know
  ## the byte order). Real codebases gate ASCII-safety checks on
  ## this; returning false keeps the happy path.
  def dummy?
    false
  end

  ## Always true — UTF-8, US-ASCII, and ASCII-8BIT (the only
  ## names we serve up) are all ASCII-compatible.
  def ascii_compatible?
    true
  end
end
Encoding::UTF_8 = Encoding.new("UTF-8")
Encoding::US_ASCII = Encoding.new("US-ASCII")
Encoding::ASCII_8BIT = Encoding.new("ASCII-8BIT")
Encoding::BINARY = Encoding::ASCII_8BIT

## Version sentinels. Real codebases use `RUBY_VERSION >= '3'`
## (tilt does at template.rb:239) to pick between bind_call and
## bind.call paths. We claim a recent CRuby version to opt into
## the modern branches. RUBY_PLATFORM identifies the host
## interpreter — "rubyrs" makes it obvious in any platform-
## conditional code that this isn't CRuby. RUBY_ENGINE follows
## CRuby's convention — "ruby" for MRI; engine-specific gems
## (msgpack's Factory::Pool, sidekiq, etc.) gate behaviour on
## `RUBY_ENGINE == "ruby"`, and reporting the canonical value
## opts into those branches. The truthful "rubyrs" engine tag
## lives in RUBY_PLATFORM for the rare consumer that wants to
## detect us specifically.
RUBY_VERSION = "3.4.0".freeze
RUBY_PLATFORM = "rubyrs".freeze
RUBY_ENGINE = "ruby".freeze
## Comparable — a stub class (we don't have Modules in this subset)
## that holds the six derived comparison methods plus `between?`
## and `clamp`, each defined in terms of `<=>`. `include Comparable`
## copies these into the target class's method table (see
## `do_call`'s include-intercept). User-defined methods on the
## including class take precedence — the copy is non-destructive.
##
## On `<=>` returning nil (incomparable pair), the four ordered
## predicates raise ArgumentError, matching CRuby. `==` returns
## `false` instead of raising — CRuby's documented exception to
## the rule (Object equality must never raise).
## MatchData — the value returned by `String#match(regex)`. Wraps
## the whole match + numbered captures. CRuby's MatchData has a
## lot of API surface (`pre_match`, `post_match`, `named_captures`,
## `regexp`); we expose only `[]`, `captures`, `to_a`, `size`,
## `to_s`, and `inspect`. Stored as a regular user-class so the
## existing instance-method dispatch carries the load.
class MatchData
  def initialize(whole, caps)
    @whole = whole
    @caps  = caps
  end
  def [](i)
    if i == 0
      @whole
    else
      @caps[i - 1]
    end
  end
  def captures
    @caps
  end
  def to_a
    [@whole] + @caps
  end
  def size
    @caps.length + 1
  end
  def length
    size
  end
  def to_s
    @whole
  end
  def inspect
    # Plain concatenation — kept simple to avoid quote/hash
    # sequences that conflict with the surrounding Rust raw
    # string delimiter.
    "<MatchData " + @whole + ">"
  end
end
class Comparable
  def <(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c < 0
  end
  def <=(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c <= 0
  end
  def >(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c > 0
  end
  def >=(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c >= 0
  end
  def ==(other)
    c = self <=> other
    return false if c.nil?
    c == 0
  end
  def between?(lo, hi)
    self >= lo && self <= hi
  end
  def clamp(*args)
    ## Range form (one arg): `clamp(lo..hi)`. Endpoints may be
    ## nil for one-sided ranges (`(..max)` / `(min..)`); a nil
    ## bound is treated as "no limit on that side", matching
    ## CRuby.
    if args.length == 1 && args[0].is_a?(Range)
      r = args[0]
      lo, hi = r.begin, r.end
      if !lo.nil? && self < lo
        lo
      elsif !hi.nil? && self > hi
        hi
      else
        self
      end
    elsif args.length == 2
      lo, hi = args[0], args[1]
      if !lo.nil? && self < lo
        lo
      elsif !hi.nil? && self > hi
        hi
      else
        self
      end
    else
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 1..2)"
    end
  end
end
## Enumerable — stub class (we don't have real Modules in this
## subset). CRuby's Enumerable defines ~50 methods (each_with_index,
## map, select, reject, inject, sort, to_a, ...) all in terms of
## a host class's `#each`. For built-in collections (Array/Hash/
## Range), iteration methods are wired in `vm/iter.rs`'s block-
## dispatch paths, not via Enumerable include; for user classes
## the host provides `def each` directly. Either way, the
## Enumerable-derived methods aren't automatically gained through
## an empty stub.
##
## Why keep the stub anyway: `class Foo; include Enumerable; def
## each; ...; end; end` (commonly executed while loading a class
## body, but also supported at arbitrary runtime points and via
## the explicit `Foo.include(Enumerable)` form) pushes Enumerable
## onto Foo's `includes` chain (vm/dispatch.rs's include arm;
## lookup walks the chain at method-dispatch time, no copy).
## Empty Enumerable adds nothing to dispatch but doesn't crash.
## Before this stub, `include Enumerable` raised "wrong argument
## type NilClass (expected Module)" and the file failed to load.
## Affected: rake/linked_list.rb at minimum (Plan A try-run
## target), plus any other codebase that does the same
## `include Enumerable + def each` pattern. Methods like `.map`
## on a user `LinkedList` instance still NoMethodError at call
## time — documented divergence, follow-up PR.
class Enumerable
end
"#;
        self.eval(PREAMBLE, "<rubyrs:preamble>")
            .expect("ICE: failed to load built-in exception preamble");
        // Tier 1 seeded `Random` class. Lives in its own file so
        // the preamble stays focused on exception-hierarchy +
        // class-shell shapes; PRNG logic is meaty enough that
        // inlining it as a const string buries the algorithm.
        // ADR 0017 row 131 puts the seeded mode in Tier 1, so
        // this loads unconditionally — not gated behind
        // `--features stdlib`.
        self.eval(
            include_str!("preamble/random.rb"),
            "<rubyrs:preamble:random>",
        )
            .expect("ICE: failed to load Random preamble");
        // Tier 1 SecureRandom shim — wraps the Random class
        // above. Same Tier 1 placement rationale as Random;
        // the cryptographic guarantee is traded for determinism
        // (ADR 0017 row 131). Loaded after Random so the
        // module's `Random.new(0)` default initialisation can
        // resolve the constant.
        self.eval(
            include_str!("preamble/securerandom.rb"),
            "<rubyrs:preamble:securerandom>",
        )
            .expect("ICE: failed to load SecureRandom preamble");
        // Tier 1 `Time` class — capability-injected via
        // `Config::time_now` / the `__time_now_raw` Kernel
        // primitive. Pure Ruby per the Path A decision documented
        // in `perf/time_microbench_results.md`. Loaded
        // unconditionally; default no-injection makes `Time.now`
        // raise (ADR 0017 Rule 1 deterministic-default).
        self.eval(
            include_str!("preamble/time.rb"),
            "<rubyrs:preamble:time>",
        )
            .expect("ICE: failed to load Time preamble");
    }

    /// Replace the runtime's stdout sink.
    ///
    /// **Per ADR 0017 the default sink is `std::io::sink()`**, not
    /// process stdout — `Runtime::new()` is silent until the host
    /// calls this method to wire up where script output should go.
    /// The CLI binary `rubyrs` (in `crates/rubyrs/src/main.rs`) wires
    /// it to `std::io::stdout()`, which is why `rubyrs script.rb`
    /// behaves like CRuby; library embedders choose their own sink
    /// (`Vec<u8>` buffer, `tempfile::NamedTempFile`, a process pipe,
    /// `std::io::sink()` itself when output should be discarded).
    pub fn set_stdout(&mut self, w: Box<dyn Write>) {
        self.vm.stdout = w;
    }

    /// Register a host function callable from Ruby code with `name(args)`.
    /// The function receives evaluated argument values and returns either
    /// a `Value` or a `Trap`.
    ///
    /// Calling `register_fn` (or [`Runtime::register_fn_v2`]) with the
    /// same name replaces any previous registration — v1 and v2 share
    /// a single slot per name. Class- or instance-attached methods
    /// installed by a C extension live in independent dispatch tables
    /// and are NOT affected by this call.
    pub fn register_fn<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&[Value]) -> Result<Value, Trap> + 'static,
    {
        let id = self.vm.interner.intern(name);
        self.vm.host_fns.insert(id, vm::HostFnSlot::V1(Rc::new(f)));
    }

    /// Variant of [`Runtime::register_fn`] whose closure also receives a
    /// [`HostCtx`] handle for reading heap-y arguments.
    ///
    /// Use this when the Ruby caller passes an `Array` or `Hash` shape
    /// that the closure needs to inspect — `Value::Array` and
    /// `Value::Hash` are opaque heap handles, so the v1 `&[Value]`-only
    /// signature can't reach their contents from inside the closure.
    /// `HostCtx::resolve_array` / `resolve_hash` borrow directly from
    /// the heap, no clone.
    ///
    /// The ctx is read-only by design (see the [`HostCtx`] doc for the
    /// soundness argument). Re-entrant eval needs the cext path, not
    /// this one.
    ///
    /// Replaces any previous v1 or v2 registration under the same name
    /// (same slot as [`Runtime::register_fn`]). Class- or instance-
    /// attached methods installed by a C extension live in independent
    /// dispatch tables and are NOT affected by this call.
    pub fn register_fn_v2<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&HostCtx, &[Value]) -> Result<Value, Trap> + 'static,
    {
        let id = self.vm.interner.intern(name);
        self.vm.host_fns.insert(id, vm::HostFnSlot::V2(Rc::new(f)));
    }

    /// Parse, compile, and run a Ruby source. The returned value is the
    /// final expression of the script; embedders can ignore it for
    /// statements with no return value.
    pub fn eval(&mut self, source: &str, filename: &str) -> Result<Value, Trap> {
        let filename_rc: Rc<str> = Rc::from(filename);
        let source_rc: Rc<str> = Rc::from(source);
        // Single source-of-truth map on the Vm. Backtrace
        // formatting (`Runtime::line_for`) and dispatch-time
        // helpers (`Method#source_location`) both consult it.
        // Kernel builtins that compile new Ruby at runtime —
        // currently `require_relative` — insert here too,
        // so traps and source_location stay accurate for code
        // loaded outside the top-level `eval` path.
        self.vm.sources.insert(filename_rc.clone(), source_rc);

        let parse_result = ruby_prism::parse(source.as_bytes());
        let mut errors_iter = parse_result.errors().peekable();
        if errors_iter.peek().is_some() {
            let msg = error::format_prism_errors(source, errors_iter);
            return Err(Trap {
                err: RubyError::SyntaxError { msg },
                backtrace: vec![],
            });
        }
        let (prog, ast_errors) = ast::tr_with_errors_on_source(
            &parse_result.node(),
            parse_result.source(),
        );
        if !ast_errors.is_empty() {
            // AST translation hit one or more Prism nodes the
            // language subset doesn't cover. Surface as a
            // SyntaxError so embedders see a Trap they can format
            // and report, rather than a host-side panic.
            return Err(Trap {
                err: RubyError::SyntaxError { msg: ast_errors.join("; ") },
                backtrace: vec![],
            });
        }
        let entry = compiler::compile_proto(
            "<main>".into(), vec![], &[prog], filename_rc,
            &mut self.vm.protos, &mut self.vm.interner, &mut self.vm.cache_counter,
        );
        let cache_count = self.vm.cache_counter as usize;
        self.vm.ensure_call_caches(cache_count);
        // A previous `eval` on this Runtime may have left frames,
        // operand-stack residue, or pins behind if it ended in a
        // Trap (uncaught exception, fuel exhaustion, deadline hit).
        // Class definitions and the heap legitimately persist
        // across calls — that's part of the embedding contract.
        // The dispatch state shouldn't. Clear it now so the new
        // eval starts from a known baseline.
        self.vm.frames.clear();
        self.vm.stack.clear();
        self.vm.pinned.clear();
        self.vm.break_signaled = false;
        self.vm.method_return = None;
        self.vm.pending_loop_transfer = None;
        // Anchor the wall-clock deadline (P2-14a) to *this* eval
        // call. Each `eval` re-computes the absolute Instant from
        // the runtime's stored Duration, so a host can reuse a
        // Runtime across many short evaluations without inheriting
        // a stale timer. `None` (unlimited) is the default.
        if let Some(d) = self.deadline {
            self.vm.deadline_at = Some(std::time::Instant::now() + d);
            self.vm.op_counter = 0;
        }
        // PinGuard balance check: pinned was just zeroed; after
        // run, it must still be zero. The assert is debug-only —
        // release builds skip so a regression won't crash a host.
        let pinned_before = self.vm.pinned.len();
        let result = self.vm.run(entry);
        // Clear the deadline after eval so subsequent calls don't
        // inherit a (now-stale) Instant.
        self.vm.deadline_at = None;
        debug_assert_eq!(
            self.vm.pinned.len(), pinned_before,
            "PinGuard imbalance: pinned was {}, now {} after eval",
            pinned_before, self.vm.pinned.len(),
        );
        result
    }

    /// Number of distinct symbols currently interned by this
    /// runtime. Hosts can use this to size `Config::max_symbols`
    /// relative to the baseline established by the preamble + any
    /// prior `eval` calls.
    pub fn symbol_count(&self) -> usize {
        self.vm.interner.len()
    }

    pub fn eval_file(&mut self, path: &Path) -> Result<Value, Trap> {
        let source = std::fs::read_to_string(path).map_err(|e| Trap {
            err: RubyError::SyntaxError {
                msg: format!("cannot read {}: {}", path.display(), e),
            },
            backtrace: vec![],
        })?;
        let filename = path.to_string_lossy().into_owned();
        self.eval(&source, &filename)
    }

    /// Format a [`Trap`] CRuby-style:
    /// `file:line:in 'method': msg (Class)`, with one `\tfrom ...` line
    /// per remaining backtrace frame.
    ///
    /// Uses the source texts retained from prior `eval` calls to resolve
    /// byte offsets into line numbers.
    pub fn format_trap(&self, trap: &Trap) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let frames = &trap.backtrace;
        // For an uncaught Ruby exception we surface the script's
        // exception class name (e.g. `MyError`), not the host-side
        // "Uncaught" tag. Matches CRuby's
        // `foo.rb:1: msg (MyError)` style.
        let cls: String = match &trap.err {
            RubyError::Uncaught { class_name, .. } => class_name.clone(),
            other => other.class_name().to_string(),
        };
        let msg = trap.err.message();
        if let Some(top) = frames.first() {
            let line = self.line_for(&top.filename, top.span.byte_offset);
            let _ = writeln!(out, "{}:{}:in `{}': {} ({})", top.filename, line, top.method, msg, cls);
            for f in frames.iter().skip(1) {
                let line = self.line_for(&f.filename, f.span.byte_offset);
                let _ = writeln!(out, "\tfrom {}:{}:in `{}'", f.filename, line, f.method);
            }
        } else {
            let _ = writeln!(out, "rubyrs: {} ({})", msg, cls);
        }
        out
    }

    fn line_for(&self, filename: &str, byte_offset: u32) -> u32 {
        match self.vm.sources.get(filename) {
            Some(src) => error::line_col(src, byte_offset).0,
            None => 0,
        }
    }

    /// Resolve a `SymId` back to its string representation.
    pub fn resolve_sym(&self, sym: SymId) -> &str {
        self.vm.interner.resolve(sym)
    }

    /// Unpack a `Value::Array` into a Rust `Vec<Value>` by cloning elements.
    /// Returns `None` if the value is not an Array.
    pub fn resolve_array(&self, val: &Value) -> Option<Vec<Value>> {
        if let Value::Array(id) = val {
            Some(self.vm.heap.array(*id).clone())
        } else {
            None
        }
    }

    /// Unpack a `Value::Hash` into a Rust `Vec<(Value, Value)>` by cloning.
    /// Returns `None` if the value is not a Hash.
    pub fn resolve_hash(&self, val: &Value) -> Option<Vec<(Value, Value)>> {
        if let Value::Hash(id) = val {
            Some(self.vm.heap.hash(*id).clone())
        } else {
            None
        }
    }
}

impl Default for Runtime {
    fn default() -> Self { Self::new() }
}

impl Drop for Runtime {
    /// PR #60 review #14: defense-in-depth reset of the per-thread
    /// cext STATE on Vm teardown. Pairs with the reset in
    /// `with_config` — together they ensure any cext call that
    /// happens between `drop(this_rt)` and `new(next_rt)` sees a
    /// clean STATE rather than this Vm's stale handles (calling
    /// cext fns without an active Runtime is a host-side bug
    /// anyway, but this keeps the failure mode predictable).
    fn drop(&mut self) {
        #[cfg(all(feature = "cext", not(target_os = "wasi")))]
        rubyrs_cext::reset_state();
    }
}
