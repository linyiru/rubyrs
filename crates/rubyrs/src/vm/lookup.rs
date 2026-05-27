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

/// One way of a per-call-site polymorphic inline cache.
/// `class_ptr == 0` means the slot is unused.
#[derive(Clone, Default)]
pub(crate) struct CallCacheEntry {
    pub(crate) class_ptr: usize,
    pub(crate) generation: u32,
    pub(crate) method: Option<Rc<Method>>,
}

/// Per-call-site polymorphic inline cache. CRuby's vm_ic carries
/// a single class shape; rubyrs widens that to `IC_WAYS` so a call
/// site whose receiver alternates among a small set of classes
/// (`each` over a heterogeneous Array, `Each` block dispatching to
/// instances of a couple of user classes, ...) keeps hitting the
/// cache instead of thrashing on every iteration. A miss when all
/// ways are full evicts via simple round-robin (`next_way`); the
/// megamorphic case (> IC_WAYS distinct classes) degenerates to
/// the same uncached walk the old single-slot cache did.
pub(crate) const IC_WAYS: usize = 4;

/// Counters for the per-call-site IC, gated behind the `ic-stats`
/// cargo feature. ZST + `#[inline(always)]` no-op methods when
/// off, so production builds pay nothing — same shape as the
/// `trace-startup` feature.
///
/// `hits` / `misses` count receiver-class dispatch via
/// `lookup_method_cached`; `toplevel_hits` / `toplevel_misses` are
/// the analogous counters for the implicit-toplevel cache
/// (`lookup_toplevel_method_cached`). Keeping them separate lets a
/// reader tell whether a low aggregate hit rate is the receiver-
/// dispatch path going megamorphic or just the toplevel-`def`
/// recompile churn that follows any DefMethod.
#[cfg(feature = "ic-stats")]
#[derive(Clone, Default, Debug)]
pub struct IcStats {
    pub hits: u64,
    pub misses: u64,
    pub toplevel_hits: u64,
    pub toplevel_misses: u64,
}

#[cfg(not(feature = "ic-stats"))]
#[derive(Clone, Default, Debug)]
pub struct IcStats;

impl IcStats {
    /// Zero-initialised constructor. Feature-aware so the
    /// caller never has to know whether `IcStats` is a unit
    /// struct (feature off) or a four-field counter struct
    /// (feature on). Using a dedicated `new()` instead of
    /// `Default::default()` keeps clippy quiet on the
    /// production (feature-off) build — `default()` on a
    /// unit struct fires `default_constructed_unit_structs`.
    #[cfg(feature = "ic-stats")]
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self {
            hits: 0,
            misses: 0,
            toplevel_hits: 0,
            toplevel_misses: 0,
        }
    }
    #[cfg(not(feature = "ic-stats"))]
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self
    }

    #[cfg(feature = "ic-stats")]
    #[inline(always)]
    pub(crate) fn record_hit(&mut self) {
        self.hits += 1;
    }
    #[cfg(not(feature = "ic-stats"))]
    #[inline(always)]
    pub(crate) fn record_hit(&mut self) {}

    #[cfg(feature = "ic-stats")]
    #[inline(always)]
    pub(crate) fn record_miss(&mut self) {
        self.misses += 1;
    }
    #[cfg(not(feature = "ic-stats"))]
    #[inline(always)]
    pub(crate) fn record_miss(&mut self) {}

    #[cfg(feature = "ic-stats")]
    #[inline(always)]
    pub(crate) fn record_toplevel_hit(&mut self) {
        self.toplevel_hits += 1;
    }
    #[cfg(not(feature = "ic-stats"))]
    #[inline(always)]
    pub(crate) fn record_toplevel_hit(&mut self) {}

    #[cfg(feature = "ic-stats")]
    #[inline(always)]
    pub(crate) fn record_toplevel_miss(&mut self) {
        self.toplevel_misses += 1;
    }
    #[cfg(not(feature = "ic-stats"))]
    #[inline(always)]
    pub(crate) fn record_toplevel_miss(&mut self) {}

    /// Aggregate hit ratio across both receiver and toplevel
    /// paths. Returns `0.0` when no lookups have been recorded so
    /// callers don't have to special-case division by zero.
    ///
    /// When the `ic-stats` cargo feature is OFF, `IcStats` is a
    /// ZST and this is a stub that always returns `0.0`. The
    /// stub is `#[inline(always)]` so callers in downstream
    /// crates can write feature-agnostic code (always-callable
    /// API) without paying any cost on production builds.
    #[cfg(feature = "ic-stats")]
    pub fn hit_rate(&self) -> f64 {
        let h = self.hits + self.toplevel_hits;
        let total = h + self.misses + self.toplevel_misses;
        if total == 0 {
            0.0
        } else {
            h as f64 / total as f64
        }
    }
    #[cfg(not(feature = "ic-stats"))]
    #[inline(always)]
    pub fn hit_rate(&self) -> f64 {
        0.0
    }
}
const TOPLEVEL_METHOD_CACHE_KEY: usize = usize::MAX;
#[derive(Clone, Default)]
pub(crate) struct CallCache {
    pub(crate) ways: [CallCacheEntry; IC_WAYS],
    /// Next slot to evict on a miss. Wraps modulo `IC_WAYS`.
    pub(crate) next_way: u8,
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
    /// `Op::Call(...,cache_id)` instruction. Hits when one of the
    /// (up to `IC_WAYS`) cached entries matches the receiver class
    /// AND the global `method_gen` hasn't bumped since the entry
    /// was stored. A miss does an uncached walk and inserts at the
    /// `next_way` slot (round-robin eviction).
    #[inline]
    pub(crate) fn lookup_method_cached(&mut self, cls: &Rc<Class>, name_id: SymId, cache_id: u16) -> Option<Rc<Method>> {
        let class_ptr = Rc::as_ptr(cls) as usize;
        let idx = cache_id as usize;
        // Fast path: scan ways for a match.
        if idx < self.call_caches.len() {
            let cc = &self.call_caches[idx];
            let cur_gen = self.method_gen;
            for w in &cc.ways {
                if w.class_ptr == class_ptr && w.generation == cur_gen {
                    self.ic_stats.record_hit();
                    return w.method.clone();
                }
            }
        }
        // Miss: walk the chain, populate next way.
        self.ic_stats.record_miss();
        let m = self.lookup_method_uncached(cls, name_id);
        if idx < self.call_caches.len() {
            let cur_gen = self.method_gen;
            let cc = &mut self.call_caches[idx];
            let slot = (cc.next_way as usize) % IC_WAYS;
            cc.ways[slot] = CallCacheEntry {
                class_ptr,
                generation: cur_gen,
                method: m.clone(),
            };
            cc.next_way = ((slot + 1) % IC_WAYS) as u8;
        }
        m
    }

