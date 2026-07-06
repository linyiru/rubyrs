//! Exception normalization + trap → Ruby exception + unwind.
//! Mirrors the raise/rescue plumbing CRuby keeps in `eval.c` /
//! `eval_error.c`. Three pieces:
//!
//! - `normalize_exception` — converts a `raise` argument into a
//!   user-visible Exception Instance (String → RuntimeError,
//!   Class → empty instance, Instance → pass-through).
//! - `trap_to_exception` — promotes a host-side Trap (the
//!   per-type errors like NoMethodError) into a Ruby Exception
//!   Instance the script can `rescue`. Resource-exhausted /
//!   uncatchable variants return None so they bypass the rescue
//!   machinery.
//! - `unwind_with_exception` — walks the frame stack looking
//!   for a matching `rescue` handler; runs the handler if found,
//!   re-raises as Uncaught Trap otherwise.

use std::hint::cold_path;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{Class, Instance, Value};

use super::{LoopTransfer, LoopTransferKind, RescueFilter, Vm, class_is_a};

impl Vm {
    /// Resolve a rescue handler's filter spec to concrete classes, AT
    /// MATCH TIME (an exception is in flight; CRuby evaluates
    /// `rescue <expr>` class expressions at exactly this point). The
    /// handler being probed always belongs to the CURRENT top frame —
    /// the unwinder pops a frame only after exhausting its handlers —
    /// so the top frame's proto (lexical scope) and locals are the
    /// handler's own. Returns `None` when the name doesn't resolve to
    /// a class (fail-closed: matches nothing), matching the old
    /// push-time behaviour for unloaded classes.
    ///
    /// `RescueFilterSpec::Sym` resolution mirrors `Op::LoadConstChain`:
    /// the compiler stamps only the bare source sym (e.g. `Sig`), but a
    /// class defined as `module M; class Sig` is keyed in `self.classes`
    /// by its QUALIFIED sym (`M::Sig`) — so the enclosing scopes are
    /// walked innermost-first, qualifying the bare name, before the
    /// bare fallback (which covers top-level classes and `rescue
    /// Foo::Bar` whose sym is already qualified). Splatted syms
    /// (`rescue *PASSTHROUGH` — minitest's PASSTHROUGH_EXCEPTIONS
    /// idiom) resolve the constant's Array VALUE through the same
    /// chain; `rescue ::Foo::Bar` absolute paths skip the lex-walk.
    fn resolve_rescue_filter(
        &mut self,
        spec: &crate::vm::RescueFilterSpec,
    ) -> Option<RescueFilter> {
        cold_path();
        match spec {
            crate::vm::RescueFilterSpec::None => None,
            // `rescue *local` — read the slot now and snapshot its
            // Array elements as the filter list. A single Class
            // coerces to a one-element filter; Nil/other values match
            // nothing (fail-closed).
            crate::vm::RescueFilterSpec::SplatLocal(src_slot) => {
                let f = self.frames.last().expect("ICE: rescue filter with no frame");
                // Capture-routed read — the filter local can be a
                // captured outer local inside a block frame.
                let src = crate::vm::Vm::frame_local_get(f, &self.locals_arena, *src_slot as usize);
                match src {
                    Value::Array(id) => {
                        let list: Vec<Rc<Class>> = self.heap.array(id).iter()
                            .filter_map(|v| match v {
                                Value::Class(c) => Some(c.clone()),
                                _ => None,
                            })
                            .collect();
                        Some(RescueFilter::Any(list))
                    }
                    Value::Class(c) => Some(RescueFilter::Class(c)),
                    _ => None,
                }
            }
            crate::vm::RescueFilterSpec::Sym(filter_sym) => {
                let filter_sym = *filter_sym;
                // Clone the `Rc<str>` instead of materializing a
                // fresh `String` — the interner returns `&Rc<str>`
                // so the clone is a refcount bump.
                let bare_name: Rc<str> = self.interner.resolve(filter_sym).clone();
                // Splatted filter (`rescue *PASSTHROUGH`) — the marked
                // name is a CONSTANT holding an Array of classes.
                // Unresolved names and non-Array/non-Class values
                // yield `None` (match nothing): fail-closed, where a
                // dropped-splat lowering would degrade to a bare
                // rescue that matched EVERY StandardError.
                if let Some(splat_inner) = crate::const_marker::strip_splat(&bare_name) {
                    let val: Option<Value> = if let Some(abs) = crate::const_marker::strip_absolute(splat_inner) {
                        let abs_sym = self.interner.intern(abs);
                        self.constants.get(&abs_sym).cloned()
                    } else {
                        let proto_idx = self.frames.last().expect("ICE: rescue filter with no frame").proto_idx;
                        let lex = self.protos[proto_idx].lexical_scope.clone();
                        let mut found = None;
                        for scope_sym in &lex {
                            let scope_name = self.interner.resolve(*scope_sym).clone();
                            let qualified = format!("{}::{}", scope_name, splat_inner);
                            let qsym = self.interner.intern(&qualified);
                            if let Some(v) = self.constants.get(&qsym) {
                                found = Some(v.clone());
                                break;
                            }
                        }
                        found.or_else(|| {
                            let inner_sym = self.interner.intern(splat_inner);
                            self.constants.get(&inner_sym).cloned()
                        })
                    };
                    match val {
                        Some(Value::Array(id)) => {
                            let list: Vec<Rc<Class>> = self.heap.array(id).iter()
                                .filter_map(|v| match v {
                                    Value::Class(c) => Some(c.clone()),
                                    _ => None,
                                })
                                .collect();
                            Some(RescueFilter::Any(list))
                        }
                        // `rescue *X` where X holds a single class —
                        // CRuby Array()-coerces, so it behaves as a
                        // one-element list.
                        Some(Value::Class(c)) => Some(RescueFilter::Class(c)),
                        _ => None,
                    }
                }
                // Absolute paths (`rescue ::Foo::Bar`) carry a leading
                // `::` marker from the AST lowering. CRuby semantics:
                // skip the lex-walk and look up the joined name at top
                // level only.
                else if let Some(absolute_bare) = crate::const_marker::strip_absolute(&bare_name) {
                    let abs_sym = self.interner.intern(absolute_bare);
                    self.classes.get(&abs_sym).cloned()
                        .or_else(|| match self.constants.get(&abs_sym) {
                            Some(Value::Class(c)) => Some(c.clone()),
                            _ => None,
                        })
                        .map(RescueFilter::Class)
                } else {
                    let proto_idx = self.frames.last().expect("ICE: rescue filter with no frame").proto_idx;
                    let lex = self.protos[proto_idx].lexical_scope.clone();
                    let mut found = None;
                    if !lex.is_empty() {
                        for scope_sym in &lex {
                            let scope_name = self.interner.resolve(*scope_sym).clone();
                            let qualified = format!("{}::{}", scope_name, bare_name);
                            let qsym = self.interner.intern(&qualified);
                            if let Some(c) = self.classes.get(&qsym).cloned() {
                                found = Some(c);
                                break;
                            }
                            if let Some(Value::Class(c)) = self.constants.get(&qsym) {
                                found = Some(c.clone());
                                break;
                            }
                        }
                    }
                    found
                        .or_else(|| self.classes.get(&filter_sym).cloned())
                        .or_else(|| match self.constants.get(&filter_sym) {
                            Some(Value::Class(c)) => Some(c.clone()),
                            _ => None,
                        })
                        .map(RescueFilter::Class)
                }
            }
        }
    }

