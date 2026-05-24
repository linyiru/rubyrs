use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::rc::Rc;

use std::io::Write;

use crate::bytecode::{Op, Proto};
use crate::error::{RubyError, Span, Trap, TrapFrame};
use crate::heap::{Heap, HeapObj};
use crate::intern::{Interner, SymId};
use crate::value::{BlockHandle, Class, Instance, Method, Value};

// ---------- VM ----------

pub(crate) struct Frame {
    pub(crate) proto_idx: usize,
    pub(crate) ip: usize,
    pub(crate) locals: Rc<RefCell<Vec<Value>>>,
    pub(crate) self_val: Value,
    pub(crate) base_sp: usize,
    pub(crate) is_class_body: bool,
    pub(crate) swap_return: Option<Value>,
    pub(crate) block_arg: Option<Rc<BlockHandle>>,
    pub(crate) rescues: Vec<RescueHandler>,
}

pub(crate) struct RescueHandler {
    pub(crate) handler_ip: usize,
    pub(crate) stack_depth: usize,
    pub(crate) bind_slot: Option<u16>,
    /// When true this entry was emitted by `Op::PushEnsure` and the
    /// unwinder pushes the exception onto the operand stack (rather than
    /// binding to a local). The ensure body re-raises with `Op::Raise`.
    pub(crate) is_ensure: bool,
}

pub(crate) type HostFn = dyn Fn(&[Value]) -> Result<Value, Trap>;

pub(crate) struct Vm {
    pub(crate) protos: Vec<Proto>,
    pub(crate) interner: Interner,
    pub(crate) classes: HashMap<SymId, Rc<Class>>,
    pub(crate) toplevel_methods: HashMap<SymId, Rc<Method>>,
    pub(crate) host_fns: HashMap<SymId, Rc<HostFn>>,
    pub(crate) class_stack: Vec<Rc<Class>>,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) heap: Heap,
    /// Native-code holding pen for heap values across GC points; see ADR 0005.
    pub(crate) pinned: Vec<Value>,
    pub(crate) stdout: Box<dyn std::io::Write>,
    pub(crate) stress_gc: bool,
    /// Remaining fuel; `Some(0)` means exhausted, `None` means unlimited.
    /// Decremented per op dispatched. Configured by `Config::fuel`.
    pub(crate) fuel: Option<u64>,
    /// Maximum simultaneously-live frames. `frames.push()` checks this
    /// against `frames.len()` before pushing. Default `None` is unlimited.
    pub(crate) max_frames: Option<usize>,
    /// Single-slot monomorphic inline cache for method dispatch on
    /// `Value::Object`. Holds the (class identity, method name, resolved
    /// `Method`) of the most recent successful lookup; subsequent calls
    /// that match both fields skip the `HashMap<SymId, _>::get`.
    ///
    /// Invalidated whenever a method is (re)defined on any class —
    /// `Op::DefMethod` clears it. For our subset (method redefinition is
    /// rare in hot loops) a single slot wins most of the time and a
    /// per-call-site cache is deferred until measurement says otherwise.
    pub(crate) call_cache: Option<(usize, SymId, Rc<Method>)>,
}

impl Vm {
    pub(crate) fn new(protos: Vec<Proto>, interner: Interner) -> Self {
        Vm {
            protos,
            interner,
            classes: HashMap::new(),
            toplevel_methods: HashMap::new(),
            host_fns: HashMap::new(),
            class_stack: vec![],
            stack: Vec::with_capacity(1024),
            frames: vec![],
            heap: Heap::new(),
            pinned: Vec::new(),
            stdout: Box::new(std::io::stdout()),
            stress_gc: env::var("STRESS_GC").is_ok(),
            fuel: None,
            max_frames: None,
            call_cache: None,
        }
    }

    /// Look up a method on `cls`, walking its superclass chain. Consults
    /// the single-slot inline cache first; on miss caches the result.
    #[inline]
    pub(crate) fn lookup_method_cached(&mut self, cls: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
        let class_ptr = Rc::as_ptr(cls) as usize;
        if let Some((cached_ptr, cached_name, cached_method)) = &self.call_cache {
            if *cached_ptr == class_ptr && *cached_name == name_id {
                return Some(cached_method.clone());
            }
        }
        let mut current = cls.clone();
        loop {
            if let Some(m) = current.methods.borrow().get(&name_id).cloned() {
                self.call_cache = Some((class_ptr, name_id, m.clone()));
                return Some(m);
            }
            let parent = current.superclass.borrow().clone();
            match parent {
                Some(p) => current = p,
                None => return None,
            }
        }
    }
}