    pub(crate) fn lookup_toplevel_method_cached(
        &mut self,
        name_id: SymId,
        cache_id: u16,
    ) -> Option<Rc<Method>> {
        // Callers must gate this with `Vm::is_builtin_name` before invoking,
        // because the slot key (`TOPLEVEL_METHOD_CACHE_KEY`) is the only
        // signal `lookup_toplevel_method_cache_hit` uses — if a builtin
        // name's user `def` ever lands in this slot, the cache-hit fast
        // path in `do_call` will return it and silently shadow the
        // builtin. The debug assert below catches a future caller that
        // forgets the gate.
        debug_assert!(
            !Self::is_builtin_name(self.interner.resolve(name_id)),
            "lookup_toplevel_method_cached called for a name in is_builtin_name; the toplevel cache must not be populated for builtins"
        );
        let idx = cache_id as usize;
        if idx < self.call_caches.len() {
            let cc = &self.call_caches[idx];
            let cur_gen = self.method_gen;
            for w in &cc.ways {
                if w.class_ptr == TOPLEVEL_METHOD_CACHE_KEY && w.generation == cur_gen {
                    self.ic_stats.record_toplevel_hit();
                    return w.method.clone();
                }
            }
        }

        self.ic_stats.record_toplevel_miss();
        let m = self.toplevel_methods.get(&name_id).cloned();
        if idx < self.call_caches.len() {
            let cur_gen = self.method_gen;
            let cc = &mut self.call_caches[idx];
            let slot = (cc.next_way as usize) % IC_WAYS;
            cc.ways[slot] = CallCacheEntry {
                class_ptr: TOPLEVEL_METHOD_CACHE_KEY,
                generation: cur_gen,
                method: m.clone(),
            };
            cc.next_way = ((slot + 1) % IC_WAYS) as u8;
        }
        m
    }

    /// Fast-path hit lookup for the toplevel cache. Sister to
    /// `lookup_toplevel_method_cached`'s hot loop, but used by
    /// `do_call(no_recv)` before the full uncached fallback path
    /// would run. Takes `&mut self` purely so the `ic-stats`
    /// counter increment can land here — without that, the fast
    /// path would silently bypass the IC accounting and the
    /// `hit_rate()` aggregate would systematically under-report
    /// for hot toplevel call sites.
    pub(crate) fn lookup_toplevel_method_cache_hit(&mut self, cache_id: u16) -> Option<Rc<Method>> {
        let idx = cache_id as usize;
        if idx >= self.call_caches.len() {
            return None;
        }
        let cc = &self.call_caches[idx];
        let cur_gen = self.method_gen;
        for w in &cc.ways {
            if w.class_ptr == TOPLEVEL_METHOD_CACHE_KEY && w.generation == cur_gen {
                self.ic_stats.record_toplevel_hit();
                return w.method.clone();
            }
        }
        None
    }

