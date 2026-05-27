//! C-ext link keep-alive — `#[used]` static references that
//! prevent linker DCE from dropping cext ABI symbols that have
//! no Rust call site (only dlopen-loaded C extensions reach
//! them via dlsym).
//!
//! C-ext compat (spike Level 0): the `rb_*` functions from `rubyrs-cext`
//! are only ever called from dlopen'd C extensions — there is no Rust
//! call site for the linker to see. Without this `#[used]` static
//! holding raw function pointers, dead-code elimination drops them from
//! the rubyrs binary and `dlsym` from the bundle returns NULL.
//!
//! `Qnil` / `Qtrue` / `Qfalse` get their own `#[used]` on the statics
//! themselves over in `rubyrs-cext`; functions need this indirection
//! because `#[used]` only applies to statics, not function definitions.
//! Function pointers are `Sync`; using strongly-typed statics avoids
//! the `fn as usize` cast which isn't allowed in const context.
//!
//! All of this is wrapped in a single `cfg(feature = "cext")` module
//! so the entire block disappears (deps, symbols, link-keep-alive
//! trick) when cext is opt-out. The 162 statics + the L3-D bulk
//! exports module move inside `_cext_link_keep_alive` unchanged;
//! `#[used]` still applies through the module boundary because the
//! linker sees mangled symbols flat, not Rust modules. See ADR 0015
//! (concentric architecture) for why `cext` is the first tier
//! boundary made opt-in.

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
