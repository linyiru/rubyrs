// Per-function `# Safety` docs are intentionally omitted across
// the cext FFI surface — the safety contract is consolidated in
// `docs/CEXT_SAFETY.md` and discussed at the policy level in ADR
// 0009 (cext panic policy). Repeating the same contract on every
// `unsafe extern "C" fn` would 100×-duplicate text without adding
// information; consumers of this crate are C extensions calling
// through the documented ABI, not Rust callers reading rustdoc.
#![allow(clippy::missing_safety_doc)]

//! rubyrs-cext — opaque C ABI for hosting CRuby-shape C extensions.
//!
//! # Level 0 spike scope
//!
//! Implements the smallest surface needed to compile and run a
//! hello-world C extension against rubyrs:
//!
//! - `VALUE` = `u64` opaque handle (Option A from the spike plan).
//!   The C side never inspects bits; all access goes through the
//!   exported functions below.
//! - `Qnil` / `Qtrue` / `Qfalse` as fixed handles (0, 1, 2).
//! - `rb_str_new_cstr` to materialise a String value.
//! - `rb_define_global_function` to register a callback that the
//!   host Vm will later dispatch from Ruby code.
//!
//! # State plumbing
//!
//! C-side code talks to a thread-local [`CExtState`]. The host Vm
//! is responsible for pushing fresh state with [`enter`] before
//! handing control to a C ext (either during `Init_<name>` or
//! during a Ruby-driven call into a registered function) and
//! pulling the resulting state back out with [`leave`].
//!
//! This is deliberately simple — a real implementation would
//! integrate handle lifetime with the GC, scope handles per call,
//! and decide whether to adopt CRuby's tagged-pointer VALUE layout
//! to support macros like `FIXNUM_P`. None of that is on the
//! critical path for the Level 0 hypothesis we're testing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong};

// Spike L3-A: rb_raise / longjmp protection. See raise.rs and
// c/setjmp_shim.c. Gated off wasi (no usable setjmp emulation).
#[cfg(not(target_os = "wasi"))]
pub mod raise;
pub mod stubs;

/// Opaque token the C side sees as `VALUE`. Numerically an index
/// into [`CExtState::values`]; semantically meaningless to C code.
pub type Value = u64;

/// CRuby's `ID` type — opaque identifier for an interned name
/// (method, symbol, class name, etc.). Returned by `rb_intern`;
/// consumed by `rb_funcall`, `rb_funcallv`, `rb_define_method`'s
/// future variants, etc. Stable across per-call `CExtState`
/// lifecycles — that's why it lives in its own intern table,
/// separate from `CExtState`'s ephemeral handle table.
///
/// **Threading scope**: the table is `thread_local!`, not a true
/// process-wide global. Given rubyrs's current single-threaded
/// cext execution model (no Ractor parallelism reaches the C
/// boundary; embedders run one [`crate::Runtime`] per thread), a
/// thread-local table is effectively process-wide. If a future
/// level adds true threaded cext dispatch, this becomes a
/// `OnceLock<Mutex<InternTable>>` — at that point the locking
/// overhead is justified by the actual sharing requirement, and
/// not before.
///
/// `0` is reserved as "no ID" / `Qundef`-ish.
pub type ID = u64;

/// Intern table for [`ID`]s. C extensions call `rb_intern("name")`
/// and stash the result in static globals (`static ID id_foo;`);
/// those IDs must remain valid across every subsequent C ext call
/// regardless of which per-call `CExtState` is active. This table
/// is the only piece of cext state that outlives a single
/// `enter`/`leave` cycle — see [`ID`]'s threading-scope note for
/// why "thread-local" is the right shape here.
struct InternTable {
    /// 0-based; ID is index + 1 so we can reserve 0 as "no such ID".
    names: Vec<String>,
    map: HashMap<String, ID>,
}

impl InternTable {
    fn new() -> Self {
        Self { names: Vec::new(), map: HashMap::new() }
    }

    fn intern(&mut self, name: &str) -> ID {
        if let Some(&id) = self.map.get(name) {
            return id;
        }
        let id = self.names.len() as ID + 1;
        self.names.push(name.to_string());
        self.map.insert(name.to_string(), id);
        id
    }

    fn resolve(&self, id: ID) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.names.get((id - 1) as usize).map(String::as_str)
    }
}

thread_local! {
    // Not `const { ... }` because HashMap::new is not const-fn.
    static INTERN: RefCell<InternTable> = RefCell::new(InternTable::new());
}

/// Resolve a previously-interned [`ID`] back to its name. Used by
/// the host VM when `rb_funcallv` lands and needs to look up the
/// method by its symbolic name. Returns `None` for `ID(0)` or any
/// ID that wasn't issued by this process's [`rb_intern`] calls.
pub fn resolve_id(id: ID) -> Option<String> {
    INTERN.with(|t| t.borrow().resolve(id).map(String::from))
}

/// Mirror of the subset of `rubyrs::Value` that crosses the C ABI.
/// Kept independent so this crate doesn't pull in the whole
/// interpreter — the host translates between the two when draining
/// registered functions and when wrapping arguments.
///
/// `Str` stores **bytes with a sentinel trailing NUL** that is NOT
/// counted by [`RSTRING_LEN`]. This lets `StringValueCStr` /
/// `RSTRING_PTR` hand out a pointer that CRuby C extensions can
/// safely pass to `strlen`, `strcmp`, etc. — matching CRuby's own
/// "always one byte of capacity past the end is `\0`" guarantee.
#[derive(Clone, Debug)]
pub enum CValue {
    Nil,
    True,
    False,
    Str(Vec<u8>), // invariant: ends with `\0`; logical length is `.len() - 1`
    /// CRuby's "Fixnum" range — for the spike all integers are i64
    /// regardless of which `NUMxxx` macro the C ext used.
    Int(i64),
    /// A handle to a class or module by its (joined) name. Returned
    /// from `rb_define_module` / `rb_define_class_under`; consumed
    /// by `rb_define_singleton_method`.
    Class(String),
    /// An Array of handles. C extensions build these via `rb_ary_new`
    /// and `rb_ary_push`; the host's `cext_handle_to_value`
    /// translates recursively into a `Value::Array` on the Vm heap
    /// on return.
    Array(Vec<Value>),
    /// A Hash of (key handle, value handle) pairs, ordered (Ruby
    /// semantics since 1.9). Built via `rb_hash_new` + `rb_hash_aset`;
    /// translated to `Value::Hash` on the Vm heap on return.
    Hash(Vec<(Value, Value)>),
    /// Spike L3-B: an already-allocated Vm-heap Object reference.
    /// Produced by `rb_data_typed_object_wrap`'s callback (which
    /// allocates `HeapObj::TypedData` eagerly so the C ext can pass
    /// the handle into nested `rb_funcall` etc. mid-call without
    /// the cext_handle_to_value translator needing to alloc at
    /// return time). `inner` is the raw `ObjId.0` (kept as plain
    /// u32 because rubyrs-cext doesn't import the main crate's
    /// ObjId type — single source of truth lives on the host side).
    HeapRef(u32),
}

impl CValue {
    /// Construct a String CValue from raw bytes, appending the
    /// sentinel NUL.
    pub fn str_from_bytes(bytes: &[u8]) -> Self {
        let mut v = Vec::with_capacity(bytes.len() + 1);
        v.extend_from_slice(bytes);
        v.push(0);
        CValue::Str(v)
    }
}