/// `child` is-a `ancestor` if `ancestor` appears anywhere in `child`'s
/// superclass chain (or `child == ancestor`).
#[allow(dead_code)] // wired up in the next commit (rescue ClassName filter)
pub(crate) fn class_is_a(child: &Rc<Class>, ancestor: &Rc<Class>) -> bool {
    let mut current = child.clone();
    loop {
        if Rc::ptr_eq(&current, ancestor) { return true; }
        let parent = current.superclass.borrow().clone();
        match parent {
            Some(p) => current = p,
            None => return false,
        }
    }
}

impl Vm {

    pub(crate) fn run(&mut self, entry: usize) -> Result<Value, Trap> {
        let proto = &self.protos[entry];
        let n_locals = proto.n_locals as usize;
        self.frames.push(Frame {
            proto_idx: entry,
            ip: 0,
            locals: Rc::new(RefCell::new(vec_nil(n_locals))),
            self_val: Value::Nil,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, rescues: vec![],
        });
        self.dispatch()?;
        Ok(self.stack.pop().unwrap_or(Value::Nil))
    }

    pub(crate) fn dispatch(&mut self) -> Result<(), Trap> {
        while !self.frames.is_empty() {
            let (proto_idx, ip) = {
                let f = self.frames.last().expect("ICE: dispatch with empty frame stack");
                (f.proto_idx, f.ip)
            };
            let op = self.protos[proto_idx].code[ip];
            self.frames.last_mut().expect("ICE: frame disappeared").ip += 1;
            if !self.step(op, proto_idx)? { return Ok(()); }
        }
        Ok(())
    }