    /// Convert a Ruby-level `raise` argument into an Exception instance.
    /// `raise "msg"` becomes `RuntimeError.new("msg")` — we construct the
    /// instance directly (skipping the `initialize` dispatch) and set
    /// `@message`. Already-Exception instances pass through unchanged.
    pub(crate) fn normalize_exception(&mut self, v: Value) -> Value {
        cold_path();
        match &v {
            Value::Object(_) => v,
            Value::Str(_) => {
                let rt_err_id = self.interner.intern("RuntimeError");
                if let Some(cls) = self.classes.get(&rt_err_id).cloned() {
                    self.maybe_gc();
                    let id = self.heap.alloc(HeapObj::Instance(Instance {
                        class: cls,
                        ivars: crate::value::IvarTable::default(),
                        singleton_class: None,
                        frozen: std::cell::Cell::new(false),
                    }));
                    let msg_id = self.interner.intern("@message");
                    self.heap.instance_mut(id).ivar_set(msg_id, v);
                    Value::Object(id)
                } else {
                    v
                }
            }
            Value::Class(cls) => {
                // `raise SomeClass` (no message) == `SomeClass.exception`
                // == `SomeClass.new` — so a user-defined `initialize`
                // (which may set a custom @message, e.g. StopIteration or
                // `def initialize(m="default"); super; end`) MUST run.
                // Dispatch `new` and use the resulting instance. Falls
                // back to a bare class-name-stamped instance if `new`
                // doesn't yield an Object (defensive — every exception
                // class's `new` returns one). The sub-dispatch is
                // self-contained (runs to completion before the caller's
                // unwind starts).
                self.stack.push(Value::Class(cls.clone()));
                let new_id = self.interner.intern("new");
                let pre = self.frames.len();
                let built = match self
                    .do_call(new_id, 0, false, u32::MAX)
                    .and_then(|()| self.dispatch_until(pre))
                {
                    Ok(()) => self.stack.pop(),
                    Err(_) => None,
                };
                match built {
                    Some(obj @ Value::Object(_)) => obj,
                    other => {
                        // Fallback: bare instance with @message = class
                        // name (the old behaviour).
                        if other.is_some() {
                            // `new` returned a non-Object; discard it.
                        }
                        self.maybe_gc();
                        let msg = Value::new_str(cls.name.clone());
                        let id = self.heap.alloc(HeapObj::Instance(Instance {
                            class: cls.clone(),
                            ivars: crate::value::IvarTable::default(),
                            singleton_class: None,
                            frozen: std::cell::Cell::new(false),
                        }));
                        let msg_id = self.interner.intern("@message");
                        self.heap.instance_mut(id).ivar_set(msg_id, msg);
                        Value::Object(id)
                    }
                }
            }
            // `raise <non-exception>` — an Integer / Float / Array /
            // bare Object / non-Exception Class, etc. CRuby raises
            // `TypeError: exception class/object expected` (a rescuable
            // StandardError), NOT the value itself. Passing the value
            // through let it escape to the top level and abort the
            // process (Sinatra's mapped_error specs do `raise 500`).
            _ => {
                let te_id = self.interner.intern("TypeError");
                match self.classes.get(&te_id).cloned() {
                    Some(cls) => {
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Instance(Instance {
                            class: cls,
                            ivars: crate::value::IvarTable::default(),
                            singleton_class: None,
                            frozen: std::cell::Cell::new(false),
                        }));
                        let msg_id = self.interner.intern("@message");
                        self.heap.instance_mut(id).ivar_set(
                            msg_id,
                            Value::new_str("exception class/object expected".to_string()),
                        );
                        Value::Object(id)
                    }
                    None => v,
                }
            }
        }
    }

    /// Convert a host-side `Trap` into a Ruby-level exception
    /// `Value::Object` whose class matches the preamble's
    /// exception hierarchy. Used by `dispatch` / `dispatch_until`
    /// to route primitive errors (NoMethodError, KeyError,
    /// ArgumentError, …) through `unwind_with_exception` so
    /// scripts can `rescue` them like CRuby does.
    ///
    /// Returns `None` for traps that intentionally bypass the
    /// Ruby exception machinery:
    ///
    /// - `ResourceExhausted` — the fuel/heap/deadline kill switch
    ///   must remain unreachable from inside scripts (see
    ///   ADR 0008 / docs/SECURITY.md).
    /// - `Uncaught` — we already failed to find a handler once;
    ///   re-running unwind would be a busy-loop.
    /// - `SyntaxError` — emitted by `ast::tr_with_errors` before
    ///   dispatch ever runs, so it shouldn't reach this code path,
    ///   but treat as uncatchable defensively.
    /// - Cases where the matching class isn't registered (e.g. a
    ///   stripped runtime missing the preamble) — propagate
    ///   instead of silently swallowing.
    pub(crate) fn trap_to_exception(&mut self, trap: &Trap) -> Option<Value> {
        cold_path();
        match &trap.err {
            RubyError::ResourceExhausted { .. }
            | RubyError::Uncaught { .. }
            | RubyError::SyntaxError { .. } => return None,
            _ => {}
        }
        // `self.classes` is keyed by QUALIFIED sym for nested
        // classes (vm/step.rs:~2634 — `module M; class Sig` is
        // stored under interned "M::Sig"). `HostException` is
        // the variant host fns use to raise a class by name —
        // pull the inner `class_name` field directly so the
        // lookup gets "SQLite3::ConstraintException" instead of
        // the discriminant string "HostException".
        let class_name: &str = match &trap.err {
            RubyError::HostException { class_name: cn, .. } => cn.as_str(),
            other => other.class_name(),
        };
        let cls_id = self.interner.intern(class_name);
        let cls = self.classes.get(&cls_id).cloned()?;
        let message = trap.err.message();
        // CRuby populates NameError#name. The "wrong constant name X" raised by
        // const_get / const_defined? on a malformed name carries X as its name
        // (a Symbol) — zeitwerk's inflector-error path re-raises a
        // Zeitwerk::NameError with `error.name`, and test_exceptions asserts it
        // equals `:"Foo-bar"`. Without this the re-raise propagates `nil`.
        let wrong_const_name: Option<String> = (class_name == "NameError")
            .then(|| message.strip_prefix("wrong constant name "))
            .flatten()
            .map(|rest| {
                rest.split(['\n', ' '])
                    .next()
                    .unwrap_or(rest)
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty());
        self.maybe_gc();
        let id = self.heap.alloc(HeapObj::Instance(Instance {
            class: cls,
            ivars: crate::value::IvarTable::default(),
            singleton_class: None,
            frozen: std::cell::Cell::new(false),
        }));
        let msg_sym = self.interner.intern("@message");
        self.heap.instance_mut(id).ivar_set(msg_sym, Value::new_str(message));
        if let Some(nm) = wrong_const_name {
            let name_sym = self.interner.intern(&nm);
            let name_ivar = self.interner.intern("@name");
            self.heap.instance_mut(id).ivar_set(name_ivar, Value::Sym(name_sym));
        }
        // `@backtrace` is filled centrally by `unwind_with_exception`
        // from the live frame stack on the first unwind hop — same
        // path the user-`raise`d Object route takes — so it stays
        // in sync regardless of how the exception got constructed.
        Some(Value::Object(id))
    }

    /// Shared raise driver: the Op::Raise semantics (nil → re-raise
    /// `$!` or "unhandled exception"; normalize; unwind; signal the
    /// native iter boundary) — also reachable from dispatch-form
    /// raises (`send(:raise, ...)`, the kernel-alias forwarder that
    /// a stub's save/restore cycle installs).
    pub(crate) fn do_raise_value(&mut self, mut v: Value) -> Result<(), Trap> {
        if matches!(v, Value::Nil) {
            match self.globals.get(&self.sym_bang).cloned() {
                Some(cur) if !matches!(cur, Value::Nil) => v = cur,
                _ => v = Value::new_str("unhandled exception".to_string()),
            }
        }
        let exc = self.normalize_exception(v);
        self.unwind_with_exception(exc)?;
        if let Some(&d) = self.dispatch_until_depths.last()
            && self.frames.len() <= d
        {
            return Err(self.trap(crate::error::RubyError::AlreadyCaught));
        }
        Ok(())
    }

    pub(crate) fn unwind_with_exception(&mut self, exc: Value) -> Result<(), Trap> {
        cold_path();
        // A real exception supersedes an in-flight `break`/`next`/
        // block-`return` transfer that is NOT parked inside an
        // ensure body (CRuby semantics: the raise wins, the break
        // value is dropped). Entries SUSPENDED in an ensure body
        // are NOT cancelled here: the exception may be raised and
        // rescued entirely WITHIN that body (directly or in a
        // callee), in which case CRuby resumes the pending
        // transfer at the body's end (`def m; return 1; ensure;
        // begin; raise; rescue; end; end` returns 1). Cancelling
        // eagerly left EndEnsure's exception path with nothing on
        // the operand stack — the "ICE: EndEnsure with empty stack
        // on exception path" net/http crash. Suspended entries are
        // instead cancelled by the ESCAPE sweeps in the handler
        // walk below, exactly when the exception leaves their
        // ensure body (a handler below the suspension baseline
        // matches, or the suspended frame pops).
        self.pending_loop_transfers.retain(|t| t.suspended.is_some());
        self.pending_method_breaks.retain(|mb| mb.suspended.is_some());
        self.sync_control_signals();
        // Implicit cause chain (CRuby): raising while another
        // exception is being handled (`$!` is a DIFFERENT live
        // exception) sets the new exception's `#cause` to it. Skips a
        // re-raise of the same object and never overwrites an already-
        // set cause. `$!` is updated to the new exception only once a
        // handler catches it (raise.rs:391), so at this point it still
        // holds the exception of the enclosing rescue.
        if let Value::Object(exc_id) = &exc
            && let Some(Value::Object(bang_id)) = self.globals.get(&self.sym_bang).cloned()
            && bang_id != *exc_id
        {
            let cause_sym = self.interner.intern("@cause");
            let already = self.heap.instance(*exc_id).ivar_get(cause_sym)
                .is_some_and(|v| !matches!(v, Value::Nil));
            if !already {
                self.heap.instance_mut(*exc_id).ivar_set(cause_sym, Value::Object(bang_id));
            }
        }
        // Populate `@backtrace` on the raised exception from the
        // current frame stack — covers both the `raise "msg"` /
        // `raise FooClass.new` Object route (where normalize_
        // exception doesn't fill backtrace) and the trap-to-
        // exception route (where the trap.backtrace has the same
        // shape). Skips when the ivar is already set (e.g.
        // re-raise of an already-rescued exception that preserves
        // its original backtrace, matching CRuby).
        // `throw`/`catch` rides the exception machinery via an internal
        // `RubyrsThrowSignal` (see the handler-walk note below). It is
        // control flow — a `catch` block consumes it and NOTHING ever
        // reads its backtrace. Eagerly materializing the full backtrace
        // (one formatted, line-computed, heap-allocated String per frame)
        // for it was measured at ~68% of a Sinatra request — which
        // `throw :halt`s on EVERY request (483µs→153µs with it skipped).
        // Skip the build for the throw carrier; real raises still get it.
        let exc_is_throw = matches!(&exc, Value::Object(id)
            if class_chain_has_name(&self.heap.real_class_of(*id), "RubyrsThrowSignal"));
        if let Value::Object(exc_id) = &exc {
            let bt_sym = self.interner.intern("@backtrace");
            let already_set = self.heap.instance(*exc_id).ivar_get(bt_sym)
                .map(|v| !matches!(v, Value::Nil))
                .unwrap_or(false);
            if !already_set && !exc_is_throw {
                // Innermost frame first (the raise site), oldest
                // last — CRuby `Exception#backtrace` ordering.
                let bt_strings: Vec<Value> = self
                    .frames
                    .iter()
                    .rev()
                    .map(|f| {
                        let proto = &self.protos[f.proto_idx];
                        let filename = proto.filename.clone();
                        let method = proto.name.clone();
                        // `f.ip` points one past the current op; map
                        // back through `op_spans` to a byte offset.
                        // Fall back to `Span::ZERO` (line 0) on the
                        // boundary case `ip == 0`, matching the
                        // existing `Vm::trap` shape.
                        let span = if f.ip > 0 && f.ip <= proto.op_spans.len() {
                            proto.op_spans[f.ip - 1]
                        } else {
                            crate::error::Span::ZERO
                        };
                        let line = match self.sources.get(&filename) {
                            Some(src) => {
                                crate::error::line_with_base(src, span.byte_offset, proto.line_base)
                            }
                            None => 0,
                        };
                        Value::new_str(format!("{}:{}:in '{}'", filename, line, method))
                    })
                    .collect();
                if !bt_strings.is_empty() {
                    // GC root-hole guard: `exc` is a Rust local
                    // (not on `self.stack` / pinned), so the
                    // upcoming `maybe_gc` would sweep the Instance
                    // we just unwrapped `exc_id` from. Pin
                    // `Value::Object(exc_id)` plus each
                    // bt_string (heap-backed Str) across the
                    // alloc, then drop the pins.
                    self.pinned.push(Value::Object(*exc_id));
                    for s in &bt_strings {
                        self.pinned.push(s.clone());
                    }
                    let n_pinned = bt_strings.len() + 1;
                    self.maybe_gc();
                    let bt_arr_id = self.heap.alloc(HeapObj::Array(bt_strings.into()));
                    // CRuby's raise funcalls `set_backtrace`, so a
                    // USER override observes the raise (minitest's
                    // BetterError fixture stamps `@bad_ivar =
                    // binding` there to poison marshalability).
                    // Detect an override by its defining class —
                    // the preamble default lives on Exception
                    // itself; anything narrower is user code. The
                    // dispatch runs BEFORE the handler walk (frames
                    // untouched); on any failure inside the
                    // override, fall back to the direct ivar write
                    // (best-effort, never compounds the unwind).
                    let sbt_sym = self.interner.intern("set_backtrace");
                    let exc_cls = self.heap.real_class_of(*exc_id);
                    let user_sbt = self.lookup_method_uncached(&exc_cls, sbt_sym).filter(|m| {
                        m.defining_class
                            .as_ref()
                            .and_then(std::rc::Weak::upgrade)
                            .is_none_or(|dc| dc.name != "Exception")
                    });
                    let mut dispatched = false;
                    if let Some(m) = user_sbt {
                        self.pinned.push(Value::Array(bt_arr_id));
                        let pre_frames = self.frames.len();
                        let invoked = self
                            .invoke_method(m, Value::Object(*exc_id), vec![Value::Array(bt_arr_id)])
                            .and_then(|()| self.dispatch_until(pre_frames));
                        self.pinned.pop();
                        if invoked.is_ok() {
                            self.stack.pop();
                            dispatched = true;
                        } else {
                            for f in &self.frames[pre_frames..] {
                                if f.dm_share {
                                    self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
                                }
                            }
                            self.frames.truncate(pre_frames);
                            // A failing user set_backtrace override
                            // could have parked an ensure walk in
                            // one of the discarded frames.
                            self.cancel_transfers_in_dead_frames(pre_frames);
                        }
                    }
                    for _ in 0..n_pinned {
                        self.pinned.pop();
                    }
                    if !dispatched {
                        self.heap.instance_mut(*exc_id)
                            .ivar_set(bt_sym, Value::Array(bt_arr_id));
                    }
                }
            }
        }
        // Resolve the raised value's class once up front; the unwind loop
        // may probe many handlers before finding (or not finding) a match.
        let exc_class: Option<Rc<Class>> = match &exc {
            // Rescue handlers match against the user-declared
            // class, not the eigenclass (CRuby: `rescue Foo`
            // matches `Foo` instances regardless of whether
            // they've had singleton methods installed).
            Value::Object(id) => Some(self.heap.real_class_of(*id)),
            _ => None,
        };
        // `throw`/`catch` is modelled on the exception machinery
        // (preamble/throw_catch.rb): a `throw` to a live tag raises an
        // internal `RubyrsThrowSignal`. CRuby's throw is an unstoppable
        // non-local jump that NO `rescue` — not even `rescue Exception`
        // — may intercept; only the matching `catch` stops it. To make
        // every ordinary rescue transparent to the carrier while still
        // letting `catch`'s own `rescue RubyrsThrowSignal` catch it, we
        // require, when the in-flight exception is the throw carrier,
        // that the handler's filter be RubyrsThrowSignal-or-narrower
        // (i.e. the filter class itself descends from RubyrsThrowSignal).
        // `rescue Exception`'s filter is Exception, which is NOT a
        // descendant, so it falls through — matching CRuby's jump.
        let exc_is_throw_signal = exc_class
            .as_ref()
            .is_some_and(|c| class_chain_has_name(c, "RubyrsThrowSignal"));
        loop {
            // Pop rescue handlers off this frame one by one. A non-ensure
            // handler with a `filter_class` skips if the exception's class
            // is outside that filter — this is what keeps
            // `ResourceExhausted` (rooted at Exception) from being caught
            // by a bare `rescue => e` (rooted at StandardError). A handler
            // that doesn't match is dropped, not re-pushed: the rescue
            // clause was tied to *this* begin/end scope, which we're
            // unwinding past anyway.
            let chosen = loop {
                let popped = self
                    .frames
                    .last_mut()
                    .expect("ICE: unwind with empty frames")
                    .pop_rescue();
                let Some(h) = popped else { break None };
                let matches = if h.is_ensure {
                    // ensure is unconditional — always runs.
                    true
                } else if let Some(filter) = self.resolve_rescue_filter(&h.filter) {
                    // explicit class filter (including bare
                    // `rescue` which compiles to StandardError),
                    // resolved NOW — at match time, CRuby's own
                    // evaluation point for a rescue class expr (the
                    // handler only carries the SymId / local slot).
                    // The splat form (`rescue *PASSTHROUGH`)
                    // matches if ANY listed class matches.
                    exc_class.as_ref().is_some_and(|cls| {
                        // Throw carrier: only a filter that is itself
                        // RubyrsThrowSignal-or-narrower may catch it.
                        let filter_ok = |f: &Rc<Class>| {
                            class_is_a(cls, f)
                                && (!exc_is_throw_signal
                                    || class_chain_has_name(f, "RubyrsThrowSignal"))
                        };
                        match &filter {
                            RescueFilter::Class(f) => filter_ok(f),
                            RescueFilter::Any(list) => list.iter().any(filter_ok),
                        }
                    })
                } else {
                    // Non-ensure handler whose filter doesn't resolve
                    // to a class means the source said `rescue Foo`
                    // where `Foo` isn't loaded. Matches nothing —
                    // keep unwinding (fail-closed).
                    false
                };
                if matches { break Some(h); }
            };
            if let Some(h) = chosen {
                // ESCAPE sweep (handler-match case): the exception
                // is about to land in a handler of THIS frame. Any
                // transfer suspended in this frame whose baseline
                // sits ABOVE the matched handler's position (the
                // remaining `rescues_len` after the pop-walk) had
                // its ensure body abandoned — the handler was
                // pushed BEFORE that body started, so control
                // jumps below/out of the body and its EndEnsure
                // tail never runs. Cancel those entries. Entries
                // with baseline <= remaining length were suspended
                // BELOW the handler's begin region — i.e. the
                // handler was pushed while running their ensure
                // body — so the exception is contained within the
                // body and they stay alive to resume at its tail.
                {
                    let fidx = self.frames.len() - 1;
                    // Frame is present (the pop-walk above just
                    // borrowed it); map_or keeps this off the
                    // panic budget — a None here would only make
                    // the retain below a no-op.
                    let remaining = self.frames.last().map_or(0, |f| f.rescues_len());
                    self.pending_loop_transfers.retain(|t| {
                        t.suspended.is_none_or(|s| {
                            s.frame_idx != fidx || s.rescues_len <= remaining
                        })
                    });
                    self.pending_method_breaks.retain(|mb| {
                        mb.suspended.is_none_or(|s| {
                            s.frame_idx != fidx || s.rescues_len <= remaining
                        })
                    });
                    self.sync_control_signals();
                }
                self.stack.truncate(h.stack_depth);
                let f = self.frames.last_mut().expect("ICE: frames disappeared");
                f.ip = h.handler_ip;
                // Truncate `loop_rescue_depths` to the snapshot taken
                // when this handler was pushed: any `while` loops the
                // exception is unwinding OUT of had their `EnterLoop`
                // entries leak on the stack (we jumped to the handler
                // without their `ExitLoop` running). Without this fix
                // a later `BreakLoop` in this frame reads an orphan
                // entry and pops the wrong handler depth. Parallel
                // truncate on `loop_stack_depths` keeps the two
                // stacks in lock-step.
                f.aux_mut()
                    .loop_rescue_depths
                    .truncate(h.loop_depth_at_push);
                f.aux_mut().loop_stack_depths.truncate(h.loop_depth_at_push);
                // Same shape as loop_rescue_depths truncation but
                // for the begin/rescue baseline stack: any inner
                // `EnterBegin` entries the exception is escaping
                // OUT of had their `ExitBegin` skipped (we jumped
                // straight to a handler). A later `retry` in this
                // (now-outer) rescue body reads `last()` of the
                // baseline stack; without this truncate it would
                // read the orphan inner baseline and shrink
                // `rescues` to the wrong depth, leaving outer
                // rescue handlers stranded or wiping siblings.
                // (Code-review #306 round 2.)
                // Truncate the begin-baseline stack length (each
                // entry now carries a triple — `rescues_len`,
                // `loop_rescue_depths_len`, `loop_stack_depths_len`
                // — captured at its `EnterBegin`; truncating by
                // count is enough since outer entries are still
                // valid).
                f.aux_mut()
                    .begin_rescue_depths
                    .truncate(h.begin_depth_at_push);
                if h.is_ensure {
                    // ensure handler: push the exception onto the operand
                    // stack; the handler's compiled code ends in `Op::Raise`
                    // which will pop it and rethrow after the ensure body
                    // has run.
                    self.stack.push(exc.clone());
                } else if let Some(slot) = h.bind_slot {
                    match &f.locals {
                        crate::vm::Locals::Stack(base) => {
                            let idx = *base as usize + slot as usize;
                            self.locals_arena[idx] = exc.clone();
                        }
                        crate::vm::Locals::Shared(rc) => {
                            // `rescue => e` where `e` is a captured
                            // outer local (block frame): bind into the
                            // canonical cell so the defining scope sees
                            // it — mirror of Op::StoreLocal's routing.
                            if let Some(cell) = f.outer_cell_for(slot as usize) {
                                crate::vm::cell_store(cell, slot as usize, exc.clone());
                            } else {
                                rc.borrow_mut()[slot as usize] = exc.clone();
                            }
                        }
                    }
                }
                // Set `$!` to the in-flight exception for the duration of
                // the rescue / ensure body. The matching restore happens
                // at `Op::ExitBegin` (and at `Op::Return` for a `return`
                // out of the body), which revert `$!` to the value
                // snapshotted at `Op::EnterBegin` — see
                // `BeginBaseline::saved_dollar_bang`. This makes `$!`
                // dynamically scoped like CRuby's errinfo: nested
                // rescues see the right value and a handled exception
                // stops leaking past its begin region.
                self.globals.insert(self.sym_bang, exc);
                return Ok(());
            }
            // No matching handler in this frame — pop it and try the caller.
            let f = self.frames.pop().expect("ICE: unwind pop empty");
            // ESCAPE sweep (frame-pop case): any transfer suspended
            // in the frame we just popped (or above) had its ensure
            // body abandoned by the exception — cancel it so a
            // later unrelated EndEnsure can't mis-resume it.
            self.cancel_transfers_in_dead_frames(self.frames.len());
            if f.dm_share {
                self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
            }
            self.stack.truncate(f.base_sp);
            // Frame-local `$~`: restore the popped method frame's saved
            // last-match as the exception propagates past it, so the
            // eventual rescue body sees the handler-owning method's
            // `$~`, not the raising callee's (block frames carry `None`).
            #[cfg(feature = "regex")]
            if let Some(saved) = f.saved_last_match {
                self.last_match = saved.map(|b| *b);
            }
            if f.is_class_body {
                self.class_stack.pop();
                self.class_visibility_stack.pop();
                self.module_function_active_stack.pop();
            }
            // Release the popped frame's locals storage (arena
            // truncate for Stack frames, recycle for Shared) — every
            // frame-pop site must do this.
            self.release_frame_locals(f.locals);
            // Recycle the aux box (its remaining handlers were already
            // drained by the pop_rescue loop above; clearing drops any
            // leftover loop/begin bookkeeping).
            self.recycle_frame_aux(f.aux);
            if self.frames.is_empty() {
                // No rescue clause anywhere — surface the exception
                // to the host as a Trap instead of terminating the
                // process. The CLI catches `Uncaught` and prints
                // the message; library hosts can pattern-match on
                // `RubyError::Uncaught { class_name, message }` and
                // decide what to do.
                let class_name = match &exc {
                    // real_class_of (vs class_of) so the host
                    // sees the user-declared class name, not
                    // the eigenclass's synthetic
                    // `#<Class:#<Foo>>`. Handles both Instance
                    // and TypedData like its caller.
                    Value::Object(id) => self.heap.real_class_of(*id).name.clone(),
                    _ => exc.type_name().to_string(),
                };
                let message = match &exc {
                    Value::Object(id) => {
                        let msg_sym = self.interner.intern("@message");
                        match self.heap.instance(*id).ivar_get(msg_sym).cloned() {
                            Some(m) => m.to_display(&self.heap, &self.interner),
                            None => String::new(),
                        }
                    }
                    _ => exc.to_display(&self.heap, &self.interner),
                };
                self.last_uncaught_exception = Some(exc.clone());
                return Err(self.trap(RubyError::Uncaught {
                    class_name,
                    message,
                }));
            }
        }
    }

    /// Kicks off a `break` / `next` transfer through any intervening
    /// `is_ensure` handlers between the source op and the target loop.
    /// Walks the frame's rescues from top down: discards plain
    /// rescues (they don't catch break/next); for each ensure it
    /// encounters, suspends the walk by jumping into the ensure's
    /// handler body. `Op::EndEnsure` at the body's tail will pick the
    /// walk back up via `continue_loop_transfer`. If no ensure sits
    /// above the target depth, lands at the target inline.
    pub(crate) fn begin_loop_transfer(
        &mut self,
        kind: LoopTransferKind,
        target_ip: usize,
        target_rescues_len: usize,
    ) -> Result<(), Trap> {
        let f = self
            .frames
            .last()
            .expect("ICE: begin_loop_transfer no frame");
        // Target loop_depth is the slot that BreakLoop/NextLoop were
        // about to consume: the innermost EnterLoop entry. Save it
        // so the eventual landing can truncate `loop_rescue_depths`
        // (entries pushed by lexically-inner whiles the transfer is
        // escaping out of get discarded along the way).
        let target_loop_depth = f.loop_depth() - 1;
        // Capture the operand-stack depth at the matching EnterLoop's
        // time. The landing path truncates to this depth so any
        // residue accumulated in the body (most importantly the
        // exception value `unwind_with_exception` pushed when
        // entering an ensure handler, in scenarios like
        // `while; begin; raise; ensure; break; end; end`) is
        // flushed before the break value lands.
        let target_stack_depth = *f
            .aux
            .as_ref()
            .and_then(|a| a.loop_stack_depths.last())
            .expect("ICE: loop_stack_depths empty at begin_loop_transfer");
        // SUPERSEDE sweep: this break/next may itself sit inside an
        // ensure body that an OLDER transfer (or method-break walk)
        // is suspended in. If the new transfer's target loop is
        // OUTSIDE that body, the body is being abandoned — the newer
        // control transfer wins (CRuby) and the older entry's
        // EndEnsure tail never runs. Cancel those. A target INSIDE
        // the body (nested `while` with its own ensure) leaves the
        // outer entry alive: the nested transfer completes within
        // the body and the outer walk resumes at the body's tail.
        //
        // "Outside the body" has two detectors, both needed:
        //  - rescue-depth: the suspension baseline sits above the
        //    target's rescue depth (the transfer pops handlers below
        //    the body's baseline);
        //  - loop-depth region (SuspendCoord::loop_depth): the target
        //    loop was ALREADY OPEN when the walk suspended
        //    (target_loop_depth < s.loop_depth) — i.e. the body is
        //    lexically inside the target loop, so landing at its join
        //    leaves the body for good. Without this the same-rescue-
        //    depth crossing (`[..].each { while true; begin return v;
        //    ensure; break; end; end; ... }`) left a STALE suspended
        //    walk behind; it was harmless while frame-pop sweeps
        //    caught it, but the abandoned-walk replay in Op::Return
        //    must never see it.
        let fidx = self.frames.len() - 1;
        self.pending_loop_transfers.retain(|t| {
            t.suspended.is_none_or(|s| {
                s.frame_idx != fidx
                    || (s.rescues_len <= target_rescues_len
                        && target_loop_depth >= s.loop_depth)
            })
        });
        self.pending_method_breaks.retain(|mb| {
            mb.suspended.is_none_or(|s| {
                s.frame_idx != fidx
                    || (s.rescues_len <= target_rescues_len
                        && target_loop_depth >= s.loop_depth)
            })
        });
        self.sync_control_signals();
        self.pending_loop_transfers.push(LoopTransfer {
            kind,
            target_ip,
            target_rescues_len,
            target_loop_depth,
            target_stack_depth,
            suspended: None,
        });
        self.continue_loop_transfer()
    }

    /// Resume the in-flight `break`/`next` transfer (top of the
    /// `pending_loop_transfers` stack). Called by `Op::EndEnsure`
    /// at the tail of an ensure handler body, and directly by
    /// `begin_loop_transfer` to do the first hop.
    pub(crate) fn continue_loop_transfer(&mut self) -> Result<(), Trap> {
        let target = self
            .pending_loop_transfers
            .last_mut()
            .expect("ICE: continue_loop_transfer with no pending transfer");
        // Resuming — clear the suspension marker; we're either
        // about to suspend again (re-stamped below) or land
        // (entry popped).
        target.suspended = None;
        let target_rescues_len = target.target_rescues_len;
        let target_ip = target.target_ip;
        let target_loop_depth = target.target_loop_depth;
        let target_stack_depth = target.target_stack_depth;
        // Walk the frame's rescues from top, dropping any non-ensure
        // entries (rescue clauses don't catch a structured break/next).
        // The first ensure we hit gets entered — the rest of the
        // walk happens on the next EndEnsure.
        loop {
            let f = self
                .frames
                .last_mut()
                .expect("ICE: continue_loop_transfer no frame");
            if f.rescues_len() <= target_rescues_len {
                break;
            }
            let h = f
                .pop_rescue()
                .expect("ICE: rescues non-empty by length check");
            if h.is_ensure {
                // Suspend the transfer here. Restore the operand
                // stack to PushEnsure depth (matching the
                // exception-path entry MINUS its exception push) —
                // the ensure body runs with whatever the
                // surrounding code already had on stack at
                // PushEnsure time. EndEnsure at the body's tail
                // matches the recorded coordinates and resumes
                // the walk.
                let rescues_after = f.rescues_len();
                let loop_depth = f.loop_depth();
                self.stack.truncate(h.stack_depth);
                f.ip = h.handler_ip;
                let coord = crate::vm::SuspendCoord {
                    frame_idx: self.frames.len() - 1,
                    rescues_len: rescues_after,
                    stack_len: h.stack_depth,
                    loop_depth,
                    seq: self.next_suspend_seq(),
                };
                // Entry is present (target was read from it at fn
                // entry); if-let keeps this off the panic budget.
                if let Some(t) = self.pending_loop_transfers.last_mut() {
                    t.suspended = Some(coord);
                }
                return Ok(());
            }
            // Plain rescue handler — silently discard. break/next
            // never trigger a rescue clause.
        }
        // No more intervening ensures. Land at the target.
        let transfer = self.pending_loop_transfers.pop().expect("ICE: just had it");
        // Flush operand-stack residue down to the EnterLoop snapshot.
        // Most importantly: if `break`/`next` started from inside an
        // ensure body that `unwind_with_exception` had entered (which
        // pushes the exception onto the operand stack), this discards
        // that stranded exception. CRuby semantics: a control transfer
        // from an ensure cancels the exception that was being unwound.
        self.stack.truncate(target_stack_depth);
        let f = self
            .frames
            .last_mut()
            .expect("ICE: loop transfer landing no frame");
        // Truncate loop_rescue_depths (+ parallel loop_stack_depths)
        // to the entry we're targeting (drops EnterLoop entries from
        // nested whiles the transfer is escaping out of).
        f.aux_mut()
            .loop_rescue_depths
            .truncate(target_loop_depth + 1);
        f.aux_mut()
            .loop_stack_depths
            .truncate(target_loop_depth + 1);
        f.ip = target_ip;
        if let LoopTransferKind::Break { value } = transfer.kind {
            // Push the break value so the loop's join sees it as
            // the loop expression's result. `next` has no value to
            // push; iter_check evaluates the cond from a clean
            // stack.
            self.stack.push(value);
        }
        Ok(())
    }
}

