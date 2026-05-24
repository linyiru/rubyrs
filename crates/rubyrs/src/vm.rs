use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::rc::Rc;

use std::io::Write;

use crate::bytecode::{Op, Proto};
use crate::error::{RubyError, Span, Trap, TrapFrame};
use crate::heap::{Heap, HeapObj};
use crate::intern::{Interner, SymId};
use crate::value::{BlockHandle, Class, Instance, Method, ObjId, Value};

// ---------- VM ----------

/// Ordering for built-in aggregation methods (`min` / `max` /
/// `sort`). Only homogeneous Int / Str / Sym arrays are supported;
/// other shapes return `None` so the caller can fall through to
/// NoMethodError. With a block-taking comparator we'd handle this
/// generically, but that's deferred to a later milestone.
///
/// Symbol comparison uses the interned string — CRuby orders
/// `:apple < :banana` lexicographically, not by interning order.
fn value_cmp_v(a: &Value, b: &Value, interner: &Interner) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Some((**x).cmp(&**y)),
        (Value::Sym(x), Value::Sym(y)) => {
            let sx = interner.resolve(*x);
            let sy = interner.resolve(*y);
            Some((**sx).cmp(&**sy))
        }
        _ => None,
    }
}

/// Which Enumerable predicate-iterator a call dispatches to.
/// `NoneM` is named with a trailing M because `None` collides with
/// `Option::None` in match arms.
#[derive(Copy, Clone, Debug)]
pub(crate) enum IterMode { Select, Reject, Find, Any, All, NoneM }

impl IterMode {
    fn bool_init(self) -> bool {
        // For `all?` we start at true and flip to false on first
        // falsy; for `none?` likewise. `any?` starts false.
        match self {
            IterMode::Any => false,
            IterMode::All | IterMode::NoneM => true,
            _ => false,
        }
    }
}

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
    /// Class filter for `rescue`. `None` means catch-all (used for
    /// `ensure` and as a future hook for internal/host-only handlers).
    /// `Some(cls)` means the handler only fires when the raised
    /// exception's class is `cls` or a descendant. Bare `rescue` (no
    /// class listed) populates this with `StandardError`, so any
    /// exception that intentionally lives outside the StandardError
    /// subtree (e.g. `ResourceExhausted`) cannot be silently swallowed
    /// by `rescue => e`. Explicit `rescue ClassName => e` is a P1-10
    /// follow-up; today every PushRescue uses StandardError.
    pub(crate) filter_class: Option<Rc<Class>>,
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
    /// Per-call-site monomorphic inline cache for method dispatch on
    /// `Value::Object`. One slot per `Op::Call(...,cache_id)` /
    /// `Op::CallNoRecv` / `Op::CallBlock` / `Op::CallNoRecvBlock` site.
    /// Each entry remembers the (class identity, gen-at-time-of-cache,
    /// resolved Method) of the last successful lookup at that site.
    ///
    /// Lookups compare against the receiver's class pointer AND the
    /// current `method_gen`. Any `Op::DefMethod` bumps `method_gen`,
    /// which effectively invalidates every cache entry — re-fill is
    /// lazy on the next call at each site.
    pub(crate) call_caches: Vec<CallCache>,
    pub(crate) method_gen: u32,
    /// `Op::Break` sets this; iteration drivers check and consume.
    pub(crate) break_signaled: bool,
}

/// One entry in the per-call-site inline cache.
#[derive(Clone)]
pub(crate) struct CallCache {
    pub(crate) class_ptr: usize, // 0 = empty
    pub(crate) generation: u32,
    pub(crate) method: Option<Rc<Method>>,
}