    /// Decrement fuel; on exhaustion return a `ResourceExhausted` trap.
    #[inline]
    pub(crate) fn check_fuel(&mut self) -> Result<(), Trap> {
        if let Some(f) = self.fuel {
            if f == 0 {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: "out of fuel".to_string(),
                }));
            }
            self.fuel = Some(f - 1);
        }
        Ok(())
    }

    /// Check the heap can accept another object. Call after `maybe_gc`
    /// (so the limit applies to *live* objects, not transient garbage).
    #[inline]
    pub(crate) fn check_alloc(&self) -> Result<(), Trap> {
        if let Some(max) = self.heap.max_live {
            if self.heap.live_count >= max {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("heap exhausted: {} live objects (max {})", self.heap.live_count, max),
                }));
            }
        }
        Ok(())
    }

    /// Check the frame stack can accept another frame.
    #[inline]
    pub(crate) fn check_frames(&self) -> Result<(), Trap> {
        if let Some(max) = self.max_frames {
            if self.frames.len() >= max {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("stack level too deep ({} frames, max {})", self.frames.len(), max),
                }));
            }
        }
        Ok(())
    }

    /// Build a Trap with a backtrace snapshot taken at the current frame stack.
    pub(crate) fn trap(&self, err: RubyError) -> Trap {
        let mut bt = Vec::with_capacity(self.frames.len());
        for f in self.frames.iter().rev() {
            let proto = &self.protos[f.proto_idx];
            let op_ip = if f.ip == 0 { 0 } else { f.ip - 1 };
            let span = proto.op_spans.get(op_ip).copied().unwrap_or(Span::ZERO);
            bt.push(TrapFrame {
                filename: proto.filename.clone(),
                method: Rc::from(proto.name.as_str()),
                span,
            });
        }
        Trap { err, backtrace: bt }
    }

    pub(crate) fn do_call(&mut self, name_id: SymId, argc: usize, no_recv: bool) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before do_call receiver"))
        };

        if no_recv {
            if let Some(res) = self.builtin_call(&name, &args) {
                self.stack.push(res?);
                return Ok(());
            }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = host(&args)?;
                self.stack.push(v);
                return Ok(());
            }
            let self_val = self.frames.last().expect("ICE: do_call with empty frames").self_val.clone();
            if let Value::Object(id) = &self_val {
                let cls = self.heap.instance(*id).class.clone();
                if let Some(m) = self.lookup_method_cached(&cls, name_id) {
                    self.invoke_method(m, self_val.clone(), args)?;
                    return Ok(());
                }
            }
            if let Some(m) = self.toplevel_methods.get(&name_id).cloned() {
                self.invoke_method(m, self_val, args)?;
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                method: name.to_string(), recv_type: self_val.type_name(),
            }));
        }

        let recv = recv.expect("ICE: receiver missing");

        if let Some(v) = primitive_call(&recv, &name, &args) {
            self.stack.push(v);
            return Ok(());
        }
        if let Some(v) = self.sym_primitive(&recv, &name, &args) {
            self.stack.push(v);
            return Ok(());
        }

        let new_id = self.interner.intern("new");
        if name_id == new_id {
            if let Value::Class(cls) = &recv {
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Instance(Instance {
                    class: cls.clone(),
                    ivars: HashMap::new(),
                }));
                let obj = Value::Object(id);
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_cached(&cls, init_id) {
                    self.invoke_method(m, obj.clone(), args)?;
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    self.stack.push(obj);
                }
                return Ok(());
            }
        }

        if let Value::Object(id) = &recv {
            let cls = self.heap.instance(*id).class.clone();
            if let Some(m) = self.lookup_method_cached(&cls, name_id) {
                self.invoke_method(m, recv.clone(), args)?;
                return Ok(());
            }
        }
        if let Some(v) = self.collection_call(&recv, &name, &args) {
            self.stack.push(v);
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }

    pub(crate) fn collection_call(&mut self, recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
        match recv {
            Value::Array(id) => {
                let id = *id;
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.array(id).len() as i64)),
                    ("push", [v]) | ("<<", [v]) => {
                        self.heap.array_mut(id).push(v.clone());
                        Some(Value::Array(id))
                    }
                    ("[]", [Value::Int(i)]) => {
                        let a = self.heap.array(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                        Some(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                    }
                    ("[]=", [Value::Int(i), v]) => {
                        let a = self.heap.array_mut(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i } as usize;
                        while a.len() <= idx { a.push(Value::Nil); }
                        a[idx] = v.clone();
                        Some(v.clone())
                    }
                    ("first", []) => Some(self.heap.array(id).first().cloned().unwrap_or(Value::Nil)),
                    ("last", []) => Some(self.heap.array(id).last().cloned().unwrap_or(Value::Nil)),
                    ("empty?", []) => Some(Value::Bool(self.heap.array(id).is_empty())),
                    _ => None,
                }
            }
            Value::Hash(id) => {
                let id = *id;
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.hash(id).len() as i64)),
                    ("[]", [k]) => {
                        let h = self.heap.hash(id);
                        for (key, val) in h {
                            if key.ruby_eq(k, &self.heap) { return Some(val.clone()); }
                        }
                        Some(Value::Nil)
                    }
                    ("[]=", [k, v]) => {
                        // Need a way to compare without borrowing heap while mutating.
                        // Snapshot positions first.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        let h = self.heap.hash_mut(id);
                        if let Some(p) = pos {
                            h[p].1 = v.clone();
                        } else {
                            h.push((k.clone(), v.clone()));
                        }
                        Some(v.clone())
                    }
                    ("empty?", []) => Some(Value::Bool(self.heap.hash(id).is_empty())),
                    ("keys", []) => {
                        let keys: Vec<Value> = self.heap.hash(id).iter().map(|(k, _)| k.clone()).collect();
                        self.maybe_gc();
                        // check_alloc would need a `?`; collection_call returns Option,
                        // so we skip the cap check here. Embedders should set
                        // max_live with a small slack to account for these
                        // derived allocations.
                        let nid = self.heap.alloc(HeapObj::Array(keys));
                        Some(Value::Array(nid))
                    }
                    ("values", []) => {
                        let vals: Vec<Value> = self.heap.hash(id).iter().map(|(_, v)| v.clone()).collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(vals));
                        Some(Value::Array(nid))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Convert a Ruby-level `raise` argument into an Exception instance.
    /// `raise "msg"` becomes `RuntimeError.new("msg")` — we construct the
    /// instance directly (skipping the `initialize` dispatch) and set
    /// `@message`. Already-Exception instances pass through unchanged.
    pub(crate) fn normalize_exception(&mut self, v: Value) -> Value {
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
            _ => v,
        }
    }

    pub(crate) fn unwind_with_exception(&mut self, exc: Value) {
        loop {
            let f = self.frames.last_mut().expect("ICE: unwind with empty frames");
            if let Some(h) = f.rescues.pop() {
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
                return;
            }
            let f = self.frames.pop().expect("ICE: unwind pop empty");
            self.stack.truncate(f.base_sp);
            if f.is_class_body { self.class_stack.pop(); }
            if self.frames.is_empty() {
                eprintln!("uncaught exception: {}", exc.to_display(&self.heap, &self.interner));
                std::process::exit(1);
            }
        }
    }

    pub(crate) fn maybe_gc(&mut self) {
        if !self.stress_gc && !self.heap.should_gc() { return; }
        // Gather roots: stack + every frame's locals + self_val + swap_return
        // + pinned (native-code accumulators). class_stack holds Rc<Class>
        // which isn't GC-managed, so we don't need to walk it.
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + self.pinned.len() + 64);
        for v in &self.stack { roots.push(v.clone()); }
        for v in &self.pinned { roots.push(v.clone()); }
        for f in &self.frames {
            roots.push(f.self_val.clone());
            for v in f.locals.borrow().iter() { roots.push(v.clone()); }
            if let Some(v) = &f.swap_return { roots.push(v.clone()); }
            if let Some(b) = &f.block_arg {
                for v in b.captured.borrow().iter() { roots.push(v.clone()); }
                roots.push(b.self_val.clone());
            }
        }
        self.heap.collect(&roots);
    }

    pub(crate) fn invoke_method(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>) -> Result<(), Trap> {
        self.invoke_method_with_block(m, self_val, args, None)
    }

    pub(crate) fn invoke_method_with_block(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>, block: Option<Rc<BlockHandle>>) -> Result<(), Trap> {
        if m.params.len() != args.len() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected {})", args.len(), m.params.len()),
            }));
        }
        self.check_frames()?;
        let proto = &self.protos[m.proto_idx];
        let n_locals = proto.n_locals as usize;
        let mut locals = vec_nil(n_locals);
        for (i, a) in args.into_iter().enumerate() {
            locals[i] = a;
        }
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals: Rc::new(RefCell::new(locals)),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: block, rescues: vec![],
        });
        Ok(())
    }

    pub(crate) fn invoke_block(&mut self, block: Rc<BlockHandle>, args: Vec<Value>) -> Result<(), Trap> {
        self.check_frames()?;
        let proto = &self.protos[block.proto_idx];
        let needed = proto.n_locals as usize;
        {
            let mut locals = block.captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            // Place args into the block's param slots
            for (i, a) in args.into_iter().enumerate() {
                if i < block.n_params as usize {
                    locals[block.param_start as usize + i] = a;
                }
            }
        }
        self.frames.push(Frame {
            proto_idx: block.proto_idx,
            ip: 0,
            locals: block.captured.clone(),
            self_val: block.self_val.clone(),
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, rescues: vec![],
        });
        Ok(())
    }

    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let block_val = self.stack.pop().expect("ICE: stack underflow before block");
        let block = if let Value::Block(b) = block_val { b } else {
            panic!("ICE: CallBlock without Block value on stack");
        };
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before block receiver"))
        };

        if let Some(r) = &recv {
            if let Some(v) = self.collection_call_block(r, &name, &args, &block)? {
                self.stack.push(v);
                return Ok(());
            }
        }

        if no_recv {
            if let Some(res) = self.builtin_call(&name, &args) { self.stack.push(res?); return Ok(()); }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = host(&args)?;
                self.stack.push(v);
                return Ok(());
            }
            let self_val = self.frames.last().expect("ICE: do_call_block no frame").self_val.clone();
            if let Value::Object(id) = &self_val {
                let cls = self.heap.instance(*id).class.clone();
                if let Some(m) = self.lookup_method_cached(&cls, name_id) {
                    self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                    return Ok(());
                }
            }
            if let Some(m) = self.toplevel_methods.get(&name_id).cloned() {
                self.invoke_method_with_block(m, self_val, args, Some(block))?;
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                method: name.to_string(), recv_type: self_val.type_name(),
            }));
        }
        let recv = recv.expect("ICE: receiver missing for block call");
        if let Some(v) = primitive_call(&recv, &name, &args) { self.stack.push(v); return Ok(()); }
        if let Some(v) = self.sym_primitive(&recv, &name, &args) { self.stack.push(v); return Ok(()); }
        let new_id = self.interner.intern("new");
        if name_id == new_id {
            if let Value::Class(cls) = &recv {
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Instance(Instance {
                    class: cls.clone(), ivars: HashMap::new(),
                }));
                let obj = Value::Object(id);
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_cached(&cls, init_id) {
                    self.invoke_method_with_block(m, obj.clone(), args, Some(block))?;
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    self.stack.push(obj);
                }
                return Ok(());
            }
        }
        if let Value::Object(id) = &recv {
            let cls = self.heap.instance(*id).class.clone();
            if let Some(m) = self.lookup_method_cached(&cls, name_id) {
                self.invoke_method_with_block(m, recv.clone(), args, Some(block))?;
                return Ok(());
            }
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }

    pub(crate) fn collection_call_block(&mut self, recv: &Value, name: &str, args: &[Value], block: &Rc<BlockHandle>) -> Result<Option<Value>, Trap> {
        Ok(match (recv, name, args) {
            (Value::Array(id), "each", []) => {
                self.pinned.push(Value::Array(*id));
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let pre_frames = self.frames.len();
                for v in snapshot {
                    self.invoke_block(block.clone(), vec![v])?;
                    self.dispatch_until(pre_frames)?;
                    self.stack.pop();
                }
                self.pinned.pop();
                Some(Value::Array(*id))
            }
            (Value::Array(id), "map", []) => {
                self.pinned.push(Value::Array(*id));
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                self.maybe_gc();
                self.check_alloc()?;
                let result_id = self.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                self.pinned.push(Value::Array(result_id));
                let pre_frames = self.frames.len();
                for v in snapshot {
                    self.invoke_block(block.clone(), vec![v])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    self.heap.array_mut(result_id).push(r);
                }
                self.pinned.pop();
                self.pinned.pop();
                Some(Value::Array(result_id))
            }
            (Value::Hash(id), "each", []) => {
                self.pinned.push(Value::Hash(*id));
                let snapshot: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let pre_frames = self.frames.len();
                for (k, v) in snapshot {
                    self.invoke_block(block.clone(), vec![k, v])?;
                    self.dispatch_until(pre_frames)?;
                    self.stack.pop();
                }
                self.pinned.pop();
                Some(Value::Hash(*id))
            }
            (Value::Int(n), "times", []) => {
                let pre_frames = self.frames.len();
                for i in 0..*n {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    self.stack.pop();
                }
                Some(Value::Int(*n))
            }
            _ => None,
        })
    }

    /// Run dispatch loop until the frame stack returns to `until_depth`.
    pub(crate) fn dispatch_until(&mut self, until_depth: usize) -> Result<(), Trap> {
        while self.frames.len() > until_depth {
            let (proto_idx, ip) = {
                let f = self.frames.last().expect("ICE: dispatch_until no frame");
                (f.proto_idx, f.ip)
            };
            let op = self.protos[proto_idx].code[ip];
            self.frames.last_mut().expect("ICE: frames empty").ip += 1;
            if !self.step(op, proto_idx)? { return Ok(()); }
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
            Op::LoadConstStr(id) => {
                let s = self.interner.resolve(id).clone();
                self.stack.push(Value::Str(s));
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
                    self.do_call(plus_id, 1, false)?;
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
                    self.do_call(plus_id, 1, false)?;
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
                            self.do_call(plus_id, 1, false)?;
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
                            self.do_call(plus_id, 1, false)?;
                            let v = self.stack.last().expect("ICE: IncIvar slow path no result").clone();
                            self.heap.instance_mut(inst_id).ivars.insert(name_id, v);
                        }
                    }
                } else {
                    // Outside class context: nil + 1 — let CRuby semantics dictate
                    self.stack.push(Value::Nil);
                    self.stack.push(Value::Int(1));
                    let plus_id = self.interner.intern("+");
                    self.do_call(plus_id, 1, false)?;
                }
            }
            Op::LoadConst(name_id) => {
                let v = self.classes.get(&name_id).map(|c| Value::Class(c.clone())).unwrap_or(Value::Nil);
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
            Op::Call(name_id, argc) => {
                self.do_call(name_id, argc as usize, false)?;
            }
            Op::CallNoRecv(name_id, argc) => {
                self.do_call(name_id, argc as usize, true)?;
            }
            Op::CallBlock(name_id, argc) => {
                self.do_call_block(name_id, argc as usize, false)?;
            }
            Op::CallNoRecvBlock(name_id, argc) => {
                self.do_call_block(name_id, argc as usize, true)?;
            }
            Op::CreateBlock(p_idx, param_start, n_params) => {
                let f = self.frames.last().expect("ICE: CreateBlock no frame");
                let captured = f.locals.clone();
                let self_val = f.self_val.clone();
                let h = BlockHandle { proto_idx: p_idx as usize, captured, self_val, param_start, n_params };
                self.stack.push(Value::Block(Rc::new(h)));
            }
            Op::Yield(argc) => {
                let block = match self.frames.last().expect("ICE: Yield no frame").block_arg.clone() {
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
                let m = Rc::new(Method { params: proto.params.clone(), proto_idx: p_idx as usize });
                if let Some(cls) = self.class_stack.last() { cls.methods.borrow_mut().insert(name_id, m); }
                else { self.toplevel_methods.insert(name_id, m); }
                // Conservatively invalidate the inline cache — any previous
                // cache entry could in theory be made stale by this definition.
                self.call_cache = None;
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
                    superclass: RefCell::new(parent.clone()),
                })).clone();
                // If the class already existed (reopened) and the user specified a parent
                // this time, update it (only if it wasn't already set to something else).
                if let Some(p) = &parent {
                    let mut sc = cls.superclass.borrow_mut();
                    if sc.is_none() {
                        *sc = Some(p.clone());
                    }
                }
                self.call_cache = None; // class structure changed
                self.class_stack.push(cls.clone());
                let proto = &self.protos[p_idx as usize];
                let n_locals = proto.n_locals as usize;
                self.frames.push(Frame {
                    proto_idx: p_idx as usize, ip: 0,
                    locals: Rc::new(RefCell::new(vec_nil(n_locals))),
                    self_val: Value::Class(cls.clone()),
                    base_sp: self.stack.len(),
                    is_class_body: true, swap_return: None, block_arg: None, rescues: vec![],
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
            Op::PushRescue(off, slot, bind) => {
                let ip = self.frames.last().expect("ICE: PushRescue no frame").ip;
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                let bind_slot = if bind != 0 { Some(slot) } else { None };
                self.frames.last_mut().expect("ICE: PushRescue no frame").rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot, is_ensure: false,
                });
            }
            Op::PopRescue => {
                self.frames.last_mut().expect("ICE: PopRescue no frame").rescues.pop();
            }
            Op::PushEnsure(off) => {
                let ip = self.frames.last().expect("ICE: PushEnsure no frame").ip;
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                self.frames.last_mut().expect("ICE: PushEnsure no frame").rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot: None, is_ensure: true,
                });
            }
            Op::PopEnsure => {
                self.frames.last_mut().expect("ICE: PopEnsure no frame").rescues.pop();
            }
            Op::Raise => {
                let v = self.stack.pop().unwrap_or(Value::Nil);
                let exc = self.normalize_exception(v);
                self.unwind_with_exception(exc);
            }
            Op::BinOpInt(kind, rhs) => {
                let a = self.stack.pop().expect("ICE: BinOpInt lhs underflow");
                if let Value::Int(x) = a {
                    self.stack.push(kind.apply_int(x, rhs));
                } else {
                    // Cold path: behave as if a generic `<op>` was dispatched
                    // with rhs boxed as an Int.
                    let b_val = Value::Int(rhs);
                    if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b_val)) {
                        self.stack.push(v);
                    } else if let Some(v) = self.sym_primitive(&a, kind.name(), std::slice::from_ref(&b_val)) {
                        self.stack.push(v);
                    } else {
                        self.stack.push(a);
                        self.stack.push(b_val);
                        let name_id = self.interner.intern(kind.name());
                        self.do_call(name_id, 1, false)?;
                    }
                }
            }
            Op::BinOp(kind) => {
                let b = self.stack.pop().expect("ICE: BinOp rhs underflow");
                let a = self.stack.pop().expect("ICE: BinOp lhs underflow");
                if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
                    self.stack.push(kind.apply_int(*x, *y));
                } else if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b)) {
                    self.stack.push(v);
                } else {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false)?;
                }
            }
            Op::Return => {
                let f = self.frames.pop().expect("ICE: Return no frame");
                let ret = self.stack.pop().unwrap_or(Value::Nil);
                self.stack.truncate(f.base_sp);
                if f.is_class_body {
                    let cls = self.class_stack.pop().expect("ICE: class_stack empty on class-body return");
                    self.stack.push(Value::Class(cls));
                } else if let Some(replacement) = f.swap_return {
                    self.stack.push(replacement);
                } else {
                    self.stack.push(ret);
                }
                if self.frames.is_empty() { return Ok(false); }
            }
        }
        Ok(true)
    }
}