impl Vm {
    /// ADR 0024 Phase A.4: kick off a block-break unwinding the
    /// yielding method's frame through any pending `is_ensure`
    /// handlers. Mirrors `begin_loop_transfer` but the target is
    /// a frame pop + return-value push, not an intra-frame jump.
    ///
    /// Walks the current frame's rescues top-down; the first
    /// `is_ensure` entry suspends the walk by jumping into its
    /// handler body and parking `pending_method_break`. The
    /// `Op::EndEnsure` at the body's tail calls
    /// `continue_method_break` to either find the next ensure
    /// or land the break.
    ///
    /// When no `is_ensure` handlers remain, lands inline: pops
    /// the yielding-method frame, truncates stack, pushes the
    /// break value into the caller's operand stack.
    ///
    /// Returns Ok(()) on success. Caller must already have
    /// truncated the block frame off `self.frames` and ensured
    /// `self.frames.last()` IS the yielding method.
    pub(crate) fn begin_method_break(
        &mut self,
        value: Value,
        target_frame_idx: usize,
        origin: crate::vm::WalkOrigin,
    ) -> Result<(), Trap> {
        // SUPERSEDE sweep: this return/block-break is about to pop
        // every frame down to and including `target_frame_idx`.
        // Transfers suspended in any of those frames had their
        // ensure bodies abandoned (the new return wins — CRuby:
        // `def m; return 1; ensure; return 2; end` returns 2) —
        // cancel them. Transfers suspended in frames BELOW the
        // target contain this whole return (e.g. an outer method's
        // suspended ensure body called the method doing the
        // returning) and stay alive to resume at their tails.
        // Non-suspended entries are superseded outright (the old
        // singleton-slot overwrite semantics).
        self.pending_loop_transfers.retain(|t| {
            t.suspended
                .is_some_and(|s| s.frame_idx < target_frame_idx)
        });
        self.pending_method_breaks.retain(|mb| {
            mb.suspended
                .is_some_and(|s| s.frame_idx < target_frame_idx)
        });
        self.pending_method_breaks.push(crate::vm::MethodBreak {
            value,
            target_frame_idx,
            suspended: None,
            origin,
        });
        self.sync_control_signals();
        self.continue_method_break()
    }

