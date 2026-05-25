//! Method resolution + receiver typing. Mirrors CRuby's
//! `vm_method.c` (method-entry lookup with inline cache) plus the
//! `class.c` ancestor walk used by rescue-by-class.
//!
//! Contents:
//!   - `CallCache` — per-call-site method inline cache (Tier1-1 / P1-B).
//!   - `Vm::ensure_call_caches` / `lookup_method_cached` /
//!     `lookup_method_uncached` — the resolution path used by every
//!     Object dispatch site in `do_call` / `do_call_block`.
//!   - `Vm::responds_to` — Object#respond_to? backend.
//!   - `Vm::class_of` — Object#class backend.
//!   - `Vm::sym_primitive` — Symbol primitives (`<=>`, `to_proc`, ...).
//!   - `class_is_a` — superclass-chain walk used by rescue filter
//!     matching in `unwind_with_exception`.

use std::rc::Rc;

use crate::intern::SymId;
use crate::value::{Class, Method, Value};

use super::Vm;

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
