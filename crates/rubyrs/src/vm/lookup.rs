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
#[derive(Default)]
pub(crate) struct CallCache {
    pub(crate) class_ptr: usize, // 0 = empty
    pub(crate) generation: u32,
    pub(crate) method: Option<Rc<Method>>,
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
    ///
    /// Lookup order at each class in the chain: own methods → included
    /// modules (recursively, depth-first; a module's own includes are
    /// part of the walk) → superclass. Mirrors CRuby's ancestor walk
    /// where `Person.ancestors == [Person, IncludedB, IncludedA, Object,
    /// Kernel, BasicObject]` and `include` chains compose transitively
    /// Walk `cls`'s `singleton_methods` table, then the superclass
    /// chain's, returning the first hit. CRuby's metaclass model
    /// gives `Sub < Super` a singleton class whose parent is
    /// `Super`'s singleton class — so `Sub.foo` finds Super's
    /// `def self.foo`. We approximate that shape with a straight
    /// superclass walk over the per-class `singleton_methods`
    /// tables. Used by both the explicit-receiver `cls.foo` path
    /// (in `do_call`) and the bare `foo` path when `self` is a
    /// Value::Class (also in `do_call`) — keeping both in lockstep
    /// avoids "self.bar finds it but bare bar doesn't" surprises.
    #[inline]
    pub(crate) fn lookup_class_singleton_method(&self, cls: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
        let mut current = cls.clone();
        loop {
            if let Some(m) = current.singleton_methods.borrow().get(&name_id).cloned() {
                return Some(m);
            }
            let parent = current.superclass.borrow().clone();
            match parent {
                Some(p) => current = p,
                None => return None,
            }
        }
    }

