//! The opcode interpreter loop. Mirrors CRuby's vm_exec.c —
//! the main switch over Op variants plus the outer driver
//! that calls `step` until a frame returns or traps.
//!
//! Contents:
//!   - `dispatch` — top-level run loop, returns when the
//!     initial frame returns.
//!   - `dispatch_until` — re-entrant run loop used by
//!     `invoke_block` / `do_call_block` to interpret nested
//!     frames without unwinding.
//!   - `step` — the per-opcode big match. The bulk of the file.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{BinOpKind, Op};
use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{BlockHandle, Class, Method, Value, Visibility};

use super::{primitive_call, vec_nil, Frame, RescueHandler, Vm};

impl Vm {
    pub(crate) fn dispatch(&mut self) -> Result<(), Trap> {
        while !self.frames.is_empty() {
            // Non-local return unwind. `Op::ReturnMethod` sets
            // `method_return`; here we honour it by popping any
            // block frames between us and the enclosing method,
            // then popping the method frame and pushing the value
            // as its return. Exit the whole dispatch if we
            // unwound off the bottom of the frame stack.
            if let Some(val) = self.method_return.take() {
                while let Some(f) = self.frames.last() {
                    if !f.is_block { break; }
                    let f = self.frames.pop().unwrap();
                    self.stack.truncate(f.base_sp);
                    // `class_eval { ... }` frames are both
                    // `is_block: true` (so this loop walks
                    // through them, matching CRuby's "return
                    // exits the method, not the block") AND
                    // `is_class_body: true` (so `def name`
                    // inside the block lands on the class).
                    // The class-body cleanup that the Op::Return
                    // arm does inline (pop class_stack +
                    // class_visibility_stack) has to happen here
                    // too — otherwise a non-local return through
                    // a class_eval block would leak class-stack
                    // entries.
                    if f.is_class_body {
                        let _cls = self.class_stack.pop()
                            .expect("ICE: class_stack empty unwinding through class_eval");
                        self.class_visibility_stack.pop();
                    }
                }
                if let Some(f) = self.frames.pop() {
                    self.stack.truncate(f.base_sp);
                    if f.is_class_body {
                        let cls = self.class_stack.pop()
                            .expect("ICE: class_stack empty on method-return");
                        self.class_visibility_stack.pop();
                        self.stack.push(Value::Class(cls));
                    } else if let Some(replacement) = f.swap_return {
                        self.stack.push(replacement);
                    } else {
                        self.stack.push(val);
                    }
                    if self.frames.is_empty() { return Ok(()); }
                } else {
                    return Ok(());
                }
                continue;
            }
            let (proto_idx, ip) = {
                let f = self.frames.last().expect("ICE: dispatch with empty frame stack");
                (f.proto_idx, f.ip)
            };
            let op = self.protos[proto_idx].code[ip];
            self.frames.last_mut().expect("ICE: frame disappeared").ip += 1;
            match self.step(op, proto_idx) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(trap) => {
                    // Try routing the trap through the Ruby
                    // rescue machinery so scripts can `rescue`
                    // primitive errors (NoMethodError, KeyError,
                    // ArgumentError, ...). ResourceExhausted /
                    // Uncaught / SyntaxError pass through
                    // unchanged.
                    if let Some(exc) = self.trap_to_exception(&trap) {
                        // Capture the original trap's site before
                        // unwind drains the frame stack — when
                        // unwind synthesises an Uncaught Trap on
                        // miss, its backtrace is empty (frames
                        // already gone). Preserve the call-site
                        // info from the trap that actually fired.
                        let original_bt = trap.backtrace.clone();
                        let original_class = trap.err.class_name().to_string();
                        let original_msg = trap.err.message();
                        match self.unwind_with_exception(exc) {
                            Ok(()) => continue, // handler set up, resume dispatch
                            Err(_) => return Err(Trap {
                                err: RubyError::Uncaught {
                                    class_name: original_class,
                                    message: original_msg,
                                },
                                backtrace: original_bt,
                            }),
                        }
                    }
                    return Err(trap);
                }
            }
        }
        Ok(())
    }



    /// Run dispatch loop until the frame stack returns to `until_depth`.
    pub(crate) fn dispatch_until(&mut self, until_depth: usize) -> Result<(), Trap> {
        while self.frames.len() > until_depth {
            // A non-local return signal means we're about to
            // unwind past `until_depth` anyway. Exit early and
            // let the iterator driver (our caller) propagate the
            // signal to its own caller. Running more ops here
            // would burn fuel inside a frame about to be discarded.
            if self.method_return.is_some() { return Ok(()); }
            let (proto_idx, ip) = {
                let f = self.frames.last().expect("ICE: dispatch_until no frame");
                (f.proto_idx, f.ip)
            };
            let op = self.protos[proto_idx].code[ip];
            self.frames.last_mut().expect("ICE: frames empty").ip += 1;
            match self.step(op, proto_idx) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(trap) => {
                    // Same convert-to-rescue dance as `dispatch`.
                    // Without this, a primitive error inside a
                    // block (`arr.each { nil.foo }`) would
                    // bypass every rescue handler all the way
                    // up the call chain.
                    if let Some(exc) = self.trap_to_exception(&trap) {
                        let original_bt = trap.backtrace.clone();
                        let original_class = trap.err.class_name().to_string();
                        let original_msg = trap.err.message();
                        match self.unwind_with_exception(exc) {
                            Ok(()) => continue,
                            Err(_) => return Err(Trap {
                                err: RubyError::Uncaught {
                                    class_name: original_class,
                                    message: original_msg,
                                },
                                backtrace: original_bt,
                            }),
                        }
                    }
                    return Err(trap);
                }
            }
        }
        Ok(())
    }



    /// Execute one op; returns Ok(false) if we just popped the last frame.
    /// `_proto_idx` is reserved for future per-op span lookup; with the
    /// global interner, ops no longer need it for string resolution.
    pub(crate) fn step(&mut self, op: Op, _proto_idx: usize) -> Result<bool, Trap> {
        self.check_fuel()?;
        match op {
            Op::LoadConstInt(i) => self.stack.push(Value::Int(i)),
            Op::LoadConstFloat(f) => self.stack.push(Value::Float(f)),
            Op::LoadConstStr(id) => {
                let s = self.interner.resolve(id).clone();
                self.stack.push(Value::new_str(s.to_string()));
            }
            Op::LoadRegex(id) => {
                let regex_rc = if let Some(r) = self.regex_cache.get(&id) {
                    r.clone()
                } else {
                    let src = self.interner.resolve(id).clone();
                    let compiled = regex::Regex::new(&src).map_err(|e| {
                        self.trap(RubyError::SyntaxError {
                            msg: format!("invalid regex /{}/: {}", src, e),
                        })
                    })?;
                    let rc = Rc::new(compiled);
                    self.regex_cache.insert(id, rc.clone());
                    rc
                };
                self.stack.push(Value::Regex(regex_rc));
            }
            Op::LoadSymbol(id) => {
                self.stack.push(Value::Sym(id));
            }
            Op::LoadNil => self.stack.push(Value::Nil),
            Op::LoadTrue => self.stack.push(Value::Bool(true)),
            Op::LoadFalse => self.stack.push(Value::Bool(false)),
            Op::LoadSelf => {
                let v = self.frames.last().expect("ICE: LoadSelf no frame").self_val.clone();
                self.stack.push(v);
            }
            Op::LoadLocal(s) => {
                let v = self.frames.last().expect("ICE: LoadLocal no frame").locals.borrow()[s as usize].clone();
                self.stack.push(v);
            }
            Op::StoreLocal(s) => {
                let v = self.stack.pop().expect("ICE: StoreLocal stack underflow");
                self.frames.last().expect("ICE: StoreLocal no frame").locals.borrow_mut()[s as usize] = v;
            }
            Op::IncLocalNoPush(s) => {
                let slot = s as usize;
                let frame = self.frames.last().expect("ICE: IncLocalNoPush no frame");
                let cur = frame.locals.borrow()[slot].clone();
                if let Value::Int(n) = cur {
                    frame.locals.borrow_mut()[slot] = Value::Int(n.wrapping_add(1));
                } else {
                    // Slow path: rebind via `+`. push, dispatch, store, drop result.
                    self.stack.push(cur);
                    self.stack.push(Value::Int(1));
                    let plus_id = self.interner.intern("+");
                    self.do_call(plus_id, 1, false, u16::MAX)?;
                    let v = self.stack.pop().unwrap_or(Value::Nil);
                    self.frames.last().expect("ICE").locals.borrow_mut()[slot] = v;
                }
            }
            Op::IncLocal(s) => {
                let slot = s as usize;
                let frame = self.frames.last().expect("ICE: IncLocal no frame");
                let cur = frame.locals.borrow()[slot].clone();
                if let Value::Int(n) = cur {
                    let new_n = n.wrapping_add(1);
                    frame.locals.borrow_mut()[slot] = Value::Int(new_n);
                    self.stack.push(Value::Int(new_n));
                } else {
                    // Slow path: replicate `slot = slot + 1` via BinOp semantics,
                    // including user-defined `+` on the receiver type.
                    self.stack.push(cur);
                    self.stack.push(Value::Int(1));
                    let plus_id = self.interner.intern("+");
                    self.do_call(plus_id, 1, false, u16::MAX)?;
                    let new_val = self.stack.last().expect("ICE: IncLocal slow path no result").clone();
                    self.frames.last().expect("ICE").locals.borrow_mut()[slot] = new_val;
                }
            }
            Op::Dup => {
                let v = self.stack.last().expect("ICE: Dup stack underflow").clone();
                self.stack.push(v);
            }
            Op::Pop => { self.stack.pop(); }
            Op::LoadIvar(name_id) => {
                let id_opt = if let Value::Object(id) = &self.frames.last().expect("ICE: LoadIvar no frame").self_val { Some(*id) } else { None };
                let v = if let Some(id) = id_opt {
                    self.heap.instance(id).ivars.get(&name_id).cloned().unwrap_or(Value::Nil)
                } else { Value::Nil };
                self.stack.push(v);
            }
            Op::StoreIvar(name_id) => {
                let v = self.stack.pop().expect("ICE: StoreIvar stack underflow");
                let id_opt = if let Value::Object(id) = &self.frames.last().expect("ICE: StoreIvar no frame").self_val { Some(*id) } else { None };
                if let Some(id) = id_opt { self.heap.instance_mut(id).ivars.insert(name_id, v); }
            }
            Op::IncIvarNoPush(name_id) => {
                let inst_id = if let Value::Object(id) = &self.frames.last().expect("ICE: IncIvarNoPush no frame").self_val {
                    Some(*id)
                } else { None };
                if let Some(inst_id) = inst_id {
                    let cur = self.heap.instance(inst_id).ivars.get(&name_id).cloned();
                    match cur {
                        Some(Value::Int(n)) => {
                            self.heap.instance_mut(inst_id).ivars.insert(name_id, Value::Int(n.wrapping_add(1)));
                        }
                        _ => {
                            let cur_v = cur.unwrap_or(Value::Nil);
                            self.stack.push(cur_v);
                            self.stack.push(Value::Int(1));
                            let plus_id = self.interner.intern("+");
                            self.do_call(plus_id, 1, false, u16::MAX)?;
                            let v = self.stack.pop().unwrap_or(Value::Nil);
                            self.heap.instance_mut(inst_id).ivars.insert(name_id, v);
                        }
                    }
                }
            }
            Op::IncIvar(name_id) => {
                let inst_id = if let Value::Object(id) = &self.frames.last().expect("ICE: IncIvar no frame").self_val {
                    Some(*id)
                } else { None };
                if let Some(inst_id) = inst_id {
                    let cur = self.heap.instance(inst_id).ivars.get(&name_id).cloned();
                    match cur {
                        Some(Value::Int(n)) => {
                            let new_n = n.wrapping_add(1);
                            self.heap.instance_mut(inst_id).ivars.insert(name_id, Value::Int(new_n));
                            self.stack.push(Value::Int(new_n));
                        }
                        _ => {
                            // Slow path: @x is nil or non-Int — replicate full `@x = @x + 1`.
                            let cur_v = cur.unwrap_or(Value::Nil);
                            self.stack.push(cur_v);
                            self.stack.push(Value::Int(1));
                            let plus_id = self.interner.intern("+");
                            self.do_call(plus_id, 1, false, u16::MAX)?;
                            let v = self.stack.last().expect("ICE: IncIvar slow path no result").clone();
                            self.heap.instance_mut(inst_id).ivars.insert(name_id, v);
                        }
                    }
                } else {
                    // Outside class context: nil + 1 — let CRuby semantics dictate
                    self.stack.push(Value::Nil);
                    self.stack.push(Value::Int(1));
                    let plus_id = self.interner.intern("+");
                    self.do_call(plus_id, 1, false, u16::MAX)?;
                }
            }
            Op::StoreConst(name_id) => {
                // `FOO = expr` — compiler emitted Dup before this so
                // the assigned value also remains on the stack as
                // the expression's result (CRuby semantics).
                let v = self.stack.pop().expect("ICE: StoreConst stack underflow");
                self.constants.insert(name_id, v);
            }
            Op::LoadConst(name_id) => {
                let v = if let Some(c) = self.classes.get(&name_id).cloned() {
                    Value::Class(c)
                } else if let Some(v) = self.constants.get(&name_id).cloned() {
                    v
                } else if &**self.interner.resolve(name_id) == "ENV" {
                    // Lazy-build ENV as a regular String-keyed Hash
                    // snapshotted from the process environment. Cached
                    // for the lifetime of the Vm so all `ENV` reads
                    // see a single object — writes via `ENV[k] = v`
                    // mutate the snapshot but not the real process
                    // env (documented divergence; would need a
                    // setenv wrapper otherwise).
                    let id = if let Some(id) = self.env_hash {
                        id
                    } else {
                        let pairs: Vec<(Value, Value)> = std::env::vars()
                            .map(|(k, v)| (Value::new_str(k), Value::new_str(v)))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Hash(pairs));
                        self.env_hash = Some(id);
                        id
                    };
                    Value::Hash(id)
                } else {
                    Value::Nil
                };
                self.stack.push(v);
            }
            Op::Jump(off) => {
                let f = self.frames.last_mut().expect("ICE: Jump no frame");
                f.ip = (f.ip as i32 + off) as usize;
            }
            Op::JumpIfFalse(off) => {
                let v = self.stack.pop().expect("ICE: JumpIfFalse stack underflow");
                if !v.is_truthy() {
                    let f = self.frames.last_mut().expect("ICE: JumpIfFalse no frame");
                    f.ip = (f.ip as i32 + off) as usize;
                }
            }
            Op::JumpIfArgGiven(slot, off) => {
                let f = self.frames.last_mut().expect("ICE: JumpIfArgGiven no frame");
                if slot < f.n_given_positional {
                    f.ip = (f.ip as i32 + off) as usize;
                }
            }
            Op::Call(name_id, argc, cache_id) => {
                self.do_call(name_id, argc as usize, false, cache_id)?;
            }
            Op::CallNoRecv(name_id, argc, cache_id) => {
                self.do_call(name_id, argc as usize, true, cache_id)?;
            }
            Op::ApplyCall(name_id, cache_id) | Op::ApplyCallNoRecv(name_id, cache_id) => {
                // Splat-call: pop the args Array, push its
                // elements back onto the stack as positional args,
                // then dispatch with that dynamic argc. Receiver
                // (when present) sits below the array on the
                // stack — same layout `do_call` expects.
                let no_recv = matches!(op, Op::ApplyCallNoRecv(_, _));
                let arr_val = self.stack.pop().expect("ICE: ApplyCall without arg array");
                let arr_id = match arr_val {
                    Value::Array(id) => id,
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Array (splat arg)", other.type_name()),
                    })),
                };
                let elems: Vec<Value> = self.heap.array(arr_id).clone();
                let argc = elems.len();
                for v in elems { self.stack.push(v); }
                self.do_call(name_id, argc, no_recv, cache_id)?;
            }
            Op::CallBlock(name_id, argc, cache_id) => {
                self.do_call_block(name_id, argc as usize, false, cache_id)?;
            }
            Op::CallNoRecvBlock(name_id, argc, cache_id) => {
                self.do_call_block(name_id, argc as usize, true, cache_id)?;
            }
            Op::Super(name_id, argc) => {
                let split = self.stack.len() - argc as usize;
                let args: Vec<Value> = self.stack.drain(split..).collect();
                let frame = self.frames.last().expect("ICE: Super no frame");
                let self_val = frame.self_val.clone();
                // Start the lookup at the *defining class's*
                // superclass, not `self.class.superclass`. The
                // latter would re-find the current method when
                // `self` is a subclass instance and recurse
                // forever. CRuby's "module of definition" rule.
                let defining = match frame.defining_class.clone() {
                    Some(c) => c,
                    None => {
                        return Err(self.trap(RubyError::NoMethodError {
                            method: "super called outside of method".to_string(),
                            recv_type: self_val.type_name(),
                        }));
                    }
                };
                let parent = match defining.superclass.borrow().clone() {
                    Some(p) => p,
                    None => {
                        return Err(self.trap(RubyError::NoMethodError {
                            method: format!("super: no superclass method `{}'",
                                self.interner.resolve(name_id)),
                            recv_type: self_val.type_name(),
                        }));
                    }
                };
                let m = match self.lookup_method_uncached(&parent, name_id) {
                    Some(m) => m,
                    None => {
                        return Err(self.trap(RubyError::NoMethodError {
                            method: format!("super: no superclass method `{}'",
                                self.interner.resolve(name_id)),
                            recv_type: self_val.type_name(),
                        }));
                    }
                };
                self.invoke_method(m, self_val, args)?;
            }
            Op::CreateBlock(p_idx, param_start, n_params, rest_slot_raw) => {
                // Snapshot the surrounding frame's captured locals
                // (shared Rc with subsequent invocations of this
                // block) and self before any mutable borrow of
                // `self`, then allocate the BlockHandle into the
                // heap. The stack value is a plain `ObjId`.
                let (captured, self_val) = {
                    let f = self.frames.last().expect("ICE: CreateBlock no frame");
                    (f.locals.clone(), f.self_val.clone())
                };
                let rest_slot = if rest_slot_raw == u16::MAX { None } else { Some(rest_slot_raw) };
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Block(BlockHandle {
                    proto_idx: p_idx as usize,
                    captured,
                    self_val,
                    param_start,
                    n_params,
                    rest_slot,
                }));
                self.stack.push(Value::Block(id));
            }
            Op::Yield(argc) => {
                let block = match self.frames.last().expect("ICE: Yield no frame").block_arg {
                    Some(b) => b,
                    None => return Err(self.trap(RubyError::RuntimeError {
                        msg: "no block given (yield)".to_string(),
                    })),
                };
                let argc = argc as usize;
                let split = self.stack.len() - argc;
                let args: Vec<Value> = self.stack.drain(split..).collect();
                self.invoke_block(block, args)?;
            }
            Op::DefMethod(name_id, p_idx) => {
                let proto = &self.protos[p_idx as usize];
                // Capture the defining class (top of class_stack
                // when we're inside `class Foo; def bar; end; end`)
                // so `super` later starts its lookup from the
                // right place. `None` for toplevel defs.
                // Stored as Weak — see Method.defining_class docs.
                let defining_class = self.class_stack.last().map(Rc::downgrade);
                let vis = self.class_visibility_stack.last().copied().unwrap_or(Visibility::Public);
                let m = Rc::new(Method {
                    params: proto.params.clone(),
                    proto_idx: p_idx as usize,
                    defining_class,
                    visibility: std::cell::Cell::new(vis),
                    closure: None,
                });
                if let Some(cls) = self.class_stack.last() { cls.methods.borrow_mut().insert(name_id, m); }
                else { self.toplevel_methods.insert(name_id, m); }
                // Conservatively invalidate the inline cache — any previous
                // cache entry could in theory be made stale by this definition.
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::DefSingletonMethod(name_id, p_idx) => {
                // `def self.foo` inside a class body. Installs `foo`
                // on the surrounding class's `singleton_methods`
                // table, dispatched against `Value::Class(c)`
                // receivers in `do_call`. Outside a class body
                // (toplevel singleton has no well-defined target)
                // we fall back to installing on `toplevel_methods`.
                let proto = &self.protos[p_idx as usize];
                let defining_class = self.class_stack.last().map(Rc::downgrade);
                let vis = self.class_visibility_stack.last().copied().unwrap_or(Visibility::Public);
                let m = Rc::new(Method {
                    params: proto.params.clone(),
                    proto_idx: p_idx as usize,
                    defining_class,
                    visibility: std::cell::Cell::new(vis),
                    closure: None,
                });
                if let Some(cls) = self.class_stack.last() {
                    cls.singleton_methods.borrow_mut().insert(name_id, m);
                } else {
                    self.toplevel_methods.insert(name_id, m);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::DefObjectSingletonMethod(name_id, p_idx) => {
                // `def obj.name; ...; end` (non-`self` receiver)
                // — instance-level singleton install. Receiver
                // was pushed by the compiler immediately before
                // this op (see `compile_expr`'s Def arm).
                let recv = self.stack.pop()
                    .expect("ICE: DefObjectSingletonMethod stack underflow");
                let obj_id = match recv {
                    Value::Object(id) => id,
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "can't define singleton method on {} (only user-class instances are supported)",
                                other.type_name(),
                            ),
                        }));
                    }
                };
                // Lazily allocate the eigenclass — the receiver
                // pays nothing for objects that never get a
                // singleton method. Repeated `def obj.x` /
                // `def obj.y` on the same object reuse the same
                // singleton class.
                let sc = self.heap.ensure_singleton_class(obj_id);
                let proto = &self.protos[p_idx as usize];
                // `defining_class` points at the eigenclass so
                // `super` from inside walks the eigenclass's
                // superclass chain (= original class), matching
                // CRuby's "module of definition" rule. Stored
                // as `Weak` so the (sc ↔ Method) cycle doesn't
                // pin the eigenclass past the receiver's
                // lifetime — see PR #31 review for the analysis.
                let m = Rc::new(Method {
                    params: proto.params.clone(),
                    proto_idx: p_idx as usize,
                    defining_class: Some(Rc::downgrade(&sc)),
                    visibility: std::cell::Cell::new(Visibility::Public),
                    closure: None,
                });
                sc.methods.borrow_mut().insert(name_id, m);
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::AliasMethod(new_id, old_id) => {
                // Resolve `old` along the surrounding class's ancestor
                // chain (or toplevel) and re-insert the same Rc<Method>
                // under `new` in the *current* class. We share the Rc
                // — alias is intentionally semantically identical to
                // the original, including its `defining_class` (so
                // `super` from inside the aliased call walks from the
                // original's super, matching CRuby's "module of
                // definition" rule for aliases).
                //
                // The walk lets `class Child < Parent; alias_method :x,
                // :parent_method; end` work: the source method lives
                // on Parent, the alias name `x` lands on Child.
                let existing = if let Some(cls) = self.class_stack.last() {
                    self.lookup_method_uncached(cls, old_id)
                } else {
                    self.toplevel_methods.get(&old_id).cloned()
                };
                let m = match existing {
                    Some(m) => m,
                    None => {
                        // CRuby raises NameError ("undefined method ...")
                        // when `alias_method`'s source name isn't found
                        // on the receiver's ancestor chain — not
                        // NoMethodError. NameError is the right shape:
                        // there's no value to call yet (alias is a
                        // class-body operation, not a dispatch site),
                        // so the previous `NoMethodError { recv_type:
                        // "Class" }` was misleading.
                        let name = self.interner.resolve(old_id).to_string();
                        let ctx = self.class_stack.last()
                            .map(|c| format!("class `{}'", c.name))
                            .unwrap_or_else(|| "main".to_string());
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method `{}' for {}", name, ctx),
                        }));
                    }
                };
                if let Some(cls) = self.class_stack.last() {
                    cls.methods.borrow_mut().insert(new_id, m);
                } else {
                    self.toplevel_methods.insert(new_id, m);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::DefMethodBlock(name_id) => {
                // Pop the BlockHandle the preceding `CreateBlock`
                // pushed, then wrap it as a closure-method. We
                // *share* the BlockHandle's `captured` Rc — the
                // method body and the original lexical scope point
                // at the same locals Vec, so the method can read &
                // write outer-scope variables (CRuby semantics).
                //
                // GC: the captured Rc keeps its slots alive via the
                // Method, which lives in Class.methods (rooted via
                // Vm.classes) or toplevel_methods. `maybe_gc`'s
                // root-gathering loops walk every installed method
                // table and add closure-captured slots to the root
                // set, so Objects/Arrays reachable through the
                // closure survive collections.
                let bv = self.stack.pop().expect("ICE: DefMethodBlock no block on stack");
                let id = if let Value::Block(id) = bv { id } else {
                    panic!("ICE: DefMethodBlock without Block on stack");
                };
                let (proto_idx, captured, param_start, n_params) = {
                    let bh = self.heap.block(id);
                    (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
                };
                let proto = &self.protos[proto_idx];
                let params = proto.params.clone();
                let defining_class = self.class_stack.last().map(Rc::downgrade);
                let vis = self.class_visibility_stack.last().copied().unwrap_or(crate::value::Visibility::Public);
                let m = Rc::new(Method {
                    params,
                    proto_idx,
                    defining_class,
                    visibility: std::cell::Cell::new(vis),
                    closure: Some(crate::value::MethodClosure { captured, param_start, n_params }),
                });
                if let Some(cls) = self.class_stack.last() { cls.methods.borrow_mut().insert(name_id, m); }
                else { self.toplevel_methods.insert(name_id, m); }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::DefObjectSingletonMethodBlock(name_id) => {
                // `recv.define_singleton_method(:foo) { |args| ... }`
                // — closure-method install on the receiver's
                // eigenclass. Compiler pushed `recv` first then
                // the `CreateBlock`-produced block, so pop in
                // that reverse order.
                let bv = self.stack.pop()
                    .expect("ICE: DefObjectSingletonMethodBlock no block on stack");
                let block_id = if let Value::Block(id) = bv { id } else {
                    panic!("ICE: DefObjectSingletonMethodBlock without Block on stack");
                };
                let recv = self.stack.pop()
                    .expect("ICE: DefObjectSingletonMethodBlock no receiver on stack");
                let obj_id = match recv {
                    Value::Object(id) => id,
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "can't define singleton method on {} (only user-class instances are supported)",
                                other.type_name(),
                            ),
                        }));
                    }
                };
                let (proto_idx, captured, param_start, n_params) = {
                    let bh = self.heap.block(block_id);
                    (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
                };
                let proto = &self.protos[proto_idx];
                let params = proto.params.clone();
                let sc = self.heap.ensure_singleton_class(obj_id);
                // `defining_class` points at the eigenclass so
                // `super` from inside walks the eigenclass's
                // superclass chain (which falls through to the
                // original class), matching `Op::DefSingletonMethod`'s
                // chain semantics.
                let m = Rc::new(Method {
                    params,
                    proto_idx,
                    // Weak — same cycle break as DefSingletonMethod.
                    defining_class: Some(Rc::downgrade(&sc)),
                    visibility: std::cell::Cell::new(Visibility::Public),
                    closure: Some(crate::value::MethodClosure { captured, param_start, n_params }),
                });
                sc.methods.borrow_mut().insert(name_id, m);
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::DefClass(name_id, p_idx) => {
                // Pop superclass (Nil for "default to Object", a Class for `class Foo < Bar`).
                let parent_val = self.stack.pop().expect("ICE: DefClass without superclass slot");
                let parent = match parent_val {
                    Value::Class(c) => Some(c),
                    _ => None, // Nil -> default; treat as no explicit parent for now
                };
                let name_str = self.interner.resolve(name_id).to_string();
                let cls = self.classes.entry(name_id).or_insert_with(|| Rc::new(Class {
                    name: name_str,
                    methods: RefCell::new(HashMap::new()),
                    singleton_methods: RefCell::new(HashMap::new()),
                    superclass: RefCell::new(parent.clone()),
                    includes: RefCell::new(Vec::new()),
                    #[cfg(feature = "cext")]
                    cext_alloc_func: std::cell::Cell::new(None),
                })).clone();
                // If the class already existed (reopened) and the user specified a parent
                // this time, update it (only if it wasn't already set to something else).
                if let Some(p) = &parent {
                    let mut sc = cls.superclass.borrow_mut();
                    if sc.is_none() {
                        *sc = Some(p.clone());
                    }
                }
                self.method_gen = self.method_gen.wrapping_add(1); // class structure changed
                self.class_stack.push(cls.clone());
                self.class_visibility_stack.push(Visibility::Public);
                let proto = &self.protos[p_idx as usize];
                let n_locals = proto.n_locals as usize;
                self.frames.push(Frame {
                    proto_idx: p_idx as usize, ip: 0,
                    locals: Rc::new(RefCell::new(vec_nil(n_locals))),
                    self_val: Value::Class(cls.clone()),
                    base_sp: self.stack.len(),
                    is_class_body: true, swap_return: None, block_arg: None, defining_class: None, is_block: false, n_given_positional: 0, rescues: vec![], loop_rescue_depths: vec![],
                });
            }
            Op::NewArray(n) => {
                self.maybe_gc();
                self.check_alloc()?;
                let n = n as usize;
                let split = self.stack.len() - n;
                let elems: Vec<Value> = self.stack.drain(split..).collect();
                let id = self.heap.alloc(HeapObj::Array(elems));
                self.stack.push(Value::Array(id));
            }
            Op::NewRange(excl) => {
                self.maybe_gc();
                self.check_alloc()?;
                let end = self.stack.pop().expect("ICE: NewRange end underflow");
                let begin = self.stack.pop().expect("ICE: NewRange begin underflow");
                let id = self.heap.alloc(HeapObj::Range(crate::heap::RangeObj {
                    begin, end, exclusive: excl != 0,
                }));
                self.stack.push(Value::Range(id));
            }
            Op::NewHash(n) => {
                self.maybe_gc();
                self.check_alloc()?;
                let n = n as usize;
                let split = self.stack.len() - n * 2;
                let flat: Vec<Value> = self.stack.drain(split..).collect();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(n);
                let mut iter = flat.into_iter();
                while let (Some(k), Some(v)) = (iter.next(), iter.next()) { pairs.push((k, v)); }
                let id = self.heap.alloc(HeapObj::Hash(pairs));
                self.stack.push(Value::Hash(id));
            }
            Op::PushRescue(off, slot, bind, filter_sym) => {
                let f = self.frames.last().expect("ICE: PushRescue no frame");
                let ip = f.ip;
                let loop_depth = f.loop_rescue_depths.len();
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                let bind_slot = if bind != 0 { Some(slot) } else { None };
                // The compiler emits the SymId of the class to filter
                // by — for bare `rescue` that's `StandardError`. If the
                // class hasn't been loaded into `self.classes` yet
                // (e.g. `rescue MyUndefinedError`), `filter_class`
                // becomes `None` and the handler will fail every
                // match check in `unwind_with_exception`. That's
                // closer to CRuby's behaviour than silently catching
                // everything would be.
                let filter = self.classes.get(&filter_sym).cloned();
                self.frames.last_mut().expect("ICE: PushRescue no frame").rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot, is_ensure: false,
                    filter_class: filter, loop_depth_at_push: loop_depth,
                });
            }
            Op::PopRescue => {
                self.frames.last_mut().expect("ICE: PopRescue no frame").rescues.pop();
            }
            Op::PushEnsure(off) => {
                let f = self.frames.last().expect("ICE: PushEnsure no frame");
                let ip = f.ip;
                let loop_depth = f.loop_rescue_depths.len();
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                self.frames.last_mut().expect("ICE: PushEnsure no frame").rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot: None, is_ensure: true,
                    filter_class: None, // ensure is unconditional
                    loop_depth_at_push: loop_depth,
                });
            }
            Op::PopEnsure => {
                self.frames.last_mut().expect("ICE: PopEnsure no frame").rescues.pop();
            }
            Op::Raise => {
                let v = self.stack.pop().unwrap_or(Value::Nil);
                let exc = self.normalize_exception(v);
                self.unwind_with_exception(exc)?;
            }
            Op::Break => {
                // Mark the surrounding native-driven loop to terminate.
                // The value the user passed (or `nil`) stays on the
                // operand stack and rides out with the subsequent
                // Op::Return; collection_call_block reads it then.
                self.break_signaled = true;
            }
            Op::EnterLoop => {
                let depth = self.frames.last().expect("ICE: EnterLoop no frame").rescues.len();
                self.frames.last_mut().expect("ICE: EnterLoop no frame")
                    .loop_rescue_depths.push(depth);
            }
            Op::ExitLoop => {
                self.frames.last_mut().expect("ICE: ExitLoop no frame")
                    .loop_rescue_depths.pop()
                    .expect("ICE: ExitLoop with empty loop_rescue_depths");
            }
            Op::BreakLoop(off) => {
                // Pop dynamic rescue/ensure handlers back down to the
                // depth recorded at `Op::EnterLoop`. This is the
                // load-bearing step for `break` inside a `begin`
                // body where some `PushRescue` is still installed,
                // and for `break` inside a partially-unwound rescue
                // chain — using the dynamic `rescues.len()` rather
                // than a compile-time count lets the same op work
                // regardless of which rescue clause caught.
                let f = self.frames.last_mut().expect("ICE: BreakLoop no frame");
                let target_depth = *f.loop_rescue_depths.last()
                    .expect("ICE: BreakLoop outside a while loop");
                // CRuby semantics for `break` inside a `begin …
                // ensure … end` inside a `while`: the ensure body
                // runs, the loop exits cleanly carrying the break
                // VALUE (no exception involved). Carrying the value
                // through the ensure-unwind chain needs a break-
                // aware Trap variant plus an `Op::Raise` hook — too
                // large to land alongside the basic break-in-while
                // fix.
                //
                // Defensive interim: refuse the case via `Uncaught`
                // so the script aborts non-zero with a clear error
                // rather than silently diverging. Two tradeoffs the
                // reviewer should know:
                //   1. `Uncaught` is intentionally NON-rescuable
                //      (raise.rs trap_to_exception bypass list).
                //      Routing through a rescuable variant like
                //      `RuntimeError` would let an outer `rescue
                //      => e` silently swallow the limitation marker
                //      while CRuby treats `break` as a structured
                //      transfer that NEVER triggers `rescue` — that
                //      false-positive rescue catch is worse than a
                //      clean abort.
                //   2. With `Uncaught` the ensure body's side
                //      effects ARE skipped (the trap doesn't go
                //      through unwind_with_exception). CRuby runs
                //      them. We accept this regression for the
                //      narrow defensive window; the proper fix
                //      restores both ensure side effects AND the
                //      break value.
                // See SUBSET.md and tests/fixtures/errors/break_through_ensure.
                let has_pending_ensure = f.rescues[target_depth..]
                    .iter().any(|h| h.is_ensure);
                if has_pending_ensure {
                    return Err(self.trap(RubyError::Uncaught {
                        class_name: "NotImplementedError".to_string(),
                        message: "break inside `ensure` of a while loop is not yet supported \
                                  (CRuby runs ensure then exits with the break value; \
                                  rubyrs aborts here so an outer rescue cannot mask the gap). \
                                  Track at SUBSET.md.".to_string(),
                    }));
                }
                while f.rescues.len() > target_depth { f.rescues.pop(); }
                // Same jump arithmetic as `Op::Jump` — dispatch has
                // already advanced `f.ip` past this BreakLoop, so the
                // patched offset reaches the loop's join label.
                f.ip = (f.ip as i32 + off) as usize;
            }
            Op::NextLoop(off) => {
                // Symmetric to BreakLoop: pop dynamic handlers down
                // to the EnterLoop snapshot, then jump. Target is
                // the loop's iter-check label (patched by while
                // codegen) rather than the join, so the loop re-
                // evaluates its condition and either continues or
                // falls through to the natural exit.
                let f = self.frames.last_mut().expect("ICE: NextLoop no frame");
                let target_depth = *f.loop_rescue_depths.last()
                    .expect("ICE: NextLoop outside a while loop");
                // Same `ensure` defense as BreakLoop. CRuby runs
                // the ensure body and continues with the next
                // iteration; doing this correctly needs a next-
                // aware Trap variant + Op::Raise hook. Until then,
                // abort with a non-rescuable Uncaught so an outer
                // `rescue` cannot mask the gap.
                let has_pending_ensure = f.rescues[target_depth..]
                    .iter().any(|h| h.is_ensure);
                if has_pending_ensure {
                    return Err(self.trap(RubyError::Uncaught {
                        class_name: "NotImplementedError".to_string(),
                        message: "next inside `ensure` of a while loop is not yet supported \
                                  (CRuby runs ensure then re-checks the loop guard; \
                                  rubyrs aborts here so an outer rescue cannot mask the gap). \
                                  Track at SUBSET.md.".to_string(),
                    }));
                }
                while f.rescues.len() > target_depth { f.rescues.pop(); }
                f.ip = (f.ip as i32 + off) as usize;
            }
            Op::BinOpInt(kind, rhs) => {
                let a = self.stack.pop().expect("ICE: BinOpInt lhs underflow");
                if let Value::Int(x) = a {
                    // Int / 0 and Int % 0 raise ZeroDivisionError;
                    // Rust's `wrapping_div` / `wrapping_rem` panic
                    // on rhs=0, so guard before delegating to
                    // `apply_int`.
                    if matches!(kind, BinOpKind::Div | BinOpKind::Mod) && rhs == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    self.stack.push(kind.apply_int(x, rhs));
                } else {
                    // Cold path: behave as if a generic `<op>` was dispatched
                    // with rhs boxed as an Int.
                    let b_val = Value::Int(rhs);
                    if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b_val), self.max_value_bytes).map_err(|e| self.trap(e))? {
                        self.stack.push(v);
                    } else if let Some(v) = self.sym_primitive(&a, kind.name(), std::slice::from_ref(&b_val)) {
                        self.stack.push(v);
                    } else {
                        self.stack.push(a);
                        self.stack.push(b_val);
                        let name_id = self.interner.intern(kind.name());
                        self.do_call(name_id, 1, false, u16::MAX)?;
                    }
                }
            }
            Op::BinOp(kind) => {
                let b = self.stack.pop().expect("ICE: BinOp rhs underflow");
                let a = self.stack.pop().expect("ICE: BinOp lhs underflow");
                if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
                    // Same guard as `Op::BinOpInt` — divide / mod
                    // by literal 0 in the Int×Int fast path. Without
                    // this, `n / m` where m happens to be 0 at
                    // runtime would panic the host process.
                    if matches!(kind, BinOpKind::Div | BinOpKind::Mod) && *y == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    self.stack.push(kind.apply_int(*x, *y));
                } else if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b), self.max_value_bytes).map_err(|e| self.trap(e))? {
                    self.stack.push(v);
                } else {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                }
            }
            Op::Return => {
                let f = self.frames.pop().expect("ICE: Return no frame");
                let ret = self.stack.pop().unwrap_or(Value::Nil);
                self.stack.truncate(f.base_sp);
                if f.is_class_body {
                    let cls = self.class_stack.pop().expect("ICE: class_stack empty on class-body return");
                    self.class_visibility_stack.pop();
                    self.stack.push(Value::Class(cls));
                } else if let Some(replacement) = f.swap_return {
                    self.stack.push(replacement);
                } else {
                    self.stack.push(ret);
                }
                if self.frames.is_empty() { return Ok(false); }
            }
            Op::ReturnMethod => {
                // Pop the value but don't pop the frame here —
                // dispatch / dispatch_until's top-of-loop check
                // sees `method_return` and unwinds the right
                // number of frames atomically. Doing it here
                // would skip the block frames between us and the
                // enclosing method.
                let v = self.stack.pop().unwrap_or(Value::Nil);
                self.method_return = Some(v);
            }
        }
        Ok(true)
    }


}