    /// Walk `cls`'s singleton_prepends → `singleton_methods` →
    /// superclass chain (with the same chain at each level),
    /// returning the first hit. CRuby's metaclass model gives
    /// `Sub < Super` a singleton class whose parent is `Super`'s
    /// singleton class — so `Sub.foo` finds Super's `def self.foo`.
    /// We approximate that shape with a straight superclass walk
    /// over the per-class `singleton_methods` tables, plus the
    /// `singleton_prepends` chain that `class << X; prepend Mod; end`
    /// populates. Used by both the explicit-receiver `cls.foo`
    /// path (in `do_call`) and the bare `foo` path when `self`
    /// is a Value::Class (also in `do_call`).
    ///
    /// Cycle defensiveness mirrors `lookup_method_uncached` — two
    /// HashSets, one for the superclass chain and one fresh per
    /// step for the singleton-prepends graph.
    #[inline]
    pub(crate) fn lookup_class_singleton_method(&self, cls: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
        // Walk a prepended module (and its own prepends/includes
        // transitively) looking for an *instance* method named
        // `name_id`. Methods on a prepended-to-singleton module
        // are stored in `methods` (the module's normal table) —
        // CRuby treats `class << X; prepend M; end` as putting
        // M's instance methods in front of X's singleton methods.
        fn walk_module(
            m: &Rc<Class>,
            name_id: SymId,
            visited: &mut std::collections::HashSet<*const Class>,
        ) -> Option<Rc<Method>> {
            if !visited.insert(Rc::as_ptr(m)) { return None; }
            for pre in m.prepends.borrow().iter() {
                if let Some(found) = walk_module(pre, name_id, visited) {
                    return Some(found);
                }
            }
            if let Some(found) = m.methods.borrow().get(&name_id).cloned() {
                return Some(found);
            }
            for inc in m.includes.borrow().iter() {
                if let Some(found) = walk_module(inc, name_id, visited) {
                    return Some(found);
                }
            }
            None
        }
        let mut sc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
        let mut current = cls.clone();
        loop {
            if !sc_visited.insert(Rc::as_ptr(&current)) { return None; }
            let mut inc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
            for pre in current.singleton_prepends.borrow().iter() {
                if let Some(found) = walk_module(pre, name_id, &mut inc_visited) {
                    return Some(found);
                }
            }
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

    /// Plain method lookup walking the class chain, with no cache
    /// touch. Used for paths that don't benefit from caching (e.g.
    /// `initialize` resolution during `Class.new`).
    ///
    /// Lookup order at each class in the chain (CRuby ancestor walk):
    /// **prepends (transitive) → own methods → included modules
    /// (transitive) → superclass**. Prepended and included modules
    /// also walk their own prepends/includes recursively, so
    /// `module M; include N; end; class C; include M; end` resolves
    /// `N`'s methods on a `C` instance.
    #[inline]
    pub(crate) fn lookup_method_uncached(&self, cls: &Rc<Class>, name_id: SymId) -> Option<Rc<Method>> {
        // Recursive helper that walks one node's prepends, own
        // methods, then includes (transitively, in dispatch order).
        // Returns `Some` on the first hit. `visited` carries an
        // Rc-pointer set across the recursion to keep diamond
        // includes O(unique-modules) and to bail on cyclic graphs
        // (cext or direct manipulation can construct
        // `A.includes B; B.includes A` even though CRuby raises
        // `ArgumentError: cyclic include detected` at insertion —
        // rubyrs doesn't enforce that today, so the walker stays
        // defensive).
        fn walk_module(
            m: &Rc<Class>,
            name_id: SymId,
            visited: &mut std::collections::HashSet<*const Class>,
        ) -> Option<Rc<Method>> {
            if !visited.insert(Rc::as_ptr(m)) {
                return None;
            }
            for pre in m.prepends.borrow().iter() {
                if let Some(found) = walk_module(pre, name_id, visited) {
                    return Some(found);
                }
            }
            if let Some(found) = m.methods.borrow().get(&name_id).cloned() {
                return Some(found);
            }
            for inc in m.includes.borrow().iter() {
                if let Some(found) = walk_module(inc, name_id, visited) {
                    return Some(found);
                }
            }
            None
        }
        // Two separate visited sets:
        // - `sc_visited` protects against superclass-chain cycles.
        // - The inner set (fresh per superclass step) protects
        //   against include/prepend graph cycles + diamonds at
        //   ONE level. We can't share one set: a module
        //   transitively included at multiple superclass levels
        //   (rare but legal) needs to be walked at each level.
        let mut sc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
        let mut current = cls.clone();
        loop {
            if !sc_visited.insert(Rc::as_ptr(&current)) {
                return None;
            }
            let mut inc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
            if let Some(m) = walk_module(&current, name_id, &mut inc_visited) {
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
            "nil?" | "to_s" | "respond_to?" | "class" | "==" | "!=" | "!" | "!@" | "<=>" | "equal?" | "eql?"
            | "send" | "__send__"
            // The ivar-introspection family (`instance_variables` /
            // `instance_variable_get` / `instance_variable_set`)
            // is implemented as universal dispatch arms in
            // `Vm::do_call`, so feature detection has to agree:
            // `obj.respond_to?(:instance_variable_get)` should be
            // true for every value even if the result will be nil
            // (primitives) or raise FrozenError (set on primitives).
            | "instance_variables" | "instance_variable_get" | "instance_variable_set"
        ) {
            return true;
        }
        match recv {
            Value::Int(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" | "pow" |
                "<" | "<=" | ">" | ">=" |
                "&" | "|" | "^" | "<<" | ">>" | "~" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" | "even?" | "odd?" |
                "zero?" | "positive?" | "negative?" |
                "succ" | "next" | "pred" | "-@" | "+@" |
                "times" | "upto" | "downto" |
                "digits" | "bit_length" | "[]" |
                "eql?" | "hash"
            ),
            // Phase A BigInt subset + Phase B.1 `**` + Phase B.2
            // unary (`-@`/`+@`/`abs`) + Phase B.3 bit ops (`~`,
            // `& | ^`, `<< >>`) + Phase B.5 `pow(exp, mod)` +
            // Phase B.5 leftover (`bit_length`, `digits`) + Phase
            // B.6 iteration helpers (`times`, `upto`, `downto`) +
            // Phase B.7 hash-key surface (`eql?`, `hash`) —
            // arithmetic, comparison, to_s/inspect, pure
            // predicates, exponentiation (auto-promote /
            // DoS-capped), unary sign/magnitude, two's-complement
            // bit ops, modular exponentiation, two's-complement
            // bit count, base-N digit decomposition, block-form
            // iteration, and the canonical-equality entry points.
            // The predicates below only READ the bigint to
            // compute a Bool/Int, so they fit cleanly in the
            // existing bigint_primitive shape.
            #[cfg(feature = "bignum")]
            Value::BigInt(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" | "pow" |
                "-@" | "+@" | "abs" | "~" |
                "&" | "|" | "^" | "<<" | ">>" |
                "<" | "<=" | ">" | ">=" |
                "to_s" | "inspect" |
                "to_i" | "to_f" |
                "zero?" | "positive?" | "negative?" |
                "even?" | "odd?" |
                "bit_length" | "digits" |
                "times" | "upto" | "downto" |
                "eql?" | "hash"
            ),
            Value::Float(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" |
                "<" | "<=" | ">" | ">=" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" |
                "zero?" | "positive?" | "negative?" |
                "nan?" | "infinite?" | "finite?" |
                "eql?" | "hash" |
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
                "encode" | "force_encoding" | "valid_encoding?" | "encoding" | "b" |
                "unpack" | "unpack1" | "bytes" |
                "match?" | "match" | "scan" | "index" | "rindex" |
                "[]" | "slice" |
                "<<" | "concat" | "prepend" | "replace" |
                "freeze" | "frozen?" | "dup" | "+@" | "-@" | "dump" | "count" |
                "hash"
            ),
            Value::Sym(_) => matches!(name, "to_sym" | "to_s" | "inspect" | "name"),
            Value::Array(_) => matches!(name,
                "freeze" | "frozen?" |
                "length" | "size" | "push" | "<<" | "[]" | "[]=" |
                "unshift" | "prepend" |
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
                "freeze" | "frozen?" |
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
            Value::Class(cls) => {
                // Built-in class-level methods (`.new`, `.name`,
                // `.ancestors`, ...) are hardcoded; user-defined
                // class methods live in `singleton_methods` and
                // in `singleton_prepends` (the
                // `class << self; prepend Mod; end` chain).
                // Without consulting both, `C.respond_to?(:foo)`
                // would be false for any `def self.foo` or
                // singleton-prepended method, even though `C.foo`
                // dispatches successfully — diverges from CRuby.
                //
                // `autoload` / `private_constant` / `public_constant`
                // / `deprecate_constant` are stub no-ops (see
                // dispatch.rs); they're in the whitelist so
                // feature-detection (`C.respond_to?(:autoload)`)
                // agrees with what dispatch will accept.
                if matches!(name,
                    "new" | "name" | "to_s" | "inspect"
                    | "method_defined?" | "instance_method" | "undef_method"
                    | "superclass" | "ancestors" | "include?"
                    | "instance_methods" | "public_instance_methods"
                    | "private_instance_methods" | "protected_instance_methods"
                    | "constants"
                    | "autoload" | "private_constant" | "public_constant"
                    | "deprecate_constant"
                    | "singleton_class"
                    // `Class#allocate` — bare-instance allocator
                    // without calling `initialize`. Implemented in
                    // dispatch.rs's `new`-arm neighbour; primitive
                    // class shells (Integer/String/etc.) raise
                    // TypeError matching CRuby ("allocator undefined
                    // for Integer"), but still respond_to? true for
                    // the method name itself.
                    | "allocate"
                ) {
                    return true;
                }
                self.lookup_class_singleton_method(cls, name_id).is_some()
            },
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, name_id).is_some()
            }
            Value::Block(_) => matches!(name, "call" | "[]" | "()" | "curry" | ">>" | "<<"),
            #[cfg(feature = "regex")]
            Value::Regex(_) => matches!(name,
                "match" | "match?" | "===" | "=~" | "source" | "to_s" | "inspect"
                // `freeze` / `frozen?` are compatibility shims:
                // Regexp is immutable by construction so freezing
                // is a no-op, but real Ruby code calls `.freeze`
                // on regex literals (e.g. `HEADER_PARAM =
                // /.../.freeze` in sinatra/base.rb:32) and
                // `respond_to?(:freeze)` must agree with the
                // primitive arm in vm/string.rs.
                | "freeze" | "frozen?"
            ),
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
            #[cfg(feature = "bignum")]
            Value::BigInt(_) => "Integer", // unified with Fixnum since CRuby 2.4
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
            Value::Class(c) => if c.is_module { "Module" } else { "Class" },
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

/// `child` is-a `ancestor` if `ancestor` appears anywhere in
/// `child`'s ancestor chain — that is, the superclass walk *plus*
/// each class's transitive `prepends` and `includes`. Returns true
/// for `child == ancestor`. Wired into rescue-by-class filter
/// matching and the `is_a?` / `include?` dispatch arms.
pub(crate) fn class_is_a(child: &Rc<Class>, ancestor: &Rc<Class>) -> bool {
    fn walks_through(
        node: &Rc<Class>,
        target: &Rc<Class>,
        visited: &mut std::collections::HashSet<*const Class>,
    ) -> bool {
        if Rc::ptr_eq(node, target) { return true; }
        if !visited.insert(Rc::as_ptr(node)) {
            // Cycle — same defensiveness as `walk_module` /
            // `flatten_ancestors`. Without it, a cyclic
            // include/prepend graph stack-overflows `is_a?`.
            return false;
        }
        // Recurse through both prepends and includes — CRuby
        // `is_a?(M)` is true for any module reachable via either
        // chain transitively. Without the prepend recursion,
        // `module M; prepend N; end; class C; include M; end`
        // would report `c.is_a?(N) == false` even though
        // dispatch finds N's methods.
        for pre in node.prepends.borrow().iter() {
            if walks_through(pre, target, visited) { return true; }
        }
        for inc in node.includes.borrow().iter() {
            if walks_through(inc, target, visited) { return true; }
        }
        false
    }
    // Two separate visited sets — same rationale as
    // `lookup_method_uncached`: superclass-chain cycles vs.
    // include/prepend-graph cycles need independent protection.
    let mut sc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
    let mut current = child.clone();
    loop {
        if !sc_visited.insert(Rc::as_ptr(&current)) { return false; }
        let mut inc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
        if walks_through(&current, ancestor, &mut inc_visited) { return true; }
        let parent = current.superclass.borrow().clone();
        match parent {
            Some(p) => current = p,
            None => return false,
        }
    }
}

/// `target` is reachable via `cls`'s own `singleton_prepends`
/// (transitively through each prepended module's prepends /
/// includes). Used to dedupe `class << self; prepend Mod; end`
/// within ONE class's singleton chain.
///
/// Deliberately does NOT walk the superclass chain. In CRuby
/// each class has its own eigenclass with an independent
/// prepends list, so `class Sub < Super; class << self; prepend
/// Wrap; end; end` must INSERT Wrap on Sub's eigenclass even
/// when Super's eigenclass already has Wrap (CRuby's chain
/// would then show Wrap twice via separate IClass wrappers).
/// rubyrs's chain representation dedupes by Module identity in
/// `super_lookup`, so the resulting cross-class behaviour
/// collapses to a single wrap rather than CRuby's double-wrap
/// — a known gap, noted in
/// `crates/rubyrs/tests/diff/singleton_class_prepend.rb`. The
/// fixture deliberately doesn't exercise that case.
///
/// Dedup still applies WITHIN the local chain — repeated
/// `prepend M` on the same class is a no-op, and `prepend M`
/// when M is reachable through an already-prepended module's
/// includes/prepends is also a no-op. The TIdem block in the
/// fixture locks the same-class transitive case.
pub(crate) fn singleton_chain_contains(cls: &Rc<Class>, target: &Rc<Class>) -> bool {
    fn walks_through(
        node: &Rc<Class>,
        target: &Rc<Class>,
        visited: &mut std::collections::HashSet<*const Class>,
    ) -> bool {
        if Rc::ptr_eq(node, target) { return true; }
        if !visited.insert(Rc::as_ptr(node)) { return false; }
        for pre in node.prepends.borrow().iter() {
            if walks_through(pre, target, visited) { return true; }
        }
        for inc in node.includes.borrow().iter() {
            if walks_through(inc, target, visited) { return true; }
        }
        false
    }
    let mut visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
    for pre in cls.singleton_prepends.borrow().iter() {
        if walks_through(pre, target, &mut visited) { return true; }
    }
    false
}

/// Flatten a class's ancestor chain into a Vec in CRuby
/// dispatch order: at each level — prepends (transitive) → the
/// class/module itself → includes (transitive) → superclass.
/// Transitive means a prepended/included module's own
/// prepends/includes are walked too.
///
/// Deduplicates by Rc pointer using a `HashSet`: a diamond-
/// shaped include/prepend graph (`M includes A; M includes B;
/// A includes C; B includes C`) yields `[..., C]` once,
/// matching CRuby's linearization. The same visited set
/// guards against cyclic graphs (`A includes B; B includes A`)
/// which would otherwise recurse unboundedly.
///
/// Used by `super` (Op::Super / Op::ApplySuper) to find the
/// next ancestor after `defining_class`. Walking the chain
/// every super call is fine — super isn't a hot path in any
/// rubyrs spec we run today.
pub(crate) fn flatten_ancestors(cls: &Rc<Class>) -> Vec<Rc<Class>> {
    fn flatten_module(
        m: &Rc<Class>,
        out: &mut Vec<Rc<Class>>,
        visited: &mut std::collections::HashSet<*const Class>,
    ) {
        if !visited.insert(Rc::as_ptr(m)) {
            return;
        }
        for pre in m.prepends.borrow().iter() {
            flatten_module(pre, out, visited);
        }
        out.push(m.clone());
        for inc in m.includes.borrow().iter() {
            flatten_module(inc, out, visited);
        }
    }
    let mut out: Vec<Rc<Class>> = Vec::new();
    let mut visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
    let mut current = cls.clone();
    loop {
        if !visited.insert(Rc::as_ptr(&current)) {
            // Cycle through the superclass chain — CRuby blocks
            // it at class-definition time, but bail rather than
            // spin if user code somehow constructs one.
            return out;
        }
        for pre in current.prepends.borrow().iter() {
            flatten_module(pre, &mut out, &mut visited);
        }
        out.push(current.clone());
        for inc in current.includes.borrow().iter() {
            flatten_module(inc, &mut out, &mut visited);
        }
        let parent = current.superclass.borrow().clone();
        match parent {
            Some(p) => current = p,
            None => return out,
        }
    }
}

impl Vm {
    /// Shared `super` lookup for both `Op::Super` (positional args)
    /// and `Op::ApplySuper` (splat-assembled args). Walks the
    /// receiver's class ancestor chain (prepends + own + includes
    /// transitively, per `flatten_ancestors`), finds where the
    /// frame's `defining_class` sits, and resumes lookup from the
    /// next ancestor.
    ///
    /// Receiver-class lookup uses `Heap::class_of` (returns the
    /// singleton class if present) so a `def obj.foo` singleton
    /// method's `super` still threads through the singleton →
    /// real-class chain — `singleton_method_spec.rb` covers that.
    /// At each ancestor we scan only `methods.borrow()` because the
    /// ancestor list is already fully flattened (`include`d /
    /// `prepend`ed modules appear as their own entries).
    pub(crate) fn super_lookup(&mut self, name_id: SymId)
        -> Result<(Rc<crate::value::Method>, Value), crate::error::Trap>
    {
        let frame = self.frames.last().expect("ICE: super with empty frames");
        let self_val = frame.self_val.clone();
        let defining = match frame.defining_class.clone() {
            Some(c) => c,
            None => {
                return Err(self.trap(crate::error::RubyError::NoMethodError {
                    method: "super called outside of method".to_string(),
                    recv_type: self_val.type_name(),
                }));
            }
        };
        // Class-method super — `def self.foo; super; end`. The
        // receiver IS the class, defining_class is the class
        // itself, and the inherited method lives in the
        // superclass's `singleton_methods` table (mirror of how
        // `lookup_class_singleton_method` walks the superclass
        // chain). The instance-method ancestor walk wouldn't find
        // it because the methods aren't in `methods` and a class's
        // own class is "Class", not its inheritance chain.
        if let Value::Class(cls) = &self_val {
            // Build the class-method ancestor chain. Each node
            // carries an `is_module` flag so the post-defining walk
            // knows where to look: a singleton-prepended module
            // stores its methods in `methods`, while a class itself
            // stores singleton methods in `singleton_methods`.
            // Cycle defensiveness mirrors `flatten_ancestors`.
            let mut chain: Vec<(Rc<Class>, bool /* is_module */)> = Vec::new();
            let mut sc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
            // Flatten a singleton-prepended module into the chain,
            // walking its own prepends/includes transitively (so a
            // module's own `prepend`/`include` ancestry is honoured).
            fn flatten_prepended_module(
                m: &Rc<Class>,
                out: &mut Vec<(Rc<Class>, bool)>,
                visited: &mut std::collections::HashSet<*const Class>,
            ) {
                if !visited.insert(Rc::as_ptr(m)) { return; }
                for pre in m.prepends.borrow().iter() {
                    flatten_prepended_module(pre, out, visited);
                }
                out.push((m.clone(), true));
                for inc in m.includes.borrow().iter() {
                    flatten_prepended_module(inc, out, visited);
                }
            }
            // `inc_visited` is shared across ALL superclass steps —
            // not fresh per step like `lookup_method_uncached`. The
            // difference: `lookup_method_uncached` searches for a
            // method, so a module transitively included at multiple
            // superclass levels needs to be walked at each level.
            // `super_lookup` builds an ancestor *chain* and finds
            // the next-after-defining method; a module appearing
            // multiple times in the chain would let `super` resolve
            // back to the same module method (or otherwise pick
            // the wrong "next" implementation), so we need full-
            // chain dedup like `flatten_ancestors`.
            let mut inc_visited = std::collections::HashSet::new();
            let mut cur = cls.clone();
            loop {
                if !sc_visited.insert(Rc::as_ptr(&cur)) { break; }
                for pre in cur.singleton_prepends.borrow().iter() {
                    flatten_prepended_module(pre, &mut chain, &mut inc_visited);
                }
                chain.push((cur.clone(), false));
                let parent = cur.superclass.borrow().clone();
                match parent {
                    Some(p) => cur = p,
                    None => break,
                }
            }
            let m = chain.iter()
                .position(|(c, _)| Rc::ptr_eq(c, &defining))
                .map(|i| i + 1)
                .and_then(|i| chain.get(i..))
                .and_then(|tail| tail.iter().find_map(|(c, is_module)| {
                    if *is_module {
                        c.methods.borrow().get(&name_id).cloned()
                    } else {
                        c.singleton_methods.borrow().get(&name_id).cloned()
                    }
                }));
            return match m {
                Some(m) => Ok((m, self_val)),
                None => Err(self.trap(crate::error::RubyError::NoMethodError {
                    method: format!("super: no superclass method `{}'",
                        self.interner.resolve(name_id)),
                    recv_type: self_val.type_name(),
                })),
            };
        }
        let recv_cls = match &self_val {
            Value::Object(id) => self.heap.class_of(*id),
            other => match self.class_of(other) {
                Value::Class(c) => c,
                _ => {
                    return Err(self.trap(crate::error::RubyError::NoMethodError {
                        method: format!("super: no superclass method `{}'",
                            self.interner.resolve(name_id)),
                        recv_type: other.type_name(),
                    }));
                }
            },
        };
        let ancs = flatten_ancestors(&recv_cls);
        let m = ancs.iter()
            .position(|a| Rc::ptr_eq(a, &defining))
            .map(|i| i + 1)
            .and_then(|i| ancs.get(i..))
            .and_then(|tail| tail.iter().find_map(|a| {
                a.methods.borrow().get(&name_id).cloned()
            }));
        match m {
            Some(m) => Ok((m, self_val)),
            None => Err(self.trap(crate::error::RubyError::NoMethodError {
                method: format!("super: no superclass method `{}'",
                    self.interner.resolve(name_id)),
                recv_type: self_val.type_name(),
            })),
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
            fixed_arity: None,
            defining_class: None,
            visibility: Cell::new(Visibility::Public),
            closure: None,
        })
    }

    fn mk_class(name: &str, superclass: Option<Rc<Class>>) -> Rc<Class> {
        Rc::new(Class {
            name: name.to_string(),
            is_module: false,
            ivars: RefCell::new(HashMap::new()),
            methods: RefCell::new(HashMap::new()),
            singleton_methods: RefCell::new(HashMap::new()),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            superclass: RefCell::new(superclass),
            class_vars: RefCell::new(HashMap::new()),
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
        for w in &c.ways {
            assert_eq!(w.class_ptr, 0);
            assert_eq!(w.generation, 0);
            assert!(w.method.is_none());
        }
        assert_eq!(c.next_way, 0);
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

        // First call: miss, walks the chain, fills way 0.
        let first = vm.lookup_method_cached(&cls, name, 0).unwrap();
        assert!(Rc::ptr_eq(&first, &method));
        assert_eq!(vm.call_caches[0].ways[0].class_ptr, Rc::as_ptr(&cls) as usize);
        assert_eq!(vm.call_caches[0].ways[0].generation, vm.method_gen);

        // Remove the method from the class so an uncached walk would
        // return None. The cache should still serve the stale entry
        // (invalidation happens on method_gen bump, not class mutation).
        cls.methods.borrow_mut().remove(&name);
        let second = vm.lookup_method_cached(&cls, name, 0);
        assert!(second.is_some(), "cached entry should serve until method_gen bump");
    }

    #[test]
    fn lookup_method_cached_polymorphic_keeps_all_ways() {
        // Receiver class alternates among IC_WAYS distinct classes,
        // all defining the same method. Each class first hits its
        // own way after a miss, and from then on every call stays
        // on the fast path — verify by removing the methods AFTER
        // priming and checking that cached lookups still succeed.
        let (mut vm, _) = mk_vm();
        vm.ensure_call_caches(1);
        let name = vm.interner.intern("ping");
        let mut classes: Vec<(Rc<Class>, Rc<Method>)> = Vec::new();
        for i in 0..IC_WAYS {
            let cls = mk_class(&format!("C{i}"), None);
            let m = mk_method();
            cls.methods.borrow_mut().insert(name, m.clone());
            classes.push((cls, m));
        }
        // Prime — fills all IC_WAYS slots, one per class.
        for (cls, m) in &classes {
            let got = vm.lookup_method_cached(cls, name, 0).unwrap();
            assert!(Rc::ptr_eq(&got, m));
        }
        // Strip the methods so uncached walks would return None.
        for (cls, _) in &classes {
            cls.methods.borrow_mut().remove(&name);
        }
        // All IC_WAYS classes still hit the cache.
        for (cls, m) in &classes {
            let got = vm.lookup_method_cached(cls, name, 0);
            assert!(got.is_some(), "polymorphic IC should keep all {IC_WAYS} ways");
            assert!(Rc::ptr_eq(&got.unwrap(), m));
        }
    }

    #[test]
    fn lookup_method_cached_megamorphic_evicts_lru_round_robin() {
        // (IC_WAYS + 1) distinct classes — the oldest entry gets
        // evicted on insertion of the (IC_WAYS+1)-th. Verify by
        // showing the evicted class falls back to an uncached walk
        // (None after method removal) while the others still hit.
        let (mut vm, _) = mk_vm();
        vm.ensure_call_caches(1);
        let name = vm.interner.intern("ping");
        let n = IC_WAYS + 1;
        let mut classes: Vec<(Rc<Class>, Rc<Method>)> = Vec::with_capacity(n);
        for i in 0..n {
            let cls = mk_class(&format!("C{i}"), None);
            let m = mk_method();
            cls.methods.borrow_mut().insert(name, m.clone());
            classes.push((cls, m));
        }
        // Prime in order; the last insertion evicts way 0.
        for (cls, _) in &classes {
            let _ = vm.lookup_method_cached(cls, name, 0);
        }
        // Strip every method so any uncached walk returns None.
        for (cls, _) in &classes {
            cls.methods.borrow_mut().remove(&name);
        }
        // Check the surviving ways FIRST — looking up the evicted
        // class first would consume a cache slot (its uncached-walk
        // result gets installed at next_way) and contaminate the
        // remaining-way check that follows.
        for (cls, m) in &classes[1..] {
            let got = vm.lookup_method_cached(cls, name, 0);
            assert!(got.is_some(), "non-evicted ways still serve from cache");
            assert!(Rc::ptr_eq(&got.unwrap(), m));
        }
        // First class was evicted by the (IC_WAYS+1)-th insertion:
        // cache miss + uncached walk = None now that the method is
        // stripped.
        let first_after = vm.lookup_method_cached(&classes[0].0, name, 0);
        assert!(first_after.is_none(), "oldest entry should have been evicted");
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

    // This test pins the `debug_assert!` inside
    // `lookup_toplevel_method_cached`. `debug_assert!` is compiled
    // OUT in release builds, so the `#[should_panic]` would fail
    // (test "did not panic as expected") whenever `cargo test
    // --release` runs — which the repo's CI does. Gate the test
    // on `debug_assertions` so it only runs in the same builds the
    // assert itself runs in.
    //
    // The actual release-build safety net for the spooky-action
    // invariant ("the populator must not be called with a builtin
    // name, because the cache slot key can't distinguish user vs
    // builtin on a future fast-path hit") is now the explicit
    // `is_builtin_name` guard at the second populator call site
    // in `vm/dispatch.rs::do_call` — see the comment there. The
    // assert remains as a debug-mode tripwire for any future
    // populator-direct caller that forgets the gate.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "is_builtin_name")]
    fn lookup_toplevel_method_cached_rejects_builtin_name() {
        let (mut vm, _) = mk_vm();
        vm.ensure_call_caches(1);
        let name = vm.interner.intern("sprintf");
        let _ = vm.lookup_toplevel_method_cached(name, 0);
    }
}
