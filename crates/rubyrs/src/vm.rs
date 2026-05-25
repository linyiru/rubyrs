use std::cell::RefCell;
// `Cell` is only used by the cext-reentrance machinery (CURRENT_VM_PTR),
// itself wasi-stubbed; gate the import so the wasi build doesn't see
// it as unused under `-D warnings`.
#[cfg(not(target_os = "wasi"))]
use std::cell::Cell;
use std::collections::HashMap;
use std::env;
use std::rc::Rc;

use std::io::Write;

use crate::bytecode::{BinOpKind, Op, Proto};
use crate::error::{RubyError, Span, Trap, TrapFrame};
use crate::heap::{Heap, HeapObj};
use crate::intern::{Interner, SymId};
use crate::value::{BlockHandle, Class, Instance, Method, ObjId, Value, Visibility};

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
        (Value::Str(x), Value::Str(y)) => Some(x.borrow().cmp(&*y.borrow())),
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
    /// Block passed to this method, as a heap-managed `Value::Block`
    /// id. Used by `yield`. `None` if the method was called without
    /// a block. Since P2-13 the block lives in the GC heap and we
    /// reference it by `ObjId` — earlier code held an
    /// `Rc<BlockHandle>` here which could cycle.
    pub(crate) block_arg: Option<ObjId>,
    /// Defining class of the method this frame is running.
    /// Used by `Op::Super` — `super` lookup starts at this
    /// class's superclass, not at `self.class.superclass` (the
    /// latter would re-find the current method on a sub-class
    /// instance, causing infinite recursion through the
    /// chain). `None` for blocks, toplevel `<main>`, class
    /// bodies; only methods set this.
    pub(crate) defining_class: Option<Rc<Class>>,
    /// True for frames pushed by `Vm::invoke_block` (the frame
    /// for a `do…end` / `{ … }` body). Used by the non-local
    /// `return`-from-block path: when `Op::ReturnMethod` sets
    /// `method_return`, the dispatch loops pop frames while
    /// `is_block` is true, then pop one more frame to exit the
    /// enclosing method. Method frames, class bodies, and the
    /// toplevel `<main>` keep `false`.
    pub(crate) is_block: bool,
    pub(crate) rescues: Vec<RescueHandler>,
}

/// RAII guard for `Vm.pinned`. Native-side code that needs heap
/// values to survive an intervening `maybe_gc` / `?` early-return
/// constructs one of these, calls `.pin(v)` for every value it
/// wants kept alive, and accesses the VM through `g.vm.foo()` while
/// the guard is in scope. When the guard drops — including on the
/// `?` unwind path — it pops exactly the values it pinned, leaving
/// `pinned` at the same length it had on entry.
///
/// Why this exists: before P0-2, every iterator driver (Array#each,
/// Hash#to_a, the Enumerable filtering family, the
/// `Class.new(args)` allocator) did `self.pinned.push(...); ...; ?;
/// ...; self.pinned.pop();` by hand. The `?` operator could short-
/// circuit past the pop on a raise from a host fn or a fuel trap,
/// leaving dead values on `pinned` that the GC then kept marking as
/// live — slow leak, hard to spot. With this guard the pop is
/// unconditional.
pub(crate) struct PinGuard<'a> {
    pub(crate) vm: &'a mut Vm,
    count: usize,
}

impl<'a> PinGuard<'a> {
    pub(crate) fn new(vm: &'a mut Vm) -> Self {
        Self { vm, count: 0 }
    }
    pub(crate) fn pin(&mut self, v: Value) {
        self.vm.pinned.push(v);
        self.count += 1;
    }
}