/// Opaque function-pointer storage. C extensions register pointers
/// with any signature (CRuby's `ANYARGS` convention); the host
/// transmutes to the correct arity-specific type at dispatch time
/// using the recorded [`CFn::arity`]. We deliberately do NOT call
/// through this type — it exists purely to carry the address across
/// the FFI boundary in a way that's `Send` + `Sync`.
pub type OpaqueFn = unsafe extern "C" fn();

/// One callback registered by a C ext during `Init_<name>` (or, in
/// later levels, by `rb_define_method` from inside a running method).
pub struct CFn {
    pub name: String,
    pub func: OpaqueFn,
    /// CRuby-style arity. cext_dispatch in the host handles 0–5;
    /// other values register but trap at invocation.
    pub arity: i32,
}

/// A class/module the C ext declared via `rb_define_module` or
/// `rb_define_class_under`. Drained by the host into `Vm.classes`
/// under the joined name.
pub struct CExtClassReg {
    pub joined_name: String,
}

/// A singleton method the C ext attached to a previously-registered
/// class. Drained by the host into a per-class dispatch table
/// consulted when a `Value::Class` receiver is called with that
/// method name.
pub struct CExtSingletonMethod {
    pub class_joined_name: String,
    pub method_name: String,
    pub func: OpaqueFn,
    pub arity: i32,
}

/// Spike L3-C: an instance method the C ext attached to a
/// previously-registered class via `rb_define_method`. Drained by
/// the host into a per-class instance-method dispatch table
/// consulted when a `Value::Object` receiver's class matches.
/// Mirrors [`CExtSingletonMethod`] in shape; the dispatch site
/// differs (Object-recv vs Class-recv) but the registration data
/// is the same.
pub struct CExtInstanceMethod {
    pub class_joined_name: String,
    pub method_name: String,
    pub func: OpaqueFn,
    pub arity: i32,
}

/// L3-F: allocator function registered via `rb_define_alloc_func`.
/// Drained by the host into the target Class so that `Klass.new(args)`
/// on the Ruby side routes:
///
///   1. host calls `func(klass_handle)` — the cext-side allocator
///      typically returns a `TypedData_Wrap_Struct`-wrapped Object
///      whose data slot holds a freshly-malloc'd C struct.
///   2. host then calls `initialize(args...)` on that Object.
///
/// Without this path, msgpack-style instance-pattern gems can't
/// produce TypedData-wrapped receivers — `.new` falls back to a
/// bare `Instance` and `TypedData_Get_Struct(self, ...)` in any
/// instance method then fails the type-check.
pub struct CExtAllocFunc {
    pub class_joined_name: String,
    pub func: OpaqueFn,
}

/// Per-thread state shared between the host Vm and any active C ext
/// call. The host swaps this around every C-side entry point so a
/// fresh handle table is in scope.
pub struct CExtState {
    /// Indexed by handle. Indices `0`, `1`, `2`, `3` are pre-populated
    /// for [`Qnil`], [`Qtrue`], [`Qfalse`], [`rb_cObject`].
    pub values: Vec<CValue>,
    /// Accumulator for [`rb_define_global_function`] calls. The host
    /// drains this after `Init_<name>` returns.
    pub registered_fns: Vec<CFn>,
    /// Modules / classes declared during this Init pass.
    pub registered_classes: Vec<CExtClassReg>,
    /// Singleton methods declared during this Init pass. Each entry
    /// references its target class by joined name.
    pub registered_singletons: Vec<CExtSingletonMethod>,
    /// Spike L3-C: instance methods declared during this Init pass
    /// via `rb_define_method`. Same shape as singletons; the host
    /// drains into a parallel per-class table consulted for
    /// `Value::Object` receivers.
    pub registered_methods: Vec<CExtInstanceMethod>,
    /// L3-F: allocator functions declared via `rb_define_alloc_func`.
    /// Drained into per-class `cext_alloc_func` slot at the host so
    /// `Klass.new(args)` can route through the cext's wrapper.
    pub registered_alloc_funcs: Vec<CExtAllocFunc>,
}

impl CExtState {
    pub fn new() -> Self {
        // 64 = headroom for typical JSON-parse-style cext calls
        // (~50 handles allocated during build). Avoids 2-3
        // Vec::grow reallocations per cext_dispatch. The
        // 20 sentinels seeded below already consume 31% of the
        // initial capacity; `intern` grows from there if needed.
        let mut values = Vec::with_capacity(64);
        values.extend([
                CValue::Nil,                                 // 0  Qnil
                CValue::True,                                // 1  Qtrue
                CValue::False,                               // 2  Qfalse
                CValue::Class(String::from("Object")),       // 3  rb_cObject
                CValue::Class(String::from("String")),       // 4  rb_cString
                CValue::Class(String::from("Array")),        // 5  rb_cArray
                CValue::Class(String::from("Hash")),         // 6  rb_cHash
                CValue::Class(String::from("Float")),        // 7  rb_cFloat
                CValue::Class(String::from("Integer")),      // 8  rb_cInteger
                CValue::Class(String::from("Numeric")),      // 9  rb_cNumeric
                CValue::Class(String::from("Symbol")),       // 10 rb_cSymbol
                CValue::Class(String::from("TrueClass")),    // 11 rb_cTrueClass
                CValue::Class(String::from("FalseClass")),   // 12 rb_cFalseClass
                CValue::Class(String::from("NilClass")),     // 13 rb_cNilClass
                CValue::Class(String::from("BasicObject")),  // 14 rb_cBasicObject
                CValue::Class(String::from("Module")),       // 15 rb_cModule
                CValue::Class(String::from("Class")),        // 16 rb_cClass
                CValue::Class(String::from("Kernel")),       // 17 rb_mKernel
                CValue::Class(String::from("Comparable")),   // 18 rb_mComparable
                CValue::Class(String::from("Enumerable")),   // 19 rb_mEnumerable
            ]);
        Self {
            values,
            registered_fns: Vec::new(),
            registered_classes: Vec::new(),
            registered_singletons: Vec::new(),
            registered_methods: Vec::new(),
            registered_alloc_funcs: Vec::new(),
        }
    }

    /// Push a fresh value into the handle table and return its token.
    pub fn intern(&mut self, v: CValue) -> Value {
        let h = self.values.len() as u64;
        self.values.push(v);
        h
    }

    /// Resolve a handle back to its value. Out-of-range handles
    /// (forged values, stale handles that outlived their
    /// `CExtState`) resolve to `&CValue::Nil` — the read path is
    /// total. Callers like `RSTRING_LEN` / `rb_num2long` /
    /// `rb_ary_entry` already type-check the resolved CValue and
    /// fall through to a defined default (0 / Qnil / null pointer)
    /// for non-matching variants, so a forged handle yields a
    /// wrong-but-defined result instead of a host-process abort.
    ///
    /// This is the trusted-loader contract documented in
    /// `docs/CEXT_SAFETY.md`: handle integrity is the C ext's
    /// responsibility; forgery doesn't get UB.
    pub fn resolve(&self, h: Value) -> &CValue {
        // Static fallback shared across calls — `CValue::Nil` is a
        // unit variant, so `&NIL` is a stable read-only reference.
        static NIL: CValue = CValue::Nil;
        self.values.get(h as usize).unwrap_or(&NIL)
    }

