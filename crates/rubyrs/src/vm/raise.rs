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
                        singleton_class: None,
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
                    singleton_class: None,
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
            singleton_class: None,
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
            // Rescue handlers match against the user-declared
            // class, not the eigenclass (CRuby: `rescue Foo`
            // matches `Foo` instances regardless of whether
            // they've had singleton methods installed).
            Value::Object(id) => Some(self.heap.real_class_of(*id)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use crate::bytecode::Proto;
    use crate::intern::Interner;

    fn mk_vm() -> Vm {
        Vm::new(Vec::<Proto>::new(), Interner::new())
    }

    fn mk_class(name: &str, superclass: Option<Rc<Class>>) -> Rc<Class> {
        Rc::new(Class {
            name: name.to_string(),
            methods: RefCell::new(HashMap::new()),
            singleton_methods: RefCell::new(HashMap::new()),
            includes: RefCell::new(Vec::new()),
            superclass: RefCell::new(superclass),
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
            ivars: HashMap::new(),
            singleton_class: None,
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
        let stored = vm.heap.instance(id).ivars.get(&msg_sym).cloned()
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
    fn normalize_exception_class_returns_empty_instance() {
        let mut vm = mk_vm();
        let cls = mk_class("MyError", None);
        let v = Value::Class(cls.clone());
        let out = vm.normalize_exception(v);
        let id = match out {
            Value::Object(id) => id,
            other => panic!("expected Object, got {other:?}"),
        };
        assert!(Rc::ptr_eq(&vm.heap.class_of(id), &cls));
        // `@message` is NOT set (no arg).
        let msg_sym = vm.interner.intern("@message");
        assert!(!vm.heap.instance(id).ivars.contains_key(&msg_sym));
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

        let trap = Trap::new(RubyError::ArgumentError { msg: "bad arg".into() });
        let out = vm.trap_to_exception(&trap).expect("class is registered");
        let id = match out {
            Value::Object(id) => id,
            other => panic!("expected Object, got {other:?}"),
        };
        assert!(Rc::ptr_eq(&vm.heap.class_of(id), &cls));

        let msg_sym = vm.interner.intern("@message");
        let stored = vm.heap.instance(id).ivars.get(&msg_sym).cloned()
            .expect("@message ivar should be set");
        // The message string carries the trap's message.
        let s = stored.to_display(&vm.heap, &vm.interner);
        assert_eq!(s, "bad arg");
    }
}

