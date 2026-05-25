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

/// Opaque token the C side sees as `VALUE`. Numerically an index
/// into [`CExtState::values`]; semantically meaningless to C code.
pub type Value = u64;

/// CRuby's `ID` type — opaque identifier for an interned name
/// (method, symbol, class name, etc.). Returned by `rb_intern`;
/// consumed by `rb_funcall`, `rb_funcallv`, `rb_define_method`'s
/// future variants, etc. Process-wide stable across per-call
/// `CExtState` lifecycles — that's why it lives in its own
/// thread-local table, separate from `CExtState`'s ephemeral
/// handle table.
///
/// `0` is reserved as "no ID" / `Qundef`-ish.
pub type ID = u64;

/// Process-wide intern table for [`ID`]s. C extensions call
/// `rb_intern("name")` and stash the result in static globals
/// (`static ID id_foo;`); those IDs must remain valid across every
/// subsequent C ext call regardless of which per-call `CExtState`
/// is active. This table is the only piece of cext state that
/// outlives a single `enter`/`leave` cycle.
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
}

impl CExtState {
    pub fn new() -> Self {
        Self {
            // Sentinel handle 3 = rb_cObject. We don't actually register
            // an Object class on the rubyrs side; this exists purely so
            // `rb_define_class_under(parent, name, rb_cObject)` accepts
            // its third argument. Superclass is ignored at the spike
            // level — flat namespace only.
            values: vec![
                CValue::Nil,
                CValue::True,
                CValue::False,
                CValue::Class(String::from("Object")),
            ],
            registered_fns: Vec::new(),
            registered_classes: Vec::new(),
            registered_singletons: Vec::new(),
        }
    }

    /// Push a fresh value into the handle table and return its token.
    pub fn intern(&mut self, v: CValue) -> Value {
        let h = self.values.len() as u64;
        self.values.push(v);
        h
    }

    /// Resolve a handle back to its value. Panics on out-of-range
    /// handles — those represent a C ext bug or a stale handle that
    /// outlived its [`CExtState`].
    pub fn resolve(&self, h: Value) -> &CValue {
        self.values
            .get(h as usize)
            .expect("ICE: cext handle out of range; C ext leaked a stale VALUE")
    }
}

impl Default for CExtState {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    // Stack of nested cext states. Level 0/1/1.5 only ever had one
    // active call at a time (no callbacks back into Ruby from C), so
    // `Option` was enough. Level 2's `rb_funcallv` can cause a C ext
    // call to re-enter the Vm, which can in turn dispatch another C
    // ext call — that needs a fresh state on top while the outer
    // state stays preserved underneath. Hence Vec.
    static STATE: RefCell<Vec<CExtState>> = const { RefCell::new(Vec::new()) };
}

/// Run `f` with mutable access to the topmost (innermost) active
/// [`CExtState`]. Panics if called from a thread that has no active
/// state — that always indicates a host-side bug.
pub fn with_state<R>(f: impl FnOnce(&mut CExtState) -> R) -> R {
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        let st = b
            .last_mut()
            .expect("ICE: rubyrs-cext STATE empty; host must call enter() first");
        f(st)
    })
}

/// Push a fresh [`CExtState`] onto the active stack. Pair with
/// [`leave`]. Nests cleanly: each `enter` adds a new state; the
/// matching `leave` pops it and reveals whatever was underneath.
pub fn enter() {
    STATE.with(|s| s.borrow_mut().push(CExtState::new()));
}

/// Pop the topmost [`CExtState`] and return ownership to the host.
pub fn leave() -> CExtState {
    STATE.with(|s| {
        s.borrow_mut()
            .pop()
            .expect("ICE: rubyrs-cext leave() without matching enter()")
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_string_value_ptr(v: *mut Value) -> *const c_char {
    unsafe { rb_string_value_cstr(v) }
}

/// Return a pointer to the underlying bytes of a String VALUE.
///
/// Spike scope: pointer is borrowed from the per-call `STATE`, so
/// it's only valid for the rest of the current C function. NOT
/// NUL-terminated; callers must use [`RSTRING_LEN`].
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_long2num(n: c_long) -> Value {
    with_state(|st| st.intern(CValue::Int(n as i64)))
}

/// Convert a Ruby Integer VALUE to a C `long`. Range overflow
/// truncates silently — spike scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_num2long(v: Value) -> c_long {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as c_long,
        _ => 0,
    })
}

/// Convert a Ruby Integer VALUE to a C `unsigned long`. Negative
/// values wrap (CRuby raises `RangeError`; spike just casts).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_num2ulong(v: Value) -> c_ulong {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as c_ulong,
        _ => 0,
    })
}

/// Convert a C `int` to a Ruby Integer VALUE.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_int2num(n: c_int) -> Value {
    with_state(|st| st.intern(CValue::Int(n as i64)))
}

/// Convert a Ruby Integer VALUE to a C `int`.
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
            arity: arity as i32,
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
        let joined = format!("{}::{}", parent_name, leaf);
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
            arity: arity as i32,
        });
    });
}

// ===== Process-wide intern table for ID =====

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
/// Panics if no callback is installed — that would mean the C ext
/// is calling `rb_funcallv` from outside a host-managed cext call,
/// which is a programmer error.
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