    /// Mutable resolve, for in-place mutation of `CValue::Array` /
    /// `CValue::Hash` via `rb_ary_push` / `rb_hash_aset`. Unlike
    /// the read-path `resolve`, mutation through a forged handle
    /// can't be silently absorbed (where would the write land?) so
    /// the function still aborts on out-of-range. C ext authors
    /// hitting this should fix their handle plumbing.
    pub fn resolve_mut(&mut self, h: Value) -> &mut CValue {
        self.values
            .get_mut(h as usize)
            .expect("ICE: cext handle out of range on write path; C ext leaked a stale VALUE")
    }
}

impl Default for CExtState {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    // L3-H: persistent per-Vm CExtState (was: stack of per-call
    // states). The `values` table now lives across cext calls so
    // VALUE handles a cext stores internally (e.g., msgpack's
    // `Unpacker.feed(bytes)` saving a string reference into its
    // TypedData buffer) still resolve when a later cext call
    // (`Unpacker.read`) dereferences them. Pre-L3-H each enter
    // pushed a fresh state and leave dropped it — stored handles
    // dangled across the boundary.
    //
    // Nested cext calls (rb_funcallv → host method → another cext
    // call) share the same state. Both `registered_*` lists and
    // `values` are shared; this is correct because:
    //   - `registered_*` are only populated during `Init_<name>`
    //     (the outermost call); subsequent calls don't register
    //     anything, so nested calls see them already drained.
    //   - `values` IS meant to be shared — that's the whole point.
    static STATE: RefCell<Option<CExtState>> = const { RefCell::new(None) };
}

/// Run `f` with mutable access to the per-Vm [`CExtState`]. Panics
/// if called from a thread that has no active state — that always
/// indicates a host-side bug.
pub fn with_state<R>(f: impl FnOnce(&mut CExtState) -> R) -> R {
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        let st = b
            .as_mut()
            .expect("ICE: rubyrs-cext STATE empty; host must call enter() first");
        f(st)
    })
}

/// Initialize the per-Vm state on first call; on subsequent calls,
/// clear the `registered_*` lists so a new Init pass starts clean.
/// `values` is NOT cleared — handles persist across calls (L3-H).
pub fn enter() {
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        if b.is_none() {
            *b = Some(CExtState::new());
        }
        let st = b.as_mut().unwrap();
        st.registered_fns.clear();
        st.registered_classes.clear();
        st.registered_singletons.clear();
        st.registered_methods.clear();
        st.registered_alloc_funcs.clear();
    });
}

/// Return a snapshot of the current state for the host to drain
/// `registered_*` from and to resolve the return-value handle
/// against. The persistent state KEEPS its `values` so future cext
/// calls can still resolve handles a cext stored internally; the
/// returned struct gets a CLONE of `values` for the host's result-
/// translation phase (which uses `state.resolve(handle)` on its
/// owned copy).
///
/// `registered_*` are moved out of the persistent state (consumed
/// exactly once per `enter`/`leave` pair) — the host iterates them
/// to install classes / methods / alloc_funcs into the Vm.
pub fn leave() -> CExtState {
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        let st = b
            .as_mut()
            .expect("ICE: rubyrs-cext leave() without matching enter()");
        CExtState {
            values: st.values.clone(),
            registered_fns: std::mem::take(&mut st.registered_fns),
            registered_classes: std::mem::take(&mut st.registered_classes),
            registered_singletons: std::mem::take(&mut st.registered_singletons),
            registered_methods: std::mem::take(&mut st.registered_methods),
            registered_alloc_funcs: std::mem::take(&mut st.registered_alloc_funcs),
        }
    })
}

// ===== Funcall callback infrastructure (Level 2) =====
//
// `rb_funcallv` from C needs to re-enter the host Vm to dispatch a
// Ruby method. rubyrs-cext can't directly depend on the Vm type
// (separate crate), so we expose a callback channel: the host
// installs a closure that knows how to invoke `recv.method(args)`
// on its Vm before transferring control to the C function, and
// `rb_funcallv` looks up the topmost installed callback.
//
// Stack semantics mirror STATE — nested cext calls each install
// their own callback (capturing their own Vm pointer), and the
// topmost wins.

/// Callback signature: receiver handle (Value), method name (str),
/// arg handles (slice of Value). Returns a new handle for the
/// result, interned into the topmost CExtState.
pub type FuncallCallback = Box<dyn Fn(Value, &str, &[Value]) -> Value>;

thread_local! {
    static FUNCALL_CB: RefCell<Vec<FuncallCallback>> = const { RefCell::new(Vec::new()) };
}

/// Install a funcall callback for the duration of the next C call.
/// The host (`Vm::cext_dispatch`) pushes one before invoking C and
/// pops the same one after C returns.
pub fn push_funcall_callback(cb: FuncallCallback) {
    FUNCALL_CB.with(|c| c.borrow_mut().push(cb));
}

/// Remove the topmost funcall callback.
pub fn pop_funcall_callback() {
    FUNCALL_CB.with(|c| {
        let _ = c.borrow_mut()
            .pop()
            .expect("ICE: pop_funcall_callback without matching push");
    });
}

// === Spike L3-B: TypedData callbacks ===
//
// `rb_data_typed_object_wrap(klass, data, type)` and
// `rb_check_typeddata(obj, type)` both need to reach into the host
// Vm to allocate / look up `HeapObj::TypedData` slots. The cext
// crate can't depend on vm.rs, so vm::cext_dispatch installs
// closures here at call entry (mirroring FUNCALL_CB) that close
// over a `*mut Vm`.

/// Wrap callback. Inputs: klass handle, raw C data pointer, raw
/// type-descriptor pointer (identity-compared by `rb_check_typeddata`),
/// optional `dfree` fn pointer extracted from the rb_data_type_t.
/// Output: a handle interned into the topmost CExtState that
/// resolves at cext-return time to a Vm `Value::Object(typed_data_id)`.
pub type TypedDataWrapCallback = Box<dyn Fn(
    Value,
    *mut std::ffi::c_void,
    *const std::ffi::c_void,
    Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
) -> Value>;

/// Type-check callback. Inputs: the TypedData handle, the type
/// descriptor the C extension expects. Output: the raw data
/// pointer (CRuby semantics: pointer-identity check; mismatch
/// raises TypeError, but the spike collapses to a panic — TypeError
/// raise wiring is L3-B.1 follow-up).
pub type TypedDataCheckCallback = std::rc::Rc<dyn Fn(Value, *const std::ffi::c_void) -> *mut std::ffi::c_void>;

thread_local! {
    static TYPED_DATA_WRAP_CB: RefCell<Vec<TypedDataWrapCallback>> = const { RefCell::new(Vec::new()) };
    static TYPED_DATA_CHECK_CB: RefCell<Vec<TypedDataCheckCallback>> = const { RefCell::new(Vec::new()) };
}

pub fn push_typed_data_wrap_callback(cb: TypedDataWrapCallback) {
    TYPED_DATA_WRAP_CB.with(|c| c.borrow_mut().push(cb));
}
pub fn pop_typed_data_wrap_callback() {
    TYPED_DATA_WRAP_CB.with(|c| {
        let _ = c.borrow_mut().pop()
            .expect("ICE: pop_typed_data_wrap_callback without matching push");
    });
}
pub fn push_typed_data_check_callback(cb: TypedDataCheckCallback) {
    TYPED_DATA_CHECK_CB.with(|c| c.borrow_mut().push(cb));
}
pub fn pop_typed_data_check_callback() {
    TYPED_DATA_CHECK_CB.with(|c| {
        let _ = c.borrow_mut().pop()
            .expect("ICE: pop_typed_data_check_callback without matching push");
    });
}