impl Drop for PinGuard<'_> {
    fn drop(&mut self) {
        for _ in 0..self.count { self.vm.pinned.pop(); }
    }
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
    /// C-ext singleton-method dispatch table. Indexed by
    /// `(class joined name, method SymId)`. Populated by
    /// `Vm::cext_require` whenever a C ext calls
    /// `rb_define_singleton_method`; consulted by `do_call` when
    /// the receiver is `Value::Class(c)`.
    pub(crate) cext_class_methods: HashMap<String, HashMap<SymId, Rc<HostFn>>>,
    pub(crate) class_stack: Vec<Rc<Class>>,
    /// Per-class-body visibility mode, parallel to `class_stack`.
    /// Pushed `Public` on `Op::DefClass` and popped when the class
    /// body returns. Read by `Op::DefMethod` to stamp new methods
    /// with the current visibility, and mutated by the no-arg
    /// `private` / `protected` / `public` calls.
    pub(crate) class_visibility_stack: Vec<Visibility>,
    /// Compiled-regex cache. Keyed by the interned source-string
    /// SymId; first `LoadRegex` for a given pattern compiles and
    /// caches, subsequent loads return the same Rc.
    pub(crate) regex_cache: HashMap<SymId, Rc<regex::Regex>>,
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
    /// Absolute wall-clock instant past which `eval` traps with
    /// `ResourceExhausted("wall-clock deadline exceeded")`. `None`
    /// means unlimited. Computed at `Runtime::eval` entry from the
    /// `Config::deadline` duration. Checked every 1024 ops (cheap
    /// enough that the syscall amortises out).
    pub(crate) deadline_at: Option<std::time::Instant>,
    /// Lightweight counter incremented per op so deadline checks
    /// only call `Instant::now()` periodically. Wraps; we only
    /// inspect the low bits.
    pub(crate) op_counter: u32,
    /// Cap on distinct interned symbols (P2-14b). `None` means
    /// unlimited. Checked at runtime intern sites (`to_sym`) before
    /// the actual `intern()` call; compile-time intern is not
    /// capped because it's already bounded by source size.
    pub(crate) max_symbols: Option<usize>,
    /// Per-value byte cap (P2-14c). Defends against single
    /// values that hog memory (`"a" * 10_000_000`, `arr <<` in
    /// a tight loop). Checked at mutation sites; see
    /// `Config::max_value_bytes` for the model.
    pub(crate) max_value_bytes: Option<usize>,
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
    /// `Op::ReturnMethod` sets this with the value to return. Both
    /// `dispatch` and `dispatch_until` check it at the top of every
    /// iteration: if `Some`, they unwind frames (block frames first,
    /// then one method frame) and push the value as the method's
    /// return. This is CRuby's non-local-return-from-block
    /// semantics: `return` inside a `do…end` exits the enclosing
    /// method, not just the block.
    pub(crate) method_return: Option<Value>,
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
            cext_class_methods: HashMap::new(),
            class_stack: vec![],
            class_visibility_stack: vec![],
            regex_cache: HashMap::new(),
            stack: Vec::with_capacity(1024),
            frames: vec![],
            heap: Heap::new(),
            pinned: Vec::new(),
            stdout: Box::new(std::io::stdout()),
            stress_gc: env::var("STRESS_GC").is_ok(),
            fuel: None,
            max_frames: None,
            deadline_at: None,
            op_counter: 0,
            max_symbols: None,
            max_value_bytes: None,
            call_caches: Vec::new(),
            method_gen: 0,
            break_signaled: false,
            method_return: None,
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

    /// `Object#respond_to?(name)` semantics: does `recv` have a
    /// callable method named `name`? Used directly by the
    /// `respond_to?` dispatch arm; doesn't invoke anything, so
    /// it's cheap to call from feature-detection guards
    /// (`spec.respond_to?(:add_dependency)`).
    ///
    /// For `Value::Object`, walks the class chain — this is the
    /// precise case and the one most user code actually cares
    /// about. For built-in types we enumerate the methods our
    /// `primitive_call` / `collection_call` / iterator-driver
    /// arms support; the list has to stay in sync as those
    /// arms grow. Universal methods (`nil?`, `to_s`,
    /// `respond_to?` itself, `==` / `!=`) are matched first
    /// regardless of receiver.
    pub(crate) fn responds_to(&self, recv: &Value, name_id: SymId) -> bool {
        let name: &str = &self.interner.resolve(name_id).clone();
        // Universal — every receiver responds to these.
        if matches!(name,
            "nil?" | "to_s" | "respond_to?" | "class" | "==" | "!=" | "!" | "!@" | "<=>" | "equal?"
        ) {
            return true;
        }
        match recv {
            Value::Int(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" |
                "<" | "<=" | ">" | ">=" |
                "&" | "|" | "^" | "<<" | ">>" | "~" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" | "even?" | "odd?" |
                "zero?" | "positive?" | "negative?" |
                "succ" | "next" | "pred" | "-@" | "+@" |
                "times" | "upto" | "downto"
            ),
            Value::Float(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" |
                "<" | "<=" | ">" | ">=" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" |
                "zero?" | "positive?" | "negative?" |
                "nan?" | "infinite?" | "finite?" |
                "floor" | "ceil" | "round" |
                "-@" | "+@"
            ),
            Value::Str(_) => matches!(name,
                "+" | "*" | "%" | "<" | "<=" | ">" | ">=" |
                "length" | "size" | "empty?" |
                "upcase" | "downcase" | "reverse" |
                "strip" | "lstrip" | "rstrip" |
                "include?" | "start_with?" | "end_with?" |
                "to_i" | "to_f" | "chars" | "split" | "to_sym" |
                "to_s" | "inspect" |
                "sub" | "gsub" | "tr" |
                "match?" | "scan" | "index" | "rindex" |
                "[]" | "slice"
            ),
            Value::Sym(_) => matches!(name, "to_sym" | "to_s" | "inspect"),
            Value::Array(_) => matches!(name,
                "length" | "size" | "push" | "<<" | "[]" | "[]=" |
                "first" | "last" | "empty?" | "include?" |
                "count" | "sum" | "min" | "max" | "sort" |
                "inject" | "reduce" |
                "to_a" | "reverse" | "uniq" | "compact" |
                "flatten" | "join" |
                "+" | "-" | "concat" | "take" | "drop" |
                "each" | "map" | "select" | "filter" |
                "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "sort_by" |
                "min_by" | "max_by" | "group_by" |
                "each_with_object" | "partition" |
                "zip" |
                "inspect"
            ),
            Value::Hash(_) => matches!(name,
                "length" | "size" | "[]" | "[]=" | "empty?" |
                "include?" | "has_key?" | "key?" | "member?" |
                "keys" | "values" | "to_h" | "to_a" |
                "merge" | "delete" | "invert" | "store" |
                "each" | "each_pair" |
                "select" | "filter" | "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "map" | "collect" | "fetch" |
                "sort" | "sort_by" | "min_by" | "max_by" | "group_by" |
                "inspect"
            ),
            Value::Range(_) => matches!(name,
                "begin" | "end" | "first" | "last" | "min" | "max" |
                "size" | "length" | "count" |
                "exclude_end?" | "include?" | "to_a" |
                "sum" | "inject" | "reduce" |
                "each" | "map" | "select" | "filter" |
                "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "each_with_object" |
                "partition" | "min_by" | "max_by" |
                "group_by" | "sort_by" | "sort"
            ),
            Value::Bool(_) | Value::Nil => matches!(name, "to_s" | "inspect"),
            Value::Class(_) => matches!(name, "new" | "name"),
            Value::Object(id) => {
                let cls = self.heap.instance(*id).class.clone();
                self.lookup_method_uncached(&cls, name_id).is_some()
            }
            Value::Block(_) => matches!(name, "call"),
            Value::Regex(_) => matches!(name, "match" | "match?" | "===" | "=~" | "source" | "to_s" | "inspect"),
        }
    }

    /// `Object#class` — returns the Class associated with a value.
    /// For user-defined instances that's the stored class; for
    /// built-in types we look up the corresponding stub class
    /// (`Integer`, `String`, ...) installed by the preamble. If
    /// the lookup misses (preamble bug or a user evaling
    /// `Integer.class.superclass` games on a stripped runtime),
    /// returns `Value::Nil` rather than panicking.
    pub(crate) fn class_of(&mut self, recv: &Value) -> Value {
        let name: &'static str = match recv {
            Value::Int(_) => "Integer",
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::Sym(_) => "Symbol",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
            Value::Range(_) => "Range",
            Value::Bool(true) => "TrueClass",
            Value::Bool(false) => "FalseClass",
            Value::Nil => "NilClass",
            Value::Block(_) => "Proc",
            Value::Class(_) => "Class",
            Value::Regex(_) => "Regexp",
            Value::Object(id) => return Value::Class(self.heap.instance(*id).class.clone()),
        };
        let sym = self.interner.intern(name);
        match self.classes.get(&sym) {
            Some(c) => Value::Class(c.clone()),
            None => Value::Nil,
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
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None, is_block: false, rescues: vec![],
        });
        self.dispatch()?;
        Ok(self.stack.pop().unwrap_or(Value::Nil))
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
                while let Some(f) = self.frames.last() {
                    if !f.is_block { break; }
                    let f = self.frames.pop().unwrap();
                    self.stack.truncate(f.base_sp);
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
        // Wall-clock deadline: piggyback on `check_fuel` since both
        // fire on every op. `Instant::now()` is a syscall on most
        // platforms, so we only call it every 1024 ops; this keeps
        // the no-deadline case to a single conditional + an i32
        // increment per op. The op_counter is intentionally `u32`
        // (wraps freely) — we never read its absolute value.
        self.op_counter = self.op_counter.wrapping_add(1);
        if self.op_counter & 1023 == 0 {
            if let Some(at) = self.deadline_at {
                if std::time::Instant::now() >= at {
                    return Err(self.trap(RubyError::ResourceExhausted {
                        msg: "wall-clock deadline exceeded".to_string(),
                    }));
                }
            }
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

    /// Re-entrant dispatch entry for C extensions calling back into
    /// Ruby via `rb_funcall*`. Invokes `recv.method(args)` through
    /// the normal `do_call` path, leaving the result on the stack
    /// where the caller can pop it.
    ///
    /// Setup mirrors what the compiler emits for a Ruby-side
    /// `recv.method(args)`: push the receiver, then each argument,
    /// then call `do_call` with `no_recv = false`. After `do_call`
    /// the result sits on top of the operand stack — pop and return.
    ///
    /// `cache_id = u16::MAX` is a sentinel that
    /// `lookup_method_cached` treats as "no cache slot": the
    /// `idx < call_caches.len()` guard naturally fails (the table
    /// is bounded by the number of compiled `Op::Call` instructions
    /// — nowhere near 65535 in any realistic program), so both the
    /// read and writeback paths short-circuit. Without this sentinel
    /// a hard-coded `cache_id = 0` would poison whichever compiled
    /// call site got slot 0 — that site would silently dispatch
    /// whatever class/method the C ext last invoked. Future work:
    /// allocate a per-`(recv-class, method)` cache for cext calls
    /// if profiling shows the uncached path matters.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn cext_invoke_method(
        &mut self,
        recv: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, Trap> {
        let name_id = self.interner.intern(method);
        let argc = args.len();
        self.stack.push(recv);
        for a in args {
            self.stack.push(a);
        }
        self.do_call(
            name_id,
            argc,
            /* no_recv = */ false,
            /* cache_id = */ u16::MAX,
        )?;
        Ok(self
            .stack
            .pop()
            .expect("ICE: cext_invoke_method: do_call produced no result"))
    }

    /// Look up `method_missing` on `recv`'s class chain. If found,
    /// prepend the missed `name_id` as a Symbol arg and invoke it
    /// (pushing a frame); returns `Ok(true)` so the caller can
    /// `return Ok(())` instead of raising. Returns `Ok(false)` when
    /// the receiver doesn't carry a `method_missing` (or isn't a
    /// `Value::Object`) — caller proceeds to raise NoMethodError.
    ///
    /// Scope of this PoC: only Object receivers (user instances).
    /// Primitive receivers (Int, Str, …) skip the lookup — adding
    /// per-primitive class chains is a follow-up.
    pub(crate) fn try_method_missing(
        &mut self,
        recv: &Value,
        name_id: SymId,
        args: Vec<Value>,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        let cls = match recv {
            Value::Object(id) => self.heap.instance(*id).class.clone(),
            _ => return Ok(false),
        };
        let mm_id = self.interner.intern("method_missing");
        let m = match self.lookup_method_uncached(&cls, mm_id) {
            Some(m) => m,
            None => return Ok(false),
        };
        let mut new_args = Vec::with_capacity(args.len() + 1);
        new_args.push(Value::Sym(name_id));
        new_args.extend(args);
        self.invoke_method_with_block(m, recv.clone(), new_args, block)?;
        Ok(true)
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
                // Stash this Vm pointer for the duration of the host
                // call so `cext_dispatch` (if the host fn is a cext
                // closure) can install a `rb_funcallv` callback that
                // re-enters this Vm. See CURRENT_VM_PTR for the
                // borrow-aliasing discussion.
                #[cfg(not(target_os = "wasi"))]
                let v = {
                    let vm_ptr: *mut Vm = self;
                    with_vm_ptr_set(vm_ptr, || host(&args))?
                };
                #[cfg(target_os = "wasi")]
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
            // `include Mod` inside a class body — `self` is the
            // class, name resolves with no receiver. Mirrors the
            // explicit-receiver branch below; see the comment
            // there for the copy semantics.
            if (&*name == "include" || &*name == "extend") && !args.is_empty() {
                if let Value::Class(target) = &self_val {
                    for a in &args {
                        let src = match a {
                            Value::Class(c) => c.clone(),
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "wrong argument type {} (expected Module)",
                                    a.type_name(),
                                ),
                            })),
                        };
                        let src_methods = src.methods.borrow();
                        let mut tgt_methods = target.methods.borrow_mut();
                        for (mid, m) in src_methods.iter() {
                            tgt_methods.entry(*mid).or_insert_with(|| m.clone());
                        }
                    }
                    self.stack.push(self_val.clone());
                    return Ok(());
                }
            }
            // `private` / `protected` / `public` inside a class
            // body. With no args, switch the current visibility
            // mode for any subsequent `def`s. With Symbol or
            // String args, retroactively flip the visibility of
            // the listed methods on the current class. Outside a
            // class body these are no-ops returning nil — same
            // shape as CRuby's Module#private at the toplevel.
            if let Some(vis) = visibility_from_name(&name) {
                if let Value::Class(cls) = &self_val {
                    if args.is_empty() {
                        if let Some(top) = self.class_visibility_stack.last_mut() {
                            *top = vis;
                        }
                    } else {
                        let methods = cls.methods.borrow();
                        for a in &args {
                            let key: Option<SymId> = match a {
                                Value::Sym(s) => Some(*s),
                                Value::Str(s) => Some(self.interner.intern(&s.borrow())),
                                _ => None,
                            };
                            if let Some(mid) = key {
                                if let Some(m) = methods.get(&mid) {
                                    m.visibility.set(vis);
                                }
                            }
                        }
                    }
                    self.stack.push(Value::Nil);
                    return Ok(());
                }
                // Toplevel `private` / `protected` / `public` —
                // CRuby treats these as visibility modifiers on
                // Object's singleton class. We don't model
                // toplevel methods as Object instance methods, so
                // the call has no observable effect; accept it as
                // a no-op rather than NoMethodError to keep
                // common preamble patterns (`private; def helper;`
                // at the toplevel) parseable.
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // method_missing fallback (PoC #2). For Object self, look
            // up the class chain — if found, hand it the missed name
            // as a Symbol arg. Primitives skip this and raise directly.
            if self.try_method_missing(&self_val, name_id, args, None)? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                method: name.to_string(), recv_type: self_val.type_name(),
            }));
        }

        let recv = recv.expect("ICE: receiver missing");

        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes)
            .map_err(|e| self.trap(e))? {
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
                // The `check_alloc()?` inside the guard is now safe — the
                // guard's Drop pops on the early-return path.
                let id = {
                    let mut g = PinGuard::new(self);
                    for a in &args { g.pin(a.clone()); }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    g.vm.heap.alloc(HeapObj::Instance(Instance {
                        class: cls.clone(),
                        ivars: HashMap::new(),
                    }))
                };
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
                // Private methods cannot be invoked with an
                // explicit receiver. CRuby additionally allows
                // `self.foo` for some private writers; we keep
                // the simpler "any explicit receiver = denied"
                // rule. Protected is enforced as Public here —
                // a real same-instance / same-class check would
                // need to walk the caller's frame, which is
                // beyond this subset's scope.
                if m.visibility.get() == Visibility::Private {
                    return Err(self.trap(RubyError::NoMethodError {
                        method: format!("private method '{name}' called"),
                        recv_type: recv.type_name(),
                    }));
                }
                self.invoke_method(m, recv.clone(), args)?;
                return Ok(());
            }
        }
        // C-ext singleton dispatch: `BCrypt::Engine.__bc_crypt(args)`
        // arrives here with recv = Value::Class(c). Look up the
        // method in the per-class cext table populated by
        // `Vm::cext_require` (rb_define_singleton_method).
        if let Value::Class(cls) = &recv {
            if let Some(table) = self.cext_class_methods.get(&cls.name) {
                if let Some(host) = table.get(&name_id).cloned() {
                    // Stash Vm pointer for the singleton-method's
                    // C body — same rationale as the top-level
                    // host_fns dispatch above.
                    #[cfg(not(target_os = "wasi"))]
                    let v = {
                        let vm_ptr: *mut Vm = self;
                        with_vm_ptr_set(vm_ptr, || host(&args))?
                    };
                    #[cfg(target_os = "wasi")]
                    let v = host(&args)?;
                    self.stack.push(v);
                    return Ok(());
                }
            }
        }
        // `include Mod` — without real Modules in the subset, we
        // approximate by copying the source class's method table
        // into the target class. Only fills methods the target
        // doesn't already define, so user overrides win (matching
        // CRuby's ancestor-chain semantics where own methods
        // shadow included ones). Defines `include` ad-hoc on
        // Class receivers; the call is a no-op for any other
        // receiver and falls through to NoMethodError.
        if let Value::Class(target) = &recv {
            if (&*name == "include" || &*name == "extend") && !args.is_empty() {
                for a in &args {
                    let src = match a {
                        Value::Class(c) => c.clone(),
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Module)",
                                a.type_name(),
                            ),
                        })),
                    };
                    let src_methods = src.methods.borrow();
                    let mut tgt_methods = target.methods.borrow_mut();
                    for (mid, m) in src_methods.iter() {
                        tgt_methods.entry(*mid).or_insert_with(|| m.clone());
                    }
                }
                self.stack.push(recv.clone());
                return Ok(());
            }
        }
        if let Some(v) = self.collection_call(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // `Object#equal?` — identity comparison. For heap-managed
        // receivers, same `ObjId`; for inline values, same content.
        // CRuby never overrides this on subclasses, so we always
        // intercept (above class-lookup would be redundant work).
        if &*name == "equal?" && args.len() == 1 {
            let same = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) => a == b,
                (Value::Array(a), Value::Array(b)) => a == b,
                (Value::Hash(a), Value::Hash(b)) => a == b,
                (Value::Range(a), Value::Range(b)) => a == b,
                (Value::Block(a), Value::Block(b)) => a == b,
                (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
                // Immediates (Int, Float, Sym, Bool, Nil, Str via
                // value equality) — fall back on ruby_eq.
                _ => recv.ruby_eq(&args[0], &self.heap),
            };
            self.stack.push(Value::Bool(same));
            return Ok(());
        }
        // `Object#==` / `Object#!=` cross-type fallback. The
        // per-type primitive arms (`String == String`,
        // `Sym == Sym`, `Class == Class`, etc.) all fired earlier
        // in this dispatch. Anything that reaches here is a
        // cross-type comparison (`"x" == nil`, `nil == :foo`,
        // `[] == ""`) — those return `false` in CRuby, not
        // NoMethodError. Same-type comparisons that we don't
        // have per-type arms for (e.g. `Array == Array`) get
        // value-equality via `ruby_eq`. Universal fallback —
        // never raises — so it must go before NoMethodError.
        if args.len() == 1 && (&*name == "==" || &*name == "!=") {
            let eq = recv.ruby_eq(&args[0], &self.heap);
            let result = if &*name == "==" { eq } else { !eq };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `===` case-equality. Used by `case/when` desugaring.
        // Per-type semantics:
        //   Range#=== → include? (numeric containment)
        //   Class#=== → instance-of (walks ancestor chain)
        //   everything else → `==` value equality
        // User classes can override `===` via class-method
        // lookup, which fires above this fallback (no shadowing
        // needed since the universal check is the last resort).
        if &*name == "===" && args.len() == 1 {
            let arg = &args[0];
            let result = match &recv {
                Value::Range(rid) => {
                    // Generic numeric containment: coerce both
                    // bounds and the arg to Float so Int/Float
                    // mixes (5 in 1..10, 5.0 in 0..10, 5 in 0.0..10.0)
                    // all work. Strings / Symbols compare
                    // lexicographically — handled below.
                    let r = self.heap.range(*rid);
                    fn to_f64(v: &Value) -> Option<f64> {
                        match v {
                            Value::Int(n) => Some(*n as f64),
                            Value::Float(f) => Some(*f),
                            _ => None,
                        }
                    }
                    let excl = r.exclusive;
                    let in_r = match (to_f64(&r.begin), to_f64(&r.end), to_f64(arg)) {
                        (Some(b), Some(e), Some(v)) => {
                            if excl { v >= b && v < e }
                            else { v >= b && v <= e }
                        }
                        _ => {
                            // Non-numeric: fall back to lexicographic
                            // compare using value_cmp_v if both bounds
                            // and the arg are the same comparable type.
                            let b = &r.begin; let e = &r.end;
                            let ge_lo = value_cmp_v(arg, b, &self.interner)
                                .map(|o| o != std::cmp::Ordering::Less)
                                .unwrap_or(false);
                            let cmp_hi = value_cmp_v(arg, e, &self.interner);
                            let le_hi = match cmp_hi {
                                Some(o) => if excl { o == std::cmp::Ordering::Less }
                                           else { o != std::cmp::Ordering::Greater },
                                None => false,
                            };
                            ge_lo && le_hi
                        }
                    };
                    in_r
                }
                Value::Class(target) => {
                    // Walk the argument's class chain looking for
                    // an Rc-identical match with `target`. For
                    // built-in receivers, look up the stub class
                    // by interned type name.
                    let start: Option<Rc<Class>> = match arg {
                        Value::Object(id) => Some(self.heap.instance(*id).class.clone()),
                        _ => {
                            let class_val = self.class_of(arg);
                            if let Value::Class(c) = class_val { Some(c) } else { None }
                        }
                    };
                    let mut cur = start;
                    let mut hit = false;
                    while let Some(cls) = cur {
                        if Rc::ptr_eq(&cls, target) { hit = true; break; }
                        cur = cls.superclass.borrow().clone();
                    }
                    hit
                }
                Value::Regex(re) => match arg {
                    Value::Str(s) => re.is_match(&s.borrow()),
                    _ => false,
                },
                _ => recv.ruby_eq(arg, &self.heap),
            };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `=~` — Regex/String matching. Returns the byte offset of
        // the first match, or nil. CRuby additionally sets `$~`
        // / `$1` etc. capture variables; we don't model `$~`, so
        // captures are accessed via `#match` only.
        if &*name == "=~" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                (Value::Regex(re), Value::Str(s)) | (Value::Str(s), Value::Regex(re)) => {
                    let bound = s.borrow();
                    match re.find(&bound) {
                        Some(m) => Value::Int(m.start() as i64),
                        None => Value::Nil,
                    }
                }
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#<=>` fallback for `Value::Object` receivers. The
        // per-type primitive_call arms above handle every built-in
        // lhs (Int / Float / Str / Bool / Nil — Sym lives in
        // sym_primitive). When we reach here on `<=>`, the only
        // remaining lhs shape is `Value::Object` whose class
        // didn't define `<=>`. CRuby's default `Object#<=>`
        // returns `0` if the two values are identical (in our
        // model: same `ObjId`) and `nil` otherwise. User-defined
        // `<=>` on a class already fired via class-method-lookup
        // earlier, so we don't shadow.
        if &*name == "<=>" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) if a == b => Value::Int(0),
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#class` — universal, no args. Returns the Class
        // associated with the receiver. For built-in types it's
        // the stub class registered by the preamble; for user
        // instances it's the instance's stored class.
        if &*name == "class" && args.is_empty() {
            let c = self.class_of(&recv);
            self.stack.push(c);
            return Ok(());
        }
        // `Object#respond_to?(name)` — pure feature detection, no
        // invocation. Goes last so user classes that override
        // `respond_to?` (we don't support that yet, but conceptually)
        // would shadow this. Accepts either a `Symbol` or a `String`
        // argument; anything else falls through to NoMethodError.
        if &*name == "respond_to?" && args.len() == 1 {
            let lookup_name: Option<SymId> = match &args[0] {
                Value::Sym(id) => Some(*id),
                Value::Str(s) => Some(self.interner.intern(&s.borrow())),
                _ => None,
            };
            if let Some(id) = lookup_name {
                let yes = self.responds_to(&recv, id);
                self.stack.push(Value::Bool(yes));
                return Ok(());
            }
        }
        if self.try_method_missing(&recv, name_id, args, None)? {
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }

    pub(crate) fn collection_call(&mut self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        Ok(match recv {
            Value::Array(id) => {
                let id = *id;
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.array(id).len() as i64)),
                    ("push", [v]) | ("<<", [v]) => {
                        // P2-14c: refuse a push that would make this
                        // Array's storage exceed the per-value byte
                        // cap. We size in bytes-of-Value because that's
                        // what the host actually pays for in RAM.
                        let new_len = self.heap.array(id).len().saturating_add(1);
                        if let Some(max) = self.max_value_bytes {
                            if new_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array.push would exceed {max} bytes"),
                                }));
                            }
                        }
                        self.heap.array_mut(id).push(v.clone());
                        Some(Value::Array(id))
                    }
                    ("[]", [Value::Int(i)]) => {
                        let a = self.heap.array(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                        Some(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                    }
                    // Internal helpers for multi-write splat
                    // destructuring (`a, *r, b = arr`).
                    //
                    // `__mw_splat(start, post)` returns the
                    // middle slice as a fresh Array; underflow
                    // (`len < start + post`) yields `[]`.
                    //
                    // `__mw_get(i, post)` returns `self[i]` if a
                    // pre-splat position truly has an element to
                    // claim once the post-splat slots reserve
                    // theirs (`i < len - post`); otherwise nil.
                    // Without this guard, `a, *m, b = [1]` would
                    // wrongly bind `a = 1` instead of `nil`.
                    ("__mw_splat", [Value::Int(start), Value::Int(post)]) => {
                        let a = self.heap.array(id);
                        let len = a.len() as i64;
                        let s = (*start).max(0).min(len);
                        let p = (*post).max(0).min((len - s).max(0));
                        let slice_len = (len - s - p).max(0) as usize;
                        let s = s as usize;
                        let slice: Vec<Value> = a[s..s + slice_len].to_vec();
                        if let Some(max) = self.max_value_bytes {
                            if slice.len().saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("multi-write splat would exceed {max} bytes"),
                                }));
                            }
                        }
                        self.maybe_gc();
                        let new_id = self.heap.alloc(HeapObj::Array(slice));
                        Some(Value::Array(new_id))
                    }
                    // `__mw_post(j, pre_count, post_count)` —
                    // returns the value for the `j`th post-splat
                    // target (0-indexed from the left of the
                    // post group). CRuby's rule:
                    // `post_start = max(pre_count, len - post_count)`,
                    // then `post[j] = arr[post_start + j]` (OOB → nil).
                    // This pins post-targets to indices >= pre_count
                    // (so pre never gets overwritten) while
                    // sliding them rightward when the array is
                    // long enough to give all post slots their
                    // natural "from the end" positions.
                    ("__mw_post", [Value::Int(j), Value::Int(pre_n), Value::Int(post_n)]) => {
                        let a = self.heap.array(id);
                        let len = a.len() as i64;
                        let pre = (*pre_n).max(0);
                        let post = (*post_n).max(0);
                        let post_start = pre.max(len - post);
                        let idx = post_start + *j;
                        if idx < 0 {
                            Some(Value::Nil)
                        } else {
                            Some(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                        }
                    }
                    ("[]=", [Value::Int(i), v]) => {
                        let a = self.heap.array_mut(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i } as usize;
                        // Same cap check as `push` — `[]=` past the
                        // end pads with `nil` and so can grow the
                        // backing Vec without bound.
                        let needed_len = idx.saturating_add(1).max(a.len());
                        if let Some(max) = self.max_value_bytes {
                            if needed_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array []= would exceed {max} bytes"),
                                }));
                            }
                        }
                        let a = self.heap.array_mut(id);
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
                                _ => return Ok(None),
                            }
                        }
                        Some(Value::Int(s))
                    }
                    ("min", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v(v, &best, &self.interner) {
                                Some(std::cmp::Ordering::Less) => best = v.clone(),
                                Some(_) => {}
                                None => return Ok(None),
                            }
                        }
                        Some(best)
                    }
                    ("max", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v(v, &best, &self.interner) {
                                Some(std::cmp::Ordering::Greater) => best = v.clone(),
                                Some(_) => {}
                                None => return Ok(None),
                            }
                        }
                        Some(best)
                    }
                    ("sort", []) => {
                        // Insertion sort with synchronous dispatch
                        // through `user_cmp`. O(n²) but correctness-
                        // critical: we can't use Rust's sort_by here
                        // because invoking a user method during the
                        // comparison closure would alias `&mut Vm`
                        // while the Vec borrow is live. For arrays
                        // of built-in types the fast path stays
                        // value_cmp_v; user-classed elements go
                        // through their `<=>` method via user_cmp.
                        //
                        // PinGuard wraps the entire impl: `copy` is
                        // a Rust local with the receiver's element
                        // ObjIds, NOT on `vm.stack`/`vm.pinned`. The
                        // `user_cmp` call may invoke a user `<=>`
                        // method which can trigger `maybe_gc` → sweeps
                        // copy's contents → next access panics with
                        // ICE use-after-free. Pin the receiver Array
                        // so its children stay reachable via the GC
                        // mark walk.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let mut copy: Vec<Value> = g.vm.heap.array(id).clone();
                        let n = copy.len();
                        for i in 1..n {
                            let mut j = i;
                            while j > 0 {
                                let ord = g.vm.user_cmp(&copy[j - 1], &copy[j])?;
                                match ord {
                                    None => return Ok(None),
                                    Some(std::cmp::Ordering::Greater) => {
                                        copy.swap(j - 1, j);
                                        j -= 1;
                                    }
                                    _ => break,
                                }
                            }
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(copy));
                        Some(Value::Array(nid))
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        let a = self.heap.array(id).clone();
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        let mut acc = a[0].clone();
                        for v in &a[1..] {
                            match (&acc, v) {
                                (Value::Int(x), Value::Int(y)) => {
                                    if matches!(kind, crate::bytecode::BinOpKind::Div | crate::bytecode::BinOpKind::Mod) && *y == 0 {
                                        return Err(self.trap(RubyError::ZeroDivisionError {
                                            msg: "divided by 0".to_string(),
                                        }));
                                    }
                                    acc = kind.apply_int(*x, *y);
                                }
                                _ => return Ok(None),
                            }
                        }
                        Some(acc)
                    }
                    ("to_a", []) => Some(Value::Array(id)),
                    ("inspect", []) => {
                        let s = Value::Array(id).to_inspect(&self.heap, &self.interner);
                        Some(Value::new_str(s))
                    }
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
                        Some(Value::new_str(parts.join("")))
                    }
                    ("join", [Value::Str(sep)]) => {
                        let parts: Vec<String> = self.heap.array(id).iter()
                            .map(|v| v.to_display(&self.heap, &self.interner))
                            .collect();
                        Some(Value::new_str(parts.join(&*sep.borrow())))
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
                        // Pin the receiver across maybe_gc: by the
                        // time we get here the receiver Array has
                        // been popped from the operand stack, so its
                        // children (the cloned ObjIds in `out`) have
                        // no GC root and STRESS_GC sweeps them.
                        let n = (*n).max(0) as usize;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().take(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("drop", [Value::Int(n)]) => {
                        let n = (*n).max(0) as usize;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().skip(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // `zip` — pairs each element of `self` with the
                    // same-index element of each Array argument.
                    // Result length is the receiver's length. Shorter
                    // arguments pad with `nil`; longer arguments are
                    // truncated. Block form (`zip { ... }` for side
                    // effects, returning nil) is not supported — use
                    // `.zip(...).each` instead.
                    ("zip", rest) => {
                        let mut others: Vec<Vec<Value>> = Vec::with_capacity(rest.len());
                        for a in rest {
                            match a {
                                Value::Array(oid) => others.push(self.heap.array(*oid).clone()),
                                _ => return Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "wrong argument type {} (must respond to :to_ary)",
                                        a.type_name(),
                                    ),
                                })),
                            }
                        }
                        let base = self.heap.array(id).clone();
                        let row_width = 1 + others.len();
                        if let Some(max) = self.max_value_bytes {
                            let projected = base.len()
                                .saturating_mul(row_width)
                                .saturating_mul(std::mem::size_of::<Value>());
                            if projected > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array#zip would exceed {max} bytes"),
                                }));
                            }
                        }
                        // PinGuard the freshly-alloc'd row Arrays: their
                        // ObjIds live in a Rust local `out` Vec, NOT on
                        // `vm.stack` / `vm.pinned`, so the explicit
                        // `maybe_gc()` after the loop (or a STRESS_GC
                        // gc on every alloc) would sweep them and the
                        // outer `heap.alloc(out)` would then panic with
                        // `ICE: use-after-free`. Same shape as L1.5 P0-A.
                        let nid = {
                            let mut g = PinGuard::new(self);
                            let mut out: Vec<Value> = Vec::with_capacity(base.len());
                            for (i, v) in base.iter().enumerate() {
                                let mut row: Vec<Value> = Vec::with_capacity(row_width);
                                row.push(v.clone());
                                for o in &others {
                                    row.push(o.get(i).cloned().unwrap_or(Value::Nil));
                                }
                                let rid = g.vm.heap.alloc(HeapObj::Array(row));
                                let rv = Value::Array(rid);
                                g.pin(rv.clone());
                                out.push(rv);
                            }
                            g.vm.maybe_gc();
                            g.vm.heap.alloc(HeapObj::Array(out))
                        };
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
                            if key.ruby_eq(k, &self.heap) { return Ok(Some(val.clone())); }
                        }
                        Some(Value::Nil)
                    }
                    ("[]=", [k, v]) => {
                        // Need a way to compare without borrowing heap while mutating.
                        // Snapshot positions first.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        // P2-14c byte cap: only a key that isn't
                        // already present grows the table. Update
                        // of an existing key is free (size-wise).
                        if pos.is_none() {
                            let new_len = self.heap.hash(id).len().saturating_add(1);
                            if let Some(max) = self.max_value_bytes {
                                if new_len.saturating_mul(std::mem::size_of::<(Value, Value)>()) > max {
                                    return Err(self.trap(RubyError::ResourceExhausted {
                                        msg: format!("Hash []= would exceed {max} bytes"),
                                    }));
                                }
                            }
                        }
                        let h = self.heap.hash_mut(id);
                        if let Some(p) = pos {
                            h[p].1 = v.clone();
                        } else {
                            h.push((k.clone(), v.clone()));
                        }
                        Some(v.clone())
                    }
                    ("empty?", []) => Some(Value::Bool(self.heap.hash(id).is_empty())),
                    ("fetch", [k]) => {
                        // 1-arg fetch: return value or raise KeyError.
                        // The Trap is routed through the rescue
                        // machinery by `dispatch`, so a script
                        // `begin ... rescue KeyError => e; ... end`
                        // catches it like CRuby.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        match pos {
                            Some(p) => Some(self.heap.hash(id)[p].1.clone()),
                            None => {
                                return Err(self.trap(RubyError::KeyError {
                                    msg: format!("key not found: {}",
                                        k.to_inspect(&self.heap, &self.interner)),
                                }));
                            }
                        }
                    }
                    ("fetch", [k, default]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        Some(match pos {
                            Some(p) => self.heap.hash(id)[p].1.clone(),
                            None => default.clone(),
                        })
                    }
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
                    ("inspect", []) => {
                        let s = Value::Hash(id).to_inspect(&self.heap, &self.interner);
                        Some(Value::new_str(s))
                    }
                    ("to_a", []) | ("sort", []) => {
                        // Hash#to_a returns an Array of two-element Arrays.
                        // Each inner [k, v] is freshly heap-allocated; we
                        // need every inner Array kept alive as we
                        // accumulate, otherwise the next loop iter's
                        // `maybe_gc` will sweep the previous pair (it's
                        // only live via the Rust-local Vec, not via any
                        // GC root). Failing to pin produces slot-reuse
                        // cycles that explode `to_display`'s recursion.
                        //
                        // Hash#sort (no block) is just to_a sorted by
                        // key using <=> — handled below with an
                        // insertion sort over the pair list. We share
                        // the build path because both produce an
                        // Array<[k, v]>.
                        let mut pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        if name == "sort" {
                            let n = pairs.len();
                            for i in 1..n {
                                let mut j = i;
                                while j > 0 {
                                    let ord = {
                                        let a = pairs[j - 1].0.clone();
                                        let b = pairs[j].0.clone();
                                        self.user_cmp(&a, &b)?
                                    };
                                    match ord {
                                        None => return Ok(None),
                                        Some(std::cmp::Ordering::Greater) => {
                                            pairs.swap(j - 1, j);
                                            j -= 1;
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                        let nid = {
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id)); // source Hash
                            let mut pair_ids: Vec<Value> = Vec::with_capacity(pairs.len());
                            for (k, v) in pairs {
                                g.vm.maybe_gc();
                                let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                                g.pin(Value::Array(pid));
                                pair_ids.push(Value::Array(pid));
                            }
                            g.vm.maybe_gc();
                            g.vm.heap.alloc(HeapObj::Array(pair_ids))
                        };
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
                // String#[] / #slice — char-indexed slicing.
                // CRuby's semantics:
                //   s[i]           -> single-char String, or nil
                //   s[i, n]        -> substring of n chars from i,
                //                     or nil if i out of bounds
                //                     (i == len is OK and gives "")
                //   s[Range]       -> substring; nil for invalid start
                // Negative indices count from the end; out-of-bounds
                // returns nil. Multibyte strings are sliced by char,
                // not by byte.
                fn str_index_char(chars: &[char], i: i64) -> Option<usize> {
                    let len = chars.len() as i64;
                    let idx = if i < 0 { len + i } else { i };
                    if idx < 0 || idx > len { None }
                    else { Some(idx as usize) }
                }
                fn str_slice(chars: &[char], start: usize, n: usize) -> String {
                    chars.iter().skip(start).take(n).collect()
                }
                if (name == "[]" || name == "slice") && args.len() == 1 {
                    let chars: Vec<char> = s.borrow().chars().collect();
                    let len = chars.len() as i64;
                    return Ok(Some(match &args[0] {
                        Value::Int(i) => {
                            let idx = if *i < 0 { len + *i } else { *i };
                            if idx < 0 || idx >= len {
                                Value::Nil
                            } else {
                                let ch = chars[idx as usize].to_string();
                                Value::new_str(ch)
                            }
                        }
                        Value::Range(rid) => {
                            let r = self.heap.range(*rid);
                            let (bi, ei, excl) = match (&r.begin, &r.end) {
                                (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                                _ => return Ok(None),
                            };
                            let start = match str_index_char(&chars, bi) {
                                Some(s) => s,
                                None => return Ok(Some(Value::Nil)),
                            };
                            // End index: positive raw; negative
                            // relative to len. Out-of-range high
                            // clamps to len; exclusive drops one.
                            let mut end = if ei < 0 { len + ei } else { ei };
                            if !excl { end += 1; }
                            let end = end.clamp(start as i64, len) as usize;
                            let slice: String = str_slice(&chars, start, end.saturating_sub(start));
                            Value::new_str(slice)
                        }
                        _ => return Ok(None),
                    }));
                }
                if (name == "[]" || name == "slice") && args.len() == 2 {
                    if let (Value::Int(i), Value::Int(n)) = (&args[0], &args[1]) {
                        let chars: Vec<char> = s.borrow().chars().collect();
                        let len = chars.len() as i64;
                        let start_raw = if *i < 0 { len + *i } else { *i };
                        if start_raw < 0 || start_raw > len || *n < 0 {
                            return Ok(Some(Value::Nil));
                        }
                        let start = start_raw as usize;
                        let n = (*n as usize).min(chars.len() - start);
                        let slice = str_slice(&chars, start, n);
                        return Ok(Some(Value::new_str(slice)));
                    }
                    return Ok(None);
                }
                // String#[]= — in-place mutation. Three shapes:
                //   s[i]      = x   → replace one char at char-index i
                //   s[i, n]   = x   → replace n chars from char-index i
                //   s[range]  = x   → replace the slice covered by the range
                // Negative indices count from the end. Out-of-range
                // raises IndexError, matching CRuby (we surface that
                // through the Trap-to-rescue path).
                //
                // The mutation works because Value::Str holds an
                // Rc<RefCell<String>>: every clone of this Value
                // shares the same RefCell, so writes through
                // `borrow_mut` are visible to all aliases.
                if name == "[]=" && args.len() == 2 {
                    if let (Value::Int(i), Value::Str(repl)) = (&args[0], &args[1]) {
                        let chars: Vec<char> = s.borrow().chars().collect();
                        let len = chars.len() as i64;
                        let idx = if *i < 0 { len + *i } else { *i };
                        if idx < 0 || idx >= len {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {i} out of string"),
                            }));
                        }
                        let mut buf: String = chars[..idx as usize].iter().collect();
                        buf.push_str(&repl.borrow());
                        buf.extend(chars[idx as usize + 1..].iter());
                        *s.borrow_mut() = buf;
                        return Ok(Some(args[1].clone()));
                    }
                    return Ok(None);
                }
                if name == "[]=" && args.len() == 3 {
                    if let (Value::Int(i), Value::Int(n), Value::Str(repl)) = (&args[0], &args[1], &args[2]) {
                        let chars: Vec<char> = s.borrow().chars().collect();
                        let len = chars.len() as i64;
                        let start_raw = if *i < 0 { len + *i } else { *i };
                        if start_raw < 0 || start_raw > len || *n < 0 {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {i} out of string"),
                            }));
                        }
                        let start = start_raw as usize;
                        let take = (*n as usize).min(chars.len() - start);
                        let mut buf: String = chars[..start].iter().collect();
                        buf.push_str(&repl.borrow());
                        buf.extend(chars[start + take..].iter());
                        *s.borrow_mut() = buf;
                        return Ok(Some(args[2].clone()));
                    }
                    return Ok(None);
                }
                match (name, args) {
                    ("chars", []) => {
                        let elems: Vec<Value> = s.borrow().chars()
                            .map(|c| Value::new_str(c.to_string()))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", []) => {
                        // No-arg `split` matches CRuby's `split(nil)`:
                        // splits on runs of whitespace, drops the
                        // leading empty token.
                        let elems: Vec<Value> = s.borrow().split_whitespace()
                            .map(|t| Value::new_str(t))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", [Value::Str(sep)]) => {
                        let elems: Vec<Value> = if sep.borrow().is_empty() {
                            // CRuby: empty-sep split returns each character.
                            s.borrow().chars().map(|c| Value::new_str(c.to_string())).collect()
                        } else {
                            s.borrow().split(&*sep.borrow()).map(|t| Value::new_str(t)).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("%", [single_arg]) => {
                        // Build the argument list. A single Array
                        // splats into positional args; everything
                        // else is a one-element list. This matches
                        // CRuby's `format`/`String#%` calling
                        // convention.
                        let owned;
                        let fmt_args: &[Value] = match single_arg {
                            Value::Array(arr_id) => {
                                owned = self.heap.array(*arr_id).clone();
                                owned.as_slice()
                            }
                            _ => std::slice::from_ref(single_arg),
                        };
                        let out = ruby_sprintf(&s.borrow(), fmt_args, &self.heap, &self.interner)
                            .map_err(|e| self.trap(e))?;
                        if let Some(max) = self.max_value_bytes {
                            if out.len() > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("String#% would exceed {max} bytes"),
                                }));
                            }
                        }
                        Some(Value::new_str(out))
                    }
                    // Literal-substring `scan` — returns a fresh
                    // Array containing one copy of the pattern for
                    // every non-overlapping occurrence in the
                    // receiver. CRuby's full `scan` accepts a
                    // Regexp and yields capture groups; literal
                    // patterns are the degenerate case where every
                    // match is the pattern itself, exactly what we
                    // implement. An empty pattern returns
                    // `[""] * (chars + 1)` to match CRuby; this is
                    // unusual but well-defined and cheap.
                    ("scan", [Value::Str(pat)]) => {
                        let parts: Vec<Value> = if pat.borrow().is_empty() {
                            std::iter::repeat_with(|| Value::new_str(""))
                                .take(s.borrow().chars().count() + 1)
                                .collect()
                        } else {
                            let mut out: Vec<Value> = Vec::new();
                            let mut i = 0;
                            let s_ref = s.borrow();
                            let bytes = s_ref.as_bytes();
                            let pat_ref = pat.borrow();
                            let plen = pat_ref.len();
                            while i + plen <= bytes.len() {
                                if &bytes[i..i + plen] == pat_ref.as_bytes() {
                                    out.push(Value::Str(pat.clone()));
                                    i += plen;
                                } else {
                                    i += 1;
                                }
                            }
                            out
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(parts));
                        Some(Value::Array(id))
                    }
                    ("to_sym", []) => {
                        // P2-14b: cap the interner before a hot loop
                        // (`arr.map { |x| x.to_s.to_sym }` and similar)
                        // can quietly grow it without bound. Existing
                        // symbols always re-resolve; only fresh strings
                        // count against the cap.
                        if let Some(max) = self.max_symbols {
                            if !self.interner.contains(&s.borrow()) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        }
                        let sym = self.interner.intern(&s.borrow());
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
                    _ => return Ok(None),
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
                    ("to_a", []) | ("sort", []) => {
                        // `sort` with no block on a Range is just
                        // `to_a` — the underlying sequence is
                        // already non-decreasing for Int bounds.
                        // Descending (bi > ei) is an empty range
                        // in CRuby; we render it as `[]` to match.
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
                        if bi > end_inc { return Ok(Some(Value::Int(init))); }
                        let n = end_inc - bi + 1;
                        let s = n.wrapping_mul(bi.wrapping_add(end_inc)) / 2;
                        Some(Value::Int(init.wrapping_add(s)))
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        let end_inc = if excl { ei - 1 } else { ei };
                        if bi > end_inc { return Ok(Some(Value::Nil)); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        let mut acc = Value::Int(bi);
                        let mut i = bi + 1;
                        while i <= end_inc {
                            match &acc {
                                Value::Int(x) => {
                                    if matches!(kind, crate::bytecode::BinOpKind::Div | crate::bytecode::BinOpKind::Mod) && i == 0 {
                                        return Err(self.trap(RubyError::ZeroDivisionError {
                                            msg: "divided by 0".to_string(),
                                        }));
                                    }
                                    acc = kind.apply_int(*x, i);
                                }
                                _ => return Ok(None),
                            }
                            i += 1;
                        }
                        Some(acc)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
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
                        // ensure is unconditional — always runs.
                        true
                    } else if let Some(filter) = &h.filter_class {
                        // explicit class filter (including bare
                        // `rescue` which compiles to StandardError).
                        exc_class.as_ref().map_or(false, |cls| class_is_a(cls, filter))
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
                    Value::Object(id) => self.heap.instance(*id).class.name.clone(),
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
            if let Some(id) = f.block_arg {
                // Block lives in the GC heap now (P2-13). Pushing
                // the Value::Block root is enough — the mark phase
                // walks the BlockHandle's `captured` and `self_val`
                // when it reaches the slot.
                roots.push(Value::Block(id));
            }
        }
        // `define_method`-installed methods carry captured-locals
        // Rcs that aren't reachable from any Frame once the lexical
        // scope has popped. Walk every class's method table (plus
        // the toplevel table) and root the captured slots so heap
        // values held only via a closure survive GC.
        //
        // Cost is O(total installed methods). For programs that
        // never use `define_method`, the inner `if let Some` short-
        // circuits — this is a single field-check per Method. ADR-
        // worthy optimisation if we ever care: track a counter of
        // closure-methods on the Vm and skip this entirely when 0.
        for cls in self.classes.values() {
            for m in cls.methods.borrow().values() {
                if let Some(cl) = &m.closure {
                    for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
                }
            }
        }
        for m in self.toplevel_methods.values() {
            if let Some(cl) = &m.closure {
                for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
            }
        }
        self.heap.collect(&roots);
    }

    pub(crate) fn invoke_method(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>) -> Result<(), Trap> {
        self.invoke_method_with_block(m, self_val, args, None)
    }

    /// Compare two values using built-in types first, then falling
    /// back to invoking the left-hand side's user-defined `<=>`.
    /// Returns `None` for incomparable pairs (built-in cross-type
    /// mismatches, or a user `<=>` that returns `nil`). Used by
    /// `Array#sort` so user classes that define `<=>` (typically
    /// via `include Comparable`) sort sensibly. Synchronously
    /// dispatches the user method by pushing a frame and running
    /// `dispatch_until` — the same pattern iterator drivers use.
    pub(crate) fn user_cmp(&mut self, a: &Value, b: &Value) -> Result<Option<std::cmp::Ordering>, Trap> {
        if let Some(ord) = value_cmp_v(a, b, &self.interner) {
            return Ok(Some(ord));
        }
        // Try the receiver's `<=>` method (user-defined). Only
        // Value::Object can have user methods; other receivers
        // would have been resolved by value_cmp_v above.
        if let Value::Object(id) = a {
            let cls = self.heap.instance(*id).class.clone();
            let spaceship = self.interner.intern("<=>");
            if let Some(m) = self.lookup_method_uncached(&cls, spaceship) {
                let pre_frames = self.frames.len();
                let mut g = PinGuard::new(self);
                g.pin(a.clone());
                g.pin(b.clone());
                g.vm.invoke_method(m, a.clone(), vec![b.clone()])?;
                g.vm.dispatch_until(pre_frames)?;
                let result = g.vm.stack.pop().unwrap_or(Value::Nil);
                drop(g);
                return Ok(match result {
                    Value::Int(n) if n < 0 => Some(std::cmp::Ordering::Less),
                    Value::Int(0) => Some(std::cmp::Ordering::Equal),
                    Value::Int(_) => Some(std::cmp::Ordering::Greater),
                    _ => None,
                });
            }
        }
        Ok(None)
    }

    pub(crate) fn invoke_method_with_block(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>, block: Option<ObjId>) -> Result<(), Trap> {
        // `define_method`-installed methods carry a captured Rc and
        // diverge from the normal fresh-locals path: their frame
        // *shares* `captured` with the lexical scope that created
        // the block. Writes to outer-scope locals from inside the
        // method body propagate back, matching CRuby semantics.
        if let Some(cl) = &m.closure {
            let given = args.len();
            let n_params = cl.n_params as usize;
            if given != n_params {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected {})", given, n_params),
                }));
            }
            self.check_frames()?;
            let proto_idx = m.proto_idx;
            let proto_n_locals = self.protos[proto_idx].n_locals as usize;
            let param_start = cl.param_start as usize;
            // Block params live *after* the captured frame's n_locals
            // (block locals layout inherits the parent — see ADR 0004).
            // Resize the shared Vec if a previous invocation hasn't
            // already grown it.
            {
                let mut caps = cl.captured.borrow_mut();
                let need = param_start.max(proto_n_locals);
                if caps.len() < need {
                    caps.resize(need, Value::Nil);
                }
                for (i, a) in args.into_iter().enumerate() {
                    caps[param_start + i] = a;
                }
            }
            self.frames.push(Frame {
                proto_idx,
                ip: 0,
                locals: cl.captured.clone(),
                self_val,
                base_sp: self.stack.len(),
                is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.clone(), is_block: false, rescues: vec![],
            });
            return Ok(());
        }
        // Default-argument support (literal defaults only): a Proto
        // carries a `defaults` vec parallel to `params`. `None`
        // entries are required; `Some(v)` entries can be omitted by
        // the caller and the slot is filled from the literal at
        // invocation time. Required params always come before
        // optionals in source order, so the legal arg-count range
        // is `[required, params.len()]`.
        let proto = &self.protos[m.proto_idx];
        let required = proto.defaults.iter().take_while(|d| d.is_none()).count();
        let max_args = m.params.len();
        let given = args.len();
        if given < required || given > max_args {
            let expected = if required == max_args {
                format!("{}", required)
            } else {
                format!("{}..{}", required, max_args)
            };
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected {})", given, expected),
            }));
        }
        self.check_frames()?;
        let n_locals = proto.n_locals as usize;
        // Snapshot defaults for the omitted-slot fill, since we're
        // about to take `&mut self` to push the frame.
        let default_fill: Vec<Value> = (given..max_args).map(|i| {
            // `i < required` is impossible: `given >= required`
            // already, so any `i in given..max_args` lands in the
            // optional range, which has Some(v).
            proto.defaults[i].clone().unwrap_or(Value::Nil)
        }).collect();
        let mut locals = vec_nil(n_locals);
        for (i, a) in args.into_iter().enumerate() {
            locals[i] = a;
        }
        for (offset, v) in default_fill.into_iter().enumerate() {
            locals[given + offset] = v;
        }
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals: Rc::new(RefCell::new(locals)),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.clone(), is_block: false, rescues: vec![],
        });
        Ok(())
    }

    pub(crate) fn invoke_block(&mut self, block_id: ObjId, args: Vec<Value>) -> Result<(), Trap> {
        self.check_frames()?;
        // Snapshot what we need out of the block's heap slot before
        // taking any `&mut self` action. BlockHandle.captured is a
        // shared `Rc<RefCell<Vec<Value>>>` — cheap to clone.
        let (proto_idx, captured, self_val, param_start, n_params) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.self_val.clone(), bh.param_start, bh.n_params)
        };
        // CRuby auto-splat: when a block declared with >1 parameter
        // is called with a single Array argument, the Array's
        // elements are spread into the parameter slots. The most
        // common ergonomic surfaces this enables:
        //   arr_of_pairs.each { |a, b| ... }       # arr = [[1,2], [3,4]]
        //   hash.each_with_index { |(k, v), i| }   # pair + index
        //   hash.to_a.sort_by { |k, v| v }         # pair after Hash#to_a
        // Hash#each / #map already yield two args directly, so this
        // path doesn't change their behaviour. Single-param blocks
        // also unaffected — they bind the whole Array.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            // Place args into the block's param slots
            for (i, a) in args.into_iter().enumerate() {
                if i < n_params as usize {
                    locals[param_start as usize + i] = a;
                }
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: captured,
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None,
            is_block: true, rescues: vec![],
        });
        Ok(())
    }

    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let block_val = self.stack.pop().expect("ICE: stack underflow before block");
        let block = if let Value::Block(id) = block_val { id } else {
            panic!("ICE: CallBlock without Block value on stack");
        };
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before block receiver"))
        };

        // P2-13: `block` (now an ObjId in a Rust local) is no
        // longer rooted after popping off the stack. Each native
        // iterator driver (`iter_array_filter`, the inline
        // `each` / `map` arms, etc.) pins the block alongside its
        // source receiver, so we don't need a guard at the
        // dispatch boundary itself. The `invoke_method_with_block`
        // path on the no_recv / Object-recv branches doesn't
        // trigger GC before installing the block as the frame's
        // `block_arg`, so the gap is safe there too.
        if let Some(r) = &recv {
            if let Some(v) = self.collection_call_block(r, &name, &args, block)? {
                self.stack.push(v);
                return Ok(());
            }
        }

        if no_recv {
            if let Some(res) = self.builtin_call(&name, &args) { self.stack.push(res?); return Ok(()); }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                #[cfg(not(target_os = "wasi"))]
                let v = {
                    let vm_ptr: *mut Vm = self;
                    with_vm_ptr_set(vm_ptr, || host(&args))?
                };
                #[cfg(target_os = "wasi")]
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
            if self.try_method_missing(&self_val, name_id, args, Some(block))? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                method: name.to_string(), recv_type: self_val.type_name(),
            }));
        }
        let recv = recv.expect("ICE: receiver missing for block call");
        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes).map_err(|e| self.trap(e))? { self.stack.push(v); return Ok(()); }
        if let Some(v) = self.sym_primitive(&recv, &name, &args) { self.stack.push(v); return Ok(()); }
        let new_id = self.interner.intern("new");
        if name_id == new_id {
            if let Value::Class(cls) = &recv {
                // Pin args during the alloc window — see the matching
                // comment in `do_call`'s new-branch for the rationale.
                let id = {
                    let mut g = PinGuard::new(self);
                    for a in &args { g.pin(a.clone()); }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    g.vm.heap.alloc(HeapObj::Instance(Instance {
                        class: cls.clone(), ivars: HashMap::new(),
                    }))
                };
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
        if self.try_method_missing(&recv, name_id, args, Some(block))? {
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }

    /// Drives an iterator-with-predicate over an Array. Used by
    /// `select` / `reject` / `find` / `any?` / `all?` / `none?`.
    /// On `break val` (caught via `self.break_signaled`) returns `val`
    /// to match CRuby's "break value short-circuits the enumerator".
    pub(crate) fn iter_array_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Value, Trap> {
        let snapshot: Vec<Value> = self.heap.array(id).clone();
        let mut g = PinGuard::new(self);
        g.pin(Value::Array(id));
        // P2-13: block lives in the GC heap; pin it for the
        // duration of the iteration so any GC fired by the block
        // body doesn't sweep it.
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
            g.pin(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        for v in snapshot {
            g.vm.invoke_block(block,vec![v.clone()])?;
            g.vm.dispatch_until(pre_frames)?;
            if g.vm.method_return.is_some() { break; }
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            if g.vm.break_signaled {
                g.vm.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Reject => if !truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Find => if truthy { find_val = v; break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
        }
        // PinGuard drops at function exit, including the `?` paths above.
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
    pub(crate) fn iter_hash_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Value, Trap> {
        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
        let mut g = PinGuard::new(self);
        g.pin(Value::Hash(id));
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Hash(Vec::new()));
            g.pin(Value::Hash(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        for (k, v) in snapshot {
            g.vm.invoke_block(block,vec![k.clone(), v.clone()])?;
            g.vm.dispatch_until(pre_frames)?;
            if g.vm.method_return.is_some() { break; }
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            if g.vm.break_signaled {
                g.vm.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Reject => if !truthy { g.vm.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Find => if truthy {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    find_val = Value::Array(pair);
                    break;
                }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
        }
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
    pub(crate) fn iter_range_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Option<Value>, Trap> {
        let (bi, ei, excl) = {
            let r = self.heap.range(id);
            match (&r.begin, &r.end) {
                (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                _ => return Ok(None),
            }
        };
        let mut g = PinGuard::new(self);
        g.pin(Value::Range(id));
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
            g.pin(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        let end_inc = if excl { ei - 1 } else { ei };
        let mut i = bi;
        while i <= end_inc {
            g.vm.invoke_block(block,vec![Value::Int(i)])?;
            g.vm.dispatch_until(pre_frames)?;
            if g.vm.method_return.is_some() { break; }
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            if g.vm.break_signaled {
                g.vm.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Reject => if !truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Find => if truthy { find_val = Value::Int(i); break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
            i += 1;
        }
        if let Some(e) = early { return Ok(Some(e)); }
        Ok(Some(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        }))
    }

    pub(crate) fn collection_call_block(&mut self, recv: &Value, name: &str, args: &[Value], block: ObjId) -> Result<Option<Value>, Trap> {
        Ok(match (recv, name, args) {
            (Value::Array(id), "each", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block,vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "map", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block,vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    g.vm.heap.array_mut(result_id).push(r);
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Hash(id), "each", []) | (Value::Hash(id), "each_pair", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.invoke_block(block,vec![k, v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "each_with_index", []) => {
                // Block invocation per CRuby: `(pair, idx)` where
                // `pair` is the fresh `[k, v]` Array. The block
                // running with a single param gets `pair` (an
                // Array). Two-param destructured form
                // (`|pair, idx|`) is what users usually want.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (i, (k, v)) in snapshot.into_iter().enumerate() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.pin(Value::Array(pair_id));
                    g.vm.invoke_block(block, vec![Value::Array(pair_id), Value::Int(i as i64)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "map", []) | (Value::Hash(id), "collect", []) => {
                // `h.map { |k, v| ... }` — yields each (k, v) and
                // collects block return values into a new Array.
                // CRuby returns an `Enumerator` for no-block, which
                // we don't have; falls through to NoMethodError if
                // misused that way.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.invoke_block(block, vec![k, v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    g.vm.heap.array_mut(result_id).push(r);
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Hash(id), "fetch", [k]) => {
                // Block form: `h.fetch(k) { |k| default_expr }`.
                // Block is invoked only on miss; CRuby ignores the
                // 2-arg fetch + block combo (warns); we silently
                // accept it (handled in non-block path too).
                let id = *id;
                let pos = self.heap.hash(id).iter()
                    .position(|(key, _)| key.ruby_eq(k, &self.heap));
                if let Some(p) = pos {
                    return Ok(Some(self.heap.hash(id)[p].1.clone()));
                }
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                g.pin(k.clone());
                let pre_frames = g.vm.frames.len();
                g.vm.invoke_block(block, vec![k.clone()])?;
                g.vm.dispatch_until(pre_frames)?;
                if g.vm.method_return.is_some() { return Ok(Some(Value::Nil)); }
                let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                Some(r)
            }
            (Value::Int(start), "upto", [Value::Int(stop)]) => {
                let start = *start;
                let stop = *stop;
                let pre_frames = self.frames.len();
                let mut early = None;
                let mut i = start;
                while i <= stop {
                    self.invoke_block(block,vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    if self.method_return.is_some() { break; }
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
                    self.invoke_block(block,vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    if self.method_return.is_some() { break; }
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
                    self.invoke_block(block,vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    if self.method_return.is_some() { break; }
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
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Range(*id)))
            }
            (Value::Array(id), "each_with_index", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (i, v) in snapshot.into_iter().enumerate() {
                    g.vm.invoke_block(block,vec![v, Value::Int(i as i64)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "each_with_object", [seed]) => {
                // `arr.each_with_object(memo) { |elem, memo| ... }`.
                // CRuby threads `memo` unchanged across iterations
                // (unlike inject which uses the block's return as the
                // next accumulator). The block's return value is
                // ignored; users mutate `memo` for side effects.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                g.pin(seed.clone());
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v, seed.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or_else(|| seed.clone()))
            }
            (Value::Array(id), "partition", []) => {
                // `arr.partition { |x| pred(x) }` returns
                // `[truthy_array, falsy_array]` — exactly two new
                // Arrays. Used a lot in routing / grouping idioms.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let yes_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(yes_id));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let no_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(no_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() {
                        g.vm.heap.array_mut(yes_id).push(v);
                    } else {
                        g.vm.heap.array_mut(no_id).push(v);
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![
                    Value::Array(yes_id), Value::Array(no_id),
                ]));
                Some(Value::Array(pair_id))
            }
            (Value::Array(id), "min_by", []) | (Value::Array(id), "max_by", []) => {
                // For each element, call the block once to produce a
                // key. Track the running winner. Returns nil for an
                // empty array (matching CRuby). Block-keys that
                // aren't mutually comparable surface as NoMethodError
                // via `value_cmp_v` returning None for one of them —
                // same shape as sort_by.
                let want_min = name == "min_by";
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut best: Option<(Value, Value)> = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    best = Some(match best {
                        None => (key, v),
                        Some((bk, bv)) => match value_cmp_v(&key, &bk, &g.vm.interner) {
                            Some(std::cmp::Ordering::Less) if want_min => (key, v),
                            Some(std::cmp::Ordering::Greater) if !want_min => (key, v),
                            // Equal or wrong direction — keep prior.
                            Some(_) => (bk, bv),
                            // Incomparable keys — fall through to None below.
                            None => return Ok(None),
                        },
                    });
                }
                if let Some(e) = early { return Ok(Some(e)); }
                Some(best.map(|(_, v)| v).unwrap_or(Value::Nil))
            }
            (Value::Array(id), "group_by", []) => {
                // Group elements into a Hash keyed by the block's
                // return value. Insertion order matches first
                // appearance of each key — CRuby semantics. Values
                // collect into a fresh Array per key.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(Vec::new()));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    // Find or create the bucket array for this key.
                    let pos = g.vm.heap.hash(result_id).iter()
                        .position(|(k, _)| k.ruby_eq(&key, &g.vm.heap));
                    if let Some(p) = pos {
                        if let Value::Array(arr_id) = g.vm.heap.hash(result_id)[p].1 {
                            g.vm.heap.array_mut(arr_id).push(v);
                        }
                    } else {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let arr_id = g.vm.heap.alloc(HeapObj::Array(vec![v]));
                        g.vm.heap.hash_mut(result_id).push((key, Value::Array(arr_id)));
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                Some(Value::Hash(result_id))
            }
            (Value::Array(id), "sort_by", []) => {
                // PinGuard wraps the entire impl — the previous code
                // dropped the guard after the key-collection loop,
                // leaving `pairs` (a Rust local) to carry ObjId-
                // bearing element Values through `user_cmp` insertion
                // sort and the trailing `maybe_gc()` with no GC root.
                // Symptom: `.to_a.sort_by` chains where the receiver
                // Array of pairs has no other anchor → pair Arrays
                // swept → dangling slots in the result.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let arr = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(arr.len());
                let mut early: Option<Value> = None;
                for v in arr {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    g.pin(key.clone());
                    g.pin(v.clone());
                    pairs.push((key, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let n = pairs.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let (k_prev, k_curr) = {
                            let (a, b) = pairs.split_at(j);
                            (a[j - 1].0.clone(), b[0].0.clone())
                        };
                        let ord = g.vm.user_cmp(&k_prev, &k_curr)?;
                        match ord {
                            None => return Ok(None),
                            Some(std::cmp::Ordering::Greater) => {
                                pairs.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                let sorted: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let nid = g.vm.heap.alloc(HeapObj::Array(sorted));
                Some(Value::Array(nid))
            }
            (Value::Array(id), "inject", []) | (Value::Array(id), "reduce", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = snapshot[0].clone();
                let mut early = None;
                for v in &snapshot[1..] {
                    g.vm.invoke_block(block,vec![acc.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "inject", [init]) | (Value::Array(id), "reduce", [init]) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                for v in &snapshot {
                    g.vm.invoke_block(block,vec![acc.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "count", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block,vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() { n += 1; }
                }
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
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = Value::Int(bi);
                let mut early = None;
                let mut i = bi + 1;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![acc.clone(), Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                    i += 1;
                }
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
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![acc.clone(), Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                    i += 1;
                }
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
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() { n += 1; }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(n)))
            }

            (Value::Array(id), "select", []) | (Value::Array(id), "filter", []) => Some(self.iter_array_filter(*id, IterMode::Select, block)?),
            (Value::Array(id), "reject", []) => Some(self.iter_array_filter(*id, IterMode::Reject, block)?),
            (Value::Array(id), "find", []) | (Value::Array(id), "detect", []) => Some(self.iter_array_filter(*id, IterMode::Find, block)?),
            (Value::Array(id), "any?", []) => Some(self.iter_array_filter(*id, IterMode::Any, block)?),
            (Value::Array(id), "all?", []) => Some(self.iter_array_filter(*id, IterMode::All, block)?),
            (Value::Array(id), "none?", []) => Some(self.iter_array_filter(*id, IterMode::NoneM, block)?),

            // Hash#min_by / #max_by — yield (k, v) to the block,
            // pick the pair whose block-returned key is the
            // extremum. Result is the winning [k, v] as a fresh
            // 2-element Array, matching CRuby. Empty hash → nil.
            (Value::Hash(id), op @ ("min_by" | "max_by"), []) => {
                let want_max = op == "max_by";
                let pairs: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                if pairs.is_empty() { return Ok(Some(Value::Nil)); }
                let mut best: Option<(Value, Value, Value)> = None;
                let mut early: Option<Value> = None;
                {
                    let mut g = PinGuard::new(self);
                    g.pin(Value::Hash(*id));
                    g.pin(Value::Block(block));
                    let pre_frames = g.vm.frames.len();
                    for (k, v) in pairs {
                        g.vm.invoke_block(block, vec![k.clone(), v.clone()])?;
                        g.vm.dispatch_until(pre_frames)?;
                        if g.vm.method_return.is_some() { break; }
                        let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                        if g.vm.break_signaled {
                            g.vm.break_signaled = false;
                            early = Some(key);
                            break;
                        }
                        best = match best {
                            None => Some((k, v, key)),
                            Some((bk, bv, bkey)) => {
                                let ord = match value_cmp_v(&key, &bkey, &g.vm.interner) {
                                    Some(o) => o,
                                    None => return Ok(None),
                                };
                                let want_replace = if want_max {
                                    ord == std::cmp::Ordering::Greater
                                } else {
                                    ord == std::cmp::Ordering::Less
                                };
                                if want_replace { Some((k, v, key)) }
                                else { Some((bk, bv, bkey)) }
                            }
                        };
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if let Some((k, v, _)) = best {
                    // PinGuard the winning pair across the explicit
                    // `maybe_gc`: previously k/v were Rust locals
                    // with no root, so STRESS_GC could sweep them
                    // before the new Array was alloc'd → dangling
                    // ObjIds inside the result.
                    let mut g = PinGuard::new(self);
                    g.pin(k.clone());
                    g.pin(v.clone());
                    g.vm.maybe_gc();
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    Some(Value::Array(pid))
                } else {
                    Some(Value::Nil)
                }
            }

            // Hash#sort_by — yield (k, v), use returned key as the
            // sort key, return an Array of [k, v] pairs in key
            // order. Stability preserved via insertion sort.
            (Value::Hash(id), "sort_by", []) => {
                // PinGuard wraps the *entire* impl, not just the
                // block-invocation phase. Previously the guard
                // dropped before the post-loop `maybe_gc`, leaving
                // `keyed` (a Rust local) holding ObjId-bearing
                // Values with no GC root → STRESS_GC swept them and
                // the resulting Array<[k,v]> had dangling slots
                // that exploded inside `to_display`.
                let pairs_in: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let mut keyed: Vec<(Value, Value, Value)> = Vec::with_capacity(pairs_in.len());
                let mut early: Option<Value> = None;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                for (k, v) in pairs_in {
                    g.vm.invoke_block(block, vec![k.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    // Pin each accumulated triple component so the
                    // next iter's invoke_block (which may GC) can't
                    // sweep them.
                    g.pin(key.clone());
                    g.pin(k.clone());
                    g.pin(v.clone());
                    keyed.push((key, k, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let n = keyed.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let ord = {
                            let a = keyed[j - 1].0.clone();
                            let b = keyed[j].0.clone();
                            g.vm.user_cmp(&a, &b)?
                        };
                        match ord {
                            None => return Ok(None),
                            Some(std::cmp::Ordering::Greater) => {
                                keyed.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                g.vm.maybe_gc();
                let mut out: Vec<Value> = Vec::with_capacity(keyed.len());
                for (_, k, v) in keyed {
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    let pv = Value::Array(pid);
                    g.pin(pv.clone());
                    out.push(pv);
                }
                let oid = g.vm.heap.alloc(HeapObj::Array(out));
                Some(Value::Array(oid))
            }

            // Hash#group_by — bucket pairs by the block's return.
            // Each bucket is an Array of [k, v] pairs; the result
            // is a Hash from group-key → Array.
            (Value::Hash(id), "group_by", []) => {
                // Same GC root-hole pattern as sort_by above: the
                // previous impl scoped PinGuard only across the
                // block invocation, then dropped it and ran more
                // alloc work (with `maybe_gc`) over `buckets` and
                // each freshly-built pair Array. Extend the guard
                // and pin each new ObjId as it's created.
                let pairs_in: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let mut buckets: Vec<(Value, Vec<Value>)> = Vec::new();
                let mut early: Option<Value> = None;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                for (k, v) in pairs_in {
                    g.vm.invoke_block(block, vec![k.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let group = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(group);
                        break;
                    }
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    let pair = Value::Array(pid);
                    g.pin(pair.clone());
                    g.pin(group.clone());
                    let pos = buckets.iter().position(|(gk, _)| gk.ruby_eq(&group, &g.vm.heap));
                    match pos {
                        Some(p) => buckets[p].1.push(pair),
                        None => buckets.push((group, vec![pair])),
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                let mut hash_pairs: Vec<(Value, Value)> = Vec::with_capacity(buckets.len());
                for (gk, vs) in buckets {
                    let aid = g.vm.heap.alloc(HeapObj::Array(vs));
                    let av = Value::Array(aid);
                    g.pin(av.clone());
                    hash_pairs.push((gk, av));
                }
                let hid = g.vm.heap.alloc(HeapObj::Hash(hash_pairs));
                Some(Value::Hash(hid))
            }

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
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let count = if excl { (ei - bi).max(0) } else { (ei - bi + 1).max(0) };
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(count as usize)));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    g.vm.heap.array_mut(result_id).push(r);
                    i += 1;
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }

            // Range Enumerable fallback: materialize as an Array
            // and re-dispatch through the Array arms above. This
            // gets each_with_index / each_with_object / partition
            // / min_by / max_by / group_by / sort_by "for free"
            // and keeps a single source of truth for the
            // iteration semantics. Cost: one Vec<Value::Int>
            // allocation. Only Int-bounded ranges (the common
            // case) qualify — heterogeneous ranges would need
            // their own dispatch.
            (Value::Range(id), name, args) if matches!(name,
                "each_with_index" | "each_with_object" |
                "partition" | "min_by" | "max_by" |
                "group_by" | "sort_by"
            ) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut elems: Vec<Value> = Vec::new();
                let mut v = bi;
                while v <= end_inc {
                    elems.push(Value::Int(v));
                    v += 1;
                }
                // Pin the block AND every incoming arg FIRST: a
                // STRESS_GC pass triggered by `maybe_gc` below could
                // otherwise sweep the block-handle slot or an arg
                // value (e.g. the memo Hash passed to
                // `each_with_object({})`) — neither is necessarily
                // on the operand stack at this point, only borrowed
                // through `&[Value]` from the dispatch caller, which
                // doesn't count as a GC root. Symptoms were the
                // "ICE: heap slot is not a Block" and "is not a Hash"
                // panics in `range_enumerable`.
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                for a in args { g.pin(a.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let arr_id = g.vm.heap.alloc(HeapObj::Array(elems));
                g.pin(Value::Array(arr_id));
                let arr_val = Value::Array(arr_id);
                return g.vm.collection_call_block(&arr_val, name, args, block);
            }
            _ => None,
        })
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
            Op::CreateBlock(p_idx, param_start, n_params) => {
                // Snapshot the surrounding frame's captured locals
                // (shared Rc with subsequent invocations of this
                // block) and self before any mutable borrow of
                // `self`, then allocate the BlockHandle into the
                // heap. The stack value is a plain `ObjId`.
                let (captured, self_val) = {
                    let f = self.frames.last().expect("ICE: CreateBlock no frame");
                    (f.locals.clone(), f.self_val.clone())
                };
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Block(BlockHandle {
                    proto_idx: p_idx as usize,
                    captured,
                    self_val,
                    param_start,
                    n_params,
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
                let defining_class = self.class_stack.last().cloned();
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
                let defining_class = self.class_stack.last().cloned();
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
                self.class_visibility_stack.push(Visibility::Public);
                let proto = &self.protos[p_idx as usize];
                let n_locals = proto.n_locals as usize;
                self.frames.push(Frame {
                    proto_idx: p_idx as usize, ip: 0,
                    locals: Rc::new(RefCell::new(vec_nil(n_locals))),
                    self_val: Value::Class(cls.clone()),
                    base_sp: self.stack.len(),
                    is_class_body: true, swap_return: None, block_arg: None, defining_class: None, is_block: false, rescues: vec![],
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
                let ip = self.frames.last().expect("ICE: PushRescue no frame").ip;
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
                self.unwind_with_exception(exc)?;
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

pub(crate) fn vec_nil(n: usize) -> Vec<Value> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n { v.push(Value::Nil); }
    v
}

impl Vm {
    pub(crate) fn builtin_call(&mut self, name: &str, args: &[Value]) -> Option<Result<Value, Trap>> {
        match name {
            "puts" => {
                // CRuby's `puts` flattens arrays: each element is
                // printed on its own line, recursively. Empty
                // string still gets a newline (so `puts ""` and
                // `puts` look identical). Empty array prints
                // nothing.
                fn puts_one(vm: &mut Vm, v: &Value) {
                    match v {
                        Value::Array(id) => {
                            let snapshot: Vec<Value> = vm.heap.array(*id).clone();
                            for item in &snapshot { puts_one(vm, item); }
                        }
                        _ => {
                            let s = v.to_display(&vm.heap, &vm.interner);
                            let _ = writeln!(vm.stdout, "{}", s);
                        }
                    }
                }
                if args.is_empty() {
                    let _ = writeln!(self.stdout);
                } else {
                    for a in args {
                        let cloned = a.clone();
                        puts_one(self, &cloned);
                    }
                }
                Some(Ok(Value::Nil))
            }
            // Kernel#p — print each arg's `inspect` form, one per
            // line. Return value mirrors CRuby: nil for zero args,
            // the lone arg for one, an Array of the args for more.
            // `pp` is aliased to `p` (we don't have a pretty-
            // printer; single-line inspect is sufficient for our
            // subset and matches CRuby for simple values).
            // `__method__` / `__callee__` — return the enclosing
            // method's name as a Symbol, or nil at the toplevel.
            // Walks the frame stack from the top, skipping block
            // and class-body frames so a `def foo; arr.each { ... }`
            // inside reports `:foo` from inside the block. CRuby
            // distinguishes the two when method aliasing is
            // involved — we don't model aliases, so both resolve
            // to the same name.
            "__method__" | "__callee__" => {
                let name_opt: Option<String> = {
                    let mut found = None;
                    for f in self.frames.iter().rev() {
                        if f.is_block || f.is_class_body { continue; }
                        let n = &self.protos[f.proto_idx].name;
                        if n == "<main>" { break; }
                        found = Some(n.clone());
                        break;
                    }
                    found
                };
                let result = match name_opt {
                    Some(n) => Value::Sym(self.interner.intern(&n)),
                    None => Value::Nil,
                };
                Some(Ok(result))
            }
            "p" | "pp" => {
                for a in args {
                    let s = a.to_inspect(&self.heap, &self.interner);
                    let _ = writeln!(self.stdout, "{}", s);
                }
                let result = match args {
                    [] => Value::Nil,
                    [one] => one.clone(),
                    many => {
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(many.to_vec()));
                        Value::Array(id)
                    }
                };
                Some(Ok(result))
            }
            // `Integer(x)` / `Float(x)` / `String(x)` — strict
            // conversion functions. Unlike `to_i` / `to_f` (which
            // are lenient — `"abc".to_i` returns 0), these raise
            // ArgumentError on input that can't be cleanly parsed.
            // The canonical Ruby idiom for "convert or fail loudly",
            // typically wrapped in an inline rescue:
            //   port = Integer(ENV['PORT']) rescue 8080
            "Integer" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let result = match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => {
                        if !f.is_finite() {
                            Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Integer", crate::heap::format_float(*f)),
                            })
                        } else { Ok(Value::Int(*f as i64)) }
                    }
                    Value::Str(s) => {
                        let raw = s.borrow();
                        let trimmed = raw.trim();
                        match trimmed.parse::<i64>() {
                            Ok(n) => Ok(Value::Int(n)),
                            Err(_) => Err(RubyError::ArgumentError {
                                msg: format!("invalid value for Integer(): \"{}\"", raw),
                            }),
                        }
                    }
                    Value::Nil => Err(RubyError::TypeError {
                        msg: "can't convert nil into Integer".into(),
                    }),
                    other => Err(RubyError::TypeError {
                        msg: format!("can't convert {} into Integer", other.type_name()),
                    }),
                };
                Some(result.map_err(|e| self.trap(e)))
            }
            "Float" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let result = match &args[0] {
                    Value::Float(f) => Ok(Value::Float(*f)),
                    Value::Int(n) => Ok(Value::Float(*n as f64)),
                    Value::Str(s) => {
                        let raw = s.borrow();
                        let trimmed = raw.trim();
                        match trimmed.parse::<f64>() {
                            Ok(f) => Ok(Value::Float(f)),
                            Err(_) => Err(RubyError::ArgumentError {
                                msg: format!("invalid value for Float(): \"{}\"", raw),
                            }),
                        }
                    }
                    Value::Nil => Err(RubyError::TypeError {
                        msg: "can't convert nil into Float".into(),
                    }),
                    other => Err(RubyError::TypeError {
                        msg: format!("can't convert {} into Float", other.type_name()),
                    }),
                };
                Some(result.map_err(|e| self.trap(e)))
            }
            // `String(x)` — calls `to_s` for any value. Lenient (it
            // doesn't raise on weird input — to_s should always
            // succeed for our built-in types).
            "String" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let s = args[0].to_display(&self.heap, &self.interner);
                Some(Ok(Value::new_str(s)))
            }
            "print" => {
                for a in args {
                    let s = a.to_display(&self.heap, &self.interner);
                    let _ = write!(self.stdout, "{}", s);
                }
                Some(Ok(Value::Nil))
            }
            // C-ext compat spike (Level 0). Only supports the literal-path
            // form (`require "/abs/path/to/hello"` with auto-extension);
            // gem/load-path resolution is deferred.
            "require" => match args {
                [Value::Str(path)] => {
                    let path = path.borrow().clone();
                    Some(self.cext_require(&path))
                }
                _ => Some(Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "require: expected 1 String arg, got {}",
                        args.len()
                    ),
                }))),
            },
            _ => None,
        }
    }

    /// Load a C extension shared library, run its `Init_<stem>` symbol,
    /// and register every function it declared via
    /// `rb_define_global_function` into `self.host_fns`.
    ///
    /// Level 0 caveats:
    /// - Only literal paths (with optional auto-extension) are resolved;
    ///   `$LOAD_PATH` and gem lookup are deferred.
    /// - Loaded libraries are leaked (never unloaded). A real impl
    ///   tracks them on the Vm and unloads on drop.
    /// - Only arity 0 callbacks dispatch correctly; other arities
    ///   register, then trap on invocation with an ArgumentError.
    ///
    /// wasm32-wasi has no `dlopen` — a separate
    /// `#[cfg(target_os = "wasi")]` stub below returns a clear Trap
    /// instead of the dlopen path.
    #[cfg(not(target_os = "wasi"))]
    fn cext_require(&mut self, path_str: &str) -> Result<Value, Trap> {
        use libloading::{Library, Symbol};
        use std::path::Path;

        // Auto-extension: `require "foo"` resolves "foo.dylib" / "foo.so"
        // / "foo.bundle" depending on host. Matches CRuby's behaviour for
        // the literal-path case.
        let exts: &[&str] = if cfg!(target_os = "macos") {
            &["dylib", "bundle"]
        } else if cfg!(windows) {
            &["dll"]
        } else {
            &["so"]
        };
        let p = Path::new(path_str);
        let so_path = if p.exists() {
            p.to_path_buf()
        } else {
            let mut found = None;
            for ext in exts {
                let with = p.with_extension(ext);
                if with.exists() {
                    found = Some(with);
                    break;
                }
            }
            found.ok_or_else(|| {
                self.trap(RubyError::RuntimeError {
                    msg: format!("cannot find C ext: {}", path_str),
                })
            })?
        };

        let stem = so_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                self.trap(RubyError::RuntimeError {
                    msg: format!("invalid C ext filename: {}", so_path.display()),
                })
            })?
            .to_string();
        let init_sym = format!("Init_{}", stem);

        // SAFETY: dlopen is intrinsically unsafe; the C ext can do
        // anything. We trust extensions we explicitly load — sandboxing
        // is for the Ruby-language layer, not the FFI layer.
        unsafe {
            rubyrs_cext::enter();
            let lib = match Library::new(&so_path) {
                Ok(l) => l,
                Err(e) => {
                    let _ = rubyrs_cext::leave();
                    return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("dlopen {}: {}", so_path.display(), e),
                    }));
                }
            };
            let init: Symbol<unsafe extern "C" fn()> = match lib.get(init_sym.as_bytes()) {
                Ok(s) => s,
                Err(e) => {
                    let _ = rubyrs_cext::leave();
                    return Err(self.trap(RubyError::RuntimeError {
                        msg: format!(
                            "symbol {} not found in {}: {}",
                            init_sym,
                            so_path.display(),
                            e
                        ),
                    }));
                }
            };
            init();
            let state = rubyrs_cext::leave();

            for cfn in state.registered_fns {
                let sym = self.interner.intern(&cfn.name);
                let func = cfn.func;
                let arity = cfn.arity;
                let cfn_name = cfn.name.clone();
                self.host_fns.insert(
                    sym,
                    Rc::new(move |args: &[Value]| {
                        // Top-level functions get Qnil as `self`,
                        // matching CRuby's `rb_define_global_function`
                        // convention.
                        cext_dispatch(&cfn_name, func, arity, args, None)
                    }),
                );
            }

            // Materialise every class/module the C ext declared, so
            // `LoadConst("BCrypt::Engine")` finds them.
            for cls in state.registered_classes {
                let name_sym = self.interner.intern(&cls.joined_name);
                let new_class = Rc::new(Class {
                    name: cls.joined_name.clone(),
                    methods: RefCell::new(HashMap::new()),
                    superclass: RefCell::new(None),
                });
                self.classes.insert(name_sym, new_class);
            }

            // Register every singleton method into the per-class
            // dispatch table consulted by `do_call`.
            for sm in state.registered_singletons {
                let method_sym = self.interner.intern(&sm.method_name);
                let func = sm.func;
                let arity = sm.arity;
                let class_name = sm.class_joined_name.clone();
                let qualified = format!("{}.{}", class_name, sm.method_name);
                self.cext_class_methods
                    .entry(sm.class_joined_name)
                    .or_default()
                    .insert(
                        method_sym,
                        Rc::new(move |args: &[Value]| {
                            // Singleton methods get the class itself
                            // as `self`, matching CRuby's
                            // `rb_define_singleton_method` contract.
                            cext_dispatch(&qualified, func, arity, args, Some(&class_name))
                        }),
                    );
            }

            // Level 0: keep the library mapped for the lifetime of the
            // process. Registered function pointers point into its
            // text segment; unmapping would dangle them. A real impl
            // stores `lib` on the Vm so it drops with the Vm.
            std::mem::forget(lib);
        }

        Ok(Value::Nil)
    }

    /// wasm32-wasi alt for [`Vm::cext_require`]. WASI has no dynamic
    /// loader, so any `require "path/to/some.so"` from Ruby on wasi
    /// has no way to succeed; we trap with a precise message instead
    /// of silently returning Nil. Native targets get the dlopen-based
    /// implementation above.
    #[cfg(target_os = "wasi")]
    fn cext_require(&mut self, path_str: &str) -> Result<Value, Trap> {
        Err(self.trap(RubyError::RuntimeError {
            msg: format!(
                "require: C-ext loading is not supported on wasm32-wasi (attempted to load {})",
                path_str
            ),
        }))
    }
}

// Thread-local raw pointer to the currently-active Vm during a
// host-fn call. Set by `do_call` (via `with_vm_ptr_set`) before
// invoking entries from `host_fns` / `cext_class_methods`, cleared
// after. Read by `cext_dispatch` when installing the `rb_funcallv`
// callback so re-entrant C-to-Ruby calls dispatch on the right Vm.
//
// SAFETY / BORROW ALIASING NOTE — this deliberately routes around
// Rust's borrow checker. When `do_call` invokes a host fn, `&mut
// self` is held for the duration of that call. If the host fn
// re-enters the Vm via `rb_funcallv`, the callback dereferences
// this raw pointer to obtain a fresh `&mut Vm`, aliasing the outer
// borrow. Stacked Borrows considers this UB; Tree Borrows is more
// permissive. In practice the two `&mut`s are time-disjoint (only
// one is used at any instant). Documented here so a future
// contributor doesn't "fix" it by sprinkling `&mut self` borrows
// that violate the invariant. See ADR (forthcoming) for the
// safer-but-bigger refactor that would move Vm into an
// `UnsafeCell`-flavoured container.
//
// Wasi-gated for the same reason `cext_dispatch` is: the cext path
// is unreachable when there's no dynamic loader.
#[cfg(not(target_os = "wasi"))]
thread_local! {
    static CURRENT_VM_PTR: Cell<*mut Vm> = const { Cell::new(std::ptr::null_mut()) };
}

/// RAII guard that restores [`CURRENT_VM_PTR`] to its previous value
/// when dropped — runs the restore on **every** scope exit, including
/// panic unwinding. Without this guard, a panic inside the host fn
/// (e.g. from arg interning before `cext_dispatch` installs its
/// `with_caught_unwind` boundary) would leave a stale Vm pointer in
/// `CURRENT_VM_PTR`; a subsequent host-fn call would then dereference
/// it as a fresh `*mut Vm`, hitting use-after-free or worse.
#[cfg(not(target_os = "wasi"))]
struct VmPtrGuard {
    prev: *mut Vm,
}

#[cfg(not(target_os = "wasi"))]
impl Drop for VmPtrGuard {
    fn drop(&mut self) {
        CURRENT_VM_PTR.with(|c| c.set(self.prev));
    }
}

/// Run `f` with [`CURRENT_VM_PTR`] set to `vm_ptr`, restoring the
/// previous value (likely null) on **all** exit paths — normal return
/// or panic unwinding — via [`VmPtrGuard`]. Save/restore lets nested
/// cext calls (rb_funcallv → another host fn) work without the inner
/// call clobbering the outer's pointer.
#[cfg(not(target_os = "wasi"))]
fn with_vm_ptr_set<R>(vm_ptr: *mut Vm, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_VM_PTR.with(|c| c.replace(vm_ptr));
    let _guard = VmPtrGuard { prev };
    f()
}

/// Read the currently-active Vm pointer. Returns null if not inside
/// a host-fn invocation; callers that hit null have an ICE.
#[cfg(not(target_os = "wasi"))]
fn current_vm_ptr() -> *mut Vm {
    CURRENT_VM_PTR.with(|c| c.get())
}

/// RAII guard around `rubyrs_cext::enter()` / `leave()`. Normal path
/// calls [`Self::into_state`] to consume the guard and receive the
/// drained `CExtState`. Panic path runs `Drop`, which discards the
/// state but always pops the stack — so a panic between `enter()`
/// and the matching pop doesn't leave a leaked CExtState on the
/// thread-local stack to corrupt subsequent cext calls.
#[cfg(not(target_os = "wasi"))]
struct CExtStateGuard {
    /// True until `into_state` consumes the guard. Tracks whether
    /// `Drop` should still pop (only on the panic path).
    active: bool,
}

#[cfg(not(target_os = "wasi"))]
impl CExtStateGuard {
    fn enter() -> Self {
        rubyrs_cext::enter();
        Self { active: true }
    }

    /// Consume the guard on the normal path, returning the drained
    /// `CExtState` for handle translation. Suppresses the `Drop`
    /// pop because the caller has already taken responsibility.
    fn into_state(mut self) -> rubyrs_cext::CExtState {
        self.active = false;
        rubyrs_cext::leave()
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for CExtStateGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = rubyrs_cext::leave();
        }
    }
}

/// RAII guard around `push_funcall_callback` / `pop_funcall_callback`.
/// Always pops on `Drop`, whether normal scope exit or panic unwinding.
/// Without this guard, a panic after the callback push but before the
/// matching pop would leak the callback into the next cext call.
#[cfg(not(target_os = "wasi"))]
struct FuncallCallbackGuard;

#[cfg(not(target_os = "wasi"))]
impl FuncallCallbackGuard {
    fn install(cb: rubyrs_cext::FuncallCallback) -> Self {
        rubyrs_cext::push_funcall_callback(cb);
        Self
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for FuncallCallbackGuard {
    fn drop(&mut self) {
        rubyrs_cext::pop_funcall_callback();
    }
}

/// Translate a C-side opaque handle back into a `Value`. Currently
/// covers exactly the `CValue` variants the spike supports.
///
/// Gated off `target_os = "wasi"` because the only caller chain
/// (`cext_dispatch` invoked from closures registered in
/// `Vm::cext_require`) is itself wasi-stubbed. Without the gate the
/// `-D dead-code` warning fires on the wasi build.
/// Bounded recursion depth for translating C-built Array/Hash
/// structures back into rubyrs `Value`. A C extension can construct
/// a self-referential `CValue::Array(_)` (e.g. `a.push(a)` from C);
/// without a depth limit the recursion would stack-overflow during
/// `cext_handle_to_value`. 256 is generous for realistic
/// JSON-shape inputs and well below the host stack limit.
#[cfg(not(target_os = "wasi"))]
const CEXT_TRANSLATE_MAX_DEPTH: usize = 256;

#[cfg(not(target_os = "wasi"))]
fn cext_handle_to_value(
    vm: &mut Vm,
    state: &rubyrs_cext::CExtState,
    h: rubyrs_cext::Value,
) -> Result<Value, Trap> {
    cext_handle_to_value_d(vm, state, h, 0)
}

#[cfg(not(target_os = "wasi"))]
fn cext_handle_to_value_d(
    vm: &mut Vm,
    state: &rubyrs_cext::CExtState,
    h: rubyrs_cext::Value,
    depth: usize,
) -> Result<Value, Trap> {
    if depth >= CEXT_TRANSLATE_MAX_DEPTH {
        // Pathological input — cycle or implausibly deep nesting in
        // the C-built Array/Hash. Surface as an ArgumentError Trap
        // (review #24 follow-up): the previous silent-Nil shape was
        // hard to debug for a C ext author. The Trap unwinds through
        // the cext call chain back into Ruby with a clear message.
        return Err(Trap::new(RubyError::ArgumentError {
            msg: format!(
                "C ext result: max translation depth {} exceeded \
                 (cycle or implausibly deep Array/Hash nesting)",
                CEXT_TRANSLATE_MAX_DEPTH
            ),
        }));
    }
    Ok(match state.resolve(h) {
        rubyrs_cext::CValue::Nil => Value::Nil,
        rubyrs_cext::CValue::True => Value::Bool(true),
        rubyrs_cext::CValue::False => Value::Bool(false),
        // CValue::Str stores bytes + sentinel NUL; the logical
        // string is `.len() - 1` bytes. Decode lossily into UTF-8
        // since rubyrs's Value::Str is `Rc<str>` (UTF-8). Binary-
        // safe storage on the rubyrs side lands in a later level.
        rubyrs_cext::CValue::Str(bytes) => {
            let logical = &bytes[..bytes.len().saturating_sub(1)];
            Value::new_str(String::from_utf8_lossy(logical))
        }
        rubyrs_cext::CValue::Int(n) => Value::Int(*n),
        // Class handles are returned from `rb_define_module` /
        // `rb_define_class_under`; bcrypt's wrappers don't return
        // them as plain values to Ruby, but if a future ext does,
        // surface as Nil for now (no Class lookup from raw name
        // outside the rubyrs::classes registry yet).
        rubyrs_cext::CValue::Class(_) => Value::Nil,
        // Recursive translation: an Array/Hash CValue is a vector of
        // C-side handles; build a Vec<Value> by recursing on each,
        // then allocate on the Vm heap. PinGuard protects the
        // children from being collected mid-build when a child's
        // recursive allocation triggers `maybe_gc`.
        rubyrs_cext::CValue::Array(handles) => {
            let mut g = PinGuard::new(vm);
            let mut elements: Vec<Value> = Vec::with_capacity(handles.len());
            for child in handles {
                let v = cext_handle_to_value_d(g.vm, state, *child, depth + 1)?;
                g.pin(v.clone());
                elements.push(v);
            }
            g.vm.maybe_gc();
            // Heap-cap exhaustion now propagates the original
            // ResourceExhausted Trap up to Ruby (review #26).
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::Array(elements));
            Value::Array(id)
        }
        rubyrs_cext::CValue::Hash(pairs) => {
            let mut g = PinGuard::new(vm);
            let mut entries: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
            for (kh, vh) in pairs {
                let k = cext_handle_to_value_d(g.vm, state, *kh, depth + 1)?;
                g.pin(k.clone());
                let v = cext_handle_to_value_d(g.vm, state, *vh, depth + 1)?;
                g.pin(v.clone());
                entries.push((k, v));
            }
            g.vm.maybe_gc();
            // Review #27 — same Trap propagation as Array arm above.
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::Hash(entries));
            Value::Hash(id)
        }
    })
}

/// Translate a rubyrs [`Value`] into the corresponding [`rubyrs_cext::CValue`]
/// so it can be interned as a C-visible handle. Supported variants today:
/// Nil, Bool, Str (binary-safe via Vec<u8> + sentinel NUL), Int. Types
/// that cross only as runtime references (Sym ids, Class<Rc>, Object/
/// Array/Hash/Range/Block heap ids) trap with `ArgumentError` until the
/// matching ABI surface (`rb_sym_new`, `rb_class_new`, heap-handle
/// translation) lands.
#[cfg(not(target_os = "wasi"))]
fn cext_value_to_cvalue(
    vm: &Vm,
    st: &mut rubyrs_cext::CExtState,
    name: &str,
    idx: usize,
    v: &Value,
) -> Result<rubyrs_cext::CValue, Trap> {
    cext_value_to_cvalue_d(vm, st, name, idx, v, 0)
}

/// Bounded-depth helper for [`cext_value_to_cvalue`]. Mirrors the
/// `CEXT_TRANSLATE_MAX_DEPTH` discipline applied on the C → Ruby
/// direction (see [`cext_handle_to_value_d`]). A Ruby-side Array
/// or Hash can also be self-referential (`a = []; a << a`) and
/// without this guard the recursion would stack-overflow when
/// crossing into a C ext via `rb_funcall`'s arg translation or
/// when returning a result. Trap with ArgumentError instead so
/// the caller sees a clean Ruby-level error.
#[cfg(not(target_os = "wasi"))]
fn cext_value_to_cvalue_d(
    vm: &Vm,
    st: &mut rubyrs_cext::CExtState,
    name: &str,
    idx: usize,
    v: &Value,
    depth: usize,
) -> Result<rubyrs_cext::CValue, Trap> {
    if depth >= CEXT_TRANSLATE_MAX_DEPTH {
        return Err(Trap::new(RubyError::ArgumentError {
            msg: format!(
                "C ext `{}': arg {} exceeds max nesting depth {} (cycle or pathological input)",
                name, idx, CEXT_TRANSLATE_MAX_DEPTH
            ),
        }));
    }
    Ok(match v {
        Value::Nil => rubyrs_cext::CValue::Nil,
        Value::Bool(true) => rubyrs_cext::CValue::True,
        Value::Bool(false) => rubyrs_cext::CValue::False,
        Value::Str(s) => rubyrs_cext::CValue::str_from_bytes(s.borrow().as_bytes()),
        Value::Int(n) => rubyrs_cext::CValue::Int(*n),
        // Array/Hash crossing Ruby → C: build a CValue::Array/Hash
        // whose elements are FRESH handles interned into `st`.
        // Recurses on contained Values, interning each child into
        // the SAME state the caller will hand the result to. This
        // is the L2-3-review-fix #10: the previous impl used the
        // thread-local `with_state` accessor, which interned children
        // into whatever state was topmost at the time — wrong if the
        // outer caller had a state pushed but the inner caller hadn't
        // pushed yet (top-level cext call), and corrupting on
        // nesting.
        Value::Array(id) => {
            // Borrow the backing Vec<Value> directly — no clone.
            // The recursive `cext_value_to_cvalue` takes `&Vm` (the
            // function's `vm` param), so the heap borrow + each
            // recursive call are both immutable borrows of `vm`;
            // multiple immutable borrows are allowed. Drops the
            // O(n) memcpy the previous `.clone()` paid on every
            // collection crossing.
            let elements = vm.heap.array(*id);
            let mut handles: Vec<rubyrs_cext::Value> = Vec::with_capacity(elements.len());
            for elem in elements {
                let cv = cext_value_to_cvalue_d(vm, st, name, idx, elem, depth + 1)?;
                handles.push(st.intern(cv));
            }
            rubyrs_cext::CValue::Array(handles)
        }
        Value::Hash(id) => {
            // Same borrow-no-clone treatment for Hash.
            let pairs = vm.heap.hash(*id);
            let mut pairs_out: Vec<(rubyrs_cext::Value, rubyrs_cext::Value)> =
                Vec::with_capacity(pairs.len());
            for (k, v) in pairs {
                let kc = cext_value_to_cvalue_d(vm, st, name, idx, k, depth + 1)?;
                let kh = st.intern(kc);
                let vc = cext_value_to_cvalue_d(vm, st, name, idx, v, depth + 1)?;
                let vh = st.intern(vc);
                pairs_out.push((kh, vh));
            }
            rubyrs_cext::CValue::Hash(pairs_out)
        }
        other => {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: format!(
                    "C ext `{}': arg {} has type {} which is not yet supported across the cext FFI",
                    name,
                    idx,
                    other.type_name()
                ),
            }));
        }
    })
}

/// Invoke a registered C extension function: intern args into a fresh
/// per-call [`CExtState`], dispatch through the correct arity-specific
/// signature, translate the returned handle back into a rubyrs [`Value`].
///
/// Spike scope (Level 1): arities 0, 1, 2 are dispatched. The
/// `unsafe extern "C" fn()` stored in `CFn::func` is transmuted to the
/// arity-specific type — safe on x86_64 SysV and ARM64 AAPCS, where
/// `VALUE = u64` arg/return passes through scalar registers and unused
/// register args are simply ignored by the callee. Other arities trap
/// loudly at invocation rather than at register-time so the failure is
/// clearly attributable to the call site, not Init.
#[cfg(not(target_os = "wasi"))]
fn cext_dispatch(
    name: &str,
    func: rubyrs_cext::OpaqueFn,
    arity: i32,
    args: &[Value],
    self_class: Option<&str>,
) -> Result<Value, Trap> {
    let expected_argc = match arity {
        0..=5 => arity as usize,
        _ => {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: format!(
                    "C ext `{}': spike dispatches arity 0..=5 (got arity {})",
                    name, arity
                ),
            }));
        }
    };
    if args.len() != expected_argc {
        return Err(Trap::new(RubyError::ArgumentError {
            msg: format!(
                "C ext `{}': expected {} args, got {}",
                name,
                expected_argc,
                args.len()
            ),
        }));
    }

    // SAFETY: `current_vm_ptr()` returns the same Vm pointer that
    // `do_call` stashed before invoking us; it stays valid until
    // `do_call` returns. The closure captures the pointer by value
    // so subsequent host_fn invocations don't have to re-stash it
    // (they will anyway, with the same value).
    //
    // Check the invariant BEFORE pushing any cext state on the
    // thread-local stacks — if this assert ever fires, no STATE or
    // callback gets leaked to corrupt the next cext call. Moved out
    // of the unsafe block so it sequences before arg translation
    // (which now needs `&Vm` for Array/Hash heap reads).
    let vm_ptr = current_vm_ptr();
    assert!(
        !vm_ptr.is_null(),
        "ICE: cext_dispatch reached with null CURRENT_VM_PTR; \
         host did not set it before calling host fn"
    );

    // SAFETY: we transmute `OpaqueFn` (zero-arg) to an arity-specific
    // signature with VALUE-shaped args. The original function was
    // registered with that exact signature by the C ext; we just
    // recovered it through the `ANYARGS` convention.
    unsafe {
        // From here on, every push has a matching RAII guard. A panic
        // (or any future early-return) will unwind through these and
        // pop both stacks in LIFO order, leaving thread-local state
        // exactly as we found it.
        let state_guard = CExtStateGuard::enter();
        let _cb_guard = FuncallCallbackGuard::install(Box::new(
            move |recv_h, method_name, arg_hs| {
                cext_funcall_to_vm(vm_ptr, recv_h, method_name, arg_hs)
            },
        ));

        // Translate args INTO the now-active state, interning each
        // (and each child for Array/Hash) directly via the same `st`
        // we're about to hand to the C ext. Trap-propagating via `?`;
        // RAII guards above drop on the early-return path.
        //
        // Previously the translation ran BEFORE `enter()` and used
        // `with_state` for child interning, which silently interned
        // Array/Hash children into the OUTER state (or panicked on
        // empty STATE for top-level calls). Fix for PR #6 review #10.
        let arg_handles: Vec<rubyrs_cext::Value> = {
            let vm_ref: &Vm = &*vm_ptr;
            rubyrs_cext::with_state(|st| {
                args.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let cv = cext_value_to_cvalue(vm_ref, st, name, i, v)?;
                        Ok::<_, Trap>(st.intern(cv))
                    })
                    .collect::<Result<Vec<_>, Trap>>()
            })?
        };

        let ret_handle = with_caught_unwind(|| {
            // Build the `self` handle:
            // - For singleton methods (`rb_define_singleton_method`),
            //   `self_class` is `Some(class_joined_name)`; intern a
            //   `CValue::Class` handle so the C ext sees its own
            //   class object as `self`, matching CRuby.
            // - For top-level functions (`rb_define_global_function`),
            //   `self_class` is `None`; pass `Qnil`, matching CRuby
            //   (top-level functions are conceptually attached to
            //   the main object, but extensions universally treat
            //   their `self` as opaque-and-unused there).
            let self_handle = match self_class {
                Some(cname) => rubyrs_cext::with_state(|st| {
                    st.intern(rubyrs_cext::CValue::Class(cname.to_string()))
                }),
                None => rubyrs_cext::Qnil,
            };
            match arity {
                0 => {
                    type F = unsafe extern "C" fn(rubyrs_cext::Value) -> rubyrs_cext::Value;
                    let f: F = std::mem::transmute(func);
                    f(self_handle)
                }
                1 => {
                    type F = unsafe extern "C" fn(
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                    ) -> rubyrs_cext::Value;
                    let f: F = std::mem::transmute(func);
                    f(self_handle, arg_handles[0])
                }
                2 => {
                    type F = unsafe extern "C" fn(
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                    ) -> rubyrs_cext::Value;
                    let f: F = std::mem::transmute(func);
                    f(self_handle, arg_handles[0], arg_handles[1])
                }
                3 => {
                    type F = unsafe extern "C" fn(
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                    ) -> rubyrs_cext::Value;
                    let f: F = std::mem::transmute(func);
                    f(self_handle, arg_handles[0], arg_handles[1], arg_handles[2])
                }
                4 => {
                    type F = unsafe extern "C" fn(
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                    ) -> rubyrs_cext::Value;
                    let f: F = std::mem::transmute(func);
                    f(
                        self_handle,
                        arg_handles[0], arg_handles[1], arg_handles[2], arg_handles[3],
                    )
                }
                5 => {
                    type F = unsafe extern "C" fn(
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                        rubyrs_cext::Value,
                    ) -> rubyrs_cext::Value;
                    let f: F = std::mem::transmute(func);
                    f(
                        self_handle,
                        arg_handles[0], arg_handles[1], arg_handles[2], arg_handles[3], arg_handles[4],
                    )
                }
                _ => unreachable!("arity validated above"),
            }
        });
        // Normal-exit cleanup. `_cb_guard` drops at end of `unsafe`
        // block (LIFO with state_guard), so we consume the state
        // guard here to extract the drained `CExtState` for handle
        // translation. `_cb_guard` then pops the callback when the
        // block ends. Both happen via `Drop` on the panic path too.
        let st = state_guard.into_state();
        let ret_handle = ret_handle.map_err(|panic_msg| {
            Trap::new(RubyError::RuntimeError {
                msg: format!("C ext `{}' panicked: {}", name, panic_msg),
            })
        })?;
        // Re-deref vm_ptr for the result translation (Array/Hash
        // returns need `&mut Vm` to allocate on the heap). Time-
        // disjoint from any earlier &Vm uses in this function.
        let vm: &mut Vm = &mut *vm_ptr;
        cext_handle_to_value(vm, &st, ret_handle)
    }
}

/// Bridge a `rubyrs_cext::FuncallCallback` invocation to
/// [`Vm::cext_invoke_method`]. Used as the body of the closure
/// installed by [`cext_dispatch`].
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a [`Vm`] for the duration of
/// this call. The caller (`cext_dispatch`) guarantees this by only
/// installing the callback while the corresponding `do_call` frame
/// is on the host's Rust stack — see [`CURRENT_VM_PTR`] for the
/// borrow-aliasing discussion.
#[cfg(not(target_os = "wasi"))]
fn cext_funcall_to_vm(
    vm_ptr: *mut Vm,
    recv: rubyrs_cext::Value,
    method: &str,
    arg_handles: &[rubyrs_cext::Value],
) -> rubyrs_cext::Value {
    // SAFETY: see CURRENT_VM_PTR doc — vm_ptr is valid for the life
    // of the surrounding cext_dispatch call. We deref the same
    // pointer twice in this function: first as `&mut Vm` (inner
    // block) for the recv/arg handle → Value translation and the
    // cext_invoke_method call; then, AFTER the inner block exits
    // and the &mut goes out of scope, as `&Vm` for the result
    // Value → handle translation. The two derefs are split into
    // separate scopes so no &mut + & alias exists at any moment —
    // the previous `let (result, vm_for_result) = unsafe { ... }`
    // pattern returned `&*vm_ptr` while `&mut *vm_ptr` was still
    // alive in the same block, which Stacked Borrows flags as UB.
    let result = unsafe {
        let vm = &mut *vm_ptr;
        // PinGuard the translated `recv_v` and each arg Value as
        // they are produced: `cext_handle_to_value` recursively
        // allocates Vm-heap Arrays/Hashes for nested C-built
        // structures, and each alloc can trigger `maybe_gc`. A
        // previously-translated recv or earlier arg sitting only
        // in a Rust local has no GC root, so STRESS_GC would sweep
        // it before `cext_invoke_method` saw it (slot-reuse → ICE
        // "use-after-free" inside dispatch). The guard is alive
        // across `cext_invoke_method` itself — which is intentional:
        // dispatch may also `maybe_gc` (e.g. compiling a string→sym,
        // alloc'ing intermediate Arrays), and we want recv/args
        // protected the whole way until they're consumed onto the
        // operand stack. The guard drops at the end of the unsafe
        // block, after the call has returned and the result Value
        // is bound.
        let mut g = PinGuard::new(vm);
        // `cext_handle_to_value` now returns Result (L2.5 Trap
        // propagation). On a translation Trap here — e.g. a cycle
        // in C-built recv/args, or heap-cap exhaustion mid-build —
        // we can't unwind into Ruby (this IS a C-ABI callback
        // entry point), so we collapse to Nil and let the inner
        // dispatch handle the degenerate input. Surfacing the
        // Trap via the rb_funcall return value requires `rb_raise`
        // / longjmp (Level 3).
        let recv_v = rubyrs_cext::with_state(|st| {
            cext_handle_to_value(g.vm, st, recv).unwrap_or(Value::Nil)
        });
        g.pin(recv_v.clone());
        let arg_vs: Vec<Value> = rubyrs_cext::with_state(|st| {
            arg_handles
                .iter()
                .map(|h| {
                    let v = cext_handle_to_value(g.vm, st, *h).unwrap_or(Value::Nil);
                    g.pin(v.clone());
                    v
                })
                .collect()
        });
        match g.vm.cext_invoke_method(recv_v, method, arg_vs) {
            Ok(v) => v,
            // Spike: propagating Trap back through the C-ABI boundary
            // needs `rb_raise` / longjmp coordination (Level 3+).
            // For now collapse to Nil so the C side gets a defined
            // return without aborting.
            Err(_trap) => Value::Nil,
        }
        // `vm: &mut Vm` drops here.
    };

    // Now safe to take a fresh `&Vm` from the same pointer — the
    // previous `&mut` is out of scope.
    let vm_for_result: &Vm = unsafe { &*vm_ptr };

    // Translate result back to a handle in the topmost CExtState.
    // `cext_value_to_cvalue` now takes the same `st` it'll be interned
    // into, so Array/Hash result children land in the correct state
    // — the topmost, which is the C ext's current state.
    rubyrs_cext::with_state(|st| {
        match cext_value_to_cvalue(vm_for_result, st, "rb_funcallv:result", 0, &result) {
            Ok(cv) => st.intern(cv),
            Err(_) => rubyrs_cext::Qnil,
        }
    })
}

/// Run `f`, catching any Rust panic that escapes from our own
/// argument-interning / handle-management code wrapping the C call.
///
/// **What this catches**: panics raised in Rust code that runs around
/// the `extern "C"` call — `state.intern`, our `Vec` building, any
/// `expect("ICE: ...")` in `rubyrs_cext::with_state`.
///
/// **What this does NOT catch**: panics raised inside the C function
/// itself. The C side cannot raise a Rust panic; if one of OUR
/// `rb_*` ABI functions panics from inside the C call, the process
/// aborts under `panic = abort` semantics (the default for `extern "C"`
/// since Rust 2018+). The cext ABI surface is documented as
/// abort-on-contract-violation in docs/PANIC_AUDIT.md — conversion to
/// error sentinels is Level 3+ work tied to `rb_raise` integration.
#[cfg(not(target_os = "wasi"))]
fn with_caught_unwind<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        if let Some(s) = p.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        }
    })
}

fn visibility_from_name(name: &str) -> Option<Visibility> {
    match name {
        "private" => Some(Visibility::Private),
        "protected" => Some(Visibility::Protected),
        "public" => Some(Visibility::Public),
        _ => None,
    }
}

/// Minimal printf-style formatter used by `String#%`. Supports the
/// flag set [- + 0 space #], optional width and precision (decimal
/// integers only — `*` for argument-driven width is not yet
/// supported), and conversion specifiers d/i, f, s, x, X, o, b, B,
/// c, p, plus the literal `%%`. Positional (`%1$d`) and named
/// (`%<name>s`) directives are out of scope; encountering them
/// raises ArgumentError so the caller can `rescue`.
pub(crate) fn ruby_sprintf(
    fmt: &str,
    args: &[Value],
    heap: &Heap,
    interner: &Interner,
) -> Result<String, RubyError> {
    let mut out = String::new();
    let mut idx: usize = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut flag_minus = false;
        let mut flag_plus = false;
        let mut flag_zero = false;
        let mut flag_space = false;
        let mut flag_hash = false;
        loop {
            match chars.peek() {
                Some('-') => { flag_minus = true; chars.next(); }
                Some('+') => { flag_plus = true; chars.next(); }
                Some('0') => { flag_zero = true; chars.next(); }
                Some(' ') => { flag_space = true; chars.next(); }
                Some('#') => { flag_hash = true; chars.next(); }
                _ => break,
            }
        }
        let mut width: Option<usize> = None;
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() {
                width = Some(width.unwrap_or(0) * 10 + (d as usize - '0' as usize));
                chars.next();
            } else { break; }
        }
        let mut precision: Option<usize> = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p: usize = 0;
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    p = p * 10 + (d as usize - '0' as usize);
                    chars.next();
                } else { break; }
            }
            precision = Some(p);
        }
        let spec = chars.next().ok_or_else(|| RubyError::ArgumentError {
            msg: "malformed format string - %".into(),
        })?;
        if spec == '%' {
            out.push('%');
            continue;
        }
        let arg = args.get(idx).ok_or_else(|| RubyError::ArgumentError {
            msg: "too few arguments".into(),
        })?;
        idx += 1;
        let mut body = match spec {
            'd' | 'i' => {
                let n = coerce_int(arg)?;
                let mut body = if n < 0 {
                    format!("-{}", n.unsigned_abs())
                } else if flag_plus {
                    format!("+{n}")
                } else if flag_space {
                    format!(" {n}")
                } else {
                    n.to_string()
                };
                if let Some(p) = precision {
                    // Precision on an integer pads with zeros to
                    // that many digit positions, ignoring any sign.
                    let (sign, digits) = match body.chars().next() {
                        Some(c @ ('-' | '+' | ' ')) => (c, &body[1..]),
                        _ => (' ', body.as_str()),
                    };
                    if digits.len() < p {
                        let pad = "0".repeat(p - digits.len());
                        body = if sign == ' ' && matches!(body.chars().next(), Some('-' | '+' | ' ')) {
                            format!("{sign}{pad}{digits}")
                        } else if sign == ' ' {
                            format!("{pad}{digits}")
                        } else {
                            format!("{sign}{pad}{digits}")
                        };
                    }
                }
                body
            }
            'x' => format_radix_int(coerce_int(arg)?, 16, false, flag_hash),
            'X' => format_radix_int(coerce_int(arg)?, 16, true, flag_hash),
            'o' => format_radix_int(coerce_int(arg)?, 8, false, flag_hash),
            'b' => format_radix_int(coerce_int(arg)?, 2, false, flag_hash),
            'B' => format_radix_int(coerce_int(arg)?, 2, true, flag_hash),
            'f' => {
                let f = coerce_float(arg)?;
                let prec = precision.unwrap_or(6);
                let mut body = format!("{f:.*}", prec);
                if !body.starts_with('-') {
                    if flag_plus { body.insert(0, '+'); }
                    else if flag_space { body.insert(0, ' '); }
                }
                body
            }
            's' => {
                let mut body = arg.to_display(heap, interner);
                if let Some(p) = precision {
                    let truncated: String = body.chars().take(p).collect();
                    body = truncated;
                }
                body
            }
            'p' => arg.to_inspect(heap, interner),
            'c' => match arg {
                Value::Int(n) => {
                    char::from_u32(*n as u32).map(|c| c.to_string()).ok_or_else(|| {
                        RubyError::ArgumentError {
                            msg: format!("invalid character code {n} for %c"),
                        }
                    })?
                }
                Value::Str(s) => s.borrow().chars().next().map(|c| c.to_string()).unwrap_or_default(),
                _ => return Err(RubyError::TypeError {
                    msg: format!("no implicit conversion to %c from {}", arg.type_name()),
                }),
            },
            other => return Err(RubyError::ArgumentError {
                msg: format!("malformed format string - %{other}"),
            }),
        };
        // Apply width / padding. Precision was already applied
        // per-spec (d uses leading zeros to `precision`; s truncates).
        if let Some(w) = width {
            if body.chars().count() < w {
                let pad_n = w - body.chars().count();
                let pad_char = if flag_zero && !flag_minus && precision.is_none()
                    && matches!(spec, 'd' | 'i' | 'x' | 'X' | 'o' | 'b' | 'B' | 'f') {
                    '0'
                } else {
                    ' '
                };
                let pad: String = std::iter::repeat(pad_char).take(pad_n).collect();
                if flag_minus {
                    body.push_str(&pad);
                } else if pad_char == '0' {
                    // Zero-pad goes inside the sign for numbers.
                    if let Some(first) = body.chars().next() {
                        if matches!(first, '-' | '+' | ' ') {
                            let rest: String = body.chars().skip(1).collect();
                            body = format!("{first}{pad}{rest}");
                        } else {
                            body = format!("{pad}{body}");
                        }
                    }
                } else {
                    body = format!("{pad}{body}");
                }
            }
        }
        out.push_str(&body);
    }
    Ok(out)
}

fn coerce_int(v: &Value) -> Result<i64, RubyError> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Float(f) => Ok(*f as i64),
        Value::Str(s) => Ok(s.borrow().trim().parse::<i64>().unwrap_or(0)),
        Value::Nil => Err(RubyError::TypeError {
            msg: "no implicit conversion from nil to Integer".into(),
        }),
        _ => Err(RubyError::TypeError {
            msg: format!("no implicit conversion of {} to Integer", v.type_name()),
        }),
    }
}

fn coerce_float(v: &Value) -> Result<f64, RubyError> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Str(s) => Ok(s.borrow().trim().parse::<f64>().unwrap_or(0.0)),
        Value::Nil => Err(RubyError::TypeError {
            msg: "no implicit conversion from nil to Float".into(),
        }),
        _ => Err(RubyError::TypeError {
            msg: format!("no implicit conversion of {} to Float", v.type_name()),
        }),
    }
}

fn format_radix_int(n: i64, radix: u32, upper: bool, alt: bool) -> String {
    let prefix: &str = if !alt { "" } else {
        match radix { 16 => if upper { "0X" } else { "0x" }, 8 => "0", 2 => if upper { "0B" } else { "0b" }, _ => "" }
    };
    let body = if n < 0 {
        // CRuby: %x on a negative int produces a two's-complement
        // representation prefixed with "..f" (an infinite ones).
        // We render just `-<unsigned digits>` which is close
        // enough for the common test cases and avoids dragging
        // in BigNum-style "..f" notation. Documented divergence.
        let mag = match radix {
            16 => format!("{:x}", (-n) as u64),
            8 => format!("{:o}", (-n) as u64),
            2 => format!("{:b}", (-n) as u64),
            _ => unreachable!(),
        };
        let mag = if upper { mag.to_uppercase() } else { mag };
        format!("-{prefix}{mag}")
    } else {
        let mag = match radix {
            16 => format!("{:x}", n as u64),
            8 => format!("{:o}", n as u64),
            2 => format!("{:b}", n as u64),
            _ => unreachable!(),
        };
        let mag = if upper { mag.to_uppercase() } else { mag };
        format!("{prefix}{mag}")
    };
    body
}

pub(crate) fn primitive_call(recv: &Value, name: &str, args: &[Value], max_value_bytes: Option<usize>) -> Result<Option<Value>, RubyError> {
    // Helper: enforce the per-value byte cap (P2-14c) at every
    // string-growing arm. Returns Err if the projected size would
    // exceed the cap; callers wrap it in `Trap` via `Vm::trap`.
    let check = |new_len: usize| -> Result<(), RubyError> {
        if let Some(max) = max_value_bytes {
            if new_len > max {
                return Err(RubyError::ResourceExhausted {
                    msg: format!("value size {new_len} bytes > cap {max}"),
                });
            }
        }
        Ok(())
    };
    Ok(match (recv, name, args) {
        (Value::Int(a), op, [Value::Int(b)]) => match op {
            "+" => Some(Value::Int(a + b)),
            "-" => Some(Value::Int(a - b)),
            "*" => Some(Value::Int(a * b)),
            "/" => {
                if *b == 0 {
                    return Err(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    });
                }
                Some(Value::Int(a / b))
            }
            "%" => {
                if *b == 0 {
                    return Err(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    });
                }
                Some(Value::Int(a % b))
            }
            "==" => Some(Value::Bool(a == b)),
            "!=" => Some(Value::Bool(a != b)),
            "<"  => Some(Value::Bool(a < b)),
            "<=" => Some(Value::Bool(a <= b)),
            ">"  => Some(Value::Bool(a > b)),
            ">=" => Some(Value::Bool(a >= b)),
            "<=>" => Some(Value::Int(a.cmp(b) as i64)),
            // Integer exponentiation. Positive exponent stays in i64
            // (saturating on overflow, matching i64::saturating_pow).
            // Negative exponent promotes to Float for the reciprocal,
            // since we don't have Rational — CRuby would give `(1/2)`,
            // we give `0.5`. Documented divergence.
            "**" => Some(if *b >= 0 {
                let exp = (*b as u64).min(u32::MAX as u64) as u32;
                Value::Int(a.saturating_pow(exp))
            } else {
                let f = (*a as f64).powi(*b as i32);
                Value::Float(f)
            }),
            // Bitwise. Ruby uses arbitrary-precision Integer; we
            // truncate to i64. `<<` on a negative shift count is
            // CRuby's right-shift (and vice versa) — we mirror with
            // a sign check rather than panicking on negative shifts.
            "&" => Some(Value::Int(a & b)),
            "|" => Some(Value::Int(a | b)),
            "^" => Some(Value::Int(a ^ b)),
            "<<" => Some(Value::Int(
                if *b >= 0 { a.wrapping_shl((*b as u32).min(63)) }
                else { a.wrapping_shr(((-b) as u32).min(63)) }
            )),
            ">>" => Some(Value::Int(
                if *b >= 0 { a.wrapping_shr((*b as u32).min(63)) }
                else { a.wrapping_shl(((-b) as u32).min(63)) }
            )),
            _ => None,
        },
        (Value::Int(a), "to_s", []) | (Value::Int(a), "inspect", []) => {
            Some(Value::new_str(a.to_string()))
        }
        (Value::Int(a), "to_i", []) => Some(Value::Int(*a)),
        (Value::Int(a), "abs", []) => Some(Value::Int(a.wrapping_abs())),
        (Value::Int(a), "-@", []) => Some(Value::Int(a.wrapping_neg())),
        (Value::Int(a), "+@", []) => Some(Value::Int(*a)),
        (Value::Int(a), "~", []) => Some(Value::Int(!a)),
        (Value::Int(a), "even?", []) => Some(Value::Bool(a % 2 == 0)),
        (Value::Int(a), "odd?", []) => Some(Value::Bool(a % 2 != 0)),
        (Value::Int(a), "zero?", []) => Some(Value::Bool(*a == 0)),
        (Value::Int(a), "positive?", []) => Some(Value::Bool(*a > 0)),
        (Value::Int(a), "negative?", []) => Some(Value::Bool(*a < 0)),
        (Value::Int(a), "succ", []) | (Value::Int(a), "next", []) => Some(Value::Int(a.wrapping_add(1))),
        (Value::Int(a), "pred", []) => Some(Value::Int(a.wrapping_sub(1))),
        (Value::Int(a), "to_f", []) => Some(Value::Float(*a as f64)),

        // Float × Float
        (Value::Float(a), op, [Value::Float(b)]) => match op {
            "+" => Some(Value::Float(a + b)),
            "-" => Some(Value::Float(a - b)),
            "*" => Some(Value::Float(a * b)),
            // Float / 0.0 == ±Infinity (or NaN), not an exception —
            // matches IEEE 754 and CRuby.
            "/" => Some(Value::Float(a / b)),
            "%" => Some(Value::Float(a % b)),
            "==" => Some(Value::Bool(a == b)),
            "!=" => Some(Value::Bool(a != b)),
            "<"  => Some(Value::Bool(a < b)),
            "<=" => Some(Value::Bool(a <= b)),
            ">"  => Some(Value::Bool(a > b)),
            ">=" => Some(Value::Bool(a >= b)),
            // `partial_cmp` returns None on NaN-involved
            // comparisons; CRuby's spec is the same: `(0.0/0.0)
            // <=> 1.0 == nil`.
            "<=>" => Some(match a.partial_cmp(b) {
                Some(o) => Value::Int(o as i64),
                None => Value::Nil,
            }),
            "**" => Some(Value::Float(a.powf(*b))),
            _ => None,
        },
        // Mixed Int/Float — CRuby's "Float wins" coercion.
        (Value::Float(a), op, [Value::Int(b)]) => {
            let b = *b as f64;
            match op {
                "+" => Some(Value::Float(a + b)),
                "-" => Some(Value::Float(a - b)),
                "*" => Some(Value::Float(a * b)),
                "/" => Some(Value::Float(a / b)),
                "%" => Some(Value::Float(a % b)),
                "==" => Some(Value::Bool(*a == b)),
                "!=" => Some(Value::Bool(*a != b)),
                "<"  => Some(Value::Bool(*a < b)),
                "<=" => Some(Value::Bool(*a <= b)),
                ">"  => Some(Value::Bool(*a > b)),
                ">=" => Some(Value::Bool(*a >= b)),
                "<=>" => Some(match a.partial_cmp(&b) {
                    Some(o) => Value::Int(o as i64),
                    None => Value::Nil,
                }),
                "**" => Some(Value::Float(a.powf(b))),
                _ => None,
            }
        }
        (Value::Int(a), op, [Value::Float(b)]) => {
            let a = *a as f64;
            match op {
                "+" => Some(Value::Float(a + b)),
                "-" => Some(Value::Float(a - b)),
                "*" => Some(Value::Float(a * b)),
                "/" => Some(Value::Float(a / b)),
                "%" => Some(Value::Float(a % b)),
                "==" => Some(Value::Bool(a == *b)),
                "!=" => Some(Value::Bool(a != *b)),
                "<"  => Some(Value::Bool(a < *b)),
                "<=" => Some(Value::Bool(a <= *b)),
                ">"  => Some(Value::Bool(a > *b)),
                ">=" => Some(Value::Bool(a >= *b)),
                "<=>" => Some(match a.partial_cmp(b) {
                    Some(o) => Value::Int(o as i64),
                    None => Value::Nil,
                }),
                "**" => Some(Value::Float(a.powf(*b))),
                _ => None,
            }
        }
        // Float predicates and conversions.
        (Value::Float(a), "to_s", []) => Some(Value::new_str(crate::heap::format_float(*a))),
        (Value::Float(a), "to_f", []) => Some(Value::Float(*a)),
        (Value::Float(a), "to_i", []) => Some(Value::Int(*a as i64)),
        (Value::Float(a), "abs", []) => Some(Value::Float(a.abs())),
        (Value::Float(a), "-@", []) => Some(Value::Float(-*a)),
        (Value::Float(a), "+@", []) => Some(Value::Float(*a)),
        (Value::Float(a), "zero?", []) => Some(Value::Bool(*a == 0.0)),
        (Value::Float(a), "positive?", []) => Some(Value::Bool(*a > 0.0)),
        (Value::Float(a), "negative?", []) => Some(Value::Bool(*a < 0.0)),
        (Value::Float(a), "nan?", []) => Some(Value::Bool(a.is_nan())),
        (Value::Float(a), "infinite?", []) => {
            // CRuby's `Float#infinite?` returns 1 / -1 / nil, not bool.
            if a.is_infinite() {
                Some(Value::Int(if *a > 0.0 { 1 } else { -1 }))
            } else {
                Some(Value::Nil)
            }
        }
        (Value::Float(a), "finite?", []) => Some(Value::Bool(a.is_finite())),
        (Value::Float(a), "floor", []) => Some(Value::Int(a.floor() as i64)),
        (Value::Float(a), "ceil", []) => Some(Value::Int(a.ceil() as i64)),
        (Value::Float(a), "round", []) => Some(Value::Int(a.round() as i64)),

        (Value::Str(a), "+", [Value::Str(b)]) => {
            check(a.borrow().len().saturating_add(b.borrow().len()))?;
            let mut s = a.borrow().clone();
            s.push_str(&b.borrow());
            Some(Value::new_str(s))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() == *b.borrow())),
        (Value::Str(a), "!=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() != *b.borrow())),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        (Value::Str(a), "length", []) | (Value::Str(a), "size", []) => Some(Value::Int(a.borrow().chars().count() as i64)),
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.borrow().is_empty())),
        (Value::Str(a), "upcase", []) => Some(Value::new_str(a.borrow().to_uppercase())),
        (Value::Str(a), "downcase", []) => Some(Value::new_str(a.borrow().to_lowercase())),
        (Value::Str(a), "reverse", []) => Some(Value::new_str(a.borrow().chars().rev().collect::<String>())),
        (Value::Str(a), "strip", []) => Some(Value::new_str(a.borrow().trim().to_string())),
        (Value::Str(a), "lstrip", []) => Some(Value::new_str(a.borrow().trim_start().to_string())),
        (Value::Str(a), "rstrip", []) => Some(Value::new_str(a.borrow().trim_end().to_string())),
        (Value::Str(a), "include?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().contains(&*b.borrow()))),
        // Literal-substring `match?` — true iff the receiver
        // contains the argument as a substring. CRuby additionally
        // accepts a Regexp here; we only handle String, in line
        // with the rest of our regex-free subset. Calls with a
        // non-String argument fall through to NoMethodError.
        (Value::Str(a), "match?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().contains(&*b.borrow()))),
        // String#match? with a Regex — proper regex match. Returns
        // bool without populating any match-data side state.
        (Value::Str(a), "match?", [Value::Regex(re)]) => {
            Some(Value::Bool(re.is_match(&a.borrow())))
        }
        // Regex#match? mirror — same semantics either side.
        (Value::Regex(re), "match?", [Value::Str(s)]) => {
            Some(Value::Bool(re.is_match(&s.borrow())))
        }
        // Regex#source — the raw pattern string.
        (Value::Regex(re), "source", []) => Some(Value::new_str(re.as_str().to_string())),
        (Value::Regex(re), "to_s", []) => Some(Value::new_str(format!("(?-mix:{})", re.as_str()))),
        (Value::Regex(re), "inspect", []) => Some(Value::new_str(format!("/{}/", re.as_str()))),
        // `index(substr)` / `rindex(substr)` — return the byte
        // offset where the substring first / last appears, or
        // nil if it's absent. CRuby reports a *character* index
        // for non-ASCII receivers; we report `String::find`'s
        // byte index, which matches for ASCII (the common case
        // for our test fixtures) and diverges for multibyte —
        // documented in SUBSET.md.
        (Value::Str(a), "index", [Value::Str(b)]) => {
            Some(match a.borrow().find(&*b.borrow()) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })
        }
        (Value::Str(a), "rindex", [Value::Str(b)]) => {
            Some(match a.borrow().rfind(&*b.borrow()) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })
        }
        // Literal-substring sub/gsub. Regex forms (`gsub(/pat/, ...)`)
        // are out of scope until we add a regex engine — documented
        // in SUBSET.md. CRuby's `gsub("", "x")` on a non-empty
        // string inserts at every character boundary; we replicate
        // that via `Rust`'s `str::replace` for non-empty patterns
        // and a hand-rolled walk for the empty-pattern case.
        (Value::Str(a), "sub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let pat_ref = pat.borrow();
            let repl_ref = repl.borrow();
            let out = if pat_ref.is_empty() {
                // CRuby: sub("", repl) inserts `repl` at index 0.
                let mut s = repl_ref.clone();
                s.push_str(&a_ref);
                s
            } else if let Some(idx) = a_ref.find(&*pat_ref) {
                let mut s = String::with_capacity(a_ref.len() + repl_ref.len());
                s.push_str(&a_ref[..idx]);
                s.push_str(&repl_ref);
                s.push_str(&a_ref[idx + pat_ref.len()..]);
                s
            } else {
                a_ref.clone()
            };
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "gsub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let pat_ref = pat.borrow();
            let repl_ref = repl.borrow();
            let out = if pat_ref.is_empty() {
                // CRuby: gsub("", repl) wraps `repl` around every
                // character — `"abc".gsub("", "X") == "XaXbXcX"`.
                let mut s = repl_ref.clone();
                for c in a_ref.chars() {
                    s.push(c);
                    s.push_str(&repl_ref);
                }
                s
            } else {
                a_ref.replace(&*pat_ref, &repl_ref)
            };
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // String#tr — character-by-character translation. Each
        // char in `from` maps to the same-index char in `to`; if
        // `to` is shorter, characters past its length map to its
        // LAST char (CRuby's "stretch" behaviour). If `to` is
        // empty, those chars are deleted. Character-range syntax
        // (`"a-z"`) is intentionally NOT expanded — flagged in
        // SUBSET.md.
        (Value::Str(a), "tr", [Value::Str(from), Value::Str(to)]) => {
            let from_chars: Vec<char> = from.borrow().chars().collect();
            let to_chars: Vec<char> = to.borrow().chars().collect();
            let a_ref = a.borrow();
            let mut out = String::with_capacity(a_ref.len());
            for ch in a_ref.chars() {
                if let Some(idx) = from_chars.iter().position(|c| *c == ch) {
                    if to_chars.is_empty() {
                        // Delete: skip this character entirely.
                    } else if idx < to_chars.len() {
                        out.push(to_chars[idx]);
                    } else {
                        out.push(*to_chars.last().unwrap());
                    }
                } else {
                    out.push(ch);
                }
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "start_with?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().starts_with(&*b.borrow()))),
        (Value::Str(a), "end_with?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().ends_with(&*b.borrow()))),
        (Value::Str(a), "to_i", []) => {
            // CRuby's `String#to_i` is famously lenient: leading
            // whitespace, optional sign, then as many digits as it
            // can read; non-numeric tail (or empty input) gives 0.
            let a_ref = a.borrow();
            let s = a_ref.trim_start();
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
        (Value::Str(a), "to_f", []) => {
            // CRuby's leniency: trim leading whitespace, parse what
            // we can, return 0.0 for "garbage". Rust's stdlib
            // `f64::from_str` is stricter (rejects trailing junk),
            // so we scan a Ruby-shaped prefix ourselves.
            let a_ref = a.borrow();
            let s = a_ref.trim_start();
            let bytes = s.as_bytes();
            let mut end = 0usize;
            if bytes.first() == Some(&b'-') || bytes.first() == Some(&b'+') {
                end += 1;
            }
            let mut saw_digit = false;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                saw_digit = true;
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'.' {
                end += 1;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    saw_digit = true;
                    end += 1;
                }
            }
            // Optional exponent
            if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
                let mut e = end + 1;
                if e < bytes.len() && (bytes[e] == b'+' || bytes[e] == b'-') { e += 1; }
                let exp_start = e;
                while e < bytes.len() && bytes[e].is_ascii_digit() { e += 1; }
                if e > exp_start { end = e; }
            }
            let parsed = if saw_digit {
                s[..end].parse::<f64>().unwrap_or(0.0)
            } else { 0.0 };
            Some(Value::Float(parsed))
        }
        (Value::Str(a), "*", [Value::Int(n)]) => {
            let n = (*n).max(0) as usize;
            check(a.borrow().len().saturating_mul(n))?;
            Some(Value::new_str(a.borrow().repeat(n)))
        }
        (Value::Str(a), "<", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() < *b.borrow())),
        (Value::Str(a), "<=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() <= *b.borrow())),
        (Value::Str(a), ">", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() > *b.borrow())),
        (Value::Str(a), "<=>", [Value::Str(b)]) => Some(Value::Int(a.borrow().cmp(&*b.borrow()) as i64)),
        (Value::Str(a), ">=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() >= *b.borrow())),
        (Value::Sym(a), "==", [Value::Sym(b)]) => Some(Value::Bool(a == b)),
        (Value::Sym(a), "!=", [Value::Sym(b)]) => Some(Value::Bool(a != b)),
        (Value::Nil, "to_s", []) => Some(Value::new_str("")),
        (Value::Nil, "inspect", []) => Some(Value::new_str("nil")),
        // String#inspect — wrap in double quotes, escape `\`,
        // `"`, and common control characters. Matches CRuby for
        // printable ASCII + the standard escape set; exotic
        // Unicode escapes (`\u{...}`) are out of scope.
        (Value::Str(s), "inspect", []) => {
            let raw = s.borrow();
            let mut out = String::with_capacity(raw.len() + 2);
            out.push('"');
            for c in raw.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"'  => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\0' => out.push_str("\\0"),
                    _ => out.push(c),
                }
            }
            out.push('"');
            Some(Value::new_str(out))
        }
        // Float#inspect — same as to_s for finite/inf/NaN values.
        (Value::Float(a), "inspect", []) => {
            Some(Value::new_str(crate::heap::format_float(*a)))
        }
        // Bool#inspect — to_s.
        (Value::Bool(b), "inspect", []) => {
            Some(Value::new_str(if *b { "true" } else { "false" }))
        }
        (Value::Nil, "nil?", []) => Some(Value::Bool(true)),
        // Object#nil? is `false` for every non-nil receiver. We
        // implement it here as a generic fallback so e.g.
        // `"abc".nil?` and `5.nil?` work without per-type arms.
        (_, "nil?", []) => Some(Value::Bool(false)),
        // Unary `!`. CRuby defines `Kernel#!` on every Object —
        // `!foo` returns `true` iff `foo` is `nil` or `false`,
        // `false` otherwise. Prism lowers a unary `!` expression
        // as a call to the `!` method, so this universal arm
        // covers every receiver. `!@` (the alternate spelling
        // used by `attr_*` / `define_method`) is the same op.
        (_, "!", []) | (_, "!@", []) => Some(Value::Bool(!recv.is_truthy())),
        (Value::Bool(b), "to_s", []) => Some(Value::new_str(if *b { "true" } else { "false" })),
        // CRuby's TrueClass / FalseClass don't define `<=>`;
        // `Object#<=>` falls back to "0 if identical instance
        // else nil". Booleans are singletons (every `true` is
        // the same instance) so `true <=> true == 0` and
        // `true <=> false == nil`. Same shape for Nil.
        (Value::Bool(a), "<=>", [Value::Bool(b)]) => {
            Some(if a == b { Value::Int(0) } else { Value::Nil })
        }
        (Value::Nil, "<=>", [Value::Nil]) => Some(Value::Int(0)),
        // Per-built-in-lhs catch-alls: when the rhs type doesn't
        // match any specific arm above, `<=>` is `nil`, not
        // NoMethodError. We have to enumerate per-lhs (rather
        // than a universal `(_, "<=>", _)`) so that user-defined
        // `<=>` on `Value::Object` still wins via the normal
        // class-method-lookup path in `do_call`.
        (Value::Int(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Float(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Str(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Bool(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Nil, "<=>", [_]) => Some(Value::Nil),
        (Value::Class(c), "name", []) | (Value::Class(c), "to_s", []) => {
            Some(Value::new_str(c.name.clone()))
        }
        // Class identity is `Rc::ptr_eq` — two `Value::Class` refer
        // to the same class iff they point at the same `Rc<Class>`.
        // Reopened classes share the same Rc by virtue of the
        // class-table lookup in `Op::DefClass`, so
        // `class Foo; end; class Foo; end; Foo == Foo` is `true`.
        (Value::Class(a), "==", [Value::Class(b)]) => Some(Value::Bool(Rc::ptr_eq(a, b))),
        (Value::Class(a), "!=", [Value::Class(b)]) => Some(Value::Bool(!Rc::ptr_eq(a, b))),
        _ => None,
    })
}

/// `Symbol#to_s` / `to_sym` need the Interner to resolve the underlying name,
/// so they live as a method on Vm rather than in the pure `primitive_call`.
impl Vm {
    pub(crate) fn sym_primitive(&self, recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
        match (recv, name, args) {
            (Value::Sym(id), "to_s", []) => Some(Value::new_str(self.interner.resolve(*id).to_string())),
            // Symbol#inspect — `:name` form (prefix with colon).
            (Value::Sym(id), "inspect", []) => {
                Some(Value::new_str(format!(":{}", self.interner.resolve(*id))))
            }
            (Value::Sym(id), "to_sym", []) => Some(Value::Sym(*id)),
            // Symbol <=> Symbol compares the interned names
            // lexicographically — matches `value_cmp_v`.
            (Value::Sym(a), "<=>", [Value::Sym(b)]) => {
                let sa = self.interner.resolve(*a);
                let sb = self.interner.resolve(*b);
                Some(Value::Int((**sa).cmp(&**sb) as i64))
            }
            // Cross-type with Symbol lhs: nil, not NoMethodError.
            (Value::Sym(_), "<=>", [_]) => Some(Value::Nil),
            _ => None,
        }
    }
}