    /// Resume the in-flight Phase A.4/A.5 block-break walk. Called by
    /// `Op::EndEnsure` when `pending_method_break.is_some()`, by
    /// `begin_method_break` to do the first hop, and by the
    /// dispatch loops at their top-of-iteration check after a
    /// Rust iter driver returns control to bytecode in a frame
    /// above the target (Phase A.5 multi-frame propagation).
    pub(crate) fn continue_method_break(&mut self) -> Result<(), Trap> {
        // Clear the top entry's suspension marker — we're either
        // about to suspend again (re-stamped below) or land
        // (entry popped).
        if let Some(mb) = self.pending_method_breaks.last_mut() {
            mb.suspended = None;
        }
        loop {
            let top_idx = self.frames.len() - 1;
            let target = self
                .pending_method_breaks
                .last()
                .expect("ICE: continue_method_break with no pending break")
                .target_frame_idx;
            debug_assert!(
                top_idx >= target,
                "ICE: continue_method_break: top frame {} below target {}",
                top_idx,
                target
            );
            // Walk this frame's rescues; first is_ensure suspends.
            let f = self
                .frames
                .last_mut()
                .expect("ICE: continue_method_break: empty frames");
            let mut found_ensure = None;
            while let Some(h) = f.pop_rescue() {
                if h.is_ensure {
                    found_ensure = Some(h);
                    break;
                }
                // Plain rescue handler — block-break doesn't
                // trigger rescue clauses, silently discard.
            }
            if let Some(h) = found_ensure {
                // Suspend walk inside ensure body. EndEnsure
                // matches the recorded coordinates and resumes via
                // `continue_method_break`. The marker also tells
                // the dispatch loops' top-of-loop check to leave
                // us alone while the body runs.
                let rescues_after = f.rescues_len();
                let loop_depth = f.loop_depth();
                self.stack.truncate(h.stack_depth);
                f.ip = h.handler_ip;
                let coord = crate::vm::SuspendCoord {
                    frame_idx: self.frames.len() - 1,
                    rescues_len: rescues_after,
                    stack_len: h.stack_depth,
                    loop_depth,
                    seq: self.next_suspend_seq(),
                };
                // Entry is present (target was read from it at the
                // loop top); if-let keeps this off the panic budget.
                if let Some(mb) = self.pending_method_breaks.last_mut() {
                    mb.suspended = Some(coord);
                }
                return Ok(());
            }
            // Current frame's rescues exhausted.
            if top_idx == target {
                // We're at the target frame itself. Final
                // landing: pop the frame, push the unwind value
                // into the caller's stack.
                //
                // Class-body case: if the target frame is the
                // class body for a `class Foo; ... end` (which
                // can happen for `return` from a block defined
                // inside a class body — A.6 path), the value
                // pushed onto the caller's stack is the class
                // itself, not the unwind value. Matches the
                // pre-A.6 method_return arm in `dispatch()` and
                // CRuby's class-body semantics ("class Foo; X;
                // end" evaluates to X's class).
                let mb = self
                    .pending_method_breaks
                    .pop()
                    .expect("ICE: pending_method_breaks vanished mid-continue");
                self.sync_control_signals();
                let popped = self
                    .frames
                    .pop()
                    .expect("ICE: continue_method_break landing with empty frames");
                if popped.dm_share {
                    self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
                }
                self.stack.truncate(popped.base_sp);
                // Frame-local `$~`: restore the popped method frame's
                // saved last-match (block frames carry `None`). Mirrors
                // the Op::Return path so a non-local `return` out of a
                // method doesn't leak that method's regex match.
                #[cfg(feature = "regex")]
                if let Some(saved) = popped.saved_last_match {
                    self.last_match = saved.map(|b| *b);
                }
                if popped.is_class_body {
                    let cls = self
                        .class_stack
                        .pop()
                        .expect("ICE: class_stack empty unwinding class-body target");
                    self.class_visibility_stack.pop();
                    self.module_function_active_stack.pop();
                    self.stack.push(crate::value::Value::Class(cls));
                    let _ = mb; // unwind value dropped; class body returns the class
                } else if let Some(replacement) = popped.swap_return {
                    self.stack.push(replacement);
                } else {
                    self.stack.push(mb.value);
                }
                self.release_frame_locals(popped.locals);
                self.recycle_frame_aux(popped.aux);
                return Ok(());
            }
            // Phase A.5: above the target. Pop this intermediate
            // frame (its ensures are done; its return value would
            // normally land in caller's stack but we're unwinding
            // past it). Truncate stack; do NOT push value. Loop
            // to walk the next frame's ensures.
            let popped = self
                .frames
                .pop()
                .expect("ICE: continue_method_break intermediate pop with empty frames");
            if popped.dm_share {
                self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
            }
            self.stack.truncate(popped.base_sp);
            // Frame-local `$~`: restore each intermediate method
            // frame's saved last-match as we unwind past it (see the
            // landing pop above).
            #[cfg(feature = "regex")]
            if let Some(saved) = popped.saved_last_match {
                self.last_match = saved.map(|b| *b);
            }
            if popped.is_class_body {
                // Class-body frames carry class_stack /
                // class_visibility_stack entries; mirror what
                // method_return / Op::Return cleanup does on a
                // class-eval-inside-block pop.
                self.class_stack.pop();
                self.class_visibility_stack.pop();
                self.module_function_active_stack.pop();
            }
            self.release_frame_locals(popped.locals);
            self.recycle_frame_aux(popped.aux);
        }
    }
}