// ===== Exported C ABI =====

/// Singleton handle for Ruby `nil`. Part of the ABI — `CExtState::new`
/// pre-populates index 0 with `CValue::Nil`.
// `#[used]` keeps these singleton statics in the final binary even
// though no Rust code references them — only dlopen'd C extensions do,
// and the linker can't see that.
#[used]
#[unsafe(no_mangle)]
pub static Qnil: Value = 0;

/// Singleton handle for Ruby `true`.
#[used]
#[unsafe(no_mangle)]
pub static Qtrue: Value = 1;

/// Singleton handle for Ruby `false`.
#[used]
#[unsafe(no_mangle)]
pub static Qfalse: Value = 2;

/// Sentinel handle for the `Object` class. Used as the third arg to
/// `rb_define_class_under(parent, name, rb_cObject)`. Pre-populated
/// at index 3 of every fresh [`CExtState`]; the rubyrs side ignores
/// superclass at spike scope.
#[used]
#[unsafe(no_mangle)]
pub static rb_cObject: Value = 3;

// Class / module sentinels 4-19. CRuby exposes each as
// `extern VALUE rb_c<Name>`; flori/json (and most non-trivial
// cexts) use them as comparison targets in fast type dispatch
// (`if (RBASIC_CLASS(obj) == rb_cString) {...}`). rubyrs's
// `rb_basic_class` maps each `CValue` variant to the matching
// sentinel — so `RBASIC_CLASS(rb_str_new(...))` returns
// `rb_cString` by pointer identity, matching the C ext's
// expectation.
//
// Numbers must match `CExtState::new`'s seeded `values` vec.
#[used] #[unsafe(no_mangle)] pub static rb_cString: Value = 4;
#[used] #[unsafe(no_mangle)] pub static rb_cArray: Value = 5;
#[used] #[unsafe(no_mangle)] pub static rb_cHash: Value = 6;
#[used] #[unsafe(no_mangle)] pub static rb_cFloat: Value = 7;
#[used] #[unsafe(no_mangle)] pub static rb_cInteger: Value = 8;
#[used] #[unsafe(no_mangle)] pub static rb_cNumeric: Value = 9;
#[used] #[unsafe(no_mangle)] pub static rb_cSymbol: Value = 10;
#[used] #[unsafe(no_mangle)] pub static rb_cTrueClass: Value = 11;
#[used] #[unsafe(no_mangle)] pub static rb_cFalseClass: Value = 12;
#[used] #[unsafe(no_mangle)] pub static rb_cNilClass: Value = 13;
#[used] #[unsafe(no_mangle)] pub static rb_cBasicObject: Value = 14;
#[used] #[unsafe(no_mangle)] pub static rb_cModule: Value = 15;
#[used] #[unsafe(no_mangle)] pub static rb_cClass: Value = 16;
#[used] #[unsafe(no_mangle)] pub static rb_mKernel: Value = 17;
#[used] #[unsafe(no_mangle)] pub static rb_mComparable: Value = 18;
#[used] #[unsafe(no_mangle)] pub static rb_mEnumerable: Value = 19;

/// # Safety
///
/// `s` must be a valid pointer to a NUL-terminated C string. The
/// bytes are copied into an owned `String`; the caller retains
/// ownership of the original buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_new_cstr(s: *const c_char) -> Value {
    assert!(!s.is_null(), "rb_str_new_cstr: null pointer");
    let cstr = unsafe { CStr::from_ptr(s) };
    let bytes = cstr.to_bytes();
    with_state(|st| st.intern(CValue::str_from_bytes(bytes)))
}

/// # Safety
///
/// `ptr` must be valid for reads of `len` bytes (or null when
/// `len == 0`). The bytes are copied into an owned `String` via
/// lossy UTF-8 — spike scope; a real impl would store `Vec<u8>` to
/// preserve binary input verbatim.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_new(ptr: *const c_char, len: c_long) -> Value {
    let bytes: &[u8] = if len == 0 {
        &[]
    } else {
        assert!(!ptr.is_null(), "rb_str_new: null pointer with len > 0");
        // SAFETY: caller guarantees `ptr..ptr+len` is readable.
        unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) }
    };
    with_state(|st| st.intern(CValue::str_from_bytes(bytes)))
}

/// CRuby's `rb_str_new_frozen` returns a frozen *copy* of the
/// string (or the original if already frozen). rubyrs doesn't yet
/// track frozenness as a runtime attribute, and the spike doesn't
/// need it for correctness, so this is a structural no-op: the
/// caller-supplied handle is returned unchanged.
///
/// Real-bcrypt thread-safety logic uses this to snapshot password /
/// salt args defensively; in our single-threaded host that's
/// already-safe-by-construction.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_new_frozen(v: Value) -> Value {
    v
}

/// CRuby's `StringValueCStr(v)` macro expands to a call into this
/// function with `&v`. It's meant to coerce `*v` to a String (via
/// `to_str`) if necessary and return a NUL-terminated `char *`.
///
/// Spike scope: we assume `*v` is already a String (the only
/// non-nil/bool CValue variant). Coercion lands when we expose
/// `rb_funcall(v, "to_str", 0)`-style call backs from C. The NUL
/// termination is honoured because [`CValue::Str`] always stores a
/// sentinel `\0` past the logical end.
///
/// # Safety
///
/// In addition to the trusted-loader handle contract used by every
/// `rb_*` entry: `v` must point at an aligned, valid, writable
/// `Value` for the duration of the call. CRuby's macro callers
/// pass `&local_value`, which satisfies this trivially.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_string_value_cstr(v: *mut Value) -> *const c_char {
    assert!(!v.is_null(), "rb_string_value_cstr: null VALUE pointer");
    let handle = unsafe { *v };
    with_state(|st| match st.resolve(handle) {
        CValue::Str(bytes) => bytes.as_ptr() as *const c_char,
        _ => std::ptr::null(),
    })
}

/// CRuby's `StringValuePtr(v)` macro counterpart. Same as
/// [`rb_string_value_cstr`] today since both extract the underlying
/// byte pointer — diverges once we track NUL-termination separately
/// for `b"\0"`-containing strings.
///
/// # Safety
///
/// In addition to the trusted-loader handle contract used by every
/// `rb_*` entry: `v` must point at an aligned, valid, writable
/// `Value` for the duration of the call. CRuby's macro callers
/// pass `&local_value`, which satisfies this trivially.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_string_value_ptr(v: *mut Value) -> *const c_char {
    unsafe { rb_string_value_cstr(v) }
}

/// Return a pointer to the underlying bytes of a String VALUE.
///
/// Spike scope: pointer is borrowed from the per-call `STATE`, so
/// it's only valid for the rest of the current C function. NOT
/// NUL-terminated; callers must use [`RSTRING_LEN`].
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn RSTRING_PTR(v: Value) -> *const c_char {
    with_state(|st| match st.resolve(v) {
        CValue::Str(bytes) => bytes.as_ptr() as *const c_char,
        _ => std::ptr::null(),
    })
}

/// Length of a String VALUE, in bytes — NOT including the sentinel
/// trailing NUL that [`CValue::Str`] stores past the logical end.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn RSTRING_LEN(v: Value) -> c_long {
    with_state(|st| match st.resolve(v) {
        // Subtract 1 for the sentinel NUL.
        CValue::Str(bytes) => (bytes.len() as c_long) - 1,
        _ => 0,
    })
}

