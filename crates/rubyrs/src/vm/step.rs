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

use super::{primitive_call, vec_nil, Frame, LoopTransferKind, RescueHandler, Vm};

impl Vm {
    /// Lazily allocate the `$LOAD_PATH` Array on first access.
    /// Idempotent — subsequent calls return the same ObjId so
    /// script mutations (`$LOAD_PATH.unshift(dir)`) land on
    /// the slot the require dispatcher later reads.
    /// `check_alloc` enforces heap caps before allocating;
    /// `maybe_gc` may sweep between calls but `Vm.load_path`
    /// is a GC root (rooted in `gc.rs`) so the slot survives.
    pub(crate) fn ensure_load_path(&mut self) -> Result<crate::value::ObjId, Trap> {
        if let Some(id) = self.load_path {
            return Ok(id);
        }
        self.maybe_gc();
        self.check_alloc()?;
        let id = self.heap.alloc(HeapObj::Array(Vec::new()));
        self.load_path = Some(id);
        Ok(id)
    }

    /// The class that owns the current `@@cvar` context, if any.
    /// Resolution mirrors CRuby's "current cref" walk:
    ///   - frame.self_val is `Value::Class(c)` (class body or
    ///     `def self.foo`) → c
    ///   - frame.self_val is `Value::Object(id)` (instance
    ///     method body) → `heap.real_class_of(id)`
    ///   - anything else (toplevel, block-in-toplevel,
    ///     primitive recv) → None, falling through to
    ///     `Vm.toplevel_cvars` at the call site
    pub(crate) fn surrounding_class(&self) -> Option<Rc<Class>> {
        let frame = self.frames.last()?;
        match &frame.self_val {
            Value::Class(c) => Some(c.clone()),
            Value::Object(id) => Some(self.heap.real_class_of(*id)),
            _ => None,
        }
    }

