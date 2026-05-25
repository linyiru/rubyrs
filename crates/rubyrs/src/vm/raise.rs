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

use std::collections::HashMap;

use std::hint::cold_path;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{Class, Instance, Value};

use super::{class_is_a, Vm};

impl Vm {
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
                        ivars: HashMap::new(),
                    }));
                    let msg_id = self.interner.intern("@message");
                    self.heap.instance_mut(id).ivars.insert(msg_id, v);
                    Value::Object(id)
                } else {
                    v
                }
            }
            Value::Class(cls) => {
                // `raise SomeClass` with no message: instantiate
                // an empty Exception of that class. We skip the
                // `initialize` dispatch — there's no argument to
                // pass — and leave `@message` unset. CRuby's
                // `SomeClass.exception` for the no-arg form would
                // call `initialize` with no args; the user's
                // `initialize` (if any) accepting a default-nil
                // message would produce the same end-state.
                self.maybe_gc();
                let id = self.heap.alloc(HeapObj::Instance(Instance {
                    class: cls.clone(),
                    ivars: HashMap::new(),
                }));
                Value::Object(id)
            }
            _ => v,
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
        let class_name = trap.err.class_name();
        let cls_id = self.interner.intern(class_name);
        let cls = self.classes.get(&cls_id).cloned()?;
        let message = trap.err.message();
        self.maybe_gc();
        let id = self.heap.alloc(HeapObj::Instance(Instance {
            class: cls,
            ivars: HashMap::new(),
        }));
        let msg_sym = self.interner.intern("@message");
        self.heap.instance_mut(id).ivars.insert(msg_sym, Value::new_str(message));
        Some(Value::Object(id))
    }

    pub(crate) fn unwind_with_exception(&mut self, exc: Value) -> Result<(), Trap> {
        cold_path();
        // Resolve the raised value's class once up front; the unwind loop
        // may probe many handlers before finding (or not finding) a match.
        let exc_class: Option<Rc<Class>> = match &exc {
            Value::Object(id) => Some(self.heap.class_of(*id)),
            _ => None,
        };
        loop {
            // Pop rescue handlers off this frame one by one. A non-ensure
            // handler with a `filter_class` skips if the exception's class
            // is outside that filter — this is what keeps
            // `ResourceExhausted` (rooted at Exception) from being caught
            // by a bare `rescue => e` (rooted at StandardError). A handler
            // that doesn't match is dropped, not re-pushed: the rescue
            // clause was tied to *this* begin/end scope, which we're
            // unwinding past anyway.
            let chosen = {
                let f = self.frames.last_mut().expect("ICE: unwind with empty frames");
                let mut chosen = None;
                while let Some(h) = f.rescues.pop() {
                    let matches = if h.is_ensure {
                        // ensure is unconditional — always runs.
                        true
                    } else if let Some(filter) = &h.filter_class {
                        // explicit class filter (including bare
                        // `rescue` which compiles to StandardError).
                        exc_class.as_ref().is_some_and(|cls| class_is_a(cls, filter))
                    } else {
                        // Non-ensure handler with no resolved filter
                        // class means the source said `rescue Foo`
                        // where `Foo` wasn't loaded at push-time.
                        // Matches nothing — keep unwinding.
                        false
                    };
                    if matches { chosen = Some(h); break; }
                }
                chosen
            };
            if let Some(h) = chosen {
                self.stack.truncate(h.stack_depth);
                let f = self.frames.last_mut().expect("ICE: frames disappeared");
                f.ip = h.handler_ip;
                if h.is_ensure {
                    // ensure handler: push the exception onto the operand
                    // stack; the handler's compiled code ends in `Op::Raise`
                    // which will pop it and rethrow after the ensure body
                    // has run.
                    self.stack.push(exc);
                } else if let Some(slot) = h.bind_slot {
                    f.locals.borrow_mut()[slot as usize] = exc;
                }
                return Ok(());
            }
            // No matching handler in this frame — pop it and try the caller.
            let f = self.frames.pop().expect("ICE: unwind pop empty");
            self.stack.truncate(f.base_sp);
            if f.is_class_body {
                self.class_stack.pop();
                self.class_visibility_stack.pop();
            }
            if self.frames.is_empty() {
                // No rescue clause anywhere — surface the exception
                // to the host as a Trap instead of terminating the
                // process. The CLI catches `Uncaught` and prints
                // the message; library hosts can pattern-match on
                // `RubyError::Uncaught { class_name, message }` and
                // decide what to do.
                let class_name = match &exc {
                    // class_of handles both Instance and TypedData (review #1).
                    Value::Object(id) => self.heap.class_of(*id).name.clone(),
                    _ => exc.type_name().to_string(),
                };
                let message = match &exc {
                    Value::Object(id) => {
                        let msg_sym = self.interner.intern("@message");
                        match self.heap.instance(*id).ivars.get(&msg_sym).cloned() {
                            Some(m) => m.to_display(&self.heap, &self.interner),
                            None => String::new(),
                        }
                    }
                    _ => exc.to_display(&self.heap, &self.interner),
                };
                return Err(self.trap(RubyError::Uncaught { class_name, message }));
            }
        }
    }
}