pub(crate) fn vec_nil(n: usize) -> Vec<Value> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n { v.push(Value::Nil); }
    v
}

impl Vm {
    pub(crate) fn builtin_call(&mut self, name: &str, args: &[Value]) -> Option<Result<Value, Trap>> {
        match name {
            "puts" => {
                if args.is_empty() {
                    let _ = writeln!(self.stdout);
                } else {
                    for a in args {
                        let s = a.to_display(&self.heap, &self.interner);
                        let _ = writeln!(self.stdout, "{}", s);
                    }
                }
                Some(Ok(Value::Nil))
            }
            "print" => {
                for a in args {
                    let s = a.to_display(&self.heap, &self.interner);
                    let _ = write!(self.stdout, "{}", s);
                }
                Some(Ok(Value::Nil))
            }
            _ => None,
        }
    }
}

pub(crate) fn primitive_call(recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
    match (recv, name, args) {
        (Value::Int(a), op, [Value::Int(b)]) => match op {
            "+" => Some(Value::Int(a + b)),
            "-" => Some(Value::Int(a - b)),
            "*" => Some(Value::Int(a * b)),
            "/" => Some(Value::Int(a / b)),
            "%" => Some(Value::Int(a % b)),
            "==" => Some(Value::Bool(a == b)),
            "!=" => Some(Value::Bool(a != b)),
            "<"  => Some(Value::Bool(a < b)),
            "<=" => Some(Value::Bool(a <= b)),
            ">"  => Some(Value::Bool(a > b)),
            ">=" => Some(Value::Bool(a >= b)),
            _ => None,
        },
        (Value::Int(a), "to_s", []) => Some(Value::Str(Rc::from(a.to_string().as_str()))),
        (Value::Str(a), "+", [Value::Str(b)]) => {
            let mut s = a.to_string();
            s.push_str(b);
            Some(Value::Str(Rc::from(s.as_str())))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(**a == **b)),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        (Value::Str(a), "length", []) => Some(Value::Int(a.chars().count() as i64)),
        (Value::Sym(a), "==", [Value::Sym(b)]) => Some(Value::Bool(a == b)),
        (Value::Sym(a), "!=", [Value::Sym(b)]) => Some(Value::Bool(a != b)),
        (Value::Nil, "to_s", []) => Some(Value::Str(Rc::from(""))),
        (Value::Nil, "inspect", []) => Some(Value::Str(Rc::from("nil"))),
        (Value::Nil, "nil?", []) => Some(Value::Bool(true)),
        (Value::Bool(b), "to_s", []) => Some(Value::Str(Rc::from(if *b { "true" } else { "false" }))),
        _ => None,
    }
}

/// `Symbol#to_s` / `to_sym` need the Interner to resolve the underlying name,
/// so they live as a method on Vm rather than in the pure `primitive_call`.
impl Vm {
    pub(crate) fn sym_primitive(&self, recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
        match (recv, name, args) {
            (Value::Sym(id), "to_s", []) => Some(Value::Str(self.interner.resolve(*id).clone())),
            (Value::Sym(id), "to_sym", []) => Some(Value::Sym(*id)),
            _ => None,
        }
    }
}