// ===== Integer ↔ VALUE =====
//
// Spike scope: all integers live in `i64`, so every NUMxxx call
// goes through the same path with a final cast. CRuby distinguishes
// Fixnum (tagged in VALUE) from Bignum (heap-allocated); we don't.

/// Convert a C `long` to a Ruby Integer VALUE.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_long2num(n: c_long) -> Value {
    // `c_long` is i64 on Linux/macOS but i32 on wasm32-wasip1, so
    // widen to i64 explicitly. Lossless on every target where
    // c_long ≤ 64 bits (all rubyrs supports). `n as i64` here
    // rather than `.into()` because on the i64-c_long targets
    // clippy's `useless_conversion` flags `.into()` as a no-op —
    // the cast spelling is no-op on those targets and
    // sign-extension on the smaller-c_long ones.
    #[allow(clippy::unnecessary_cast)]
    with_state(|st| st.intern(CValue::Int(n as i64)))
}

/// Convert a Ruby Integer VALUE to a C `long`. Range overflow
/// truncates silently — spike scope.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_num2long(v: Value) -> c_long {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as c_long,
        _ => 0,
    })
}

/// Convert a Ruby Integer VALUE to a C `unsigned long`. Negative
/// values wrap (CRuby raises `RangeError`; spike just casts).
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_num2ulong(v: Value) -> c_ulong {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as c_ulong,
        _ => 0,
    })
}

/// Convert a C `int` to a Ruby Integer VALUE.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_int2num(n: c_int) -> Value {
    with_state(|st| st.intern(CValue::Int(n as i64)))
}

/// Convert a Ruby Integer VALUE to a C `int`.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_num2int(v: Value) -> c_int {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as c_int,
        _ => 0,
    })
}

/// # Safety
///
/// `name` must be a valid NUL-terminated C string. `func` must
/// remain callable for the lifetime of the host runtime (the
/// usual contract: it lives in the loaded shared library, and
/// the library is never unloaded).
///
/// `func` may have any arity-compatible signature (CRuby `ANYARGS`
/// convention); we type it as zero-arg here purely as opaque
/// storage and transmute to the correct shape at dispatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_global_function(
    name: *const c_char,
    func: OpaqueFn,
    arity: c_int,
) {
    assert!(!name.is_null(), "rb_define_global_function: null name");
    let name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    with_state(|st| {
        st.registered_fns.push(CFn {
            name,
            func,
            arity,
        });
    });
}

/// Declare a module by name. The host drains this into `Vm.classes`
/// after Init returns; the joined name is used flat (no nesting).
/// Returns a `CValue::Class(name)` handle.
///
/// # Safety
///
/// `name` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_module(name: *const c_char) -> Value {
    assert!(!name.is_null(), "rb_define_module: null name");
    let name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    with_state(|st| {
        st.registered_classes.push(CExtClassReg {
            joined_name: name.clone(),
        });
        st.intern(CValue::Class(name))
    })
}

/// Declare a class nested under `parent` (e.g. `Engine` under
/// `BCrypt`), inheriting from `_super`. Spike scope: superclass is
/// ignored; nesting becomes a `parent::name` joined string used
/// flat for top-level lookup.
///
/// # Safety
///
/// `name` must be a valid NUL-terminated C string. `parent` must be
/// a class/module handle returned by an earlier `rb_define_module`
/// or `rb_define_class_under`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_class_under(
    parent: Value,
    name: *const c_char,
    _super: Value,
) -> Value {
    assert!(!name.is_null(), "rb_define_class_under: null name");
    let leaf = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    with_state(|st| {
        let parent_name = match st.resolve(parent) {
            CValue::Class(n) => n.clone(),
            other => panic!(
                "rb_define_class_under: parent handle resolved to non-class {:?}",
                other
            ),
        };
        // CRuby special case: when `parent` is `Object` (the root
        // class), the new class is registered as a TOP-LEVEL
        // constant `leaf`, not `Object::leaf`. Real ruby.h does
        // this via `rb_define_class_under(rb_cObject, ...)` being
        // semantically `rb_define_class(...)`. Without this, a
        // cext doing `rb_define_class_under(rb_cObject, "Counter", ...)`
        // would land as `Object::Counter` and `Counter.foo` from
        // Ruby would NameError.
        let joined = if parent_name == "Object" {
            leaf.clone()
        } else {
            format!("{}::{}", parent_name, leaf)
        };
        st.registered_classes.push(CExtClassReg {
            joined_name: joined.clone(),
        });
        st.intern(CValue::Class(joined))
    })
}

/// Register a singleton method on a previously-declared class.
/// `func` is dispatched the same way as
/// `rb_define_global_function`-registered callbacks (transmute by
/// arity, per-call CExtState).
///
/// # Safety
///
/// `name` must be a valid NUL-terminated C string. `klass` must be
/// a class handle; `func` must remain callable for the lifetime of
/// the host runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_singleton_method(
    klass: Value,
    name: *const c_char,
    func: OpaqueFn,
    arity: c_int,
) {
    assert!(!name.is_null(), "rb_define_singleton_method: null name");
    let method_name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    with_state(|st| {
        let class_name = match st.resolve(klass) {
            CValue::Class(n) => n.clone(),
            other => panic!(
                "rb_define_singleton_method: klass resolved to non-class {:?}",
                other
            ),
        };
        st.registered_singletons.push(CExtSingletonMethod {
            class_joined_name: class_name,
            method_name,
            func,
            arity,
        });
    });
}

/// L3-C: Register an instance method on a previously-declared class.
/// Same dispatch shape as `rb_define_singleton_method` — the
/// function pointer is stashed in a per-call CExtState and drained
/// by the host into a per-class instance-method table — except the
/// receiver at call time is the Ruby `Value::Object` whose class
/// matches, not the class itself.
///
/// `self` passed into the C function at call time is the receiver
/// (the Object), not the class. C exts use `TypedData_Get_Struct`
/// to unwrap the receiver's data pointer.
///
/// # Safety
///
/// `klass` must be a class/module handle returned by
/// `rb_define_class_under` OR `rb_define_module` in the **same
/// CExtState** — both produce a `CValue::Class` handle and the
/// dispatch path doesn't distinguish (matches CRuby's
/// "methods on modules" support).
///
/// **VALUE handles are per-call, not process-stable** — unlike
/// CRuby where a `VALUE` is a pointer/tag that stays valid for
/// the object's lifetime, rubyrs-cext indexes into the topmost
/// `CExtState` which is pushed at every cext entry and popped on
/// return. Calling `rb_define_method` with a `klass` handle
/// stashed in a C `static` from a prior `Init_<name>` call (or
/// any prior dispatch) panics: the index points into a popped
/// state's value table.
///
/// Practical rule: register classes/modules AND their methods in
/// a single Init pass. To define methods later, look the class
/// up by name (e.g. through `rb_funcall(rb_cObject, "const_get",
/// ...)` once that's wired) — don't cache the VALUE.
///
/// `name` must be a NUL-terminated C string. `func` must be
/// callable with the registered arity (the host transmutes
/// per-call based on `arity`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_method(
    klass: Value,
    name: *const c_char,
    func: OpaqueFn,
    arity: c_int,
) {
    assert!(!name.is_null(), "rb_define_method: null name");
    let method_name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    with_state(|st| {
        let class_name = match st.resolve(klass) {
            CValue::Class(n) => n.clone(),
            other => panic!(
                "rb_define_method: klass resolved to non-class {:?}",
                other
            ),
        };
        st.registered_methods.push(CExtInstanceMethod {
            class_joined_name: class_name,
            method_name,
            func,
            arity,
        });
    });
}