    pub(crate) fn dispatch(&mut self) -> Result<(), Trap> {
        while !self.frames.is_empty() {
            // Non-local return unwind. `Op::ReturnMethod` sets
            // `method_return`; here we honour it by popping any
            // block frames between us and the enclosing method,
            // then popping the method frame and pushing the value
            // as its return. Exit the whole dispatch if we
            // unwound off the bottom of the frame stack.
            if let Some(val) = self.method_return.take() {
                // A non-local `return` from inside an ensure body
                // that was running due to a pending break/next
                // supersedes the structured transfer (CRuby
                // semantics: `return` wins, the break value is
                // dropped). Clear the slot so no EndEnsure in a
                // surviving frame can resume into the now-stale
                // target IP. EndEnsure is reachable on TWO paths
                // (exception unwind via `unwind_with_exception`,
                // AND the loop-transfer walk via
                // `continue_loop_transfer`), but neither runs
                // automatically as we pop frames here — so a
                // stale pending could in principle be consumed
                // later. This clear closes that window.
                self.pending_loop_transfer = None;
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
            Op::LoadConstStrBytes(idx) => {
                // Binary-literal pool lives on the current proto
                // (the interner is UTF-8-only). Clone the Rc<[u8]>
                // slot into a fresh Vec<u8> so each load yields an
                // independent String — mutations via `<<` /
                // `concat` shouldn't bleed into the pool entry that
                // future loads share.
                let bytes: Vec<u8> = {
                    let proto_idx = self.frames.last().expect("ICE: LoadConstStrBytes no frame").proto_idx;
                    self.protos[proto_idx].byte_literals[idx as usize].to_vec()
                };
                self.stack.push(Value::new_str_bytes(bytes));
            }
            #[cfg(feature = "regex")]
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
            #[cfg(feature = "regex")]
            Op::CompileRegex => {
                // Top of stack: a `Value::Str` produced by the
                // InterpolatedRegex build sequence. The assembled
                // pattern is interned so cache lookups can dedup
                // repeated identical expansions (same pattern
                // emitted by different call sites collapses to
                // one compiled Regex).
                let pat_val = self.stack.pop().unwrap_or(Value::Nil);
                let s = match &pat_val {
                    Value::Str(s) => s.clone(),
                    other => {
                        // Defensive: the compiler always emits a
                        // string-producing sequence before this op,
                        // but if a host-defined `to_s`/`String#+`
                        // override returns a non-String we'd rather
                        // raise a Ruby-level TypeError than panic
                        // or miscompile.
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "interpolated regex pattern must be a String, got {}",
                                other.type_name()
                            ),
                        }));
                    }
                };
                // `with_str_lossy` is the borrowed fast path — for
                // valid UTF-8 strings (the common case for regex
                // patterns) the closure sees a `&str` backed by the
                // RubyStr's RefCell content without an owning copy.
                // Cache hits never allocate a String; only the cold
                // path (cache miss → intern → compile) needs to
                // materialise an owned String (interner takes one
                // anyway). Error formatting is also rare and reads
                // through the same borrow.
                let regex_rc = s.with_str_lossy::<Result<Rc<regex::Regex>, Trap>>(|pat| {
                    // ResourceCap: respect `Config::max_symbols` the
                    // same way `String#to_sym` does. Dynamic patterns
                    // generated in a hot loop (e.g.
                    // `1000.times { |i| /#{i}/ }`) would otherwise
                    // grow the interner — and the SymId-keyed
                    // `regex_cache` — without bound. Skip the check
                    // when the pattern is already interned; a cache
                    // hit costs no new symbol.
                    if let Some(max) = self.max_symbols
                        && !self.interner.contains(pat) && self.interner.len() >= max {
                            return Err(self.trap(RubyError::ResourceExhausted {
                                msg: format!("interner exhausted: {} symbols", max),
                            }));
                        }
                    let id = self.interner.intern(pat);
                    if let Some(r) = self.regex_cache.get(&id) {
                        return Ok(r.clone());
                    }
                    let compiled = regex::Regex::new(pat).map_err(|e| {
                        self.trap(RubyError::SyntaxError {
                            msg: format!("invalid regex /{}/: {}", pat, e),
                        })
                    })?;
                    let rc = Rc::new(compiled);
                    self.regex_cache.insert(id, rc.clone());
                    Ok(rc)
                })?;
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
                // `@foo` reads route to whichever table `self`
                // carries: instance ivars for `Value::Object`,
                // class-level ivars for `Value::Class` (the
                // "class instance variable" CRuby spelling, used
                // by `module Tilt; @default = ...` patterns).
                // Anything else returns nil — matches CRuby's
                // "uninitialized ivar reads as nil" rule.
                let self_val = self.frames.last().expect("ICE: LoadIvar no frame").self_val.clone();
                let v = match &self_val {
                    Value::Object(id) => self.heap.instance(*id).ivars.get(&name_id).cloned().unwrap_or(Value::Nil),
                    Value::Class(c) => c.ivars.borrow().get(&name_id).cloned().unwrap_or(Value::Nil),
                    _ => Value::Nil,
                };
                self.stack.push(v);
            }
            Op::StoreIvar(name_id) => {
                let v = self.stack.pop().expect("ICE: StoreIvar stack underflow");
                let self_val = self.frames.last().expect("ICE: StoreIvar no frame").self_val.clone();
                match &self_val {
                    Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, v); }
                    Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, v); }
                    _ => { /* drop — CRuby raises but the toplevel/primitive cases are rare */ }
                }
            }
            Op::LoadCvar(name_id) => {
                // Surrounding class resolution order:
                //   - class body / `def self.foo`: self_val IS the
                //     class → use it directly.
                //   - instance method: self_val is an Object →
                //     `heap.real_class_of` gives the class.
                //   - toplevel / block-in-toplevel: no class on
                //     hand → fall back to Vm.toplevel_cvars.
                // Tier 1: no hierarchy walk — each class's
                // `class_vars` is independent of parent/child.
                let cls_opt = self.surrounding_class();
                let v = match cls_opt {
                    Some(cls) => cls.class_vars.borrow().get(&name_id).cloned().unwrap_or(Value::Nil),
                    None => self.toplevel_cvars.get(&name_id).cloned().unwrap_or(Value::Nil),
                };
                self.stack.push(v);
            }
            Op::StoreCvar(name_id) => {
                let v = self.stack.pop().expect("ICE: StoreCvar stack underflow");
                let cls_opt = self.surrounding_class();
                match cls_opt {
                    Some(cls) => { cls.class_vars.borrow_mut().insert(name_id, v); }
                    None => { self.toplevel_cvars.insert(name_id, v); }
                }
            }
            Op::IncIvarNoPush(name_id) => {
                // `@x = @x + 1` fast path, statement form. Mirrors
                // Op::IncIvar but discards the result. Class-level
                // ivars routed via `Value::Class` so the same
                // pattern in a class method bumps the right table.
                let self_val = self.frames.last().expect("ICE: IncIvarNoPush no frame").self_val.clone();
                let cur = match &self_val {
                    Value::Object(id) => self.heap.instance(*id).ivars.get(&name_id).cloned(),
                    Value::Class(c) => c.ivars.borrow().get(&name_id).cloned(),
                    _ => None,
                };
                let new_v = match cur {
                    Some(Value::Int(n)) => Some(Value::Int(n.wrapping_add(1))),
                    Some(_) | None => {
                        // Slow path — call `+`.
                        let cur_v = cur.unwrap_or(Value::Nil);
                        self.stack.push(cur_v);
                        self.stack.push(Value::Int(1));
                        let plus_id = self.interner.intern("+");
                        self.do_call(plus_id, 1, false, u16::MAX)?;
                        Some(self.stack.pop().unwrap_or(Value::Nil))
                    }
                };
                if let Some(v) = new_v {
                    match &self_val {
                        Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, v); }
                        Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, v); }
                        _ => { /* drop */ }
                    }
                }
            }
            Op::IncIvar(name_id) => {
                // `@x = @x + 1` fast path, expression form. Same as
                // IncIvarNoPush but leaves the new value on stack.
                let self_val = self.frames.last().expect("ICE: IncIvar no frame").self_val.clone();
                let cur = match &self_val {
                    Value::Object(id) => self.heap.instance(*id).ivars.get(&name_id).cloned(),
                    Value::Class(c) => c.ivars.borrow().get(&name_id).cloned(),
                    _ => None,
                };
                let new_v: Value = match cur {
                    Some(Value::Int(n)) => {
                        let nv = Value::Int(n.wrapping_add(1));
                        match &self_val {
                            Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, nv.clone()); }
                            Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, nv.clone()); }
                            _ => {}
                        }
                        nv
                    }
                    _ => {
                        // Slow path: replicate full `@x = @x + 1`.
                        let cur_v = cur.unwrap_or(Value::Nil);
                        self.stack.push(cur_v);
                        self.stack.push(Value::Int(1));
                        let plus_id = self.interner.intern("+");
                        self.do_call(plus_id, 1, false, u16::MAX)?;
                        let v = self.stack.last().expect("ICE: IncIvar slow path no result").clone();
                        match &self_val {
                            Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, v.clone()); }
                            Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, v.clone()); }
                            _ => {}
                        }
                        // Slow path already left value on stack via do_call result.
                        return Ok(true);
                    }
                };
                self.stack.push(new_v);
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
                    // ADR 0017 Rule 1+2: the ENV map a script sees is
                    // exactly the one the host provided via
                    // `Config::env` — never the host process's real
                    // env vars. `None` (default) → empty Hash; the
                    // CLI binary `rubyrs` populates from
                    // `std::env::vars()` to preserve `rubyrs script.rb`
                    // ergonomics. Cached for the lifetime of the Vm
                    // so all `ENV` reads see a single object — writes
                    // via `ENV[k] = v` mutate the snapshot but not
                    // anything host-side (documented divergence).
                    let id = if let Some(id) = self.env_hash {
                        id
                    } else {
                        // Order matters: do the fallible
                        // `maybe_gc` + `check_alloc()?` step BEFORE
                        // consuming `env_override`. Calling `take()`
                        // first and then trapping on the heap cap
                        // would permanently drop the host-injected
                        // ENV map (the `?` early-return preserves no
                        // override state), and any subsequent `ENV`
                        // access would rebuild as empty — a silent
                        // capability loss the host has no way to
                        // recover from.
                        self.maybe_gc();
                        self.check_alloc()?;
                        // ADR 0017 Rule 1 requires deterministic
                        // iteration. `Config::env: HashMap` has
                        // randomised hash order, so collect the
                        // entries into a key-sorted Vec before
                        // materialising the Ruby Hash (which preserves
                        // insertion order); otherwise `ENV.each` /
                        // `ENV.to_a` / `ENV.inspect` would vary across
                        // runs even for identical host injection.
                        //
                        // `take()` consumes the override on first
                        // build (now that we know alloc will succeed):
                        // once the Ruby Hash is allocated it is the
                        // canonical ENV, so keeping a second copy on
                        // `Vm` would just retain duplicate memory and
                        // force per-entry String clones every time.
                        // Moving the Strings into `Value::new_str`
                        // avoids both.
                        let pairs: Vec<(Value, Value)> = match self.env_override.take() {
                            Some(map) => {
                                let mut entries: Vec<(String, String)> = map.into_iter().collect();
                                entries.sort_by(|a, b| a.0.cmp(&b.0));
                                entries
                                    .into_iter()
                                    .map(|(k, v)| (Value::new_str(k), Value::new_str(v)))
                                    .collect()
                            }
                            None => Vec::new(),
                        };
                        let id = self.heap.alloc(HeapObj::Hash(pairs));
                        self.env_hash = Some(id);
                        id
                    };
                    Value::Hash(id)
                } else {
                    // CRuby raises `NameError: uninitialized constant
                    // <name>` for missing constants — silent-nil here
                    // masks real user errors AND lets downstream code
                    // see a Nil where a class/module was expected
                    // (e.g. `nil.new` instead of NameError, which is
                    // confusing to debug). Match CRuby.
                    //
                    // Op-write read positions (`FOO ||= ...`) need
                    // silent-nil — they use `Op::LoadConstOrNil`
                    // instead.
                    let name = self.interner.resolve(name_id).clone();
                    return Err(self.trap(crate::error::RubyError::NameError {
                        msg: format!("uninitialized constant {}", name),
                    }));
                };
                self.stack.push(v);
            }
            Op::LoadConstOrNil(name_id) => {
                // Silent-nil variant of `LoadConst`. See the op's
                // doc comment in bytecode.rs — only the AST `||=`
                // read position emits this. No ENV intercept:
                // `ENV ||= ...` is not idiomatic, and any sane
                // ENV access goes through `LoadConst` where the
                // intercept lives.
                let v = if let Some(c) = self.classes.get(&name_id).cloned() {
                    Value::Class(c)
                } else if let Some(v) = self.constants.get(&name_id).cloned() {
                    v
                } else {
                    Value::Nil
                };
                self.stack.push(v);
            }
            Op::LoadGlobal(name_id) => {
                // Special-globals intercept. `$$` is the canonical
                // case from tilt/template.rb (`"...-#{$$}"`); add
                // others here as real codebases need them. `$0`
                // returns the script's filename (we use the top
                // frame's proto filename, which Runtime::eval set
                // to whatever the host passed).
                let name = self.interner.resolve(name_id).clone();
                // `$1`, `$2`, ..., `$10`, `$11`, ... — numbered
                // capture references, written by ast.rs as
                // `GVarRead("$N")` (the AST arm for
                // `NumberedReferenceReadNode`). N-th group from the
                // most recent successful match, or nil if no match
                // or the group did not participate. CRuby allows
                // any positive index (`$10` reads the 10th group),
                // so accept all digits after `$` rather than just
                // a single one. `$0` is excluded — it's a separate
                // global (the script filename) handled below.
                // Branched out of the `match` below so it can stay
                // strictly statement-shaped (no allocator call
                // needed — just clones a String).
                #[cfg(feature = "regex")]
                if name.len() >= 2
                    && name.starts_with('$')
                    && name.as_bytes()[1] != b'0'
                    && name.as_bytes()[1..].iter().all(|c| c.is_ascii_digit())
                {
                    let n: usize = name[1..].parse().unwrap_or(0);
                    let v = match &self.last_match {
                        Some(m) if n >= 1 => match m.caps.get(n - 1) {
                            Some(Some(cap)) => Value::new_str(cap.clone()),
                            _ => Value::Nil,
                        },
                        _ => Value::Nil,
                    };
                    self.stack.push(v);
                    return Ok(true);
                }
                // `$~` — MatchData of the last successful match,
                // or nil. Materialises a fresh MatchData instance
                // on each read (same `@whole`/`@caps` shape as
                // `String#match`'s return value). Branched out so
                // we can call `maybe_gc` + `check_alloc?` cleanly.
                #[cfg(feature = "regex")]
                if &*name == "$~" {
                    // Borrow first: avoid cloning the whole
                    // `LastMatch` (Vec + every capture String) just
                    // to materialise. We only need to clone the
                    // specific strings we hand to `Value::new_str`.
                    let v = if self.last_match.is_some() {
                        // Materialise capture Values up front so the
                        // immutable borrow of `last_match` is dropped
                        // before any `&mut self` calls (`maybe_gc`,
                        // `check_alloc`, `heap.alloc`).
                        let caps: Vec<Value> = self.last_match.as_ref().unwrap()
                            .caps.iter()
                            .map(|c| match c {
                                Some(s) => Value::new_str(s.clone()),
                                None => Value::Nil,
                            })
                            .collect();
                        let whole_str = self.last_match.as_ref().unwrap().whole.clone();
                        self.materialize_match_data(whole_str, caps)?
                    } else {
                        Value::Nil
                    };
                    self.stack.push(v);
                    return Ok(true);
                }
                let v = match &*name {
                    // ADR 0017 Rule 1: the script never reads the
                    // host process's real PID. `Config::pid = Some(n)`
                    // → `$$` returns `n`; `None` (default) → returns
                    // `0` as a sentinel. CLI binary `rubyrs` fills
                    // this from `std::process::id()` to preserve
                    // CRuby parity.
                    "$$" => Value::Int(self.pid.unwrap_or(0)),
                    "$0" => {
                        // Bottommost frame = script entry; its
                        // proto's filename is the script's top-level
                        // filename (or "<inline>" for eval calls).
                        let name = self.frames.first()
                            .map(|f| self.protos[f.proto_idx].filename.to_string())
                            .unwrap_or_else(|| "-".to_string());
                        Value::new_str(name)
                    }
                    // `$LOAD_PATH` / `$:` — the require-search-path
                    // Array. Lazily materialised on first read so
                    // scripts that don't touch it pay no startup
                    // cost. The Array is mutable and persistent —
                    // `$LOAD_PATH.unshift(dir)` adds an entry that
                    // subsequent `require` calls consult (see
                    // `Vm::ruby_source_candidates`).
                    "$LOAD_PATH" | "$:" => {
                        let id = self.ensure_load_path()?;
                        Value::Array(id)
                    }
                    _ => self.globals.get(&name_id).cloned().unwrap_or(Value::Nil),
                };
                self.stack.push(v);
            }
            Op::StoreGlobal(name_id) => {
                // `$foo = expr` — pop the value and store. In
                // statement position the compiler does NOT emit a
                // preceding Dup (mirrors ConstWrite/IVarWrite); in
                // expression position it emits Dup first, so the
                // value remains on the stack as the assignment's
                // result. Special-global writes (`$$ = 42`) are
                // silently accepted into `Vm.globals` but the next
                // read still intercepts and returns the computed
                // value — a documented spike divergence.
                let v = self.stack.pop().expect("ICE: StoreGlobal stack underflow");
                self.globals.insert(name_id, v);
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
            Op::ApplySuper(name_id) => {
                // Pop assembled args Array and drain elements
                // into a Vec<Value>. From here the super-
                // lookup path is identical to Op::Super; the
                // only difference is how the args Vec was
                // produced (splat-assembled at the call site
                // vs. pushed individually by Op::Super).
                let args_val = self.stack.pop().expect("ICE: ApplySuper without args slot");
                let args: Vec<Value> = match args_val {
                    Value::Array(aid) => self.heap.array(aid).clone(),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("ApplySuper expected Array args, got {}", other.type_name()),
                    })),
                };
                let (m, self_val) = self.super_lookup(name_id)?;
                self.invoke_method(m, self_val, args)?;
            }
            Op::Super(name_id, argc) => {
                let split = self.stack.len() - argc as usize;
                let args: Vec<Value> = self.stack.drain(split..).collect();
                let (m, self_val) = self.super_lookup(name_id)?;
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
                        // Source name not in the user-Method table.
                        // Before raising NameError, check whether the
                        // surrounding class's primitive whitelist
                        // responds to it (`Symbol#name`, `Integer#+`,
                        // ...). If so, synthesise a forwarder Method
                        // whose body is `LoadSelf; LoadLocal(0);
                        // ApplyCall(old_id, ...); Return` — i.e.
                        // call the primitive on `self` with any
                        // forwarded args. This is what lets the
                        // msgpack-ruby `lib/msgpack/symbol.rb`
                        // `alias_method :to_msgpack_ext, :name`
                        // shape work without rewriting upstream.
                        // Variadic forwarding via a rest param so
                        // arities other than 0 also forward
                        // correctly.
                        let cls_ref = self.class_stack.last().cloned();
                        if let Some(cls) = &cls_ref
                            && self.primitive_class_responds_to(&cls.name.borrow(), old_id) {
                            let synth = self.synth_primitive_forwarder(cls, old_id);
                            cls.methods.borrow_mut().insert(new_id, synth);
                            self.method_gen = self.method_gen.wrapping_add(1);
                            self.stack.push(Value::Nil);
                            return Ok(true);
                        }
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
                            .map(|c| format!("class `{}'", c.name.borrow()))
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
            Op::AliasSingletonMethod(new_id, old_id) => {
                // `alias new old` inside `class << X` body.
                // Mirrors Op::AliasMethod's shape but resolves
                // `old` via `lookup_class_singleton_method` (walks
                // the surrounding class's singleton_methods chain
                // including its superclass chain) and installs into
                // `singleton_methods`, not `methods`. Outside a
                // class body, falls back to toplevel_methods like
                // the regular alias op — toplevel `class << X` is
                // legal but rarely used and the surface area is
                // identical there.
                let existing = if let Some(cls) = self.class_stack.last() {
                    self.lookup_class_singleton_method(cls, old_id)
                } else {
                    self.toplevel_methods.get(&old_id).cloned()
                };
                let m = match existing {
                    Some(m) => m,
                    None => {
                        let name = self.interner.resolve(old_id).to_string();
                        // Use the same "class `Foo'" context wording
                        // as Op::AliasMethod's NameError so the two
                        // sites diff cleanly. (CRuby itself spells
                        // these differently in some cases but the
                        // singleton/instance distinction is rarely
                        // load-bearing in real error logs.)
                        let ctx = self.class_stack.last()
                            .map(|c| format!("class `{}'", c.name.borrow()))
                            .unwrap_or_else(|| "main".to_string());
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method `{}' for {}", name, ctx),
                        }));
                    }
                };
                if let Some(cls) = self.class_stack.last() {
                    cls.singleton_methods.borrow_mut().insert(new_id, m);
                } else {
                    self.toplevel_methods.insert(new_id, m);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::SingletonChainPrepend => {
                // Pop the module/class value and push it onto the
                // surrounding class's `singleton_prepends` chain.
                // The AST recogniser is purely syntactic (it matches
                // any `class << self; prepend Mod; end` regardless
                // of enclosing scope), so the install-target check
                // is enforced HERE at runtime: use
                // `class_stack.last()` when present; trap with
                // SyntaxError otherwise (toplevel / class-eval
                // contexts where there's no class on the stack).
                //
                // CRuby parity:
                // 1. The arg must be a Module — Classes (i.e.
                //    `is_module == false`) raise TypeError. Plain
                //    non-Class values too.
                // 2. Idempotency is ancestor-chain-aware, NOT just
                //    direct-vec — if `M` is already reachable
                //    transitively (e.g. via a prepended-of-prepend
                //    chain), the explicit `prepend M` is a no-op.
                //    Without this, the chain would reorder and
                //    method resolution would diverge from CRuby.
                let arg = self.stack.pop().expect("ICE: SingletonChainPrepend with empty stack");
                let src = match arg {
                    Value::Class(c) if c.is_module => c,
                    Value::Class(_) => return Err(self.trap(RubyError::TypeError {
                        msg: "wrong argument type Class (expected Module)".into(),
                    })),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Module)",
                            other.type_name(),
                        ),
                    })),
                };
                // Install target resolution: prefer the lexical
                // class body on `class_stack` (the common case —
                // `class C; class << self; prepend M; end; end`).
                // Fall back to the current frame's `self` when
                // `self` is itself a Class — that covers the
                // method-body case (`class C; def self.install!;
                // class << self; prepend M; end; end; end`), where
                // CRuby installs on C's eigenclass because `self`
                // inside `install!` is C. Only raise when neither
                // path yields a class — toplevel / instance-method
                // contexts where rubyrs doesn't model the
                // eigenclass distinctly.
                let target = self.class_stack.last().cloned().or_else(|| {
                    self.frames.last().and_then(|f| match &f.self_val {
                        Value::Class(c) => Some(c.clone()),
                        _ => None,
                    })
                });
                let target = match target {
                    Some(c) => c,
                    None => {
                        return Err(self.trap(RubyError::SyntaxError {
                            msg: "`class << self; prepend Mod; end` is not supported outside a class/module body (no singleton-class install target — main's / instance eigenclasses not modelled in rubyrs)".into(),
                        }));
                    }
                };
                // Ancestor-aware dedup: walk every module
                // already in `singleton_prepends`, recursing
                // through each one's own prepends/includes,
                // and skip insertion if `src` is reachable
                // anywhere. Matches the instance-side `prepend`
                // recogniser's `class_is_a` gate.
                if !super::lookup::singleton_chain_contains(&target, &src) {
                    target.singleton_prepends.borrow_mut().insert(0, src);
                    self.method_gen = self.method_gen.wrapping_add(1);
                }
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
            Op::DefClass(name_id, p_idx, qual_id) | Op::DefModule(name_id, p_idx, qual_id) => {
                // `DefModule` distinguishes the source keyword
                // (`module X; end`) so the resulting Class shell
                // gets `is_module: true`. Otherwise identical to
                // DefClass — same body-frame push, same constant-
                // alias plumbing.
                let is_module = matches!(op, Op::DefModule(..));
                // Pop superclass (Nil for "default to Object", a Class for `class Foo < Bar`).
                let parent_val = self.stack.pop().expect("ICE: DefClass without superclass slot");
                let parent = match parent_val {
                    Value::Class(c) => Some(c),
                    _ => None, // Nil -> default; treat as no explicit parent for now
                };
                // Use the qualified name for `Class.name` when the
                // class is being defined inside a module/class body
                // (`module Foo; class Bar; ...; end; end` →
                // qual_id = SymId for "Foo::Bar"). Top-level
                // classes leave the third arg as the u32::MAX
                // sentinel and fall back to the bare name. Only
                // takes effect on first creation — class re-open
                // (`or_insert_with` short-circuits) keeps whatever
                // name the original define stamped.
                let name_str = if qual_id.0 == u32::MAX {
                    self.interner.resolve(name_id).to_string()
                } else {
                    self.interner.resolve(qual_id).to_string()
                };
                let cls = self.classes.entry(name_id).or_insert_with(|| Rc::new(Class {
                    name: std::cell::RefCell::new(name_str),
                    is_module,
                    ivars: RefCell::new(HashMap::new()),
                    methods: RefCell::new(HashMap::new()),
                    singleton_methods: RefCell::new(HashMap::new()),
                    superclass: RefCell::new(parent.clone()),
                    includes: RefCell::new(Vec::new()),
                    prepends: RefCell::new(Vec::new()),
                    singleton_prepends: RefCell::new(Vec::new()),
                    class_vars: RefCell::new(HashMap::new()),
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
                    is_class_body: true, swap_return: None, block_arg: None, defining_class: None, is_block: false, n_given_positional: 0, rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
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
                // by — for bare `rescue` that's `StandardError`, for
                // `rescue Foo::Bar` the qualified-form SymId stamped
                // by the lexical dual-write. We resolve through the
                // same fallback chain as `Op::LoadConst`: `classes`
                // first (bare names + dual-write copies), then
                // `constants` (where user `Foo::Bar = …` aliases land).
                // If neither hits, `filter_class` stays `None` and
                // the handler fails every match check — closer to
                // CRuby than silently catching everything.
                let filter = self.classes.get(&filter_sym).cloned().or_else(|| {
                    match self.constants.get(&filter_sym) {
                        Some(Value::Class(c)) => Some(c.clone()),
                        _ => None,
                    }
                });
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
                let f = self.frames.last().expect("ICE: EnterLoop no frame");
                let depth = f.rescues.len();
                let stack_depth = self.stack.len();
                let f = self.frames.last_mut().expect("ICE: EnterLoop no frame");
                f.loop_rescue_depths.push(depth);
                f.loop_stack_depths.push(stack_depth);
            }
            Op::ExitLoop => {
                let f = self.frames.last_mut().expect("ICE: ExitLoop no frame");
                f.loop_rescue_depths.pop()
                    .expect("ICE: ExitLoop with empty loop_rescue_depths");
                f.loop_stack_depths.pop()
                    .expect("ICE: ExitLoop with empty loop_stack_depths");
            }
            Op::BreakLoop(off) => {
                // Compute the loop-target IP at the source site (the
                // dispatcher has already advanced f.ip past this op,
                // so the patched offset lands on the loop's join).
                let f = self.frames.last().expect("ICE: BreakLoop no frame");
                let target_depth = *f.loop_rescue_depths.last()
                    .expect("ICE: BreakLoop outside a while loop");
                let target_ip = (f.ip as i32 + off) as usize;
                // Break value was pushed by the compiler immediately
                // before this op. Take it off so it doesn't pollute
                // the ensure-body stack we may be about to enter,
                // and so we can re-push it once the transfer lands.
                let value = self.stack.pop().expect("ICE: BreakLoop with no value on stack");
                self.begin_loop_transfer(LoopTransferKind::Break { value }, target_ip, target_depth)?;
            }
            Op::NextLoop(off) => {
                // Symmetric to BreakLoop: jumps to iter-check instead
                // of join; no value to carry (while has no iteration
                // value).
                let f = self.frames.last().expect("ICE: NextLoop no frame");
                let target_depth = *f.loop_rescue_depths.last()
                    .expect("ICE: NextLoop outside a while loop");
                let target_ip = (f.ip as i32 + off) as usize;
                self.begin_loop_transfer(LoopTransferKind::Next, target_ip, target_depth)?;
            }
            Op::EndEnsure => {
                // Tail of an ensure handler body. Two paths:
                //   - Loop-transfer in flight: `pending_loop_transfer`
                //     is Some because BreakLoop/NextLoop kicked off a
                //     walk through this ensure. Resume the walk.
                //   - Normal exception unwind: the ensure was entered
                //     by `unwind_with_exception` which pushed the
                //     exception onto the operand stack. Pop and
                //     re-raise so unwind continues.
                if self.pending_loop_transfer.is_some() {
                    self.continue_loop_transfer()?;
                } else {
                    // Stack invariant on the exception path: the
                    // unwinder pushed exactly one exception value
                    // when entering this ensure handler, and the
                    // ensure body is compile_stmt-balanced (every
                    // statement Pops its result). An empty stack
                    // here means stack-balance regression — surface
                    // it loudly rather than silently materialising
                    // a Nil exception.
                    let v = self.stack.pop()
                        .expect("ICE: EndEnsure with empty stack on exception path");
                    let exc = self.normalize_exception(v);
                    self.unwind_with_exception(exc)?;
                }
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