    /// (`module M; include N; end; class C; include M; end` ⇒ C
    /// resolves N's methods).
    #[inline]
    pub(crate) fn lookup_method_uncached(&self, cls: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
        // Recursive helper that walks one node's own methods + its
        // includes (transitively). Returns `Some` on the first hit.
        fn walk_module(m: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
            if let Some(found) = m.methods.borrow().get(&name_id).cloned() {
                return Some(found);
            }
            for inc in m.includes.borrow().iter() {
                if let Some(found) = walk_module(inc, name_id) {
                    return Some(found);
                }
            }
            None
        }
        let mut current = cls.clone();
        loop {
            if let Some(m) = walk_module(&current, name_id) {
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
        let name: &str = self.interner.resolve(name_id);
        // Universal — every receiver responds to these.
        // `send` / `__send__` go here because the `do_call`
        // recogniser handles them on any receiver type (primitive
        // or user-defined), so `obj.respond_to?(:send)` should
        // be true for every value — feature-detection has to
        // agree with what dispatch will actually accept.
        if matches!(name,
            "nil?" | "to_s" | "respond_to?" | "class" | "==" | "!=" | "!" | "!@" | "<=>" | "equal?"
            | "send" | "__send__"
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
                "times" | "upto" | "downto" |
                "digits" | "bit_length" | "[]"
            ),
            Value::Float(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" |
                "<" | "<=" | ">" | ">=" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" |
                "zero?" | "positive?" | "negative?" |
                "nan?" | "infinite?" | "finite?" |
                "floor" | "ceil" | "round" | "truncate" |
                "-@" | "+@"
            ),
            Value::Str(_) => matches!(name,
                "+" | "*" | "%" | "<" | "<=" | ">" | ">=" |
                "length" | "size" | "empty?" |
                "upcase" | "downcase" | "reverse" |
                "strip" | "lstrip" | "rstrip" |
                "center" | "ljust" | "rjust" |
                "include?" | "start_with?" | "end_with?" |
                "to_i" | "to_f" | "chars" | "split" | "to_sym" |
                "to_s" | "inspect" |
                "sub" | "gsub" | "tr" | "squeeze" |
                "encode" | "force_encoding" | "valid_encoding?" | "encoding" |
                "unpack" | "unpack1" | "bytes" |
                "match?" | "match" | "scan" | "index" | "rindex" |
                "[]" | "slice" |
                "<<" | "concat" | "prepend" | "replace" |
                "freeze" | "frozen?" | "dup"
            ),
            Value::Sym(_) => matches!(name, "to_sym" | "to_s" | "inspect" | "name"),
            Value::Array(_) => matches!(name,
                "length" | "size" | "push" | "<<" | "[]" | "[]=" |
                "shift" | "pop" | "reverse_each" |
                "first" | "last" | "empty?" | "include?" |
                "count" | "sum" | "min" | "max" | "sort" | "tally" |
                "combination" | "permutation" | "assoc" | "rassoc" | "pack" |
                "inject" | "reduce" |
                "to_a" | "reverse" | "uniq" | "compact" |
                "flatten" | "join" |
                "+" | "-" | "concat" | "take" | "drop" |
                "each" | "map" | "select" | "filter" |
                "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "sort_by" |
                "min_by" | "max_by" | "group_by" |
                "each_with_object" | "partition" | "chunk_while" | "bsearch" |
                "take_while" | "drop_while" |
                "zip" |
                "sort!" | "uniq!" | "compact!" | "flatten!" | "reverse!" |
                "flat_map" | "collect_concat" | "chunk" | "filter_map" |
                "each_slice" | "each_cons" |
                "inspect"
            ),
            Value::Hash(_) => matches!(name,
                "length" | "size" | "[]" | "[]=" | "empty?" |
                "include?" | "has_key?" | "key?" | "member?" |
                "keys" | "values" | "to_h" | "to_a" |
                "merge" | "delete" | "invert" | "store" | "except" | "slice" |
                "each" | "each_pair" |
                "select" | "filter" | "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "map" | "collect" | "fetch" |
                "sort" | "sort_by" | "min_by" | "max_by" | "group_by" |
                "transform_keys" | "transform_values" |
                "compact" | "compact!" | "filter_map" |
                "inspect"
            ),
            Value::Range(_) => matches!(name,
                "begin" | "end" | "first" | "last" | "min" | "max" |
                "size" | "length" | "count" |
                "exclude_end?" | "include?" | "cover?" | "step" | "to_a" |
                "sum" | "inject" | "reduce" |
                "each" | "map" | "select" | "filter" |
                "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "each_with_object" |
                "partition" | "min_by" | "max_by" |
                "group_by" | "sort_by" | "sort"
            ),
            Value::Bool(_) | Value::Nil => matches!(name, "to_s" | "inspect"),
            Value::Class(_) => matches!(name, "new" | "name" | "to_s" | "inspect" | "method_defined?" | "instance_method" | "undef_method"),
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, name_id).is_some()
            }
            Value::Block(_) => matches!(name, "call" | "[]" | "()" | "curry" | ">>" | "<<"),
            #[cfg(feature = "regex")]
            Value::Regex(_) => matches!(name, "match" | "match?" | "===" | "=~" | "source" | "to_s" | "inspect"),
            Value::BoundMethod(_) => matches!(name, "call" | "[]" | "()" | "unbind" | "arity" | "parameters" | "==" | ">>" | "<<" | "curry" | "to_proc" | "owner" | "receiver" | "hash" | "source_location"),
            Value::UnboundMethod(_) => matches!(name, "bind" | "arity" | "parameters" | "==" | "owner" | "hash" | "source_location"),
            Value::CurriedProc(_) => matches!(name, "call" | "[]" | "()"),
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
            #[cfg(feature = "regex")]
            Value::Regex(_) => "Regexp",
            Value::BoundMethod(_) => "Method",
            Value::UnboundMethod(_) => "UnboundMethod",
            Value::CurriedProc(_) => "Proc",
            // `Object#class` script call: CRuby reports the
            // user-declared class, not the eigenclass. Use
            // `real_class_of` so a `def obj.foo` installation
            // doesn't change what `obj.class` returns.
            Value::Object(id) => return Value::Class(self.heap.real_class_of(*id)),
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
    fn walks_through(node: &Rc<Class>, target: &Rc<Class>) -> bool {
        if Rc::ptr_eq(node, target) { return true; }
        for inc in node.includes.borrow().iter() {
            if walks_through(inc, target) { return true; }
        }
        false
    }
    let mut current = child.clone();
    loop {
        // Recursively walk included modules so transitive includes
        // (`include M; M includes N` ⇒ `class_is_a(C, N) == true`)
        // resolve. Matches CRuby's rescue-filter behaviour and the
        // `is_a?` / `kind_of?` predicates.
        if walks_through(&current, ancestor) { return true; }
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
            // Symbol#name (Ruby 3.0+) returns the same content as
            // #to_s. CRuby distinguishes by returning a frozen
            // String for #name vs. a mutable copy for #to_s;
            // rubyrs doesn't model the frozen distinction at
            // Value level, so the two are operationally the same
            // here. Lets msgpack-ruby `lib/msgpack/symbol.rb`'s
            // `if method_defined?(:name)` Ruby-version probe land
            // on the modern branch.
            (Value::Sym(id), "name", []) => Some(Value::new_str(self.interner.resolve(*id).to_string())),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use crate::bytecode::Proto;
    use crate::intern::Interner;
    use crate::value::Visibility;

    /// Minimal Method for tests — no params, no closure, proto_idx 0.
    /// Tests below only compare by Rc identity, so the field contents
    /// don't matter.
    fn mk_method() -> Rc<Method> {
        Rc::new(Method {
            params: Vec::new(),
            proto_idx: 0,
            defining_class: None,
            visibility: Cell::new(Visibility::Public),
            closure: None,
        })
    }

    fn mk_class(name: &str, superclass: Option<Rc<Class>>) -> Rc<Class> {
        Rc::new(Class {
            name: name.to_string(),
            methods: RefCell::new(HashMap::new()),
            singleton_methods: RefCell::new(HashMap::new()),
            includes: RefCell::new(Vec::new()),
            superclass: RefCell::new(superclass),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        })
    }

    /// Single-arg `Vm` constructor wrapped for tests. Uses an empty
    /// protos vec and a fresh interner — every test below only
    /// touches the class/method tables, never executes bytecode.
    fn mk_vm() -> (Vm, Interner) {
        // Vm consumes the interner; we keep a clone-free shadow by
        // building a second Interner for places that need to read
        // SymIds back. Both are deterministic since intern is order-
        // dependent on insertion; we only intern strings via vm.interner
        // in the tests below, so the shadow is unused.
        let interner = Interner::new();
        let vm = Vm::new(Vec::<Proto>::new(), interner);
        (vm, Interner::new())
    }

    #[test]
    fn call_cache_default_is_empty() {
        let c = CallCache::default();
        assert_eq!(c.class_ptr, 0);
        assert_eq!(c.generation, 0);
        assert!(c.method.is_none());
    }

    #[test]
    fn class_is_a_self_match() {
        let a = mk_class("A", None);
        assert!(class_is_a(&a, &a));
    }

    #[test]
    fn class_is_a_walks_superclass_chain() {
        let grandparent = mk_class("Animal", None);
        let parent = mk_class("Dog", Some(grandparent.clone()));
        let child = mk_class("Puppy", Some(parent.clone()));

        // Each is-a relation along the chain holds.
        assert!(class_is_a(&child, &parent));
        assert!(class_is_a(&child, &grandparent));
        assert!(class_is_a(&parent, &grandparent));

        // The reverse direction does not.
        assert!(!class_is_a(&parent, &child));
        assert!(!class_is_a(&grandparent, &child));
    }

    #[test]
    fn class_is_a_unrelated_returns_false() {
        let a = mk_class("A", None);
        let b = mk_class("B", None);
        assert!(!class_is_a(&a, &b));
        assert!(!class_is_a(&b, &a));
    }

    #[test]
    fn lookup_method_uncached_hits_own_class() {
        let (mut vm, _) = mk_vm();
        let name = vm.interner.intern("greet");
        let cls = mk_class("Greeter", None);
        let method = mk_method();
        cls.methods.borrow_mut().insert(name, method.clone());

        let found = vm.lookup_method_uncached(&cls, name);
        assert!(found.is_some());
        // Same Rc — method storage is identity-cloned.
        assert!(Rc::ptr_eq(&found.unwrap(), &method));
    }

    #[test]
    fn lookup_method_uncached_walks_superclass_chain() {
        let (mut vm, _) = mk_vm();
        let name = vm.interner.intern("bark");

        let animal = mk_class("Animal", None);
        let method = mk_method();
        animal.methods.borrow_mut().insert(name, method.clone());

        let dog = mk_class("Dog", Some(animal.clone()));
        // dog has no own method — lookup must walk to animal.

        let found = vm.lookup_method_uncached(&dog, name);
        assert!(found.is_some());
        assert!(Rc::ptr_eq(&found.unwrap(), &method));
    }

    #[test]
    fn lookup_method_uncached_returns_none_for_missing() {
        let (mut vm, _) = mk_vm();
        let name = vm.interner.intern("nonexistent");
        let cls = mk_class("Empty", None);

        assert!(vm.lookup_method_uncached(&cls, name).is_none());
    }

    #[test]
    fn lookup_method_cached_fills_then_serves_from_cache() {
        let (mut vm, _) = mk_vm();
        vm.ensure_call_caches(1);
        let name = vm.interner.intern("ping");
        let cls = mk_class("Pinger", None);
        let method = mk_method();
        cls.methods.borrow_mut().insert(name, method.clone());

        // First call: miss, walks the chain, fills slot 0.
        let first = vm.lookup_method_cached(&cls, name, 0).unwrap();
        assert!(Rc::ptr_eq(&first, &method));
        assert_eq!(vm.call_caches[0].class_ptr, Rc::as_ptr(&cls) as usize);
        assert_eq!(vm.call_caches[0].generation, vm.method_gen);

        // Remove the method from the class so an uncached walk would
        // return None. The cache should still serve the stale entry
        // (invalidation happens on method_gen bump, not class mutation).
        cls.methods.borrow_mut().remove(&name);
        let second = vm.lookup_method_cached(&cls, name, 0);
        assert!(second.is_some(), "cached entry should serve until method_gen bump");
    }

    #[test]
    fn lookup_method_cached_misses_after_method_gen_bump() {
        let (mut vm, _) = mk_vm();
        vm.ensure_call_caches(1);
        let name = vm.interner.intern("ping");
        let cls = mk_class("Pinger", None);
        let method = mk_method();
        cls.methods.borrow_mut().insert(name, method.clone());

        // Prime the cache.
        let _ = vm.lookup_method_cached(&cls, name, 0);

        // Bump method_gen (simulating a new Op::DefMethod elsewhere).
        vm.method_gen += 1;
        cls.methods.borrow_mut().remove(&name);

        // Cache now stale, walks the chain, finds nothing.
        let after = vm.lookup_method_cached(&cls, name, 0);
        assert!(after.is_none());
    }
}