/// L3-F: register a custom allocator for `klass`. When the Ruby side
/// calls `klass.new(args)`, the host dispatches the call as:
///
///   1. Call `func(klass)` — typically returns a `TypedData_Wrap_Struct`-
///      wrapped Object whose data slot holds a freshly-malloc'd C struct.
///   2. Call `initialize(args...)` on that Object.
///
/// Without this, msgpack-style instance-pattern gems can't produce
/// TypedData-wrapped receivers — `.new` falls back to a bare
/// `Instance` and `TypedData_Get_Struct(self, ...)` in any subsequent
/// instance method then fails the type-check.
///
/// # Safety
///
/// `klass` must resolve to a previously-registered class handle.
/// `func` will be invoked under cext dispatch context (`enter`/`leave`
/// pushed by the host).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_alloc_func(klass: Value, func: OpaqueFn) {
    with_state(|st| {
        let class_name = match st.resolve(klass) {
            CValue::Class(n) => n.clone(),
            other => panic!(
                "rb_define_alloc_func: klass resolved to non-class {:?}",
                other
            ),
        };
        st.registered_alloc_funcs.push(CExtAllocFunc {
            class_joined_name: class_name,
            func,
        });
    });
}

// ===== Array C ABI (Level 2-3) =====

/// Allocate an empty Array and return its handle.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ary_new() -> Value {
    with_state(|st| st.intern(CValue::Array(Vec::new())))
}

/// Allocate an empty Array, ignoring the capacity hint. CRuby's
/// `rb_ary_new_capa` pre-reserves storage; we don't — `Vec` grows
/// on its own and a wrong hint hurts more than it helps. In
/// particular, the previous `Vec::with_capacity(capa.max(0) as
/// usize)` would attempt a giant allocation (or panic on overflow,
/// which in an `extern "C"` boundary translates to a process
/// abort) when given `c_long::MAX` or any large positive hint
/// from a buggy C extension. Honestly ignoring the value matches
/// the existing doc-comment intent and is forward-compatible with
/// a future productionising pass that DOES pre-reserve.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ary_new_capa(_capa: c_long) -> Value {
    with_state(|st| st.intern(CValue::Array(Vec::new())))
}

/// Append `v` to the Array `ary`. Returns `ary` for chaining,
/// matching CRuby.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ary_push(ary: Value, v: Value) -> Value {
    with_state(|st| match st.resolve_mut(ary) {
        CValue::Array(elems) => {
            elems.push(v);
            ary
        }
        other => panic!(
            "ICE: rb_ary_push on non-Array CValue: {:?}",
            std::mem::discriminant(other)
        ),
    })
}

/// Read element at index `idx`. Negative indices count from the end
/// (CRuby semantics). Returns [`Qnil`] for out-of-range.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ary_entry(ary: Value, idx: c_long) -> Value {
    with_state(|st| match st.resolve(ary) {
        CValue::Array(elems) => {
            let len = elems.len() as c_long;
            // Use checked addition for the negative-index case:
            // `LONG_MIN + len` overflows c_long and would abort in
            // debug builds — fatal across the extern "C" boundary
            // (no unwinding). On overflow, treat as out-of-range.
            let resolved = if idx < 0 {
                match idx.checked_add(len) {
                    Some(r) => r,
                    None => return Qnil,
                }
            } else {
                idx
            };
            if resolved < 0 || resolved >= len {
                Qnil
            } else {
                elems[resolved as usize]
            }
        }
        _ => Qnil,
    })
}

/// Length of the Array in elements.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn RARRAY_LEN(ary: Value) -> c_long {
    with_state(|st| match st.resolve(ary) {
        CValue::Array(elems) => elems.len() as c_long,
        _ => 0,
    })
}

// ===== Hash C ABI (Level 2-3) =====

/// Allocate an empty Hash and return its handle.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_new() -> Value {
    with_state(|st| st.intern(CValue::Hash(Vec::new())))
}

/// Set `h[key] = value`. If `key` is already present, replace its
/// value (matching CRuby Hash semantics). Returns `value`.
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_aset(h: Value, key: Value, value: Value) -> Value {
    with_state(|st| {
        // Lift the existing-key check out of the borrow so we can mutate.
        let existing_idx = if let CValue::Hash(pairs) = st.resolve(h) {
            pairs
                .iter()
                .position(|(k, _)| cvalue_eq(st, *k, key))
        } else {
            None
        };
        match st.resolve_mut(h) {
            CValue::Hash(pairs) => {
                if let Some(i) = existing_idx {
                    pairs[i].1 = value;
                } else {
                    pairs.push((key, value));
                }
            }
            other => panic!(
                "ICE: rb_hash_aset on non-Hash CValue: {:?}",
                std::mem::discriminant(other)
            ),
        }
        value
    })
}

/// Get `h[key]`. Returns [`Qnil`] for missing keys (CRuby returns
/// the Hash's default; spike just uses Nil).
///
/// # Safety
///
/// Trusted-loader contract: handle integrity is the dlopen'd C
/// extension's responsibility. Forged `Value` handles either
/// resolve to a no-op or return [`Qnil`] (range-checked inside
/// `with_state`); no path here dereferences a raw Rust reference
/// derived from a handle, so a bad value yields wrong results
/// but never undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_hash_aref(h: Value, key: Value) -> Value {
    with_state(|st| {
        if let CValue::Hash(pairs) = st.resolve(h) {
            for (k, v) in pairs {
                if cvalue_eq(st, *k, key) {
                    return *v;
                }
            }
        }
        Qnil
    })
}

/// Bounded recursion depth for [`cvalue_eq`]. C extensions can build
/// self-referential `CValue::Array` / `CValue::Hash` (`a.push(a)`
/// from C); without a depth limit, comparing such a value against
/// any equal-shape peer stack-overflows. 256 is generous for
/// realistic key shapes and well below the host stack limit.
const CVALUE_EQ_MAX_DEPTH: usize = 256;

/// CValue equality for Hash key lookup. Compares by handle identity
/// first, then falls back to content equality:
///
///   - Nil / True / False / Str / Int : per-variant value compare
///   - Array : same length AND pairwise-equal elements (recursive)
///   - Hash  : same length AND every (k, v) in self has a matching
///     pair somewhere in other (recursive, order-independent)
///   - Class : handle-identity only (CRuby's Module / Class compare
///     by identity by default; spike doesn't model singleton
///     classes that might override)
///
/// Matches Ruby's `eql?` semantics for the variants we model. Recursive
/// equality matters because L2-3 lets a C ext build a Hash key as
/// `rb_ary_new()`-based or `rb_hash_new()`-based, and a lookup with
/// content-equal but distinct-handle key would otherwise miss.
///
/// Recursion is depth-limited (see [`CVALUE_EQ_MAX_DEPTH`]) so a
/// C-built self-referential Array/Hash bottoms out as `false` instead
/// of stack-overflowing.
fn cvalue_eq(st: &CExtState, a: Value, b: Value) -> bool {
    cvalue_eq_d(st, a, b, 0)
}

