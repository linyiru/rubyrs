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
use std::ffi::{CStr, c_char, c_int};

/// Opaque token the C side sees as `VALUE`. Numerically an index
/// into [`CExtState::values`]; semantically meaningless to C code.
pub type Value = u64;

/// Mirror of the subset of `rubyrs::Value` that crosses the C ABI
/// at Level 0. Kept independent so this crate doesn't pull in the
/// whole interpreter — the host translates between the two when
/// draining registered functions and when wrapping arguments.
#[derive(Clone, Debug)]
pub enum CValue {
    Nil,
    True,
    False,
    Str(String),
}

/// One callback registered by a C ext during `Init_<name>` (or, in
/// later levels, by `rb_define_method` from inside a running method).
pub struct CFn {
    pub name: String,
    pub func: unsafe extern "C" fn(Value) -> Value,
    /// CRuby-style arity. Level 0 only honours `0`.
    pub arity: i32,
}

/// Per-thread state shared between the host Vm and any active C ext
/// call. The host swaps this around every C-side entry point so a
/// fresh handle table is in scope.
pub struct CExtState {
    /// Indexed by handle. Indices `0`, `1`, `2` are pre-populated
    /// for [`Qnil`], [`Qtrue`], [`Qfalse`] so their on-disk constants
    /// resolve correctly.
    pub values: Vec<CValue>,
    /// Accumulator for [`rb_define_global_function`] calls. The host
    /// drains this after `Init_<name>` returns.
    pub registered_fns: Vec<CFn>,
}

impl CExtState {
    pub fn new() -> Self {
        Self {
            values: vec![CValue::Nil, CValue::True, CValue::False],
            registered_fns: Vec::new(),
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
    static STATE: RefCell<Option<CExtState>> = const { RefCell::new(None) };
}

/// Run `f` with mutable access to the active [`CExtState`]. Panics
/// if called from a thread that hasn't been wrapped in [`enter`] /
/// [`leave`] — that always indicates a host-side bug.
pub fn with_state<R>(f: impl FnOnce(&mut CExtState) -> R) -> R {
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        let st = b
            .as_mut()
            .expect("ICE: rubyrs-cext STATE not initialised; host must call enter() first");
        f(st)
    })
}

/// Push a fresh [`CExtState`] onto the current thread. Pair with
/// [`leave`].
pub fn enter() {
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        assert!(
            b.is_none(),
            "ICE: nested rubyrs-cext enter() without intervening leave()"
        );
        *b = Some(CExtState::new());
    });
}

/// Pop the active [`CExtState`] and return ownership to the host.
pub fn leave() -> CExtState {
    STATE.with(|s| {
        s.borrow_mut()
            .take()
            .expect("ICE: rubyrs-cext leave() without matching enter()")
    })
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

/// # Safety
///
/// `s` must be a valid pointer to a NUL-terminated C string. The
/// bytes are copied into an owned `String`; the caller retains
/// ownership of the original buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_new_cstr(s: *const c_char) -> Value {
    assert!(!s.is_null(), "rb_str_new_cstr: null pointer");
    let cstr = unsafe { CStr::from_ptr(s) };
    let owned = cstr.to_string_lossy().into_owned();
    with_state(|st| st.intern(CValue::Str(owned)))
}

/// # Safety
///
/// `name` must be a valid NUL-terminated C string. `func` must
/// remain callable for the lifetime of the host runtime (the
/// usual contract: it lives in the loaded shared library, and
/// the library is never unloaded).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_define_global_function(
    name: *const c_char,
    func: unsafe extern "C" fn(Value) -> Value,
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