/// True if `cls` or any of its superclasses is named `name`.
/// Used to recognise the `RubyrsThrowSignal` throw carrier (and
/// any hypothetical subclass) so ordinary `rescue` clauses can
/// stay transparent to a `throw` in flight — see the unwind loop.
fn class_chain_has_name(cls: &Rc<Class>, name: &str) -> bool {
    let mut cur = Some(cls.clone());
    while let Some(c) = cur {
        if c.name == name {
            return true;
        }
        cur = c.superclass.borrow().clone();
    }
    false
}

/// ADR 0025 Phase 2: build an Interrupt instance for the
/// safe-point check to feed into `unwind_with_exception`. The
/// shape matches what `raise Interrupt, "msg"` would produce
/// from script-level Ruby: a Value::Object whose class is the
/// preamble's Interrupt class, with `@message = "interrupt"`.
///
/// Returns None when the Interrupt class is missing (preamble
/// not loaded or a host disabled Phase 0). The caller falls back
/// to a host-level `RubyError::Interrupt` Trap in that case.
#[allow(dead_code)]
pub(crate) fn build_interrupt_exception(vm: &mut crate::vm::Vm) -> Option<crate::value::Value> {
    use crate::heap::HeapObj;
    use crate::value::{Instance, RStr, Value};
    let cls_id = vm.interner.intern("Interrupt");
    let cls = vm.classes.get(&cls_id).cloned()?;
    vm.maybe_gc();
    // Allocation failure here would also break the safe-point
    // delivery — fall back to None and let the caller decide.
    if vm.check_alloc().is_err() {
        return None;
    }
    let id = vm.heap.alloc(HeapObj::Instance(Instance {
        class: cls,
        ivars: crate::value::IvarTable::default(),
        singleton_class: None,
        frozen: std::cell::Cell::new(false),
    }));
    let message_sym = vm.interner.intern("@message");
    let msg_val = Value::Str(std::rc::Rc::new(RStr::new("interrupt".to_string())));
    vm.heap.instance_mut(id).ivar_set(message_sym, msg_val);
    Some(Value::Object(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Proto;
    use crate::intern::Interner;
    use std::cell::RefCell;

    fn mk_vm() -> Vm {
        Vm::new(Vec::<Proto>::new(), Interner::new())
    }

    fn mk_class(name: &str, superclass: Option<Rc<Class>>) -> Rc<Class> {
        Rc::new(Class {
            name: name.to_string(),
            is_module: false,
            ivars: RefCell::new(crate::intern::FxHashMap::default()),
            methods: RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            singleton_includes: RefCell::new(Vec::new()),
            singleton_view: RefCell::new(None),
            singleton_target: RefCell::new(None),
            superclass: RefCell::new(superclass),
            undefed: RefCell::new(crate::intern::FxHashSet::default()),
            anon_serial: std::cell::Cell::new(0),
            ivar_shape: std::cell::RefCell::new(crate::value::IvarShape::default()),
            class_vars: RefCell::new(crate::intern::FxHashMap::default()),
            consts: RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: RefCell::new(None),
            class_tag: None,
            frozen: std::cell::Cell::new(false),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        })
    }

    /// Register `RuntimeError` on the Vm's class table so the
    /// `Value::Str → Exception` normalisation has somewhere to
    /// look. Without it normalize_exception falls into the
    /// "no-class-registered" path and returns the input
    /// unchanged.
    fn register_runtime_error(vm: &mut Vm) -> Rc<Class> {
        let cls = mk_class("RuntimeError", None);
        let cls_id = vm.interner.intern("RuntimeError");
        vm.classes.insert(cls_id, cls.clone());
        cls
    }

    #[test]
    fn normalize_exception_passes_object_through() {
        let mut vm = mk_vm();
        let cls = mk_class("Anything", None);
        // Allocate an empty Instance directly.
        let id = vm.heap.alloc(HeapObj::Instance(Instance {
            class: cls,
            ivars: crate::value::IvarTable::default(),
            singleton_class: None,
            frozen: std::cell::Cell::new(false),
        }));
        let v = Value::Object(id);
        let out = vm.normalize_exception(v.clone());
        // Same ObjId comes back — the value passes through.
        if let (Value::Object(a), Value::Object(b)) = (&v, &out) {
            assert_eq!(a, b);
        } else {
            panic!("expected Object pass-through");
        }
    }

    #[test]
    fn normalize_exception_wraps_string_as_runtime_error() {
        let mut vm = mk_vm();
        register_runtime_error(&mut vm);
        let msg = Value::new_str("boom".to_string());
        let out = vm.normalize_exception(msg);
        let id = match out {
            Value::Object(id) => id,
            other => panic!("expected Object, got {other:?}"),
        };
        // Class is RuntimeError.
        assert_eq!(vm.heap.class_of(id).name, "RuntimeError");
        // `@message` is the original string.
        let msg_sym = vm.interner.intern("@message");
        let stored = vm.heap.instance(id).ivar_get(msg_sym).cloned()
            .expect("@message ivar should be set");
        assert!(matches!(stored, Value::Str(_)));
    }

    #[test]
    fn normalize_exception_string_without_class_passes_through() {
        // No RuntimeError registered → normalize bails and the
        // input passes through unchanged. Matches the documented
        // "stripped runtime missing the preamble" fall-through.
        let mut vm = mk_vm();
        let msg = Value::new_str("boom".to_string());
        let out = vm.normalize_exception(msg);
        assert!(matches!(out, Value::Str(_)));
    }

    #[test]
    fn normalize_exception_class_dispatches_new() {
        // `raise SomeClass` == `SomeClass.new` — normalize_exception
        // dispatches `new` so a user `initialize` runs (the `@message`
        // default is initialize's job; in a real runtime that yields
        // `e.message == "SomeClass"`, covered byte-for-byte by the
        // `raise MyError` diff test). Here, in a bare VM with no
        // preamble Exception#initialize, the contract we assert is the
        // structural one: the result is an instance of the class.
        let mut vm = mk_vm();
        let cls = mk_class("MyError", None);
        let v = Value::Class(cls.clone());
        let out = vm.normalize_exception(v);
        let id = match out {
            Value::Object(id) => id,
            other => panic!("expected Object, got {other:?}"),
        };
        assert!(Rc::ptr_eq(&vm.heap.class_of(id), &cls));
    }

    #[test]
    fn trap_to_exception_returns_none_for_resource_exhausted() {
        let mut vm = mk_vm();
        let trap = Trap::new(RubyError::ResourceExhausted { msg: "fuel".into() });
        assert!(vm.trap_to_exception(&trap).is_none());
    }

    #[test]
    fn trap_to_exception_returns_none_for_uncaught() {
        let mut vm = mk_vm();
        let trap = Trap::new(RubyError::Uncaught {
            class_name: "X".into(),
            message: "y".into(),
        });
        assert!(vm.trap_to_exception(&trap).is_none());
    }

    #[test]
    fn trap_to_exception_returns_none_for_syntax_error() {
        let mut vm = mk_vm();
        let trap = Trap::new(RubyError::SyntaxError { msg: "bad".into() });
        assert!(vm.trap_to_exception(&trap).is_none());
    }

    #[test]
    fn trap_to_exception_returns_none_when_class_missing() {
        // ArgumentError isn't routed to None by the variant filter,
        // but with no `ArgumentError` class registered the
        // `self.classes.get(&cls_id).cloned()?` propagates None.
        let mut vm = mk_vm();
        let trap = Trap::new(RubyError::ArgumentError { msg: "bad".into() });
        assert!(vm.trap_to_exception(&trap).is_none());
    }

    #[test]
    fn trap_to_exception_builds_exception_for_registered_class() {
        let mut vm = mk_vm();
        let cls = mk_class("ArgumentError", None);
        let cls_id = vm.interner.intern("ArgumentError");
        vm.classes.insert(cls_id, cls.clone());

        let trap = Trap::new(RubyError::ArgumentError {
            msg: "bad arg".into(),
        });
        let out = vm.trap_to_exception(&trap).expect("class is registered");
        let id = match out {
            Value::Object(id) => id,
            other => panic!("expected Object, got {other:?}"),
        };
        assert!(Rc::ptr_eq(&vm.heap.class_of(id), &cls));

        let msg_sym = vm.interner.intern("@message");
        let stored = vm.heap.instance(id).ivar_get(msg_sym).cloned()
            .expect("@message ivar should be set");
        // The message string carries the trap's message.
        let s = stored.to_display(&vm.heap, &vm.interner);
        assert_eq!(s, "bad arg");
    }
}