fn cvalue_eq_d(st: &CExtState, a: Value, b: Value, depth: usize) -> bool {
    if a == b {
        return true;
    }
    if depth >= CVALUE_EQ_MAX_DEPTH {
        // Pathological input (cycle or implausible depth). Bottom
        // out as not-equal rather than overflow the stack; the
        // identity check at the top already handled the trivial
        // same-handle case.
        return false;
    }
    match (st.resolve(a), st.resolve(b)) {
        (CValue::Nil, CValue::Nil) => true,
        (CValue::True, CValue::True) => true,
        (CValue::False, CValue::False) => true,
        (CValue::Str(x), CValue::Str(y)) => x == y,
        (CValue::Int(x), CValue::Int(y)) => x == y,
        (CValue::Array(x), CValue::Array(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(ah, bh)| cvalue_eq_d(st, *ah, *bh, depth + 1))
        }
        (CValue::Hash(x), CValue::Hash(y)) => {
            // Order-independent: every (k, v) in x must have a
            // matching (k', v') in y where k eql k' AND v eql v'.
            // O(n²) lookup — spike scope; CRuby uses an indexed
            // table for the same compare.
            x.len() == y.len()
                && x.iter().all(|(ak, av)| {
                    y.iter().any(|(bk, bv)| {
                        cvalue_eq_d(st, *ak, *bk, depth + 1)
                            && cvalue_eq_d(st, *av, *bv, depth + 1)
                    })
                })
        }
        _ => false,
    }
}

// ===== Intern table for ID (thread-local; see `pub type ID` docs) =====

/// Look up or create the [`ID`] for `name`. CRuby C extensions cache
/// the returned `ID` in static globals at `Init_` time:
///
/// ```c
/// static ID id_to_s;
/// void Init_foo(void) {
///     id_to_s = rb_intern("to_s");
///     ...
/// }
/// ```
///
/// Those cached `ID`s are then passed to `rb_funcall` / `rb_funcallv`
/// to dispatch named methods. Process-wide stability is the contract
/// — the `ID` for a given name must compare equal across every C ext
/// call in the same process, regardless of which per-call
/// [`CExtState`] is active.
///
/// # Safety
///
/// `name` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_intern(name: *const c_char) -> ID {
    assert!(!name.is_null(), "rb_intern: null name");
    let s = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    INTERN.with(|t| t.borrow_mut().intern(&s))
}

/// Dispatch a Ruby method from C: invoke `recv.<id>(argv[..argc])`
/// on the host Vm and return a handle to the result.
///
/// Looks up the topmost [`FuncallCallback`] (installed by the host
/// before transferring control to the C extension) and delegates.
///
/// # Panic policy
///
/// Three contract-violation conditions `assert!` / `expect!` and
/// abort the process under Rust's default `extern "C"` `nounwind`
/// semantics (Rust 2018+):
///
/// - `mid` not previously interned via `rb_intern` (unknown ID)
/// - `argc < 0` (signed-int ABI contract violation)
/// - no [`FuncallCallback`] installed (called from outside an
///   active cext dispatch, e.g. from `Init_<name>` or a thread the
///   host didn't set up)
///
/// All three are programmer-error / C-ABI-contract violations, not
/// runtime conditions arising from valid input. Aborting loudly
/// is intentional, defined behaviour — **not** UB despite the
/// `extern "C"` boundary. See
/// [ADR 0009](../../docs/adr/0009-cext-panic-policy.md) for the
/// full rationale (and the forward path: once `rb_raise` /
/// longjmp-coordinated exception propagation lands, these convert
/// to catchable Ruby exceptions, and ADR 0009 supersedes).
///
/// Ruby-level errors from the dispatched method itself do NOT
/// panic — the host-side `FuncallCallback` catches `Trap` and
/// collapses to `Qnil` (a spike-level concession also noted in
/// ADR 0009's forward path).
///
/// # Safety
///
/// `argv` must be valid for reads of `argc` `VALUE`s when `argc > 0`.
/// When `argc == 0`, `argv` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_funcallv(
    recv: Value,
    mid: ID,
    argc: c_int,
    argv: *const Value,
) -> Value {
    let method = resolve_id(mid)
        .expect("ICE: rb_funcallv with unknown ID; missing rb_intern call?");
    // `argc` is signed (matches CRuby's `int argc`); a negative
    // value indicates the C extension is violating the ABI
    // contract. The previous `if argc > 0 { ... } else { vec![] }`
    // would silently drop all args for negative argc and dispatch
    // a wrong-arity call. Refuse to enter that state.
    assert!(
        argc >= 0,
        "rb_funcallv: negative argc {} (C ext ABI violation)",
        argc
    );
    let args: Vec<Value> = if argc > 0 {
        assert!(!argv.is_null(), "rb_funcallv: null argv with argc > 0");
        // SAFETY: caller guarantees argv..argv+argc is readable.
        unsafe { std::slice::from_raw_parts(argv, argc as usize).to_vec() }
    } else {
        Vec::new()
    };
    FUNCALL_CB.with(|c| {
        let cb = c.borrow();
        let cb = cb
            .last()
            .expect("ICE: rb_funcallv called outside an active cext dispatch");
        cb(recv, &method, &args)
    })
}

// ===== TypedData ABI (Spike L3-B) =====

/// Function pointers stashed inside `rb_data_type_t::function`.
/// Matches CRuby's layout one-to-one so a cext written against
/// ruby.h can declare a static `rb_data_type_t` with no per-host
/// adjustments.
///
/// `dmark` is parsed but unused in the spike (TypedData objects
/// holding Ruby refs is L3-B.1 follow-up — requires stable handle
/// space + a GC mark-worklist hook). `dsize` is parsed but unused
/// (used by ObjectSpace memory accounting in CRuby; not load-bearing).
// Names match `ruby.h` exactly so cexts written against CRuby
// don't need a layer of `typedef` aliases. `non_camel_case_types`
// would otherwise fail `-D warnings` (review #4).
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_data_type_function_t {
    pub dmark: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
    pub dfree: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
    pub dsize: Option<unsafe extern "C" fn(*const std::ffi::c_void) -> usize>,
    /// CRuby reserves two slots here for future expansion (dcompact /
    /// dreference). Match the layout so cexts declaring this struct
    /// from ruby.h get the right padding.
    pub reserved: [*const std::ffi::c_void; 2],
}

/// The TypedData type descriptor. C extensions declare ONE static
/// instance per wrapped C type, e.g.
///
/// ```c
/// static const rb_data_type_t counter_type = {
///     "Counter",
///     { NULL, counter_free, NULL, { NULL, NULL } },
///     NULL, NULL, 0,
/// };
/// ```
///
/// The host treats descriptor identity (pointer equality) as the
/// type check — same as CRuby. Subtype-via-`parent` isn't honoured
/// in the spike; mismatch panics rather than walks the parent chain.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_data_type_t {
    pub wrap_struct_name: *const std::ffi::c_char,
    pub function: rb_data_type_function_t,
    pub parent: *const rb_data_type_t,
    pub data: *const std::ffi::c_void,
    pub flags: Value,
}

