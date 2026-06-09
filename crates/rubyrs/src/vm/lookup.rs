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

use crate::error::{RubyError, Trap};
use crate::intern::SymId;
use crate::value::{BuiltinMeta, Class, Method, Value};

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
///
/// Sized at 5 ways: PR #175 measured a sharp cliff at exactly
/// `IC_WAYS + 1 = 5` shapes under the prior `IC_WAYS = 4`
/// (hit rate ~0.5 with round-robin eviction). PR #185 widened
/// to 5 ways so 5-shape workloads now hit; the cliff moved to
/// 6 shapes (still ~0.5 there). 5 is the smallest width that
/// comfortably absorbs the common "Array of 4 user-class
/// instances plus a sentinel" pattern.
/// Each extra way is ~24 bytes per call site, so 1 000 call
/// sites cost ~24 KB and ~10 000 call sites cost ~240 KB —
/// negligible against any real-world memory budget but worth
/// pricing accurately if a future widening is considered.
pub(crate) const IC_WAYS: usize = 5;

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
            // Walk modules extended into this class's singleton —
            // `Klass.extend Mod`. M's instance methods sit between
            // Klass's own singleton_methods and the superclass step,
            // matching CRuby's metaclass ancestor walk
            // (Klass.singleton_class → Mod → superclass.singleton_class).
            for inc in current.singleton_includes.borrow().iter() {
                if let Some(found) = walk_module(inc, name_id, &mut inc_visited) {
                    return Some(found);
                }
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

    /// Resolve `Method#super_method`: walk the ancestor chain of
    /// `cls` in dispatch order (prepend → own → include → super)
    /// and return the next `(class, method)` pair after passing the
    /// `defining_class` anchor. Mirrors the chain
    /// `lookup_method_uncached` walks; the only difference is the
    /// `past_anchor` toggle that suppresses hits before reaching
    /// the anchor and re-enables them after. Returns None when the
    /// anchor isn't on the chain (rare — would mean the method
    /// snapshot disagrees with the receiver class graph), or when
    /// the super chain terminates without another definition.
    pub(crate) fn lookup_super_method_uncached(
        &self,
        cls: &Rc<Class>,
        name_id: SymId,
        defining_class: &Rc<Class>,
    ) -> Option<(Rc<Class>, Rc<Method>)> {
        let anchor = Rc::as_ptr(defining_class);
        fn walk_module(
            m: &Rc<Class>,
            name_id: SymId,
            anchor: *const Class,
            past_anchor: &mut bool,
            visited: &mut std::collections::HashSet<*const Class>,
        ) -> Option<(Rc<Class>, Rc<Method>)> {
            if !visited.insert(Rc::as_ptr(m)) {
                return None;
            }
            for pre in m.prepends.borrow().iter() {
                if let Some(r) = walk_module(pre, name_id, anchor, past_anchor, visited) {
                    return Some(r);
                }
            }
            if Rc::as_ptr(m) == anchor {
                *past_anchor = true;
            } else if *past_anchor
                && let Some(found) = m.methods.borrow().get(&name_id).cloned()
            {
                return Some((m.clone(), found));
            }
            for inc in m.includes.borrow().iter() {
                if let Some(r) = walk_module(inc, name_id, anchor, past_anchor, visited) {
                    return Some(r);
                }
            }
            None
        }
        let mut past_anchor = false;
        let mut sc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
        let mut current = cls.clone();
        loop {
            if !sc_visited.insert(Rc::as_ptr(&current)) {
                return None;
            }
            let mut inc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
            if let Some(r) = walk_module(&current, name_id, anchor, &mut past_anchor, &mut inc_visited) {
                return Some(r);
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
    pub(crate) fn responds_to(&self, recv: &Value, name_id: SymId, include_private: bool) -> bool {
        // Host fns (`register_fn(...)` — battery / cext / embed-host
        // wiring) are reachable as bareword calls from any frame. The
        // matching `__defined_method?` arm in `vm/kernel.rs` already
        // treats them as "method" hits; `respond_to?` is the
        // companion reflection surface and has to agree. Without
        // this, the canonical capability-detection idiom
        //   `respond_to?(:__rubyrs_some_battery_fn)`
        // silently reports false even though
        //   `defined?(__rubyrs_some_battery_fn)`
        // already reports "method". Sinatra GAPS Gap #5 — recorded
        // there as the reflection-paths-disagree class of bug.
        // Receiver-agnostic on purpose: host fns aren't bound to a
        // class, dispatch on them ignores receiver shape.
        if self.host_fns.contains_key(&name_id) {
            return true;
        }
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
            // Universal dispatch arms wired by the
            // object_id/__id__/hash/frozen?/inspect PR. All
            // succeed on every receiver type, so feature
            // detection (`obj.respond_to?(:object_id)`) must
            // agree.
            | "object_id" | "__id__" | "hash" | "frozen?" | "freeze" | "inspect"
            // Object-extras family (Kleisli `then`/`yield_self`,
            // debug `tap`, identity `itself`) — universal arms
            // in `do_call` succeed on every receiver, so feature
            // detection has to agree.
            | "itself" | "tap" | "then" | "yield_self"
            // The ivar-introspection family (`instance_variables` /
            // `instance_variable_get` / `instance_variable_set`)
            // is implemented as universal dispatch arms in
            // `Vm::do_call`, so feature detection has to agree:
            // `obj.respond_to?(:instance_variable_get)` should be
            // true for every value even if the result will be nil
            // (primitives) or raise FrozenError (set on primitives).
            | "instance_variables" | "instance_variable_get" | "instance_variable_set"
            | "instance_variable_defined?"
            // `instance_exec` is a universal dispatch arm (block-form
            // self-swap, parity with `instance_eval`). Whitelisted
            // here so feature detection agrees with what dispatch
            // accepts on every receiver type. `instance_eval`
            // joins for the same reason — both are universal
            // dispatch arms and both surface in BasicObject's
            // reflection registry.
            | "instance_exec" | "instance_eval"
            // Receiver-side method introspection — `methods` /
            // `public_methods` / `private_methods` /
            // `protected_methods` / `singleton_methods` are
            // implemented as universal dispatch arms in
            // `Vm::do_call`. Non-Object/non-Class receivers
            // succeed by returning an empty Array (rubyrs's
            // subset doesn't enumerate Kernel-level entries per
            // value), so feature detection can stay universal.
            | "methods" | "public_methods" | "private_methods" | "protected_methods"
            | "singleton_methods"
            // Method getter triple — `method` is universal too;
            // `singleton_method` / `public_method` join as
            // narrowed siblings (NameError when the lookup
            // doesn't match the variant's filter). Dispatch
            // succeeds for every receiver that `method(:name)`
            // already worked on; primitive arms intercept their
            // own bound-method shapes elsewhere.
            | "method" | "singleton_method" | "public_method"
        ) {
            return true;
        }
        let yes = match recv {
            Value::Int(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" | "pow" |
                "<" | "<=" | ">" | ">=" |
                "&" | "|" | "^" | "<<" | ">>" | "~" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" | "even?" | "odd?" |
                "zero?" | "positive?" | "negative?" |
                "succ" | "next" | "pred" | "-@" | "+@" |
                "times" | "upto" | "downto" | "step" |
                "digits" | "bit_length" | "size" | "[]" |
                "allbits?" | "anybits?" | "nobits?" |
                "gcd" | "lcm" | "fdiv" | "divmod" |
                "ceil" | "floor" | "round" | "truncate" |
                "chr" | "coerce" |
                "to_r" | "rationalize" |
                "eql?" | "hash" |
                "dup" | "clone"
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
                "bit_length" | "digits" | "size" |
                "allbits?" | "anybits?" | "nobits?" |
                "gcd" | "lcm" | "fdiv" | "divmod" |
                "ceil" | "floor" | "round" | "truncate" |
                "times" | "upto" | "downto" |
                "succ" | "next" | "pred" |
                "chr" | "coerce" |
                "to_r" | "rationalize" |
                "eql?" | "hash" |
                "dup" | "clone"
            ),
            Value::Float(_) => matches!(name,
                "+" | "-" | "*" | "/" | "%" | "**" |
                "<" | "<=" | ">" | ">=" |
                "to_s" | "inspect" |
                "to_i" | "to_f" | "abs" |
                "zero?" | "positive?" | "negative?" |
                "nan?" | "infinite?" | "finite?" |
                "eql?" | "hash" |
                "floor" | "ceil" | "round" | "truncate" | "divmod" | "step" |
                "-@" | "+@" |
                "to_r" | "rationalize" |
                "coerce" |
                "dup" | "clone"
            ),
            Value::Str(_) => matches!(name,
                "+" | "*" | "%" | "<" | "<=" | ">" | ">=" |
                "length" | "size" | "empty?" |
                "upcase" | "downcase" | "reverse" |
                "capitalize" | "swapcase" |
                "upcase!" | "downcase!" | "reverse!" |
                "capitalize!" | "swapcase!" |
                "strip" | "lstrip" | "rstrip" |
                "strip!" | "lstrip!" | "rstrip!" |
                "chomp" | "chomp!" |
                "tr!" | "squeeze!" |
                "center" | "ljust" | "rjust" |
                "include?" | "start_with?" | "end_with?" |
                "delete_prefix" | "delete_suffix" | "delete_prefix!" | "delete_suffix!" |
                "to_i" | "to_f" | "chars" | "split" | "lines" | "each_line" | "to_sym" | "intern" |
                "to_s" | "to_str" | "inspect" |
                "sub" | "sub!" | "gsub" | "gsub!" |
                "tr" | "squeeze" |
                "encode" | "force_encoding" | "valid_encoding?" | "encoding" | "b" |
                "unpack" | "unpack1" | "bytes" | "getbyte" |
                "match?" | "match" | "scan" | "index" | "rindex" |
                "[]" | "slice" |
                "<<" | "concat" | "prepend" | "replace" |
                "freeze" | "frozen?" | "dup" | "+@" | "-@" | "dump" | "count" |
                "hash"
            ),
            Value::Sym(_) => matches!(name, "to_sym" | "to_s" | "inspect" | "name" | "succ" | "next" | "dup" | "clone"
                | "empty?" | "length" | "size" | "upcase" | "downcase" | "capitalize" | "swapcase"),
            Value::Array(_) => matches!(name,
                "freeze" | "frozen?" |
                "length" | "size" | "push" | "<<" | "[]" | "[]=" |
                "unshift" | "prepend" | "insert" |
                "shift" | "pop" | "delete" | "reverse_each" |
                "first" | "last" | "empty?" | "include?" | "member?" |
                "count" | "sum" | "min" | "max" | "sort" | "tally" |
                "combination" | "permutation" | "assoc" | "rassoc" | "pack" |
                "inject" | "reduce" |
                "to_a" | "reverse" | "uniq" | "compact" |
                "flatten" | "join" |
                "+" | "-" | "concat" | "replace" | "clear" | "take" | "drop" |
                "find_index" | "index" |
                "each" | "map" | "collect" | "select" | "filter" |
                "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "each_index" | "sort_by" |
                "min_by" | "max_by" | "group_by" |
                "each_with_object" | "partition" | "chunk_while" | "slice_when" | "bsearch" |
                "take_while" | "drop_while" |
                "zip" |
                "sort!" | "uniq!" | "compact!" | "flatten!" | "reverse!" |
                "rotate" | "rotate!" |
                "map!" | "collect!" | "sort_by!" |
                "delete_if" | "reject!" | "keep_if" | "select!" | "filter!" |
                "flat_map" | "collect_concat" | "chunk" | "filter_map" |
                "each_slice" | "each_cons" | "cycle" |
                "inspect" |
                // `dup` / `clone` — shallow copy. Tier-1 Arrays
                // don't model `freeze` beyond a no-op so the two
                // are indistinguishable. (TRY_RUNS layer #26.)
                "dup" | "clone"
            ),
            Value::Hash(_) => matches!(name,
                "freeze" | "frozen?" |
                "length" | "size" | "[]" | "[]=" | "empty?" |
                "include?" | "has_key?" | "key?" | "member?" |
                "keys" | "values" | "to_h" | "to_hash" | "to_a" |
                "merge" | "merge!" | "update" | "replace" | "clear" | "delete" | "invert" | "key" | "store" | "except" | "slice" | "dup" | "clone" |
                "each" | "each_pair" |
                "select" | "filter" | "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "map" | "collect" | "fetch" |
                "sort" | "sort_by" | "min_by" | "max_by" | "group_by" |
                "transform_keys" | "transform_values" |
                "transform_keys!" | "transform_values!" |
                "compact" | "compact!" | "filter_map" |
                "default" | "default_proc" | "default_proc=" | "count" | "each_with_object" |
                "flat_map" | "collect_concat" | "reduce" | "inject" | "sum" |
                "first" | "min" | "max" | "one?" | "partition" |
                "take" | "drop" | "take_while" | "drop_while" | "find_index" |
                "tally" | "uniq" | "zip" |
                "each_slice" | "each_cons" | "chunk_while" | "slice_when" |
                "inspect"
            ),
            Value::Range(_) => matches!(name,
                "begin" | "end" | "first" | "last" | "min" | "max" |
                "size" | "length" | "count" |
                "exclude_end?" | "include?" | "member?" | "cover?" | "step" | "to_a" |
                "sum" | "inject" | "reduce" |
                "each" | "map" | "select" | "filter" |
                "reject" | "find" | "detect" |
                "any?" | "all?" | "none?" |
                "each_with_index" | "each_with_object" |
                "partition" | "min_by" | "max_by" |
                "group_by" | "sort_by" | "sort" |
                "each_slice" | "each_cons" | "chunk_while" | "slice_when"
            ),
            Value::Nil => matches!(name, "to_s" | "inspect" | "dup" | "clone" | "to_a" | "to_h"),
            Value::Bool(_) => matches!(name, "to_s" | "inspect" | "dup" | "clone"),
            // Phase C.1 readers + Phase C.2 arithmetic / comparison.
            // `coerce` is included so cross-type promotion (Rational
            // arg with Int/Float receiver) routes through the
            // standard protocol — `1 + Rational(1, 2)` goes through
            // `try_rational_binop` directly, but `1.send(:+, r)` via
            // method-call dispatch consults respond_to.
            Value::Rational(_) => matches!(name,
                "numerator" | "denominator" |
                "to_s" | "inspect" | "to_r" |
                "to_i" | "to_f" |
                "+" | "-" | "*" | "/" | "**" |
                "<" | "<=" | ">" | ">=" | "<=>" |
                "coerce"
            ),
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
                    | "method_defined?" | "instance_method" | "undef_method" | "remove_method"
                    | "ancestors" | "include?"
                    | "<" | "<=" | ">" | ">="
                    | "instance_methods" | "public_instance_methods"
                    | "private_instance_methods" | "protected_instance_methods"
                    | "constants"
                    | "autoload" | "autoload?" | "const_defined?" | "const_get" | "const_set" | "private_constant" | "public_constant"
                    | "deprecate_constant"
                    | "private_class_method" | "public_class_method"
                    | "module_function"
                    | "singleton_class"
                    // Bridge keeping the bare-call shape (inside a
                    // class body, e.g. `class C; class_eval(...); end`)
                    // working — the no-recv dispatch in dispatch.rs
                    // forwards these names to receiver-form dispatch
                    // when `self` is a Class. Keep this list in lockstep
                    // with the bridge whitelist there.
                    | "class_eval" | "module_eval"
                    // Runtime-dispatch `Module#define_method` arms in
                    // dispatch.rs accept both explicit-receiver and
                    // no_recv shapes on any Class/Module:
                    //   - block form (`define_method(:foo) { … }`)
                    //     handled in `do_call_block` (no_recv reads
                    //     self_val from the current frame).
                    //   - no-block form raises ArgumentError via
                    //     `try_dispatch_class_intrinsics`; the
                    //     no_recv bare-call reaches the recv form
                    //     through the `do_call` bridge (PR #245
                    //     Copilot round 2 #1), so the bridge
                    //     whitelist stays in lockstep with this one.
                    | "define_method"
                    // `define_singleton_method` parallels
                    // `define_method` but installs into the
                    // class's own `singleton_methods` table
                    // (class methods), so `C.respond_to?` must
                    // agree.
                    | "define_singleton_method"
                ) {
                    return true;
                }
                // `Class#allocate` — fence on Modules only.
                // CRuby: `Module.respond_to?(:allocate)` → false,
                // `Module.new.respond_to?(:allocate)` → false,
                // `Integer.respond_to?(:allocate)` → true,
                // `Class.respond_to?(:allocate)` → true (CRuby
                // allows Class.allocate to produce an anonymous
                // Class — rubyrs treats that as a KNOWN GAP but
                // mirrors CRuby's respond_to surface here so
                // feature-detection idioms agree on truthiness).
                // Without this fence the whitelist returned true
                // on Module receivers where dispatch raises
                // TypeError, breaking `m.respond_to?(:allocate)
                // ? m.allocate : …` on module references.
                // PR #181 code-review #2.
                // `Class#superclass` — fence on Modules. CRuby:
                // `M.respond_to?(:superclass)` → false because
                // `M.superclass` raises NoMethodError. Module
                // receivers need to report the same truthiness so
                // feature-detection patterns like
                // `cls.respond_to?(:superclass) && cls.superclass`
                // don't try-and-trip.
                if name == "superclass" && !cls.is_module {
                    return true;
                }
                if name == "allocate"
                    && !cls.is_module
                    && cls.name != "Module"
                {
                    return true;
                }
                // `Kernel.respond_to?(:load)` and the other Kernel
                // module functions. CRuby exposes Kernel's methods as
                // `module_function`s — callable as `Kernel.foo`, so
                // `Kernel.respond_to?(:foo)` is true. The explicit-
                // receiver dispatch routes these through
                // `builtin_call` (see `do_call`); feature detection
                // has to agree. rouge.rb:43 (`Kernel::load ...`) and
                // any `Kernel.respond_to?(:require)` guard depend on
                // this.
                if cls.is_module
                    && cls.name.as_str() == "Kernel"
                    && Self::is_kernel_module_function(name)
                {
                    return true;
                }
                self.lookup_class_singleton_method(cls, name_id)
                    .is_some_and(|m| include_private
                        || m.visibility.get() == crate::value::Visibility::Public)
            },
            Value::Object(id) => {
                // The universal `dup`/`clone` arm in
                // `Vm::do_call` handles plain Value::Object via
                // a shallow Instance copy, so report true even
                // when no user method exists.
                if matches!(name, "dup" | "clone" | "extend" | "define_singleton_method") {
                    return true;
                }
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, name_id)
                    .is_some_and(|m| include_private
                        || m.visibility.get() == crate::value::Visibility::Public)
            }
            Value::Block(_) => matches!(name, "call" | "[]" | "()" | "yield" | "arity" | "curry" | ">>" | "<<"),
            #[cfg(feature = "regex")]
            Value::Regex(_) => matches!(name,
                "match" | "match?" | "===" | "=~" | "source" | "to_s" | "inspect" | "options"
                // `freeze` / `frozen?` are compatibility shims:
                // Regexp is immutable by construction so freezing
                // is a no-op, but real Ruby code calls `.freeze`
                // on regex literals (e.g. `HEADER_PARAM =
                // /.../.freeze` in sinatra/base.rb:32) and
                // `respond_to?(:freeze)` must agree with the
                // primitive arm in vm/string.rs.
                | "freeze" | "frozen?"
            ),
            // `==` / `!=` / `eql?` / `hash` are in the universal
            // whitelist at the top of this fn — don't list them
            // again here. (Keeping `==` historically muddied the
            // story; dropping all four for consistency.)
            Value::BoundMethod(_) => matches!(name, "call" | "[]" | "()" | "unbind" | "bind_call" | "arity" | "parameters" | ">>" | "<<" | "curry" | "to_proc" | "owner" | "receiver" | "name" | "original_name" | "source_location" | "super_method" | "dup" | "clone"),
            Value::UnboundMethod(_) => matches!(name, "bind" | "bind_call" | "arity" | "parameters" | "owner" | "name" | "original_name" | "source_location" | "super_method" | "dup" | "clone"),
            Value::CurriedProc(_) => matches!(name, "call" | "[]" | "()" | "arity"),
        };
        if yes {
            return true;
        }
        // Fallback: a method REOPENED onto a primitive's class
        // (`class Array; def deconstruct; … end`) — or inherited from a
        // module the class `include`s (Enumerable on Array/Hash/Range)
        // — isn't in the hardcoded builtin lists above. Object / Class
        // already consulted their full method tables, so restrict this
        // to the primitive / value arms. Without it
        // `[].respond_to?(:deconstruct)` (and pattern matching, which
        // guards array/hash deconstruction on it) reported false even
        // though `[].deconstruct` dispatches fine.
        if !matches!(recv, Value::Object(_) | Value::Class(_)) {
            let cname: &str = match recv {
                Value::Int(_) => "Integer",
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => "Integer",
                Value::Float(_) => "Float",
                Value::Str(_) => "String",
                Value::Sym(_) => "Symbol",
                Value::Array(_) => "Array",
                Value::Hash(_) => "Hash",
                Value::Range(_) => "Range",
                Value::Bool(true) => "TrueClass",
                Value::Bool(false) => "FalseClass",
                Value::Nil => "NilClass",
                Value::Block(_) | Value::CurriedProc(_) => "Proc",
                #[cfg(feature = "regex")]
                Value::Regex(_) => "Regexp",
                Value::BoundMethod(_) => "Method",
                Value::UnboundMethod(_) => "UnboundMethod",
                Value::Rational(_) => "Rational",
                Value::Object(_) | Value::Class(_) => unreachable!(),
            };
            if let Some(sym) = self.interner.get_id(cname)
                && let Some(cls) = self.classes.get(&sym).cloned()
                && self.lookup_method_uncached(&cls, name_id)
                    .is_some_and(|m| include_private
                        || m.visibility.get() == crate::value::Visibility::Public)
            {
                return true;
            }
        }
        false
    }

    /// `Object#class` — returns the Class associated with a value.
    /// For user-defined instances that's the stored class; for
    /// built-in types we look up the corresponding stub class
    /// (`Integer`, `String`, ...) installed by the preamble. If
    /// the lookup misses (preamble bug or a user evaling
    /// `Integer.class.superclass` games on a stripped runtime),
    /// returns `Value::Nil` rather than panicking.
    pub(crate) fn class_of(&mut self, recv: &Value) -> Value {
        // A Hash-subclass instance carries its real class as a tag;
        // report that (so `obj.class` / `is_a?` see the subclass).
        if let Value::Hash(id) = recv
            && let Some(c) = self.heap.hash_class_tag(*id) {
            return Value::Class(c);
        }
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
            Value::Rational(_) => "Rational",
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

/// `class_is_a` variant that walks ONLY one of the include /
/// prepend chains (and the same chain transitively through each
/// module's own chain, plus through the superclass chain). Used by
/// include/prepend idempotency so `include M; prepend M` on the
/// same target succeeds at both steps — CRuby treats the two
/// chains as distinct insertion slots and the per-chain
/// reachability is what gates each side. `walk_prepend=true` walks
/// the prepend chain; otherwise includes.
///
/// Returns true for `child == target` (and for `current == target`
/// along the superclass walk) — consistency with `class_is_a`.
pub(crate) fn class_reaches_via_chain(
    child: &Rc<Class>,
    target: &Rc<Class>,
    walk_prepend: bool,
) -> bool {
    // Inside `walks_through` we follow BOTH prepend AND include
    // edges, mirroring `class_is_a` — a module's ancestor graph
    // is the union of both chains. The outer loop's
    // `walk_prepend` only selects which top-level chain of
    // `current` we start scanning from; once we're inside a
    // module's body we treat it as a full ancestor-graph node
    // so transitive cross-chain reachability is honored.
    //
    // Without this, `prepend Outer` (where Outer includes Inner)
    // followed by `prepend Inner` would mis-skip CRuby's
    // idempotency rule and insert Inner again — CRuby treats
    // Inner as already-reachable through Outer.includes and
    // makes the second prepend a no-op.
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
    // Self-equality short-circuit, matching `class_is_a`. Without
    // this guard the include/prepend idempotency check would let
    // `module M; include M; end` (or `prepend M`) insert M into
    // its own chain, creating a self-cycle that the next ancestor
    // walk would stack-overflow on (despite each walker's visited
    // set — the first iteration sets up the cycle before visited
    // sees the node).
    if Rc::ptr_eq(child, target) { return true; }
    let mut sc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
    let mut current = child.clone();
    loop {
        if !sc_visited.insert(Rc::as_ptr(&current)) { return false; }
        // `current == target` along the superclass walk also
        // counts as reachable — same consistency rule as
        // `class_is_a` enforces inside its inner walker.
        if Rc::ptr_eq(&current, target) { return true; }
        let mut inc_visited: std::collections::HashSet<*const Class> = std::collections::HashSet::new();
        // Seed visited with `current` so any cyclic include/prepend
        // graph (`A includes B; B includes A`) that walks back into
        // `current` short-circuits instead of re-borrowing
        // `current.includes` / `current.prepends` while it's still
        // borrowed by `chain` below — that would trigger a RefCell
        // borrow panic. `lookup_method_uncached` carries the same
        // defensiveness comment.
        inc_visited.insert(Rc::as_ptr(&current));
        // Clone the chain into a Vec so the RefCell borrow ends
        // before recursion. Otherwise a cyclic graph that walks
        // through a Module which itself borrows the chain panics.
        // The chains are typically small (n=0..4), so the alloc cost
        // is negligible compared to the safety win.
        let chain_snapshot: Vec<Rc<Class>> = if walk_prepend {
            current.prepends.borrow().clone()
        } else {
            current.includes.borrow().clone()
        };
        for m in chain_snapshot.iter() {
            if walks_through(m, target, &mut inc_visited) {
                return true;
            }
        }
        let parent = current.superclass.borrow().clone();
        match parent {
            Some(p) => current = p,
            None => return false,
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
    /// Walk `start`'s ancestor chain looking for a constant named
    /// `bare` (an interned bare SymId like `Text`), returning the
    /// first hit. This is CRuby's step-2 of bare-constant resolution:
    /// after the lexical nesting (`Module.nesting`) is exhausted, the
    /// ancestors of the INNERMOST lexical cref are searched, each
    /// scope's OWN constant table only.
    ///
    /// Order mirrors `flatten_ancestors` — `[prepends…, self,
    /// includes…, superclass + its prepends/includes, …]` — WITH ONE
    /// CRuby-specific twist: the start class's own constant table is
    /// consulted BEFORE its prepended modules. CRuby's `rb_const_get`
    /// checks the class itself first, then walks the (prepend-aware)
    /// super chain, so a constant defined on both the class and a
    /// prepended module resolves to the class's own
    /// (`prepend Pre; X=…on C` → `C::X`, not `Pre::X`), even though
    /// METHOD lookup would pick the prepend. We model that by trying
    /// `start` first, then the full `flatten_ancestors` list (which
    /// re-lists `start` harmlessly after the prepends).
    ///
    /// rubyrs keys module/class constants in the GLOBAL `classes` /
    /// `constants` maps under their qualified name (`M::FOO`), so for
    /// each named ancestor we probe `"<ancestor.name>::<bare>"`.
    /// Anonymous ancestors (empty name, e.g. a `Class.new` module)
    /// keep their constants in the per-class `consts` table, which we
    /// probe directly. Returns `None` if no ancestor defines it (the
    /// caller then falls through to its NameError / nil path).
    pub(crate) fn const_via_ancestors(
        &mut self,
        start: &Rc<Class>,
        bare: SymId,
    ) -> Option<Value> {
        // Own table first (CRuby checks the class before its
        // prepends for constants), then the flattened ancestry.
        let mut chain: Vec<Rc<Class>> = Vec::new();
        chain.push(start.clone());
        chain.extend(super::flatten_ancestors(start));
        let bare_name = self.interner.resolve(bare).to_string();
        for anc in &chain {
            // Anonymous scope: const_set / nested defines land in the
            // per-class `consts` map keyed by the bare SymId.
            if anc.name.is_empty() {
                if let Some(v) = anc.consts.borrow().get(&bare).cloned() {
                    return Some(v);
                }
                continue;
            }
            // Named scope: probe the qualified global key. Only intern
            // when the name already exists so a miss doesn't grow the
            // interner (mirrors `resolve_const_path`'s `contains`
            // guard — keeps `const_defined?` misses cheap).
            let mut qual = String::with_capacity(anc.name.len() + 2 + bare_name.len());
            qual.push_str(&anc.name);
            qual.push_str("::");
            qual.push_str(&bare_name);
            if !self.interner.contains(&qual) {
                continue;
            }
            let qid = self.interner.intern(&qual);
            if let Some(c) = self.classes.get(&qid).cloned() {
                return Some(Value::Class(c));
            }
            if let Some(v) = self.constants.get(&qid).cloned() {
                return Some(v);
            }
        }
        None
    }

    /// Install synthesised `Method` records on the Kernel module
    /// (loaded by preamble/object.rb) so `Kernel.instance_method(:foo)`
    /// reflection — `.arity`, `.parameters`, `.source_location` —
    /// returns real values instead of `proto_idx`-derived defaults.
    ///
    /// The records carry no executable bytecode (`proto_idx = 0`,
    /// never read). Invocation routes back to inline primitive
    /// dispatch via the `builtin` short-circuit in
    /// `invoke_method_with_block`. Bind/call from a captured
    /// UnboundMethod hits the snapshot path and dispatches as if
    /// the script wrote `recv.foo`.
    ///
    /// The set covers the common reflection targets: zero-arg
    /// metadata accessors (`class`, `nil?`, `frozen?`, `to_s`,
    /// `inspect`, `hash`, `object_id`, `itself`), single-arg type
    /// predicates (`is_a?`, `kind_of?`, `instance_of?`), and the
    /// variadic dispatchers (`send`, `respond_to?`).
    /// `equal?`, `__send__`, `instance_exec`, `instance_eval`,
    /// `==`, `!=`, `!`, `__id__` are intentionally absent — CRuby
    /// defines them on BasicObject; see
    /// `install_basic_object_builtins`. Methods NOT in either set
    /// continue through the primitive-sentinel `instance_method`
    /// path with `proto_idx`-default reflection.
    pub(crate) fn install_kernel_builtins(&mut self) {
        let kernel_sym = self.interner.intern("Kernel");
        // Defensive: preamble/object.rb must load before this. If
        // Kernel isn't present, populating the registry doesn't
        // help — `instance_method(:foo)` on Kernel can't resolve
        // anyway. Skip silently.
        if !self.classes.contains_key(&kernel_sym) {
            return;
        }
        // Cache the SymId for O(1) class lookup in
        // `kernel_builtin_method` later. The interner doesn't
        // shift SymIds post-install, so this stays stable for
        // the lifetime of the Vm.
        self.kernel_class_sym = Some(kernel_sym);
        // (name, arity, params, source_label)
        //
        // Arity follows CRuby's `Method#arity` encoding:
        //   N≥0 = exactly N required positional
        //   -(N+1) = at least N required, rest accepted
        //
        // Parameter names are deliberately None — CRuby's
        // C-implemented methods don't expose source-level names,
        // so `.parameters` reports `[[:req]]` / `[[:rest]]` with
        // no symbol. Mirroring that gives byte-for-byte parity.
        //
        // `instance_exec` is NOT in this set: CRuby defines it on
        // BasicObject, not Kernel. `Kernel.instance_method(:instance_exec)`
        // raises NameError; the registered version lives in
        // `install_basic_object_builtins` below.
        // Per-entry shape: (method-name, arity, parameters,
        // source_label). The parameters slice mirrors
        // `BuiltinMeta.parameters` — each element is (kind, name)
        // where kind is "req"/"opt"/"rest"/"keyrest"/"block" and
        // name is `Some(...)` for a Ruby-source-visible name or
        // `None` to surface as anonymous in `Method#parameters`
        // (CRuby's C-defined methods report anonymous names).
        // Aliased so clippy's type_complexity lint doesn't trip
        // (the inline 4-tuple-with-nested-slice form was reading as
        // dense without saving any ergonomics).
        //
        // `&'static str` everywhere: makes the no-allocation /
        // no-leak guarantee explicit at the type level so a
        // future edit can't accidentally introduce a non-static
        // label that BuiltinMeta would have to leak to store.
        type KernelEntry =
            (&'static str, i64, &'static [(&'static str, Option<&'static str>)], &'static str);
        let entries: &[KernelEntry] = &[
            // Zero-arg metadata accessors
            ("class", 0, &[], "<internal:kernel>"),
            ("nil?", 0, &[], "<internal:kernel>"),
            ("frozen?", 0, &[], "<internal:kernel>"),
            ("to_s", 0, &[], "<internal:kernel>"),
            ("inspect", 0, &[], "<internal:kernel>"),
            ("hash", 0, &[], "<internal:kernel>"),
            ("object_id", 0, &[], "<internal:kernel>"),
            ("itself", 0, &[], "<internal:kernel>"),
            // Single-arg type predicates (required positional,
            // anonymous in CRuby's C-defined parameter list).
            // `equal?` and `__send__` are intentionally NOT here —
            // CRuby defines them on BasicObject (not Kernel); see
            // the BasicObject registry in `install_basic_object_builtins`.
            ("is_a?", 1, &[("req", None)], "<internal:kernel>"),
            ("kind_of?", 1, &[("req", None)], "<internal:kernel>"),
            ("instance_of?", 1, &[("req", None)], "<internal:kernel>"),
            // Variadic dispatchers (CRuby: arity -1, params [[:rest]])
            ("send", -1, &[("rest", None)], "<internal:kernel>"),
            ("respond_to?", -1, &[("rest", None)], "<internal:kernel>"),
        ];
        for (name, arity, params, src_label) in entries {
            let name_id = self.interner.intern(name);
            let parameters: Vec<(&'static str, Option<String>)> = params
                .iter()
                .map(|(k, n)| (*k, n.map(|s| s.to_string())))
                .collect();
            let meta = std::rc::Rc::new(BuiltinMeta {
                name_id,
                arity: *arity,
                parameters,
                // `src_label` is already a `&'static str` (string
                // literal in the entries table); store directly
                // rather than allocating + leaking. The leak in
                // the prior version was a harmless drop-in until
                // someone added a non-static label.
                source_label: Some(src_label),
                source_line: 0,
            });
            self.kernel_builtin_metas.insert(name_id, meta);
        }
    }

    /// BasicObject builtins — installed alongside Kernel's
    /// (preamble/object.rb runs before this). The set mirrors
    /// CRuby's `BasicObject.instance_methods(false)`:
    ///   - `__id__` (alias of object_id)
    ///   - `__send__` (public-receiver-only send variant —
    ///     reserved name CRuby guarantees user code can't shadow)
    ///   - `equal?` (identity comparison)
    ///   - `instance_eval` / `instance_exec` (self-swap evaluators)
    ///   - `==` / `!=` / `!` (universal operators)
    ///
    /// Same off-chain design as Kernel — stored in
    /// `Vm.basic_object_builtin_metas`, consulted by the
    /// `instance_method` arm when receiver is BasicObject.
    pub(crate) fn install_basic_object_builtins(&mut self) {
        let bo_sym = self.interner.intern("BasicObject");
        if !self.classes.contains_key(&bo_sym) {
            return;
        }
        self.basic_object_class_sym = Some(bo_sym);
        // BasicObject methods report `source_location: nil` in
        // CRuby (verified: `BasicObject.instance_method(:__id__)
        // .source_location` returns nil, unlike Kernel which
        // returns `["<internal:kernel>", N]`). Mirror by passing
        // `None` for source_label.
        // `&'static str` everywhere + type alias — same shape as
        // install_kernel_builtins, minus the source-label slot
        // (BasicObject methods uniformly report `nil` for
        // source_location). Alias keeps clippy's type_complexity
        // lint quiet for symmetry with KernelEntry.
        type BasicObjectEntry =
            (&'static str, i64, &'static [(&'static str, Option<&'static str>)]);
        let entries: &[BasicObjectEntry] = &[
            ("__id__", 0, &[]),
            ("!", 0, &[]),
            ("equal?", 1, &[("req", None)]),
            ("==", 1, &[("req", None)]),
            ("!=", 1, &[("req", None)]),
            ("__send__", -1, &[("rest", None)]),
            ("instance_eval", -1, &[("rest", None)]),
            ("instance_exec", -1, &[("rest", None)]),
        ];
        for (name, arity, params) in entries {
            let name_id = self.interner.intern(name);
            let parameters: Vec<(&'static str, Option<String>)> = params
                .iter()
                .map(|(k, n)| (*k, n.map(|s| s.to_string())))
                .collect();
            let meta = std::rc::Rc::new(BuiltinMeta {
                name_id,
                arity: *arity,
                parameters,
                source_label: None,
                source_line: 0,
            });
            self.basic_object_builtin_metas.insert(name_id, meta);
        }
    }

    /// Materialise the synth Method for a Kernel builtin (or None
    /// if `name_id` isn't a registered builtin). Used by the
    /// `Kernel.instance_method(:foo)` arm to wrap a UnboundMethod
    /// snapshot without inserting on Kernel's actual methods
    /// table.
    pub(crate) fn kernel_builtin_method(&self, name_id: SymId) -> Option<Rc<Method>> {
        let meta = self.kernel_builtin_metas.get(&name_id)?.clone();
        let cls = self.classes.get(&self.kernel_class_sym?)?.clone();
        Some(Self::materialise_builtin_method(meta, &cls))
    }

    /// Same as `kernel_builtin_method` but for the BasicObject
    /// registry. Looked up by the `instance_method` arm when the
    /// receiver is BasicObject.
    pub(crate) fn basic_object_builtin_method(&self, name_id: SymId) -> Option<Rc<Method>> {
        let meta = self.basic_object_builtin_metas.get(&name_id)?.clone();
        let cls = self.classes.get(&self.basic_object_class_sym?)?.clone();
        Some(Self::materialise_builtin_method(meta, &cls))
    }

    /// Walk `cls`'s ancestor chain looking for a registered
    /// Kernel or BasicObject builtin matching `name_id`. Used by
    /// `instance_method` when the live method table misses: a
    /// user class that inherits Kernel via include (i.e. any
    /// `class User; end` since PR #256) should surface `#class`,
    /// `#nil?`, etc. through reflection just like
    /// `Kernel.instance_method(:class)` does directly. Without
    /// this walk, only the direct Kernel/BasicObject receivers
    /// would hit the registry.
    ///
    /// Returns None if `cls` doesn't transitively include Kernel
    /// AND doesn't transitively inherit from BasicObject — i.e.
    /// the unusual case of a class whose chain bypasses both
    /// roots (BasicObject subclasses opt out of Kernel; that's
    /// the only way to lose both).
    pub(crate) fn builtin_method_via_ancestor_chain(
        &self,
        cls: &Rc<Class>,
        name_id: SymId,
    ) -> Option<Rc<Method>> {
        // Kernel first — most-common: every Object descendant
        // includes Kernel transitively, so this branch handles
        // the vast majority of user classes.
        if let Some(ksym) = self.kernel_class_sym
            && let Some(kernel) = self.classes.get(&ksym)
            && class_is_a(cls, kernel)
            && let Some(m) = self.kernel_builtin_method(name_id)
        {
            return Some(m);
        }
        // BasicObject — root for everything that inherits Object,
        // also for opt-out classes (`class X < BasicObject; end`)
        // that skip Kernel entirely.
        if let Some(bsym) = self.basic_object_class_sym
            && let Some(bo) = self.classes.get(&bsym)
            && class_is_a(cls, bo)
            && let Some(m) = self.basic_object_builtin_method(name_id)
        {
            return Some(m);
        }
        None
    }

    /// Shared materialisation for both Kernel and BasicObject
    /// builtin Method records. Synthesises a Method with the meta
    /// as the introspection payload, a placeholder `proto_idx = 0`
    /// (never read because invoke_method_with_block short-
    /// circuits on `builtin.is_some()`), and FixedArity sized
    /// to match the meta's anonymous-or-named params. The
    /// anonymous-param `arg{i}` placeholder names exist as a
    /// belt-and-braces guard against the fixed-arity fast path
    /// indexing into a too-small locals vector — the builtin
    /// short-circuit should always bypass that fast path, but
    /// the structural mismatch was a real footgun in the cycle-1
    /// review.
    fn materialise_builtin_method(meta: std::rc::Rc<BuiltinMeta>, cls: &Rc<Class>) -> Rc<Method> {
        let params_strings: Vec<String> = meta
            .parameters
            .iter()
            .enumerate()
            .map(|(i, (_, n))| n.clone().unwrap_or_else(|| format!("arg{}", i)))
            .collect();
        let fixed_arity = if meta.arity >= 0 {
            Some(crate::value::FixedArity {
                required: meta.arity as u16,
                n_locals: params_strings.len() as u16,
            })
        } else {
            None
        };
        Rc::new(Method {
            params: params_strings,
            proto_idx: 0,
            fixed_arity,
            defining_class: Some(Rc::downgrade(cls)),
            visibility: std::cell::Cell::new(crate::value::Visibility::Public),
            closure: None,
            original_name: Some(meta.name_id),
            builtin: Some(meta),
        })
    }

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
        // CRuby allows `super` inside a block — it forwards to the
        // enclosing METHOD's super-chain. The block frames in between
        // inherit their method_name from the enclosing method proto
        // (set in compile_block via parent.method_name.clone), so the
        // name_id passed in is already correct for the method's
        // dispatch; only the defining_class lookup needs to find the
        // method.
        //
        // Resolve that method LEXICALLY — the same
        // `find_lexical_owner_frame` walk yield / block_given? use —
        // NOT by the call-stack-nearest frame carrying a
        // defining_class. They diverge when the block is invoked
        // through a user method on ANOTHER object, e.g. liquid's
        // `context.stack { collection.each { result << super } }`:
        // the nearest defining_class is `Context#stack`'s, which
        // isn't in self's (the TableRow's) ancestry, so the
        // post-defining ancestor walk can't find where to resume and
        // raises a spurious "no superclass method `render'". The
        // lexical owner is the method that textually contains the
        // `super` (`TableRow#render`), whose defining_class IS in
        // self's chain. For `super` directly in a method body the
        // walk returns that method frame itself (non-block,
        // ptr_eq on its own locals). The `.or_else` fallback covers
        // a block that escaped its lexical owner (stored as a Proc,
        // owner already returned): no live owner frame, so use the
        // nearest defining_class as before.
        let lexical_defining = self.frames.last()
            .map(|f| f.locals.clone())
            .and_then(|seed| self.find_lexical_owner_frame(&seed))
            .and_then(|idx| self.frames[idx].defining_class.clone());
        let defining = match lexical_defining
            .or_else(|| self.frames.iter().rev().find_map(|f| f.defining_class.clone()))
        {
            Some(c) => c,
            None => {
                return Err(self.trap(crate::error::RubyError::NoMethodError {
                    kind: crate::error::NoMethodErrorKind::Missing,
                    method: "super called outside of method".to_string(),
                    recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
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
                // Modules extended into this class's singleton sit
                // BETWEEN the class itself and its superclass in the
                // metaclass ancestor walk — same position as where
                // `lookup_class_singleton_method` consults
                // singleton_includes. Without this, `super` from
                // within an extended module's method skips straight
                // to the superclass and either misses the inherited
                // class method or finds an unrelated method,
                // breaking the `def get(...); super; end` shape
                // sinatra-contrib/MultiRoute uses.
                for inc in cur.singleton_includes.borrow().iter() {
                    flatten_prepended_module(inc, &mut chain, &mut inc_visited);
                }
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
                    kind: crate::error::NoMethodErrorKind::SuperNoSuperclass,
                    method: self.interner.resolve(name_id).to_string(),
                    recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
                })),
            };
        }
        let recv_cls = match &self_val {
            Value::Object(id) => self.heap.class_of(*id),
            other => match self.class_of(other) {
                Value::Class(c) => c,
                _ => {
                    return Err(self.trap(crate::error::RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::SuperNoSuperclass,
                        method: self.interner.resolve(name_id).to_string(),
                        recv_type: std::borrow::Cow::Borrowed(other.type_name()),
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
                kind: crate::error::NoMethodErrorKind::SuperNoSuperclass,
                method: self.interner.resolve(name_id).to_string(),
                recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
            })),
        }
    }

    /// `super` dispatch wrapper for Op::Super / Op::ApplySuper
    /// that intercepts the "no superclass method" failure for
    /// CRuby's lifecycle hooks (`inherited`, `included`,
    /// `extended`) and substitutes a no-op (push Nil).
    ///
    /// Why this exists: CRuby ships real no-op
    /// implementations of these hooks on `Class` / `Module`,
    /// so an overriding hook body can call `super` without
    /// worrying about whether anything's above it. rubyrs's
    /// hook-firing path (step.rs Op::DefClass) dispatches
    /// directly via `lookup_class_singleton_method` rather
    /// than installing real methods on Class/Module, so the
    /// super chain walks past Sinatra::Base → Object →
    /// BasicObject without finding `inherited` — even though
    /// CRuby's would resolve to a no-op terminator.
    ///
    /// Discovered: TRY_RUNS pass-15 — sinatra-4's
    /// `Sinatra::Base.inherited(subclass)` calls \`super\` to
    /// invoke the (no-op) default. Without this intercept the
    /// inherited hook firing during base.rb's own load (when
    /// `Sinatra::Application < Sinatra::Base` is defined)
    /// raises NoMethodError. (Layer #20.)
    /// `super` to the builtin `Class#new` — allocate a default
    /// instance of `cls` and run its `initialize` (if any),
    /// yielding the new object as the super expression's value.
    /// Used when an override `def self.new; ...; super; end`
    /// resolves super to the builtin allocator (CRuby ships a
    /// real `Class#new`; rubyrs handles it inline, so the super
    /// chain finds no user method and we substitute the builtin
    /// here).
    ///
    /// Runs `initialize` synchronously via `dispatch_until` (the
    /// same reentrant pattern `invoke_inherited_hook` uses) so we
    /// can discard initialize's return and push the OBJECT — `new`
    /// yields the instance, not whatever `initialize` returned.
    /// The fresh object is pinned across the initialize call so a
    /// GC triggered inside it can't sweep the not-yet-rooted
    /// instance.
    pub(crate) fn super_builtin_class_new(
        &mut self,
        cls: &Rc<crate::value::Class>,
        args: Vec<Value>,
    ) -> Result<(), crate::error::Trap> {
        let obj = self.alloc_default_instance(cls)?;
        self.pinned.push(obj.clone());
        let init_id = self.interner.intern("initialize");
        let ruby_init = self.lookup_method_uncached(cls, init_id);
        if let Some(m) = ruby_init {
            let pre_frames = self.frames.len();
            if let Err(t) = self.invoke_method(m, obj.clone(), args) {
                self.pinned.pop();
                return Err(t);
            }
            if let Err(t) = self.dispatch_until(pre_frames) {
                self.pinned.pop();
                return Err(t);
            }
            // initialize's return value is discarded by `new`.
            self.stack.pop();
        }
        self.pinned.pop();
        self.stack.push(obj);
        Ok(())
    }

    pub(crate) fn super_call_with_lifecycle_noop(
        &mut self,
        name_id: SymId,
        args: Vec<Value>,
    ) -> Result<(), crate::error::Trap> {
        match self.super_lookup(name_id) {
            Ok((m, self_val)) => self.invoke_method(m, self_val, args),
            Err(trap) => {
                // `super` to a builtin Class / BasicObject method
                // that rubyrs handles inline (so the ancestor walk
                // finds no user Method above the override). CRuby
                // ships real `Class#new` / `Class#allocate` /
                // `BasicObject#initialize`, so an overriding
                // `def self.new` / `def initialize` can call super.
                // Mustermann's `def self.new(...); ...; super(...)
                // { ... } end` (pattern.rb) and Sinatra::Templates'
                // `def initialize; ...; super; end` both depend on
                // this. Gate on the same typed `SuperNoSuperclass`
                // miss the lifecycle-hook intercept below uses.
                if matches!(
                    &trap.err,
                    crate::error::RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::SuperNoSuperclass,
                        ..
                    },
                ) {
                    let cur_self = self.frames.last().map(|f| f.self_val.clone());
                    let nm = self.interner.resolve(name_id).to_string();
                    match (nm.as_str(), cur_self) {
                        ("new", Some(Value::Class(cls))) => {
                            return self.super_builtin_class_new(&cls, args);
                        }
                        ("allocate", Some(Value::Class(cls))) => {
                            let obj = self.alloc_default_instance(&cls)?;
                            self.stack.push(obj);
                            return Ok(());
                        }
                        ("initialize", Some(Value::Object(_))) => {
                            // BasicObject#initialize is a no-op
                            // returning nil. (CRuby raises
                            // ArgumentError if the default
                            // initialize is handed args; the common
                            // `super()` forwarding shape passes
                            // none, so we accept any and no-op —
                            // documented spike divergence.)
                            self.stack.push(Value::Nil);
                            return Ok(());
                        }
                        // `super` from a method defined on a Hash
                        // subclass → the Hash PRIMITIVE of the same
                        // name (`class M < Hash; def [](k); super(
                        // k.downcase); end`). The ancestor walk finds
                        // no user Method above the override because
                        // Hash's methods are inline primitives; route
                        // to `collection_call`. `initialize` has no
                        // collection_call arm (Hash#initialize is a
                        // no-op), so it falls through to nil.
                        (_, Some(Value::Hash(id))) if self.heap.hash_class_tag(id).is_some() => {
                            let recv = Value::Hash(id);
                            if let Some(v) = self.collection_call(&recv, &nm, &args)? {
                                self.stack.push(v);
                                return Ok(());
                            }
                            if nm == "initialize" {
                                self.stack.push(Value::Nil);
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                }
                // Only intercept the specific "no superclass
                // method on the ancestor chain" shape — NOT the
                // sibling "super called outside of method"
                // case (which also raises NoMethodError but for
                // a fundamentally broken call site that shouldn't
                // silently succeed). super_lookup tags its
                // ancestor-chain miss with the typed
                // `SuperNoSuperclass` kind so the discrimination
                // is compile-checked rather than coupled to the
                // formatted message string. (Code-review #363
                // round 1 introduced the gate; round 3 swapped
                // the brittle prefix match for the typed tag.)
                let is_no_super = matches!(
                    &trap.err,
                    crate::error::RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::SuperNoSuperclass,
                        ..
                    },
                );
                let resolved = self.interner.resolve(name_id);
                let is_lifecycle_hook = matches!(
                    &**resolved,
                    "inherited" | "included" | "prepended" | "extended"
                        | "method_added" | "singleton_method_added"
                        | "method_removed" | "singleton_method_removed"
                        | "method_undefined" | "singleton_method_undefined",
                );
                // Restrict the no-op to Class/Module singleton-hook
                // contexts only. If `self` is anything else (e.g. an
                // ordinary user object whose author happened to name
                // an instance method `included`), preserve CRuby
                // semantics and propagate the NoMethodError. Modules
                // are also represented by `Value::Class` (with an
                // `is_module` flag), so a single arm covers both.
                // Code-review #363 round 2.
                let on_class_or_module = self
                    .frames
                    .last()
                    .map(|f| matches!(f.self_val, Value::Class(_)))
                    .unwrap_or(false);
                if is_no_super && is_lifecycle_hook && on_class_or_module {
                    self.stack.push(Value::Nil);
                    Ok(())
                } else {
                    Err(trap)
                }
            }
        }
    }
}

/// `Symbol#to_s` / `to_sym` need the Interner to resolve the underlying name,
/// so they live as a method on Vm rather than in the pure `primitive_call`.
impl Vm {
    pub(crate) fn sym_primitive(&mut self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        Ok(match (recv, name, args) {
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
            // Symbol#inspect — `:name` for symbols whose name is a
            // bare identifier / operator, else the quoted `:"..."`
            // form with string-style escaping (CRuby:
            // `:"with space"`, `:""`, `:"1abc"`). Discovery: P3
            // Jekyll spike surfaced `:"".inspect` / spaced symbols
            // diverging from `p`.
            (Value::Sym(id), "inspect", []) => {
                Some(Value::new_str(crate::heap::symbol_inspect(self.interner.resolve(*id))))
            }
            (Value::Sym(id), "to_sym", []) => Some(Value::Sym(*id)),
            // Symbol#empty? / #length / #size operate on the
            // underlying name (`:"".empty?` → true, `:abc.length`
            // → 3). Length counts characters, not bytes, matching
            // CRuby's Symbol#length. forwardable-extended's
            // `def_modern_delegator` calls `accessor.empty?`.
            (Value::Sym(id), "empty?", []) => {
                Some(Value::Bool(self.interner.resolve(*id).is_empty()))
            }
            (Value::Sym(id), "length", []) | (Value::Sym(id), "size", []) => {
                Some(Value::Int(self.interner.resolve(*id).chars().count() as i64))
            }
            // Symbol#upcase / #downcase / #capitalize / #swapcase
            // case-transform the name and re-intern, returning a
            // Symbol (CRuby: `:abc.upcase` → :ABC). Gated on
            // `max_symbols` like #succ — a transform can mint a new
            // name. ASCII-simple casing matches the String builtins.
            (Value::Sym(id), op @ ("upcase" | "downcase" | "capitalize" | "swapcase"), []) => {
                let src = self.interner.resolve(*id);
                let transformed = match op {
                    "upcase" => src.to_uppercase(),
                    "downcase" => src.to_lowercase(),
                    "swapcase" => src.chars().map(|c| {
                        if c.is_uppercase() { c.to_lowercase().next().unwrap_or(c) }
                        else if c.is_lowercase() { c.to_uppercase().next().unwrap_or(c) }
                        else { c }
                    }).collect(),
                    _ /* capitalize */ => {
                        let mut cs = src.chars();
                        match cs.next() {
                            Some(f) => f.to_uppercase().chain(cs.flat_map(|c| c.to_lowercase())).collect(),
                            None => String::new(),
                        }
                    }
                };
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&transformed) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Some(Value::Sym(self.interner.intern(&transformed)))
            }
            // Symbol#succ / Symbol#next — alphanumeric successor of
            // the underlying name, then re-interned. Matches
            // `String#succ` semantics; CRuby treats Symbol#succ as
            // `:"#{to_s.succ}".to_sym`. Used by spec idiom
            // `transform_keys(&:succ)`.
            //
            // ResourceCap: gate the intern on `Config::max_symbols`
            // the same way `String#to_sym` does (`vm/string.rs`'s
            // `to_sym` arm). Without this, a tight loop like
            // `sym = :a; loop { sym = sym.succ }` grows the
            // interner unbounded, bypassing the cap that
            // String→Symbol coercion enforces. Existing names re-
            // resolve and don't count toward the cap.
            (Value::Sym(id), "succ", []) | (Value::Sym(id), "next", []) => {
                let next_name = crate::vm::string::str_succ(self.interner.resolve(*id));
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&next_name) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Some(Value::Sym(self.interner.intern(&next_name)))
            }
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
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
            builtin: None,
            original_name: None,
        })
    }

    fn mk_class(name: &str, superclass: Option<Rc<Class>>) -> Rc<Class> {
        Rc::new(Class {
            name: name.to_string(),
            is_module: false,
            ivars: RefCell::new(crate::intern::FxHashMap::default()),
            methods: RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            singleton_includes: RefCell::new(Vec::new()),
            singleton_view: RefCell::new(None),
            singleton_target: RefCell::new(None),
            superclass: RefCell::new(superclass),
            class_vars: RefCell::new(crate::intern::FxHashMap::default()),
            consts: RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: RefCell::new(None),
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
