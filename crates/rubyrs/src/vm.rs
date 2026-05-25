use std::cell::RefCell;
// `Cell` is only used by the cext-reentrance machinery (CURRENT_VM_PTR),
// itself wasi-stubbed; gate the import so the wasi build doesn't see
// it as unused under `-D warnings`.
#[cfg(not(target_os = "wasi"))]
use std::cell::Cell;
use std::collections::HashMap;
use std::env;
use std::rc::Rc;

use crate::bytecode::{BinOpKind, Op, Proto};
use crate::error::{RubyError, Span, Trap, TrapFrame};
use crate::heap::{Heap, HeapObj};
use crate::intern::{Interner, SymId};
use crate::value::{BlockHandle, Class, Instance, Method, ObjId, Value, Visibility};

mod array;
mod cext;
mod dispatch;
mod fileops;
mod hash;
mod iter;
mod kernel;
mod numeric;
mod raise;
mod range;
mod sprintf;
mod string;
pub(crate) use sprintf::ruby_sprintf;
#[cfg(not(target_os = "wasi"))]
use cext::cext_dispatch;

// ---------- VM ----------

/// Ordering for built-in aggregation methods (`min` / `max` /
/// `sort`). Only homogeneous Int / Str / Sym arrays are supported;
/// other shapes return `None` so the caller can fall through to
/// NoMethodError. With a block-taking comparator we'd handle this
/// generically, but that's deferred to a later milestone.
///
/// Symbol comparison uses the interned string — CRuby orders
/// `:apple < :banana` lexicographically, not by interning order.
pub(crate) fn value_cmp_v(a: &Value, b: &Value, interner: &Interner) -> Option<std::cmp::Ordering> {
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
    /// Lazily-built ENV Hash, shared across every `ENV`
    /// reference. Set on first `LoadConst("ENV")` and reused
    /// thereafter so script code observes a single mutable
    /// snapshot of the process env.
    pub(crate) env_hash: Option<ObjId>,
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
            env_hash: None,
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
                "match?" | "match" | "scan" | "index" | "rindex" |
                "[]" | "slice" |
                "<<" | "concat" | "prepend" | "replace" |
                "freeze" | "frozen?" | "dup"
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
                "sort!" | "uniq!" | "compact!" | "flatten!" | "reverse!" |
                "flat_map" | "collect_concat" | "chunk" |
                "each_slice" | "each_cons" |
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
                let cls = self.heap.class_of(*id);
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
            Value::Object(id) => return Value::Class(self.heap.class_of(*id)),
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

    pub(crate) fn collection_call(&mut self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        Ok(match recv {
            Value::Array(id) => return self.array_collection_call(*id, name, args),
            Value::Hash(id) => return self.hash_collection_call(*id, name, args),
            Value::Str(s) => return self.string_collection_call(s.clone(), name, args),
            Value::Range(id) => return self.range_collection_call(*id, name, args),
            _ => None,
        })
    }


    pub(crate) fn maybe_gc(&mut self) {
        if !self.stress_gc && !self.heap.should_gc() { return; }
        // Gather roots: stack + every frame's locals + self_val + swap_return
        // + pinned (native-code accumulators). class_stack holds Rc<Class>
        // which isn't GC-managed, so we don't need to walk it.
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + self.pinned.len() + 64);
        for v in &self.stack { roots.push(v.clone()); }
        for v in &self.pinned { roots.push(v.clone()); }
        // ENV hash, once initialised, is reachable from script
        // code via the `ENV` constant — pin it so the cache
        // doesn't get swept between LoadConst loads.
        if let Some(id) = self.env_hash { roots.push(Value::Hash(id)); }
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
        let pending_frees = self.heap.collect(&roots);
        // Run TypedData dfree callbacks AFTER `collect` has
        // returned and the &mut Heap borrow is released (review #2
        // on PR #19). Conservative shape — even though
        // well-behaved cexts shouldn't re-enter the VM from dfree,
        // this avoids the aliasing footgun if one ever does.
        for (f, p) in pending_frees {
            // SAFETY: `f` and `p` originate from a TypedData slot
            // we just swept (the slot was unreachable from any GC
            // root, so the cext can't observe `p` again). The
            // cext's contract for `dfree` is to release ownership
            // of `p` — exactly what we want here.
            unsafe { f(p); }
        }
    }

    /// Compare two values using built-in types first, then falling
    /// back to invoking the left-hand side's user-defined `<=>`.
    /// Returns `None` for incomparable pairs (built-in cross-type
    /// mismatches, or a user `<=>` that returns `nil`). Used by
    /// `Array#sort` so user classes that define `<=>` (typically
    /// via `include Comparable`) sort sensibly. Synchronously
    /// dispatches the user method by pushing a frame and running
    /// `dispatch_until` — the same pattern iterator drivers use.
    /// One step of nested lookup for `Hash#dig` / `Array#dig`.
    /// Hash receivers use `ruby_eq` key lookup; Array uses Int
    /// index (negative wraps from end). Anything else → nil so
    /// the caller can short-circuit cleanly.
    pub(crate) fn dig_step(&self, recv: &Value, key: &Value) -> Result<Value, Trap> {
        Ok(match recv {
            Value::Hash(id) => {
                let h = self.heap.hash(*id);
                h.iter()
                    .find(|(k, _)| k.ruby_eq(key, &self.heap))
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Value::Nil)
            }
            Value::Array(id) => {
                if let Value::Int(i) = key {
                    let a = self.heap.array(*id);
                    let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                    a.get(idx as usize).cloned().unwrap_or(Value::Nil)
                } else {
                    Value::Nil
                }
            }
            _ => Value::Nil,
        })
    }

    pub(crate) fn user_cmp(&mut self, a: &Value, b: &Value) -> Result<Option<std::cmp::Ordering>, Trap> {
        if let Some(ord) = value_cmp_v(a, b, &self.interner) {
            return Ok(Some(ord));
        }
        // Try the receiver's `<=>` method (user-defined). Only
        // Value::Object can have user methods; other receivers
        // would have been resolved by value_cmp_v above.
        if let Value::Object(id) = a {
            let cls = self.heap.class_of(*id);
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
                let v = if let Some(c) = self.classes.get(&name_id).cloned() {
                    Value::Class(c)
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
    pub(crate) static CURRENT_VM_PTR: Cell<*mut Vm> = const { Cell::new(std::ptr::null_mut()) };
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
pub(crate) fn with_vm_ptr_set<R>(vm_ptr: *mut Vm, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_VM_PTR.with(|c| c.replace(vm_ptr));
    let _guard = VmPtrGuard { prev };
    f()
}

// `current_vm_ptr` + `CExtStateGuard` + `FuncallCallbackGuard` +
// `TypedDataCallbackGuard` moved to `vm/cext.rs`.

// `file_class_dispatch` moved to `vm/fileops.rs`.
//
// (The `with_caught_unwind` helper that used to live here was
// removed in Spike L3-A — once the cext call routes through the
// pure-C `rubyrs_jmp_invoke`, there's no Rust frame between
// setjmp and the cext fn that a catch_unwind could meaningfully
// cover. Re-introducing it on master appears to have been an
// accidental revert in the kernel.rs extraction refactor; this
// branch drops it again to keep the panic-budget honest and
// -D warnings green.)


pub(crate) fn visibility_from_name(name: &str) -> Option<Visibility> {
    match name {
        "private" => Some(Visibility::Private),
        "protected" => Some(Visibility::Protected),
        "public" => Some(Visibility::Public),
        _ => None,
    }
}


pub(crate) fn primitive_call(recv: &Value, name: &str, args: &[Value], max_value_bytes: Option<usize>) -> Result<Option<Value>, RubyError> {
    // Helper: enforce the per-value byte cap (P2-14c) at every
    // string-growing arm. Returns Err if the projected size would
    // exceed the cap; callers wrap it in `Trap` via `Vm::trap`.
    // Underscore-prefixed: the closure isn't currently called
    // because every path inside primitive_call performs the byte-
    // cap check inline; keeping the helper definition documents
    // the discipline (P2-14c) for future arms and satisfies
    // `-D unused-variables`.
    let _check = |new_len: usize| -> Result<(), RubyError> {
        if let Some(max) = max_value_bytes {
            if new_len > max {
                return Err(RubyError::ResourceExhausted {
                    msg: format!("value size {new_len} bytes > cap {max}"),
                });
            }
        }
        Ok(())
    };
    // Per-type sub-dispatchers (mirror CRuby's split). Each
    // returns Some on a hit, None to fall through to the local
    // match for Bool / Nil / Sym / Class / cross-type arms.
    if let Some(v) = numeric::numeric_call(recv, name, args, max_value_bytes)? {
        return Ok(Some(v));
    }
    if let Some(v) = string::string_call(recv, name, args, max_value_bytes)? {
        return Ok(Some(v));
    }
    Ok(match (recv, name, args) {

        (Value::Sym(a), "==", [Value::Sym(b)]) => Some(Value::Bool(a == b)),
        (Value::Sym(a), "!=", [Value::Sym(b)]) => Some(Value::Bool(a != b)),
        (Value::Nil, "to_s", []) => Some(Value::new_str("")),
        (Value::Nil, "inspect", []) => Some(Value::new_str("nil")),
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