/// Wrap an arbitrary C pointer in a Ruby Object of class `klass`,
/// using `type_ptr` as the descriptor. The host allocates a fresh
/// `HeapObj::TypedData` slot whose `dfree` is extracted from
/// `(*type_ptr).function.dfree`; when the slot is swept, that
/// dfree fires on the data pointer.
///
/// # Safety
///
/// `klass` must resolve to a Class handle in the current CExtState
/// (returned by `rb_define_class_under` etc.). `type_ptr` must be a
/// valid static `rb_data_type_t` whose pointer remains live for the
/// lifetime of every wrapped object (CRuby semantics — descriptors
/// are typically `const static`). `data` ownership transfers to the
/// host; the dfree callback is what releases it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_data_typed_object_wrap(
    klass: Value,
    data: *mut std::ffi::c_void,
    type_ptr: *const rb_data_type_t,
) -> Value {
    let dfree = if type_ptr.is_null() {
        None
    } else {
        // SAFETY: caller's contract — type_ptr is a valid static.
        unsafe { (*type_ptr).function.dfree }
    };
    TYPED_DATA_WRAP_CB.with(|c| {
        let cb = c.borrow();
        let cb = cb.last().expect(
            "ICE: rb_data_typed_object_wrap called outside an active cext dispatch",
        );
        cb(klass, data, type_ptr as *const std::ffi::c_void, dfree)
    })
}

/// Type-check + extract pattern. Returns the raw data pointer if
/// `obj` is a TypedData wrapped with EXACTLY `type_ptr` (pointer
/// identity, mirroring CRuby's check); panics otherwise.
///
/// The CRuby-shape `TypedData_Get_Struct` macro in `ruby.h`
/// wraps this and casts the returned pointer to the C ext's
/// concrete struct type.
///
/// # Safety
///
/// `type_ptr` must be either null or a valid pointer to an
/// `rb_data_type_t` whose lifetime covers the call. CRuby's
/// `TypedData_Get_Struct` macro always passes a file-scope
/// `&dtype_static`, satisfying the lifetime requirement trivially.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_check_typeddata(
    obj: Value,
    type_ptr: *const rb_data_type_t,
) -> *mut std::ffi::c_void {
    // Clone the Rc out and DROP the RefCell borrow before invoking
    // the callback. The callback may longjmp via rb_raise on a
    // type mismatch (PR #27 finding #1 — Counter.new.bump), and
    // a longjmp through a still-active RefCell borrow leaves the
    // refcount permanently incremented, panicking the next pop on
    // the same cell with "RefCell already borrowed".
    let cb = TYPED_DATA_CHECK_CB.with(|c| {
        c.borrow()
            .last()
            .cloned()
            .expect("ICE: rb_check_typeddata called outside an active cext dispatch")
    });
    cb(obj, type_ptr as *const std::ffi::c_void)
}

// ---------------------------------------------------------------
// L3-D tricky fns: semantically load-bearing, kept in the main
// lib.rs because they touch internal state shape. Categorized
// stubs live in `stubs/` submodules.
// ---------------------------------------------------------------

/// CRuby exposes `RBASIC_CLASS(obj)` as a struct-field accessor
/// returning the object's class VALUE. flori/json (and most
/// non-trivial cexts) use it for fast type dispatch — comparing
/// the returned handle against `rb_cString` / `rb_cArray` etc.
/// rather than going through `rb_obj_class` + method-call
/// machinery.
///
/// We map each `CValue` variant to its built-in class sentinel
/// (seeded at handles 4-19 in [`CExtState::new`]). `HeapRef` —
/// references to objects on the rubyrs Vm heap, including
/// user-defined classes — falls back to Qnil; the cext side
/// can't introspect Vm-side class identity without a host
/// round-trip we haven't wired. flori/json's dispatch only
/// touches built-ins in the hot path, so Qnil for HeapRef is
/// the documented gap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_basic_class(v: Value) -> Value {
    with_state(|st| match st.resolve(v) {
        CValue::Str(_) => rb_cString,
        CValue::Int(_) => rb_cInteger,
        CValue::Array(_) => rb_cArray,
        CValue::Hash(_) => rb_cHash,
        CValue::True => rb_cTrueClass,
        CValue::False => rb_cFalseClass,
        CValue::Nil => rb_cNilClass,
        CValue::Class(_) => rb_cClass,
        CValue::HeapRef(_) => Qnil,
    })
}

/// CRuby's macro `RTYPEDDATA_DATA(obj)` is an lvalue used both
/// as a read (`p = RTYPEDDATA_DATA(v)`) and as an assignment
/// target (`RTYPEDDATA_DATA(v) = NULL`, eg parser.rl:304's
/// `rvalue_stack_eagerly_release` after stack drain).
///
/// We expose a `void**` slot helper rather than a value getter.
/// The header's `#define RTYPEDDATA_DATA(obj) (*rb_typeddata_data_slot(obj))`
/// dereferences the returned `void**` so the macro's lvalue
/// usage compiles unchanged.
///
/// SPIKE STATUS: returns null. flori/json's parser uses this
/// only in a runtime path (not Init); for L4 dlopen success the
/// symbol need only exist. A correct impl would route through
/// the host's TypedData slot store — TODO when we actually
/// drive a `JSON.parse` call.
///
/// # Safety
///
/// Caller must hold a valid TypedData VALUE. Calling on a
/// non-TypedData value (or after the wrapping value is GC'd) is
/// caller-bug — we return a slot pointing at a sentinel, which
/// would silently corrupt if the caller writes through it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_typeddata_data_slot(_obj: Value) -> *mut *mut std::ffi::c_void {
    // Thread-local sentinel storage. Returned by every call;
    // good enough for parser.c's `RTYPEDDATA_DATA(h) = NULL`
    // pattern (where the write is observed only by a subsequent
    // read in the same path — both land here). Will need
    // proper per-VALUE slots before any cext actually relies on
    // the slot reading back what was written across multiple
    // TypedData objects in the same call.
    thread_local! {
        static SLOT: std::cell::UnsafeCell<*mut std::ffi::c_void> =
            const { std::cell::UnsafeCell::new(std::ptr::null_mut()) };
    }
    SLOT.with(|s| s.get())
}

/// `rb_path_to_class("Foo::Bar")` is CRuby's "look up an
/// already-defined class by its fully-qualified path string"
/// helper, used in cexts that need a class handle without
/// pre-storing one (e.g. exception classes defined on the Ruby
/// side that the cext only raises against).
///
/// Two CRuby entry points share semantics:
/// - `rb_path_to_class(VALUE pathname)` — takes a Ruby String.
/// - `rb_path2class(const char *path)` — takes a C string.
///
/// We implement both. They search the current `CExtState`'s
/// values for a `CValue::Class(joined_name)` matching the
/// argument. If not found, we intern a fresh `CValue::Class`
/// stub so the returned handle is non-nil and usable as a
/// `rb_raise` target — the spike doesn't model class
/// inheritance, so any class-handle works at the dispatch
/// layer.
fn path_lookup_or_stub(name: String) -> Value {
    with_state(|st| {
        for (idx, v) in st.values.iter().enumerate() {
            if let CValue::Class(n) = v
                && n == &name {
                    return idx as Value;
                }
        }
        st.intern(CValue::Class(name))
    })
}

/// # Safety
///
/// `pathname` must be a valid VALUE handle resolving to a String.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_path_to_class(pathname: Value) -> Value {
    let name = with_state(|st| match st.resolve(pathname) {
        CValue::Str(bytes) => {
            // Strip trailing NUL sentinel.
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        }
        _ => String::new(),
    });
    path_lookup_or_stub(name)
}

/// # Safety
///
/// `path` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_path2class(path: *const c_char) -> Value {
    assert!(!path.is_null(), "rb_path2class: null path");
    let name = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    path_lookup_or_stub(name)
}