impl Default for CallCache {
    fn default() -> Self { CallCache { class_ptr: 0, generation: 0, method: None } }
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
            call_caches: Vec::new(),
            method_gen: 0,
            break_signaled: false,
        }
    }

    /// Make sure `call_caches` has at least `n` entries (one per
    /// emitted call op). Called by the host (`Runtime::eval`) after a
    /// compile pass when the cache-id counter is known.
    pub(crate) fn ensure_call_caches(&mut self, n: usize) {
        if self.call_caches.len() < n {
            self.call_caches.resize(n, CallCache::default());
        }
    }

    /// Per-call-site cached lookup. `cache_id` is the slot from the
    /// `Op::Call(...,cache_id)` instruction. Hit when both class
    /// pointer and `method_gen` match what was cached.
    #[inline]
    pub(crate) fn lookup_method_cached(&mut self, cls: &Rc<Class>, name_id: SymId, cache_id: u16) -> Option<Rc<Method>> {
        let class_ptr = Rc::as_ptr(cls) as usize;
        let idx = cache_id as usize;
        // Fast path
        if idx < self.call_caches.len() {
            let c = &self.call_caches[idx];
            if c.class_ptr == class_ptr && c.generation == self.method_gen {
                return c.method.clone();
            }
        }
        // Miss: walk the chain, populate slot
        let m = self.lookup_method_uncached(cls, name_id);
        if idx < self.call_caches.len() {
            self.call_caches[idx] = CallCache {
                class_ptr,
                generation: self.method_gen,
                method: m.clone(),
            };
        }
        m
    }

    /// Plain method lookup walking the class chain, with no cache touch.
    /// Used for paths that don't benefit from caching (e.g. `initialize`
    /// resolution during `Class.new`).
    #[inline]
    pub(crate) fn lookup_method_uncached(&self, cls: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
        let mut current = cls.clone();
        loop {
            if let Some(m) = current.methods.borrow().get(&name_id).cloned() {
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

    pub(crate) fn do_call(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
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
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
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
                // `args` and `recv` were popped off the operand stack by
                // do_call's setup; while we're about to trigger GC via
                // `maybe_gc`, they exist only as Rust locals. Pin any
                // heap values inside `args` (Class is `Rc`-managed and
                // doesn't need pinning) so the GC's root walk sees them.
                let pin_n = args.len();
                for a in &args { self.pinned.push(a.clone()); }
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Instance(Instance {
                    class: cls.clone(),
                    ivars: HashMap::new(),
                }));
                for _ in 0..pin_n { self.pinned.pop(); }
                let obj = Value::Object(id);
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_uncached(&cls, init_id) {
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
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
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
                    ("include?", [needle]) => {
                        let a = self.heap.array(id);
                        let hit = a.iter().any(|x| x.ruby_eq(needle, &self.heap));
                        Some(Value::Bool(hit))
                    }
                    ("count", []) => Some(Value::Int(self.heap.array(id).len() as i64)),
                    ("count", [needle]) => {
                        let a = self.heap.array(id);
                        let n = a.iter().filter(|x| x.ruby_eq(needle, &self.heap)).count();
                        Some(Value::Int(n as i64))
                    }
                    ("sum", []) | ("sum", [Value::Int(_)]) => {
                        let init = match args { [Value::Int(n)] => *n, _ => 0 };
                        let a = self.heap.array(id);
                        let mut s: i64 = init;
                        for v in a {
                            match v {
                                Value::Int(n) => s = s.wrapping_add(*n),
                                _ => return None,
                            }
                        }
                        Some(Value::Int(s))
                    }
                    ("min", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Some(Value::Nil); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v(v, &best, &self.interner) {
                                Some(std::cmp::Ordering::Less) => best = v.clone(),
                                Some(_) => {}
                                None => return None,
                            }
                        }
                        Some(best)
                    }
                    ("max", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Some(Value::Nil); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v(v, &best, &self.interner) {
                                Some(std::cmp::Ordering::Greater) => best = v.clone(),
                                Some(_) => {}
                                None => return None,
                            }
                        }
                        Some(best)
                    }
                    ("sort", []) => {
                        let mut copy: Vec<Value> = self.heap.array(id).clone();
                        if copy.windows(2).any(|w| value_cmp_v(&w[0], &w[1], &self.interner).is_none()) {
                            return None;
                        }
                        let interner = &self.interner;
                        copy.sort_by(|a, b| value_cmp_v(a, b, interner).unwrap_or(std::cmp::Ordering::Equal));
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(copy));
                        Some(Value::Array(nid))
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        let a = self.heap.array(id).clone();
                        if a.is_empty() { return Some(Value::Nil); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = crate::bytecode::BinOpKind::from_op_name(&op_name)?;
                        let mut acc = a[0].clone();
                        for v in &a[1..] {
                            match (&acc, v) {
                                (Value::Int(x), Value::Int(y)) => acc = kind.apply_int(*x, *y),
                                _ => return None,
                            }
                        }
                        Some(acc)
                    }
                    ("to_a", []) => Some(Value::Array(id)),
                    ("reverse", []) => {
                        let rev: Vec<Value> = self.heap.array(id).iter().rev().cloned().collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(rev));
                        Some(Value::Array(nid))
                    }
                    ("uniq", []) => {
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if !out.iter().any(|x| x.ruby_eq(v, &self.heap)) {
                                out.push(v.clone());
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("compact", []) => {
                        let out: Vec<Value> = self.heap.array(id).iter()
                            .filter(|v| !matches!(v, Value::Nil))
                            .cloned()
                            .collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("flatten", []) => {
                        // Depth-1 flatten — same as CRuby's default `flatten(1)`
                        // is recursive; ours stops at depth 1 to match the
                        // CRuby behaviour we exercise in fixtures. Document
                        // unbounded recursion as a follow-up if needed.
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if let Value::Array(inner) = v {
                                for x in self.heap.array(*inner) { out.push(x.clone()); }
                            } else {
                                out.push(v.clone());
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("join", []) => {
                        let parts: Vec<String> = self.heap.array(id).iter()
                            .map(|v| v.to_display(&self.heap, &self.interner))
                            .collect();
                        Some(Value::Str(Rc::from(parts.join("").as_str())))
                    }
                    ("join", [Value::Str(sep)]) => {
                        let parts: Vec<String> = self.heap.array(id).iter()
                            .map(|v| v.to_display(&self.heap, &self.interner))
                            .collect();
                        Some(Value::Str(Rc::from(parts.join(&**sep).as_str())))
                    }
                    ("+", [Value::Array(other)]) => {
                        let mut out: Vec<Value> = self.heap.array(id).clone();
                        let extra: Vec<Value> = self.heap.array(*other).clone();
                        out.extend(extra);
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("-", [Value::Array(other)]) => {
                        let src = self.heap.array(id).clone();
                        let exclude = self.heap.array(*other).clone();
                        let out: Vec<Value> = src.into_iter()
                            .filter(|v| !exclude.iter().any(|x| x.ruby_eq(v, &self.heap)))
                            .collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("concat", [Value::Array(other)]) => {
                        // In-place: extend self with other's elements, return self.
                        let extra: Vec<Value> = self.heap.array(*other).clone();
                        self.heap.array_mut(id).extend(extra);
                        Some(Value::Array(id))
                    }
                    ("take", [Value::Int(n)]) => {
                        let n = (*n).max(0) as usize;
                        let out: Vec<Value> = self.heap.array(id).iter().take(n).cloned().collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("drop", [Value::Int(n)]) => {
                        let n = (*n).max(0) as usize;
                        let out: Vec<Value> = self.heap.array(id).iter().skip(n).cloned().collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
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
                    ("include?", [k]) | ("has_key?", [k]) | ("key?", [k]) | ("member?", [k]) => {
                        let h = self.heap.hash(id);
                        let hit = h.iter().any(|(key, _)| key.ruby_eq(k, &self.heap));
                        Some(Value::Bool(hit))
                    }
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
                    ("to_h", []) => Some(Value::Hash(id)),
                    ("to_a", []) => {
                        // Hash#to_a returns an Array of two-element Arrays.
                        // Each inner [k, v] is freshly heap-allocated; we
                        // need to pin every inner Array onto `self.pinned`
                        // as we accumulate, otherwise the next loop iter's
                        // `maybe_gc` will sweep the previous pair (it's
                        // only live via the Rust-local Vec, not via any GC
                        // root). Failing to pin produces slot-reuse cycles
                        // that explode `to_display`'s recursion later.
                        // Also pin the source Hash since recv was popped
                        // off the operand stack before we got here.
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        self.pinned.push(Value::Hash(id));
                        let pair_count = pairs.len();
                        let mut pair_ids: Vec<Value> = Vec::with_capacity(pair_count);
                        for (k, v) in pairs {
                            self.maybe_gc();
                            let pid = self.heap.alloc(HeapObj::Array(vec![k, v]));
                            self.pinned.push(Value::Array(pid));
                            pair_ids.push(Value::Array(pid));
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(pair_ids));
                        for _ in 0..pair_count { self.pinned.pop(); }
                        self.pinned.pop(); // source Hash
                        Some(Value::Array(nid))
                    }
                    ("merge", [Value::Hash(other)]) => {
                        // CRuby: keys in `other` overwrite keys in `self`,
                        // and `other`'s key-order is appended after self's
                        // (existing keys retain their position).
                        let mut out: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        let extra: Vec<(Value, Value)> = self.heap.hash(*other).clone();
                        for (k, v) in extra {
                            let pos = out.iter().position(|(ek, _)| ek.ruby_eq(&k, &self.heap));
                            if let Some(p) = pos {
                                out[p].1 = v;
                            } else {
                                out.push((k, v));
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(out));
                        Some(Value::Hash(nid))
                    }
                    ("delete", [k]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        if let Some(p) = pos {
                            let removed = self.heap.hash_mut(id).remove(p).1;
                            Some(removed)
                        } else {
                            Some(Value::Nil)
                        }
                    }
                    ("invert", []) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).iter()
                            .map(|(k, v)| (v.clone(), k.clone()))
                            .collect();
                        // Later duplicates win for invert — same as CRuby:
                        // if two original values collide as inverted keys,
                        // the last one through wins. Dedup keeping latest.
                        let mut out: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
                        for (k, v) in pairs {
                            let pos = out.iter().position(|(ek, _)| ek.ruby_eq(&k, &self.heap));
                            if let Some(p) = pos { out[p].1 = v; } else { out.push((k, v)); }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(out));
                        Some(Value::Hash(nid))
                    }
                    ("store", [k, v]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        let h = self.heap.hash_mut(id);
                        if let Some(p) = pos { h[p].1 = v.clone(); }
                        else { h.push((k.clone(), v.clone())); }
                        Some(v.clone())
                    }
                    _ => None,
                }
            }
            Value::Str(s) => {
                let s = s.clone();
                match (name, args) {
                    ("chars", []) => {
                        let elems: Vec<Value> = s.chars()
                            .map(|c| Value::Str(Rc::from(c.to_string().as_str())))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", []) => {
                        // No-arg `split` matches CRuby's `split(nil)`:
                        // splits on runs of whitespace, drops the
                        // leading empty token.
                        let elems: Vec<Value> = s.split_whitespace()
                            .map(|t| Value::Str(Rc::from(t)))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", [Value::Str(sep)]) => {
                        let elems: Vec<Value> = if sep.is_empty() {
                            // CRuby: empty-sep split returns each character.
                            s.chars().map(|c| Value::Str(Rc::from(c.to_string().as_str()))).collect()
                        } else {
                            s.split(&**sep).map(|t| Value::Str(Rc::from(t))).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("to_sym", []) => {
                        let sym = self.interner.intern(&s);
                        Some(Value::Sym(sym))
                    }
                    _ => None,
                }
            }
            Value::Range(id) => {
                let id = *id;
                let (b, e, excl) = {
                    let r = self.heap.range(id);
                    (r.begin.clone(), r.end.clone(), r.exclusive)
                };
                let (bi, ei) = match (&b, &e) {
                    (Value::Int(a), Value::Int(c)) => (*a, *c),
                    _ => return None,
                };
                let count = if excl { (ei - bi).max(0) } else { (ei - bi + 1).max(0) };
                match (name, args) {
                    ("begin", []) | ("first", []) | ("min", []) => Some(b.clone()),
                    ("end", []) | ("last", []) => Some(e.clone()),
                    ("max", []) => Some(if excl { Value::Int(ei - 1) } else { e.clone() }),
                    ("size", []) | ("length", []) | ("count", []) => Some(Value::Int(count)),
                    ("exclude_end?", []) => Some(Value::Bool(excl)),
                    ("include?", [Value::Int(v)]) => {
                        let in_r = if excl { *v >= bi && *v < ei } else { *v >= bi && *v <= ei };
                        Some(Value::Bool(in_r))
                    }
                    ("to_a", []) => {
                        let mut elems = Vec::with_capacity(count.max(0) as usize);
                        let end_inclusive = if excl { ei - 1 } else { ei };
                        for v in bi..=end_inclusive { elems.push(Value::Int(v)); }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(nid))
                    }
                    ("sum", []) | ("sum", [Value::Int(_)]) => {
                        let init = match args { [Value::Int(n)] => *n, _ => 0 };
                        let end_inc = if excl { ei - 1 } else { ei };
                        if bi > end_inc { return Some(Value::Int(init)); }
                        let n = end_inc - bi + 1;
                        let s = n.wrapping_mul(bi.wrapping_add(end_inc)) / 2;
                        Some(Value::Int(init.wrapping_add(s)))
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        let end_inc = if excl { ei - 1 } else { ei };
                        if bi > end_inc { return Some(Value::Nil); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = crate::bytecode::BinOpKind::from_op_name(&op_name)?;
                        let mut acc = Value::Int(bi);
                        let mut i = bi + 1;
                        while i <= end_inc {
                            match &acc {
                                Value::Int(x) => acc = kind.apply_int(*x, i),
                                _ => return None,
                            }
                            i += 1;
                        }
                        Some(acc)
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
        // Resolve the raised value's class once up front; the unwind loop
        // may probe many handlers before finding (or not finding) a match.
        let exc_class: Option<Rc<Class>> = match &exc {
            Value::Object(id) => Some(self.heap.instance(*id).class.clone()),
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
                        true
                    } else if let Some(filter) = &h.filter_class {
                        exc_class.as_ref().map_or(false, |cls| class_is_a(cls, filter))
                    } else {
                        true
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
                return;
            }
            // No matching handler in this frame — pop it and try the caller.
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

    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
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
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
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
                // Pin args during the alloc window — see the matching
                // comment in `do_call`'s new-branch for the rationale.
                let pin_n = args.len();
                for a in &args { self.pinned.push(a.clone()); }
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Instance(Instance {
                    class: cls.clone(), ivars: HashMap::new(),
                }));
                for _ in 0..pin_n { self.pinned.pop(); }
                let obj = Value::Object(id);
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_uncached(&cls, init_id) {
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
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                self.invoke_method_with_block(m, recv.clone(), args, Some(block))?;
                return Ok(());
            }
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }

    /// Drives an iterator-with-predicate over an Array. Used by
    /// `select` / `reject` / `find` / `any?` / `all?` / `none?`.
    /// On `break val` (caught via `self.break_signaled`) returns `val`
    /// to match CRuby's "break value short-circuits the enumerator".
    pub(crate) fn iter_array_filter(&mut self, id: ObjId, mode: IterMode, block: &Rc<BlockHandle>) -> Result<Value, Trap> {
        let snapshot: Vec<Value> = self.heap.array(id).clone();
        self.pinned.push(Value::Array(id));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            self.maybe_gc();
            self.check_alloc()?;
            let rid = self.heap.alloc(HeapObj::Array(Vec::new()));
            self.pinned.push(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = self.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        for v in snapshot {
            self.invoke_block(block.clone(), vec![v.clone()])?;
            self.dispatch_until(pre_frames)?;
            let r = self.stack.pop().unwrap_or(Value::Nil);
            if self.break_signaled {
                self.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { self.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Reject => if !truthy { self.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Find => if truthy { find_val = v; break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
        }
        if acc_id.is_some() { self.pinned.pop(); }
        self.pinned.pop();
        if let Some(e) = early { return Ok(e); }
        Ok(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        })
    }

    /// Same shape as `iter_array_filter`, but the source is a Hash.
    /// The block receives two args (key, value). `select`/`reject`
    /// return a Hash; `find` returns a `[k, v]` two-element Array (or nil).
    pub(crate) fn iter_hash_filter(&mut self, id: ObjId, mode: IterMode, block: &Rc<BlockHandle>) -> Result<Value, Trap> {
        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
        self.pinned.push(Value::Hash(id));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            self.maybe_gc();
            self.check_alloc()?;
            let rid = self.heap.alloc(HeapObj::Hash(Vec::new()));
            self.pinned.push(Value::Hash(rid));
            Some(rid)
        } else { None };
        let pre_frames = self.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        for (k, v) in snapshot {
            self.invoke_block(block.clone(), vec![k.clone(), v.clone()])?;
            self.dispatch_until(pre_frames)?;
            let r = self.stack.pop().unwrap_or(Value::Nil);
            if self.break_signaled {
                self.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { self.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Reject => if !truthy { self.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Find => if truthy {
                    self.maybe_gc();
                    self.check_alloc()?;
                    let pair = self.heap.alloc(HeapObj::Array(vec![k, v]));
                    find_val = Value::Array(pair);
                    break;
                }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
        }
        if acc_id.is_some() { self.pinned.pop(); }
        self.pinned.pop();
        if let Some(e) = early { return Ok(e); }
        Ok(match mode {
            IterMode::Select | IterMode::Reject => Value::Hash(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        })
    }

    /// Same shape as `iter_array_filter`, but iterates an Int Range.
    /// Returns `None` (Option) to the caller if the range's endpoints
    /// aren't both Ints — callers fall through to NoMethodError.
    pub(crate) fn iter_range_filter(&mut self, id: ObjId, mode: IterMode, block: &Rc<BlockHandle>) -> Result<Option<Value>, Trap> {
        let (bi, ei, excl) = {
            let r = self.heap.range(id);
            match (&r.begin, &r.end) {
                (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                _ => return Ok(None),
            }
        };
        self.pinned.push(Value::Range(id));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            self.maybe_gc();
            self.check_alloc()?;
            let rid = self.heap.alloc(HeapObj::Array(Vec::new()));
            self.pinned.push(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = self.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        let end_inc = if excl { ei - 1 } else { ei };
        let mut i = bi;
        while i <= end_inc {
            self.invoke_block(block.clone(), vec![Value::Int(i)])?;
            self.dispatch_until(pre_frames)?;
            let r = self.stack.pop().unwrap_or(Value::Nil);
            if self.break_signaled {
                self.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { self.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Reject => if !truthy { self.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Find => if truthy { find_val = Value::Int(i); break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
            i += 1;
        }
        if acc_id.is_some() { self.pinned.pop(); }
        self.pinned.pop();
        if let Some(e) = early { return Ok(Some(e)); }
        Ok(Some(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        }))
    }

    pub(crate) fn collection_call_block(&mut self, recv: &Value, name: &str, args: &[Value], block: &Rc<BlockHandle>) -> Result<Option<Value>, Trap> {
        Ok(match (recv, name, args) {
            (Value::Array(id), "each", []) => {
                self.pinned.push(Value::Array(*id));
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let pre_frames = self.frames.len();
                let mut early = None;
                for v in snapshot {
                    self.invoke_block(block.clone(), vec![v])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                self.pinned.pop();
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "map", []) => {
                self.pinned.push(Value::Array(*id));
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                self.maybe_gc();
                self.check_alloc()?;
                let result_id = self.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                self.pinned.push(Value::Array(result_id));
                let pre_frames = self.frames.len();
                let mut early = None;
                for v in snapshot {
                    self.invoke_block(block.clone(), vec![v])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    self.heap.array_mut(result_id).push(r);
                }
                self.pinned.pop();
                self.pinned.pop();
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Hash(id), "each", []) | (Value::Hash(id), "each_pair", []) => {
                let id = *id;
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                self.pinned.push(Value::Hash(id));
                let pre_frames = self.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    self.invoke_block(block.clone(), vec![k, v])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                self.pinned.pop();
                return Ok(Some(early.unwrap_or(Value::Hash(id))));
            }
            (Value::Int(start), "upto", [Value::Int(stop)]) => {
                let start = *start;
                let stop = *stop;
                let pre_frames = self.frames.len();
                let mut early = None;
                let mut i = start;
                while i <= stop {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            (Value::Int(start), "downto", [Value::Int(stop)]) => {
                let start = *start;
                let stop = *stop;
                let pre_frames = self.frames.len();
                let mut early = None;
                let mut i = start;
                while i >= stop {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    i -= 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            (Value::Int(n), "times", []) => {
                let pre_frames = self.frames.len();
                let mut early = None;
                for i in 0..*n {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Int(*n)))
            }
            (Value::Range(id), "each", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                self.pinned.push(Value::Range(*id));
                let pre_frames = self.frames.len();
                let mut early = None;
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                while i <= end_inc {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    i += 1;
                }
                self.pinned.pop();
                Some(early.unwrap_or(Value::Range(*id)))
            }
            (Value::Array(id), "each_with_index", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                self.pinned.push(Value::Array(*id));
                let pre_frames = self.frames.len();
                let mut early = None;
                for (i, v) in snapshot.into_iter().enumerate() {
                    self.invoke_block(block.clone(), vec![v, Value::Int(i as i64)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                self.pinned.pop();
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "sort_by", []) => {
                // Compute the sort key for every element by calling the
                // block once, then sort element/key pairs by key. The
                // existing `value_cmp` only knows how to compare Ints and
                // Strs, so block-returned keys outside those types fall
                // through to NoMethodError (Option<None> from value_cmp).
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                self.pinned.push(Value::Array(*id));
                let pre_frames = self.frames.len();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
                let mut early = None;
                for v in snapshot {
                    self.invoke_block(block.clone(), vec![v.clone()])?;
                    self.dispatch_until(pre_frames)?;
                    let key = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    pairs.push((key, v));
                }
                if let Some(e) = early {
                    self.pinned.pop();
                    return Ok(Some(e));
                }
                // Bail if any key is uncomparable — leave callers a path to
                // see NoMethodError instead of a silent equal-everywhere sort.
                if pairs.iter().any(|(k1, _)| pairs.iter().any(|(k2, _)| value_cmp_v(k1, k2, &self.interner).is_none())) {
                    self.pinned.pop();
                    return Ok(None);
                }
                let interner = &self.interner;
                pairs.sort_by(|a, b| value_cmp_v(&a.0, &b.0, interner).unwrap_or(std::cmp::Ordering::Equal));
                let sorted: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
                self.maybe_gc();
                self.check_alloc()?;
                let nid = self.heap.alloc(HeapObj::Array(sorted));
                self.pinned.pop();
                Some(Value::Array(nid))
            }
            (Value::Array(id), "inject", []) | (Value::Array(id), "reduce", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                self.pinned.push(Value::Array(*id));
                let pre_frames = self.frames.len();
                let mut acc = snapshot[0].clone();
                let mut early = None;
                for v in &snapshot[1..] {
                    self.invoke_block(block.clone(), vec![acc.clone(), v.clone()])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                }
                self.pinned.pop();
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "inject", [init]) | (Value::Array(id), "reduce", [init]) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                self.pinned.push(Value::Array(*id));
                let pre_frames = self.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                for v in &snapshot {
                    self.invoke_block(block.clone(), vec![acc.clone(), v.clone()])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                }
                self.pinned.pop();
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "count", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                self.pinned.push(Value::Array(*id));
                let pre_frames = self.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                for v in snapshot {
                    self.invoke_block(block.clone(), vec![v])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() { n += 1; }
                }
                self.pinned.pop();
                Some(early.unwrap_or(Value::Int(n)))
            }
            (Value::Range(id), "inject", []) | (Value::Range(id), "reduce", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                if bi > end_inc { return Ok(Some(Value::Nil)); }
                self.pinned.push(Value::Range(*id));
                let pre_frames = self.frames.len();
                let mut acc = Value::Int(bi);
                let mut early = None;
                let mut i = bi + 1;
                while i <= end_inc {
                    self.invoke_block(block.clone(), vec![acc.clone(), Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                    i += 1;
                }
                self.pinned.pop();
                Some(early.unwrap_or(acc))
            }
            (Value::Range(id), "inject", [init]) | (Value::Range(id), "reduce", [init]) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                self.pinned.push(Value::Range(*id));
                let pre_frames = self.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    self.invoke_block(block.clone(), vec![acc.clone(), Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                    i += 1;
                }
                self.pinned.pop();
                Some(early.unwrap_or(acc))
            }
            (Value::Range(id), "count", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                self.pinned.push(Value::Range(*id));
                let pre_frames = self.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() { n += 1; }
                    i += 1;
                }
                self.pinned.pop();
                Some(early.unwrap_or(Value::Int(n)))
            }

            (Value::Array(id), "select", []) | (Value::Array(id), "filter", []) => Some(self.iter_array_filter(*id, IterMode::Select, block)?),
            (Value::Array(id), "reject", []) => Some(self.iter_array_filter(*id, IterMode::Reject, block)?),
            (Value::Array(id), "find", []) | (Value::Array(id), "detect", []) => Some(self.iter_array_filter(*id, IterMode::Find, block)?),
            (Value::Array(id), "any?", []) => Some(self.iter_array_filter(*id, IterMode::Any, block)?),
            (Value::Array(id), "all?", []) => Some(self.iter_array_filter(*id, IterMode::All, block)?),
            (Value::Array(id), "none?", []) => Some(self.iter_array_filter(*id, IterMode::NoneM, block)?),

            (Value::Hash(id), "select", []) | (Value::Hash(id), "filter", []) => Some(self.iter_hash_filter(*id, IterMode::Select, block)?),
            (Value::Hash(id), "reject", []) => Some(self.iter_hash_filter(*id, IterMode::Reject, block)?),
            (Value::Hash(id), "find", []) | (Value::Hash(id), "detect", []) => Some(self.iter_hash_filter(*id, IterMode::Find, block)?),
            (Value::Hash(id), "any?", []) => Some(self.iter_hash_filter(*id, IterMode::Any, block)?),
            (Value::Hash(id), "all?", []) => Some(self.iter_hash_filter(*id, IterMode::All, block)?),
            (Value::Hash(id), "none?", []) => Some(self.iter_hash_filter(*id, IterMode::NoneM, block)?),

            (Value::Range(id), "select", []) | (Value::Range(id), "filter", []) => self.iter_range_filter(*id, IterMode::Select, block)?,
            (Value::Range(id), "reject", []) => self.iter_range_filter(*id, IterMode::Reject, block)?,
            (Value::Range(id), "find", []) | (Value::Range(id), "detect", []) => self.iter_range_filter(*id, IterMode::Find, block)?,
            (Value::Range(id), "any?", []) => self.iter_range_filter(*id, IterMode::Any, block)?,
            (Value::Range(id), "all?", []) => self.iter_range_filter(*id, IterMode::All, block)?,
            (Value::Range(id), "none?", []) => self.iter_range_filter(*id, IterMode::NoneM, block)?,

            (Value::Range(id), "map", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                self.pinned.push(Value::Range(*id));
                self.maybe_gc();
                self.check_alloc()?;
                let count = if excl { (ei - bi).max(0) } else { (ei - bi + 1).max(0) };
                let result_id = self.heap.alloc(HeapObj::Array(Vec::with_capacity(count as usize)));
                self.pinned.push(Value::Array(result_id));
                let pre_frames = self.frames.len();
                let mut early = None;
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                while i <= end_inc {
                    self.invoke_block(block.clone(), vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    self.heap.array_mut(result_id).push(r);
                    i += 1;
                }
                self.pinned.pop();
                self.pinned.pop();
                Some(early.unwrap_or(Value::Array(result_id)))
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
            Op::Call(name_id, argc, cache_id) => {
                self.do_call(name_id, argc as usize, false, cache_id)?;
            }
            Op::CallNoRecv(name_id, argc, cache_id) => {
                self.do_call(name_id, argc as usize, true, cache_id)?;
            }
            Op::CallBlock(name_id, argc, cache_id) => {
                self.do_call_block(name_id, argc as usize, false, cache_id)?;
            }
            Op::CallNoRecvBlock(name_id, argc, cache_id) => {
                self.do_call_block(name_id, argc as usize, true, cache_id)?;
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
                self.method_gen = self.method_gen.wrapping_add(1); // class structure changed
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
            Op::PushRescue(off, slot, bind) => {
                let ip = self.frames.last().expect("ICE: PushRescue no frame").ip;
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                let bind_slot = if bind != 0 { Some(slot) } else { None };
                // Bare `rescue` (the only form we compile today) filters on
                // StandardError to match CRuby's default. Lookup at push-
                // time costs one Interner hit + HashMap probe; the class
                // hierarchy is already loaded by the preamble.
                let stderr_id = self.interner.intern("StandardError");
                let filter = self.classes.get(&stderr_id).cloned();
                self.frames.last_mut().expect("ICE: PushRescue no frame").rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot, is_ensure: false,
                    filter_class: filter,
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
                    filter_class: None, // ensure is unconditional
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
            Op::Break => {
                // Mark the surrounding native-driven loop to terminate.
                // The value the user passed (or `nil`) stays on the
                // operand stack and rides out with the subsequent
                // Op::Return; collection_call_block reads it then.
                self.break_signaled = true;
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
                        self.do_call(name_id, 1, false, u16::MAX)?;
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
                    self.do_call(name_id, 1, false, u16::MAX)?;
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
        (Value::Int(a), "to_i", []) => Some(Value::Int(*a)),
        (Value::Int(a), "abs", []) => Some(Value::Int(a.wrapping_abs())),
        (Value::Int(a), "-@", []) => Some(Value::Int(a.wrapping_neg())),
        (Value::Int(a), "+@", []) => Some(Value::Int(*a)),
        (Value::Int(a), "even?", []) => Some(Value::Bool(a % 2 == 0)),
        (Value::Int(a), "odd?", []) => Some(Value::Bool(a % 2 != 0)),
        (Value::Int(a), "zero?", []) => Some(Value::Bool(*a == 0)),
        (Value::Int(a), "positive?", []) => Some(Value::Bool(*a > 0)),
        (Value::Int(a), "negative?", []) => Some(Value::Bool(*a < 0)),
        (Value::Int(a), "succ", []) | (Value::Int(a), "next", []) => Some(Value::Int(a.wrapping_add(1))),
        (Value::Int(a), "pred", []) => Some(Value::Int(a.wrapping_sub(1))),
        (Value::Str(a), "+", [Value::Str(b)]) => {
            let mut s = a.to_string();
            s.push_str(b);
            Some(Value::Str(Rc::from(s.as_str())))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(**a == **b)),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        (Value::Str(a), "length", []) | (Value::Str(a), "size", []) => Some(Value::Int(a.chars().count() as i64)),
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.is_empty())),
        (Value::Str(a), "upcase", []) => Some(Value::Str(Rc::from(a.to_uppercase().as_str()))),
        (Value::Str(a), "downcase", []) => Some(Value::Str(Rc::from(a.to_lowercase().as_str()))),
        (Value::Str(a), "reverse", []) => Some(Value::Str(Rc::from(a.chars().rev().collect::<String>().as_str()))),
        (Value::Str(a), "strip", []) => Some(Value::Str(Rc::from(a.trim()))),
        (Value::Str(a), "lstrip", []) => Some(Value::Str(Rc::from(a.trim_start()))),
        (Value::Str(a), "rstrip", []) => Some(Value::Str(Rc::from(a.trim_end()))),
        (Value::Str(a), "include?", [Value::Str(b)]) => Some(Value::Bool(a.contains(&**b))),
        (Value::Str(a), "start_with?", [Value::Str(b)]) => Some(Value::Bool(a.starts_with(&**b))),
        (Value::Str(a), "end_with?", [Value::Str(b)]) => Some(Value::Bool(a.ends_with(&**b))),
        (Value::Str(a), "to_i", []) => {
            // CRuby's `String#to_i` is famously lenient: leading
            // whitespace, optional sign, then as many digits as it
            // can read; non-numeric tail (or empty input) gives 0.
            let s = a.trim_start();
            let (sign, rest) = match s.as_bytes().first() {
                Some(b'-') => (-1i64, &s[1..]),
                Some(b'+') => (1i64, &s[1..]),
                _ => (1i64, s),
            };
            let mut n: i64 = 0;
            let mut saw_digit = false;
            for c in rest.chars() {
                if let Some(d) = c.to_digit(10) {
                    saw_digit = true;
                    n = n.wrapping_mul(10).wrapping_add(d as i64);
                } else { break; }
            }
            Some(Value::Int(if saw_digit { sign.wrapping_mul(n) } else { 0 }))
        }
        (Value::Str(a), "*", [Value::Int(n)]) => {
            let n = (*n).max(0) as usize;
            Some(Value::Str(Rc::from(a.repeat(n).as_str())))
        }
        (Value::Str(a), "<", [Value::Str(b)]) => Some(Value::Bool(**a < **b)),
        (Value::Str(a), "<=", [Value::Str(b)]) => Some(Value::Bool(**a <= **b)),
        (Value::Str(a), ">", [Value::Str(b)]) => Some(Value::Bool(**a > **b)),
        (Value::Str(a), ">=", [Value::Str(b)]) => Some(Value::Bool(**a >= **b)),
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
