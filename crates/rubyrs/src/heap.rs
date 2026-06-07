use std::rc::Rc;

use crate::intern::Interner;
use crate::value::{BlockHandle, Class, Instance, ObjId, Value};

// ---------- GC Heap ----------

pub(crate) enum HeapObj {
    Instance(Instance),
    Array(Vec<Value>),
    Hash(HashObj),
    Range(RangeObj),
    /// Arbitrary-precision integer (the heap-side of `Value::BigInt`).
    /// Contains no nested `Value` — GC walk is a no-op for this
    /// variant. Cfg-gated on the `bignum` feature alongside
    /// `Value::BigInt`. ADR 0018 BigInt placement.
    #[cfg(feature = "bignum")]
    BigInt(num_bigint::BigInt),
    /// Heap-side of `Value::Rational`. Always in lowest terms with
    /// `den > 0` — invariants enforced by `Kernel#Rational(n, d)`'s
    /// gcd-normalize + sign-normalize at construction. Contains no
    /// nested `Value`, so GC walk is a no-op. Phase C.1 stores
    /// `i64` num/den only; BigInt num/den is a Phase C.4 follow-up.
    Rational(RationalRepr),
    /// A `proc { ... }` value. Lives in the heap (P2-13) so blocks
    /// participate in mark-sweep — earlier `Rc<BlockHandle>` form
    /// cycled whenever a block's `captured` held the block itself.
    Block(BlockHandle),
    /// Spike L3-B: a C extension's TypedData object — wraps an
    /// arbitrary C `void*` plus a CRuby-shape `rb_data_type_t`
    /// descriptor that tells the GC how to free the native state.
    /// Used by gems like sqlite3-ruby, redis-client, openssl to
    /// hold a long-lived native resource (DB handle, socket FD,
    /// SSL context) that the Ruby script-level object owns.
    ///
    /// When the slot is swept, [`Heap::collect`] invokes `dfree`
    /// on `data_ptr` so the C side can release the resource. Mark-
    /// phase support for Ruby objects HELD INSIDE the C struct
    /// (`dmark`) is L3-B.1 follow-up — most real-world wrappers
    /// only hold native data, so the wedge defers it.
    ///
    /// `class` is kept so future `obj.class` / `is_a?` checks
    /// resolve to the right user-facing class without needing a
    /// separate per-instance class slot.
    ///
    /// `cfg_attr` on (wasi OR cext-off): the variant is allocated
    /// only through the cext bridge, which is itself gated on
    /// `cfg(all(feature = "cext", not(target_os = "wasi")))`. In
    /// either of the suppressed configurations nothing constructs
    /// the variant, so `-D warnings` would flag it as dead. The
    /// allow is narrowed to those two configurations (rather than
    /// unconditional) so native+cext builds still catch accidental
    /// loss of callers — original spirit: review #1 on PR #22;
    /// `not(feature = "cext")` arm added by PR #75 review #1.
    #[cfg_attr(any(target_os = "wasi", not(feature = "cext")), allow(dead_code))]
    TypedData(TypedDataObj),
    /// `Object#method(:name)` result. `recv` is any Value the GC
    /// must walk; `name_id` is the captured method name. `.call`
    /// dispatches the captured method on the captured receiver.
    ///
    /// `method` mirrors `UnboundMethod.method` — an optional
    /// snapshot of the resolved `Method` captured at the time
    /// the BoundMethod was constructed. Used so `bm.call` works
    /// after a subsequent `remove_method` on the captured class
    /// (CRuby parity: capture-then-remove-then-call returns the
    /// original body). The snapshot is also propagated through
    /// `unbind`/`bind` round-trips: `ubm.bind(x).call` keeps the
    /// snapshot inherited from the UnboundMethod. When None,
    /// `.call` falls back to live chain lookup on the receiver.
    BoundMethod {
        recv: Value,
        name_id: crate::intern::SymId,
        method: Option<std::rc::Rc<crate::value::Method>>,
    },
    /// `Method#unbind` result. `class` is the receiver's class
    /// at unbind time; `bind(obj)` checks `obj.is_a?(class)`
    /// before reconstituting a BoundMethod. `Rc<Class>` is not
    /// a heap reference, so this variant carries no GC
    /// obligation.
    ///
    /// `method` is an optional snapshot of the resolved `Method`
    /// captured AT THE TIME the UnboundMethod was constructed.
    /// It exists so `bind` / `bind_call` survive a subsequent
    /// `remove_method` that strips the entry from the captured
    /// class's methods table between capture and call. Tilt-2.7.0
    /// uses exactly this pattern in `compile_template_method`
    /// (lib/tilt/template.rb:489-490): `instance_method(name)`
    /// to capture, then `remove_method(name)` to clean up,
    /// then `bind_call` on the captured handle to invoke. When
    /// the snapshot is present, bind/bind_call use it directly
    /// (no class-chain re-lookup); otherwise they fall back to
    /// the live `lookup_method_uncached(class, name_id)` path.
    UnboundMethod {
        class: std::rc::Rc<crate::value::Class>,
        name_id: crate::intern::SymId,
        method: Option<std::rc::Rc<crate::value::Method>>,
    },
    /// `Method#curry` / `Proc#curry` partial-application state.
    /// `underlying` is the callable (BoundMethod or Block) being
    /// curried; `gathered` are args accumulated so far; once
    /// `gathered.len() >= target_arity` the underlying is invoked.
    /// All three fields walked by GC — `underlying` and each
    /// gathered Value can hold heap references.
    CurriedProc { underlying: Value, gathered: Vec<Value>, target_arity: u16 },
    /// P1c (ADR 0023) — heap home for a Tier-2 Fiber. Wraps a
    /// [`crate::vm::fiber::FiberObject`]: the body block, the
    /// suspended snapshot, lifecycle state, and last-yielded
    /// value. Cfg-gated on `_fiber`; absent in Tier 1 builds.
    ///
    /// GC walk: the snapshot's `frames` (locals + self_val +
    /// swap_return + block_arg), `stack`, `pinned`,
    /// `method_return`, plus the FiberObject's own
    /// `last_value` and `body_block`. Anything heap-bearing
    /// inside the suspended Fiber must be reached this way —
    /// the regular root walker (vm.stack / vm.frames) only sees
    /// the actively-resumed Fiber's state, never the suspended
    /// ones' snapshots.
    #[cfg(feature = "_fiber")]
    Fiber(crate::vm::fiber::FiberObject),
}

/// Heap representation of a CRuby-shape TypedData object. See
/// [`HeapObj::TypedData`] for the design context.
//
// cfg_attr on (wasi OR cext-off): fields ARE read on native+cext
// (`class` by `class_of`, `type_ptr` by the rb_check_typeddata
// callback). In either suppressed configuration the whole
// TypedData path is stubbed so nothing constructs or reads the
// struct; the conditional allow keeps those builds green without
// silencing real dead-field warnings on the live path (original:
// review #1 on PR #22; cext-off arm: PR #75 review #2).
#[cfg_attr(any(target_os = "wasi", not(feature = "cext")), allow(dead_code))]
pub(crate) struct TypedDataObj {
    pub(crate) class: Rc<crate::value::Class>,
    /// Owned C pointer. Treated as opaque by the host.
    pub(crate) data_ptr: *mut std::ffi::c_void,
    /// Optional descriptor pointer — currently used only by
    /// `rb_check_typeddata` to identity-compare against the type
    /// the C extension expected. CRuby checks the pointer for
    /// identity, not the contents, which we mirror.
    pub(crate) type_ptr: *const std::ffi::c_void,
    /// Optional free function. Invoked from the sweep phase on
    /// `data_ptr` when the wrapping slot is collected. Wrapped
    /// in `Option` because some types are statically allocated
    /// and don't need cleanup.
    pub(crate) dfree: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
}

/// Heap representation of a Rational number.
/// Canonical form: `den > 0`, `gcd(|num|, den) == 1`. The
/// invariants are enforced by `Kernel#Rational(n, d)` at the
/// constructor boundary so every reader (numerator / denominator /
/// to_s / inspect) can trust them without re-normalizing.
///
/// Storage is cfg-dual to keep the no-bignum tier (WASM CI gate)
/// alive: under `bignum`, num/den are arbitrary-precision BigInt
/// (Phase C.4.1 widening — lifts the i64::MIN / 2**64-receiver
/// limits documented in PR #310). Without `bignum`, num/den stay
/// i64 and arithmetic overflow surfaces as RangeError, matching
/// Phase C.1–C.3 behavior. `Display` works identically on both
/// since both types implement `fmt::Display`.
///
/// `Copy` is only derivable on the i64 form (BigInt isn't `Copy`).
/// The 2 historical `*self.heap.rational(*id)` deref-copy sites
/// now use `.clone()` instead.
#[cfg(feature = "bignum")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RationalRepr {
    pub(crate) num: num_bigint::BigInt,
    pub(crate) den: num_bigint::BigInt,
}

#[cfg(not(feature = "bignum"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RationalRepr {
    pub(crate) num: i64,
    pub(crate) den: i64,
}

/// `r == n` for a canonical Rational and i64. Cross-multiplies
/// without intermediate overflow: i128 widening covers the no-
/// bignum tier (max i64 × i64 = i128); BigInt path is infallible.
#[cfg(feature = "bignum")]
pub(crate) fn rational_eq_int(r: &RationalRepr, n: i64) -> bool {
    use num_bigint::BigInt;
    BigInt::from(n) * &r.den == r.num
}
#[cfg(not(feature = "bignum"))]
pub(crate) fn rational_eq_int(r: &RationalRepr, n: i64) -> bool {
    (n as i128) * (r.den as i128) == r.num as i128
}

/// Lossy f64 demote for a canonical Rational. Used by Float
/// cross-type equality / comparison and by `Rational#to_f`.
/// Under bignum the BigInt division goes through f64 via
/// `bigint_to_f64_sign_preserving`; under no-bignum it's the
/// straight `num/den` cast.
#[cfg(feature = "bignum")]
pub(crate) fn rational_to_f64(r: &RationalRepr) -> f64 {
    crate::vm::bignum::bigint_to_f64_sign_preserving(&r.num)
        / crate::vm::bignum::bigint_to_f64_sign_preserving(&r.den)
}
#[cfg(not(feature = "bignum"))]
pub(crate) fn rational_to_f64(r: &RationalRepr) -> f64 {
    r.num as f64 / r.den as f64
}

/// `r <=> other` cross-multiply. Returns `None` for non-numeric
/// `other` (caller surfaces as `Value::Nil`). Canonical `den > 0`
/// on both sides, so cross-multiply preserves sign. Bigint path
/// uses BigInt::cmp directly; no-bignum path uses i128 widening.
pub(crate) fn rational_cmp_other(
    r: &RationalRepr,
    other: &Value,
    heap: &Heap,
) -> Option<std::cmp::Ordering> {
    #[cfg(feature = "bignum")]
    {
        use num_bigint::BigInt;
        match other {
            Value::Rational(oid) => {
                let o = heap.rational(*oid);
                let lhs = &r.num * &o.den;
                let rhs = &o.num * &r.den;
                Some(lhs.cmp(&rhs))
            }
            Value::Int(n) => {
                let rhs = BigInt::from(*n) * &r.den;
                Some(r.num.cmp(&rhs))
            }
            Value::BigInt(id) => {
                let rhs = heap.bigint(*id) * &r.den;
                Some(r.num.cmp(&rhs))
            }
            Value::Float(f) => rational_to_f64(r).partial_cmp(f),
            _ => None,
        }
    }
    #[cfg(not(feature = "bignum"))]
    {
        match other {
            Value::Rational(oid) => {
                let o = heap.rational(*oid);
                let lhs = (r.num as i128) * (o.den as i128);
                let rhs = (o.num as i128) * (r.den as i128);
                Some(lhs.cmp(&rhs))
            }
            Value::Int(n) => {
                let lhs = r.num as i128;
                let rhs = (*n as i128) * (r.den as i128);
                Some(lhs.cmp(&rhs))
            }
            Value::Float(f) => rational_to_f64(r).partial_cmp(f),
            _ => None,
        }
    }
}

/// A Ruby Range. For our subset, both endpoints must be `Value::Int`.
#[derive(Clone)]
pub(crate) struct RangeObj {
    pub(crate) begin: Value,
    pub(crate) end: Value,
    pub(crate) exclusive: bool,
}

/// Heap representation of a Hash. Carries the key/value pairs,
/// an optional default-block ObjId for `Hash.new { |h, k| ... }`-
/// style auto-vivification, and an optional scalar default value
/// for `Hash.new(default)` (the simpler form — returned as-is on
/// missing keys, not mutated). The block is a `Value::Block`'s id;
/// `Hash#[]` invokes it with `(self_hash, key)` when the key is
/// missing. GC walks the pairs, the default-block (if present),
/// and the scalar default (if present) so nothing dangles.
///
/// Constructor `HashObj::with_pairs(pairs)` keeps all 11 internal
/// `HeapObj::Hash` allocations short. The `default_block` slot
/// gets populated by:
///
///   - `Hash.new { |h, k| ... }` in `vm/dispatch.rs` (the
///     primary entry point)
///   - `Hash#merge` (and other derivers in the future) — when the
///     receiver has a default-block, the new Hash inherits it so
///     `Hash.new { ... }.merge(x)[:y]` still auto-vivifies.
///
/// Both paths use `Heap::hash_set_default_block` after the alloc.
pub(crate) struct HashObj {
    pub(crate) pairs: Vec<(Value, Value)>,
    pub(crate) default_block: Option<ObjId>,
    /// Scalar default — set by `Hash.new(default_value)`. Returned
    /// as-is from `Hash#[]` on missing keys; NOT cached into the
    /// Hash (i.e. `h[:missing]` returns the default but doesn't
    /// add `:missing` to the pairs). Mutually exclusive with
    /// `default_block` in CRuby semantics (block takes precedence
    /// if both are set — but rubyrs's `Hash.new` already raises
    /// ArgumentError on the both-given form, so the slots are
    /// effectively exclusive at allocation time).
    pub(crate) default_value: Option<Value>,
    /// `Some(c)` when this Hash is an instance of a user subclass of
    /// Hash (`class CaseAgnosticMap < Hash`). A Hash-subclass instance
    /// IS a Hash (so Hash primitives — `[]=`, `merge!`, `size`, … —
    /// dispatch on it), but reports `c` as its class and consults
    /// `c`'s method chain for user overrides before the primitives.
    /// `None` for plain `{}` / `Hash.new` literals. Held as an
    /// `Rc<Class>` which is also rooted in `Vm.classes`, so no extra
    /// GC marking is needed.
    pub(crate) class_tag: Option<Rc<Class>>,
    /// Instance variables, for Hash-subclass instances that set
    /// `@foo` in their methods. Empty (and never touched) for plain
    /// `{}` / `Hash.new`. Values are GC-marked alongside `pairs`.
    pub(crate) ivars: std::collections::HashMap<crate::intern::SymId, Value>,
    /// O(1) key index: `ruby_hash(key)` → the positions in `pairs`
    /// whose key hashes there (usually one). `None` means "not built
    /// / invalidated" — rebuilt lazily on the next indexed lookup.
    /// Holds only `u32` offsets (no `Value`s), so the GC never has to
    /// mark it. Without this, every `Hash#[]` / `[]=` / `key?` was an
    /// O(n) linear `ruby_eql` scan over `pairs` (O(n²) to build a
    /// hash), which the Jekyll-build profile showed dominating run
    /// time (>50% in `ruby_eq`/`ruby_eql`). The map uses a
    /// passthrough hasher (`U64BuildHasher`) because `ruby_hash`
    /// already returns a well-mixed 64-bit value — re-hashing it
    /// through SipHash would just burn cycles.
    pub(crate) index: Option<HashIndex>,
}

/// `ruby_hash(key)` → pair positions, keyed directly on the 64-bit
/// `ruby_hash` (no SipHash re-mix).
pub(crate) type HashIndex =
    std::collections::HashMap<u64, Vec<u32>, std::hash::BuildHasherDefault<U64Hasher>>;

/// Passthrough `Hasher` for `u64` keys whose source value is already a
/// high-quality hash. `write_u64` stores the value; any other `write`
/// path (unused by the index) folds bytes in cheaply.
#[derive(Default)]
pub(crate) struct U64Hasher(u64);
impl std::hash::Hasher for U64Hasher {
    #[inline]
    fn finish(&self) -> u64 { self.0 }
    #[inline]
    fn write_u64(&mut self, n: u64) { self.0 = n; }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Not on the index hot path (keys go through write_u64), but a
        // correct fallback for any other use.
        for &b in bytes {
            self.0 = (self.0.rotate_left(8)) ^ b as u64;
        }
    }
}

impl HashObj {
    pub(crate) fn with_pairs(pairs: Vec<(Value, Value)>) -> Self {
        Self {
            pairs,
            default_block: None,
            default_value: None,
            class_tag: None,
            ivars: std::collections::HashMap::new(),
            index: None,
        }
    }
}

pub(crate) enum Slot {
    Live(HeapObj),
    Dead,
}

pub(crate) struct Heap {
    pub(crate) slots: Vec<Slot>,
    pub(crate) marks: Vec<bool>,
    pub(crate) free: Vec<u32>,
    pub(crate) live_count: usize,
    pub(crate) next_gc: usize,
    /// When `Some(n)`, the runtime refuses to allocate past `n` live
    /// objects; the caller traps with `ResourceExhausted`. Hosts running
    /// untrusted scripts should set this; default (None) is unlimited.
    pub(crate) max_live: Option<usize>,
    /// P2 #20 (ADR 0023): monotonic count of Fiber allocations
    /// ever made on this heap. Unlike `count_live_fibers` (which
    /// drops back to 0 after sweep), this counter only goes up —
    /// so a test can detect a transient Fiber alloc even if GC
    /// reaps it before the next observation point. Used by the
    /// Array-fast-path perf-regression guard.
    #[cfg(feature = "_fiber")]
    pub(crate) fiber_alloc_count: u64,
}

impl Heap {
    pub(crate) fn new() -> Self {
        Heap {
            slots: vec![],
            marks: vec![],
            free: vec![],
            live_count: 0,
            // Match the post-sweep min threshold so cold-start
            // workloads (preamble load + first eval) get the same
            // 4 KB-slot budget the steady-state sweep settles on.
            // Tunable via RUBYRS_GC_MIN_THRESHOLD (read inside
            // `sweep` for steady-state; init reads it too so a
            // tight-RSS embedder sees the lower bound immediately).
            next_gc: std::env::var("RUBYRS_GC_MIN_THRESHOLD")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(4096),
            max_live: None,
            #[cfg(feature = "_fiber")]
            fiber_alloc_count: 0,
        }
    }
    pub(crate) fn alloc(&mut self, obj: HeapObj) -> ObjId {
        self.live_count += 1;
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = Slot::Live(obj);
            self.marks[i as usize] = false;
            return ObjId(i);
        }
        let i = self.slots.len() as u32;
        self.slots.push(Slot::Live(obj));
        self.marks.push(false);
        ObjId(i)
    }
    pub(crate) fn get(&self, id: ObjId) -> &HeapObj {
        match &self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("ICE: use-after-free ObjId({})", id.0),
        }
    }
    pub(crate) fn get_mut(&mut self, id: ObjId) -> &mut HeapObj {
        match &mut self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("ICE: use-after-free ObjId({})", id.0),
        }
    }
    pub(crate) fn instance(&self, id: ObjId) -> &Instance {
        if let HeapObj::Instance(i) = self.get(id) { i } else { panic!("ICE: heap slot is not an Instance") }
    }

    /// Class to start method lookup from for any `Value::Object(id)`,
    /// regardless of whether the underlying slot is a plain `Instance`
    /// (script-defined class) or a `TypedData` (C ext-wrapped state).
    /// Use this in preference to `instance(id).class` whenever code
    /// reaches for "where do I look for a method on this Object" —
    /// TypedData objects don't have an Instance struct so the direct
    /// accessor panics.
    ///
    /// **Returns the singleton class if one was installed** (via
    /// `def obj.foo` or `define_singleton_method`); the singleton's
    /// `superclass` chain walks back to the real class transparently,
    /// so dispatch stays a single chain walk. For script-visible
    /// `Object#class` semantics (which CRuby reports as the original,
    /// not the eigenclass), use `real_class_of` instead.
    pub(crate) fn class_of(&self, id: ObjId) -> Rc<crate::value::Class> {
        match self.get(id) {
            HeapObj::Instance(i) => match &i.singleton_class {
                Some(sc) => sc.clone(),
                None => i.class.clone(),
            },
            HeapObj::TypedData(d) => d.class.clone(),
            _ => panic!("ICE: class_of called on non-Object slot"),
        }
    }
    /// Original class — what `Object#class` returns to script code
    /// (CRuby skips the eigenclass when reporting). Same shape as
    /// `class_of` but doesn't substitute the singleton class.
    pub(crate) fn real_class_of(&self, id: ObjId) -> Rc<crate::value::Class> {
        match self.get(id) {
            HeapObj::Instance(i) => i.class.clone(),
            HeapObj::TypedData(d) => d.class.clone(),
            _ => panic!("ICE: real_class_of called on non-Object slot"),
        }
    }
    /// Fallible variant of `real_class_of`. Returns `None` if
    /// the slot is dead or doesn't carry an Object payload —
    /// the panic-ing accessor's "ICE" cases. Used by error-path
    /// formatting (e.g. `recv_desc_for_error`) so a corrupt
    /// `Value::Object(id)` turns a NoMethodError into the next-
    /// best string instead of aborting the host on the failure
    /// path. (Code-review #291 round 2.)
    pub(crate) fn try_real_class_of(&self, id: ObjId) -> Option<Rc<crate::value::Class>> {
        let idx = id.0 as usize;
        if idx >= self.slots.len() { return None; }
        match &self.slots[idx] {
            Slot::Live(HeapObj::Instance(i)) => Some(i.class.clone()),
            Slot::Live(HeapObj::TypedData(d)) => Some(d.class.clone()),
            _ => None,
        }
    }
    /// Lazily install + return the singleton class for an Object.
    /// Idempotent: returns the same `Rc<Class>` on subsequent calls.
    /// The synthesised class has `superclass = i.class.clone()` so
    /// the chain walk transparently falls through to the original
    /// class after exhausting singleton methods.
    pub(crate) fn ensure_singleton_class(&mut self, id: ObjId) -> Rc<crate::value::Class> {
        use std::cell::RefCell;
        use std::collections::HashMap;
        let inst = self.instance_mut(id);
        if let Some(sc) = &inst.singleton_class {
            return sc.clone();
        }
        let original = inst.class.clone();
        let sc = Rc::new(crate::value::Class {
            name: format!("#<Class:#<{}>>", original.name),
            is_module: false,
            ivars: RefCell::new(HashMap::new()),
            methods: RefCell::new(HashMap::new()),
            // Eigenclasses have no per-class singleton-method
            // table of their own — `def self.foo` (master's
            // class-level singletons) doesn't apply to a
            // synthetic singleton class. Keep this empty so
            // dispatch sites that walk the chain don't break.
            singleton_methods: RefCell::new(HashMap::new()),
            superclass: RefCell::new(Some(original)),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            singleton_includes: RefCell::new(Vec::new()),
            singleton_view: RefCell::new(None),
            singleton_target: RefCell::new(None),
            class_vars: RefCell::new(HashMap::new()),
            consts: RefCell::new(HashMap::new()),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        });
        inst.singleton_class = Some(sc.clone());
        sc
    }
    pub(crate) fn instance_mut(&mut self, id: ObjId) -> &mut Instance {
        if let HeapObj::Instance(i) = self.get_mut(id) { i } else { panic!("ICE: heap slot is not an Instance") }
    }
    pub(crate) fn array(&self, id: ObjId) -> &Vec<Value> {
        if let HeapObj::Array(a) = self.get(id) { a } else { panic!("ICE: heap slot is not an Array") }
    }
    pub(crate) fn array_mut(&mut self, id: ObjId) -> &mut Vec<Value> {
        if let HeapObj::Array(a) = self.get_mut(id) { a } else { panic!("ICE: heap slot is not an Array") }
    }
    pub(crate) fn hash(&self, id: ObjId) -> &Vec<(Value, Value)> {
        if let HeapObj::Hash(h) = self.get(id) { &h.pairs } else { panic!("ICE: heap slot is not a Hash") }
    }
    pub(crate) fn hash_mut(&mut self, id: ObjId) -> &mut Vec<(Value, Value)> {
        // A caller taking `&mut pairs` may insert / delete / reorder
        // entries the index can't track, so invalidate it — the next
        // indexed lookup rebuilds it lazily. Single-key fast paths use
        // `hash_insert` / `hash_delete` instead, which keep the index
        // live (so building a Hash stays O(1) per key, not O(n²)).
        if let HeapObj::Hash(h) = self.get_mut(id) {
            h.index = None;
            &mut h.pairs
        } else {
            panic!("ICE: heap slot is not a Hash")
        }
    }
    fn hash_obj_mut(&mut self, id: ObjId) -> &mut HashObj {
        if let HeapObj::Hash(h) = self.get_mut(id) { h } else { panic!("ICE: heap slot is not a Hash") }
    }
    /// Build the key index (`ruby_hash(key)` → positions) if it isn't
    /// present. After this returns, `HashObj.index` is `Some`.
    fn ensure_hash_index(&mut self, id: ObjId) {
        if let HeapObj::Hash(h) = self.get(id) {
            if h.index.is_some() { return; }
        } else {
            panic!("ICE: heap slot is not a Hash");
        }
        let n = self.hash(id).len();
        let mut map = HashIndex::with_capacity_and_hasher(n, Default::default());
        for i in 0..n {
            let kh = self.hash(id)[i].0.ruby_hash(self);
            map.entry(kh).or_default().push(i as u32);
        }
        self.hash_obj_mut(id).index = Some(map);
    }
    /// O(1)-amortised position of `key` in the Hash, or `None`.
    /// Replaces the old `pairs.iter().position(ruby_eql)` linear scan.
    pub(crate) fn hash_index_lookup(&mut self, id: ObjId, key: &Value) -> Option<usize> {
        self.ensure_hash_index(id);
        let kh = key.ruby_hash(self);
        if let HeapObj::Hash(h) = self.get(id)
            && let Some(cands) = h.index.as_ref().and_then(|m| m.get(&kh))
        {
            for &i in cands {
                if h.pairs[i as usize].0.ruby_eql(key, self) {
                    return Some(i as usize);
                }
            }
        }
        None
    }
    /// Insert or update `key => val`, keeping the index live. Returns
    /// the previous value when the key already existed (CRuby keeps the
    /// ORIGINAL key object and only swaps the value), else `None`.
    pub(crate) fn hash_insert(&mut self, id: ObjId, key: Value, val: Value) -> Option<Value> {
        self.ensure_hash_index(id);
        let kh = key.ruby_hash(self);
        let existing: Option<usize> = if let HeapObj::Hash(h) = self.get(id) {
            h.index
                .as_ref()
                .and_then(|m| m.get(&kh))
                .and_then(|cands| {
                    cands
                        .iter()
                        .copied()
                        .find(|&i| h.pairs[i as usize].0.ruby_eql(&key, self))
                        .map(|i| i as usize)
                })
        } else {
            None
        };
        match existing {
            Some(i) => {
                let h = self.hash_obj_mut(id);
                Some(std::mem::replace(&mut h.pairs[i].1, val))
            }
            None => {
                let h = self.hash_obj_mut(id);
                let new_i = h.pairs.len() as u32;
                h.pairs.push((key, val));
                if let Some(m) = h.index.as_mut() {
                    m.entry(kh).or_default().push(new_i);
                }
                None
            }
        }
    }
    /// Delete `key`, returning its value (or `None`). Removal shifts
    /// later positions, so the index is dropped and rebuilt lazily —
    /// delete is rare relative to insert/lookup, so an O(n) reindex on
    /// the next lookup is an acceptable trade for keeping insertion
    /// order intact.
    pub(crate) fn hash_delete(&mut self, id: ObjId, key: &Value) -> Option<Value> {
        let i = self.hash_index_lookup(id, key)?;
        let h = self.hash_obj_mut(id);
        let (_, v) = h.pairs.remove(i);
        h.index = None;
        Some(v)
    }
    /// The user Hash-subclass this Hash is an instance of, if any
    /// (`class M < Hash; end; M.new` → `Some(M)`). `None` for plain
    /// `{}` / `Hash.new`.
    pub(crate) fn hash_class_tag(&self, id: ObjId) -> Option<Rc<Class>> {
        if let HeapObj::Hash(h) = self.get(id) { h.class_tag.clone() } else { None }
    }
    /// Read `@name` ivar off a (subclass) Hash; `None` if unset.
    pub(crate) fn hash_ivar_get(&self, id: ObjId, name: crate::intern::SymId) -> Option<Value> {
        if let HeapObj::Hash(h) = self.get(id) { h.ivars.get(&name).cloned() } else { None }
    }
    /// Set `@name` ivar on a (subclass) Hash.
    pub(crate) fn hash_ivar_set(&mut self, id: ObjId, name: crate::intern::SymId, v: Value) {
        if let HeapObj::Hash(h) = self.get_mut(id) { h.ivars.insert(name, v); }
    }
    /// Clone a (subclass) Hash's full ivar table — used by dup/clone.
    pub(crate) fn hash_ivars_clone(&self, id: ObjId) -> std::collections::HashMap<crate::intern::SymId, Value> {
        if let HeapObj::Hash(h) = self.get(id) { h.ivars.clone() } else { std::collections::HashMap::new() }
    }
    /// Default-value block stored alongside the Hash by `Hash.new {
    /// |h, k| ... }`. None for hash literals (`{}`) and the common
    /// `Hash.new` no-arg form. `Hash#[]` checks this slot when the
    /// key is missing — if present, invokes the block with `(self,
    /// key)` and returns the result. Mirrors CRuby's `default_proc`
    /// semantics, narrowed to the common shape (no static default
    /// value yet, no `default=` assignment — both are deferred gaps).
    pub(crate) fn hash_default_block(&self, id: ObjId) -> Option<ObjId> {
        if let HeapObj::Hash(h) = self.get(id) { h.default_block }
        else { panic!("ICE: heap slot is not a Hash (hash_default_block)") }
    }
    /// Install the default-value block; one-shot at allocation
    /// time from the `Hash.new { ... }` dispatch arm. The existing
    /// 11 in-VM hash allocations all pass through allocation
    /// directly with `default_block: None`, so this is only used
    /// when the script explicitly opts in. Panics on type
    /// mismatch (consistent with `hash()` / `hash_mut()`) so
    /// internal ObjId-routing bugs surface loudly rather than
    /// silently no-op.
    pub(crate) fn hash_set_default_block(&mut self, id: ObjId, block: Option<ObjId>) {
        if let HeapObj::Hash(h) = self.get_mut(id) { h.default_block = block; }
        else { panic!("ICE: heap slot is not a Hash (hash_set_default_block)") }
    }
    /// Scalar default — set by `Hash.new(default)`. Returned as-is
    /// on missing-key lookup. Cloned on read to avoid sharing a
    /// `&Value` into a method that's about to mutate the heap.
    /// Panics on type mismatch, consistent with `hash()`.
    pub(crate) fn hash_default_value(&self, id: ObjId) -> Option<Value> {
        if let HeapObj::Hash(h) = self.get(id) { h.default_value.clone() }
        else { panic!("ICE: heap slot is not a Hash (hash_default_value)") }
    }
    pub(crate) fn hash_set_default_value(&mut self, id: ObjId, value: Option<Value>) {
        if let HeapObj::Hash(h) = self.get_mut(id) { h.default_value = value; }
        else { panic!("ICE: heap slot is not a Hash (hash_set_default_value)") }
    }
    pub(crate) fn range(&self, id: ObjId) -> &RangeObj {
        if let HeapObj::Range(r) = self.get(id) { r } else { panic!("ICE: heap slot is not a Range") }
    }

    // P1c (ADR 0023) — Fiber heap accessors. cfg(_fiber) only.
    #[cfg(feature = "_fiber")]
    #[allow(dead_code)] // P1c.2 (bytecode wiring) consumes these
    pub(crate) fn fiber(&self, id: ObjId) -> &crate::vm::fiber::FiberObject {
        if let HeapObj::Fiber(f) = self.get(id) {
            f
        } else {
            panic!("ICE: heap slot is not a Fiber")
        }
    }

    /// Allocate a `HeapObj::Fiber` wrapping a fresh `FiberObject`
    /// (state = Created, empty snapshot) for the given body
    /// block. Returns the new slot's `ObjId` — callers wrap as
    /// `Value::Object(id)` (no dedicated `Value::Fiber` variant
    /// today; dispatch goes through the registered Fiber class).
    #[cfg(feature = "_fiber")]
    #[allow(dead_code)] // P1c.2 consumes this
    pub(crate) fn alloc_fiber(&mut self, body_block: ObjId) -> ObjId {
        self.fiber_alloc_count = self.fiber_alloc_count.saturating_add(1);
        self.alloc(HeapObj::Fiber(crate::vm::fiber::FiberObject::new(body_block)))
    }

    /// P1e.1: count currently-live `HeapObj::Fiber` slots.
    /// O(heap_size) — fine for the once-per-fiber-alloc cap
    /// check; the alternative would be a Vm-level counter
    /// kept in sync with sweep (more bookkeeping than the
    /// rate of fiber allocation justifies for typical
    /// HTTP-server workloads).
    #[cfg(feature = "_fiber")]
    #[allow(dead_code)] // P1e.1 host fn consumes this
    pub(crate) fn count_live_fibers(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| matches!(slot, Slot::Live(HeapObj::Fiber(_))))
            .count()
    }
    #[cfg(feature = "bignum")]
    pub(crate) fn bigint(&self, id: ObjId) -> &num_bigint::BigInt {
        if let HeapObj::BigInt(b) = self.get(id) { b } else { panic!("ICE: heap slot is not a BigInt") }
    }
    pub(crate) fn rational(&self, id: ObjId) -> &RationalRepr {
        if let HeapObj::Rational(r) = self.get(id) { r } else { panic!("ICE: heap slot is not a Rational") }
    }
    pub(crate) fn block(&self, id: ObjId) -> &BlockHandle {
        if let HeapObj::Block(b) = self.get(id) { b } else { panic!("ICE: heap slot is not a Block") }
    }
    pub(crate) fn bound_method(&self, id: ObjId) -> (&Value, crate::intern::SymId) {
        if let HeapObj::BoundMethod { recv, name_id, .. } = self.get(id) { (recv, *name_id) }
        else { panic!("ICE: heap slot is not a BoundMethod") }
    }
    /// Extended accessor that also returns the snapshot Method
    /// (Some at instance_method / Object#method / bind capture
    /// time, None for `unbind`'d legacy values). Callers that
    /// drive introspection (arity / parameters / source_location
    /// / owner) should prefer the snapshot so the captured
    /// metadata survives a subsequent `remove_method`.
    pub(crate) fn bound_method_full(&self, id: ObjId) -> (&Value, crate::intern::SymId, &Option<std::rc::Rc<crate::value::Method>>) {
        if let HeapObj::BoundMethod { recv, name_id, method } = self.get(id) { (recv, *name_id, method) }
        else { panic!("ICE: heap slot is not a BoundMethod") }
    }
    pub(crate) fn unbound_method(&self, id: ObjId) -> (std::rc::Rc<crate::value::Class>, crate::intern::SymId) {
        if let HeapObj::UnboundMethod { class, name_id, .. } = self.get(id) { (class.clone(), *name_id) }
        else { panic!("ICE: heap slot is not an UnboundMethod") }
    }
    /// Same shape as `bound_method_full` for UnboundMethod —
    /// introspection paths prefer the snapshot when present.
    pub(crate) fn unbound_method_full(&self, id: ObjId) -> (std::rc::Rc<crate::value::Class>, crate::intern::SymId, Option<std::rc::Rc<crate::value::Method>>) {
        if let HeapObj::UnboundMethod { class, name_id, method } = self.get(id) {
            (class.clone(), *name_id, method.clone())
        }
        else { panic!("ICE: heap slot is not an UnboundMethod") }
    }
    pub(crate) fn curried_proc(&self, id: ObjId) -> (&Value, &Vec<Value>, u16) {
        if let HeapObj::CurriedProc { underlying, gathered, target_arity } = self.get(id) {
            (underlying, gathered, *target_arity)
        } else { panic!("ICE: heap slot is not a CurriedProc") }
    }
    /// Read a TypedData slot. Panics if the slot holds a different
    /// HeapObj variant — the caller must have proven the type
    /// via `rb_check_typeddata` (or equivalent) at the cext boundary
    /// before reaching this accessor.
    ///
    /// `cfg_attr` on (wasi OR cext-off): only called from the cext
    /// bridge, which is `#[cfg(all(feature = "cext",
    /// not(target_os = "wasi")))]`. In either suppressed
    /// configuration the cext path is stubbed so this accessor has
    /// no callers and `-D warnings` would flag it as dead. Narrow
    /// to those two configurations so native+cext builds still
    /// catch accidental loss of callers (original: review #1 on
    /// PR #22 — the previous unconditional `#[allow(dead_code)]`
    /// silenced the warning on every target, defeating the
    /// panic-budget-style discipline of catching dead host code;
    /// `not(feature = "cext")` arm added by PR #75 review #3).
    /// Non-panicking TypedData accessor for the rb_check_typeddata
    /// callback path. CRuby's rb_check_typeddata raises TypeError
    /// when the slot isn't TypedData OR the descriptor doesn't
    /// match; this fn lets the cext bridge inspect the slot and
    /// rb_raise without going through the panicking accessor —
    /// closes PR #27 review finding #1 (Counter.new.bump aborting
    /// the process when the C ext expects a TypedData but the
    /// generic `.new` path produced a plain Instance).
    #[cfg_attr(any(target_os = "wasi", not(feature = "cext")), allow(dead_code))]
    pub(crate) fn try_typed_data(&self, id: ObjId) -> Option<&TypedDataObj> {
        if let HeapObj::TypedData(d) = self.get(id) { Some(d) } else { None }
    }
    pub(crate) fn should_gc(&self) -> bool { self.live_count >= self.next_gc }

    /// Run a mark-and-sweep collection.
    ///
    /// Returns a list of pending TypedData `dfree` callbacks for
    /// the caller (typically `Vm::maybe_gc`) to invoke AFTER
    /// `collect` has returned and the `&mut Heap` borrow is gone
    /// (review #2 on PR #19). Running `dfree` while still inside
    /// `collect` would alias the heap with any cext code that
    /// `dfree` transitively reaches — even though we don't expect
    /// well-behaved cexts to re-enter the VM from a free callback,
    /// the conservative shape avoids relying on that contract.
    pub(crate) fn collect(
        &mut self,
        roots: &[Value],
    ) -> Vec<(unsafe extern "C" fn(*mut std::ffi::c_void), *mut std::ffi::c_void)> {
        for m in self.marks.iter_mut() { *m = false; }
        let mut worklist: Vec<ObjId> = Vec::new();
        for v in roots { Heap::visit_value(v, &mut self.marks, &mut worklist); }
        // Mark phase: iterate each greyed object's children in place.
        // The previous impl `let children: Vec<Value> = ...clone()` per
        // pop step turned every mark visit into a full copy of the
        // container's contents — quadratic on a heap that's mostly one
        // large Array. Split-borrow `self.slots` (read) vs `self.marks`
        // (write) on disjoint fields lets us walk references directly.
        while let Some(id) = worklist.pop() {
            match &self.slots[id.0 as usize] {
                Slot::Live(HeapObj::Instance(inst)) => {
                    for v in inst.ivars.values() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Walk singleton-class closure methods too:
                    // singleton classes aren't in `Vm.classes`
                    // (they're per-object, allocated by
                    // `ensure_singleton_class`), so the
                    // `maybe_gc` root-walker that handles regular
                    // classes never reaches them. A closure-
                    // method installed via
                    // `define_singleton_method` captures locals
                    // through its `MethodClosure.captured` Rc;
                    // without this loop, those locals would be
                    // unreachable once the lexical scope returns
                    // and the GC would sweep them out from under
                    // the singleton method.
                    if let Some(sc) = &inst.singleton_class {
                        for m in sc.methods.borrow().values() {
                            if let Some(cl) = &m.closure {
                                for v in cl.captured.borrow().iter() {
                                    Heap::visit_value(v, &mut self.marks, &mut worklist);
                                }
                            }
                        }
                        // Singleton-class ivars (PR #102 addendum).
                        // `Vm.maybe_gc` walks the registered-class
                        // table for class-level ivars; eigenclasses
                        // attached to Instances live here on the
                        // heap and need their own pass — without
                        // it, a heap value stored in a singleton
                        // class's ivar table could be swept while
                        // the carrying Instance is still live.
                        for v in sc.ivars.borrow().values() {
                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                        }
                    }
                }
                Slot::Live(HeapObj::Array(a)) => {
                    for v in a {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                Slot::Live(HeapObj::Hash(h)) => {
                    for (k, v) in &h.pairs {
                        Heap::visit_value(k, &mut self.marks, &mut worklist);
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Default-block is a heap-managed Block; without
                    // a mark walk it would be swept while the Hash
                    // still references it via `Hash.new { ... }`.
                    if let Some(blk_id) = h.default_block
                        && !self.marks[blk_id.0 as usize]
                    {
                        self.marks[blk_id.0 as usize] = true;
                        worklist.push(blk_id);
                    }
                    // Scalar default — set by `Hash.new(default)`.
                    // May itself reference the heap (e.g. a default
                    // String or Array); walk via the usual Value
                    // visitor.
                    if let Some(v) = &h.default_value {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Hash-subclass instance variables.
                    for v in h.ivars.values() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                Slot::Live(HeapObj::Range(r)) => {
                    Heap::visit_value(&r.begin, &mut self.marks, &mut worklist);
                    Heap::visit_value(&r.end, &mut self.marks, &mut worklist);
                }
                Slot::Live(HeapObj::Block(bh)) => {
                    // Walk captured locals (shared Rc<RefCell> with
                    // any frame currently executing this block, but
                    // immutably borrowed only here) and the block's
                    // `self_val`. The visit_value calls do not
                    // recurse — they mark + worklist-push only —
                    // so the RefCell borrow stays scoped to this
                    // arm and can't conflict with itself.
                    let captured = bh.captured.borrow();
                    for v in captured.iter() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    drop(captured);
                    Heap::visit_value(&bh.self_val, &mut self.marks, &mut worklist);
                }
                Slot::Live(HeapObj::BoundMethod { recv, method, .. }) => {
                    // Walk the captured receiver. The method name
                    // is a SymId (not heap-managed) so no further
                    // visit is needed.
                    Heap::visit_value(recv, &mut self.marks, &mut worklist);
                    // Same captured-locals walk as UnboundMethod
                    // below: the snapshot may carry closure
                    // captures that are unreachable via the
                    // class table after a `remove_method`.
                    if let Some(m) = method
                        && let Some(cl) = &m.closure {
                        for v in cl.captured.borrow().iter() {
                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                        }
                    }
                }
                Slot::Live(HeapObj::UnboundMethod { method, .. }) => {
                    // The snapshot Method may carry a closure
                    // whose `captured` Vec holds heap-referenced
                    // Values (`define_method { ... }` captures
                    // locals). When the class table entry is
                    // dropped via `remove_method`, the snapshot
                    // is the sole holder — the regular
                    // `Vm.maybe_gc` root walker won't reach it
                    // because it only iterates `Vm.classes`'s
                    // method tables. Walk explicitly here so the
                    // captured locals stay reachable for as long
                    // as the UnboundMethod is alive. Parallel to
                    // the singleton-class loop at line ~425.
                    // `class` is `Rc<Class>` (not heap-managed)
                    // so no further visit needed; `name_id` is a
                    // SymId.
                    if let Some(m) = method
                        && let Some(cl) = &m.closure {
                        for v in cl.captured.borrow().iter() {
                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                        }
                    }
                }
                Slot::Live(HeapObj::CurriedProc { underlying, gathered, .. }) => {
                    Heap::visit_value(underlying, &mut self.marks, &mut worklist);
                    for v in gathered {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                #[cfg(feature = "_fiber")]
                Slot::Live(HeapObj::Fiber(fiber)) => {
                    // P1c (ADR 0023 v2 §"Correctness" #3): mark
                    // walks every heap-bearing slot inside the
                    // suspended snapshot — frames' locals +
                    // self_val + swap_return + block_arg, plus
                    // the operand stack and pinned set. Without
                    // this, a Value reachable only from a
                    // suspended Fiber gets swept while the
                    // Fiber still holds it. P1d adds the dual-
                    // location walk (walk both vm.frames AND
                    // every suspended FiberObject snapshot
                    // unconditionally); this arm covers the
                    // suspended-snapshot side.
                    let body_block_id = fiber.body_block;
                    if !self.marks[body_block_id.0 as usize] {
                        self.marks[body_block_id.0 as usize] = true;
                        worklist.push(body_block_id);
                    }
                    Heap::visit_value(
                        &fiber.last_value.borrow(),
                        &mut self.marks,
                        &mut worklist,
                    );
                    let snap = fiber.snapshot.borrow();
                    for frame in &snap.frames {
                        let locals = frame.locals.borrow();
                        for v in locals.iter() {
                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                        }
                        drop(locals);
                        Heap::visit_value(&frame.self_val, &mut self.marks, &mut worklist);
                        if let Some(v) = &frame.swap_return {
                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                        }
                        if let Some(bid) = frame.block_arg
                            && !self.marks[bid.0 as usize]
                        {
                            self.marks[bid.0 as usize] = true;
                            worklist.push(bid);
                        }
                    }
                    for v in &snap.stack {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    for v in &snap.pinned {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    if let Some(v) = &snap.method_return {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                _ => {}
            }
        }
        // Sweep phase: same as before, plus the L3-B `dfree`
        // callback for TypedData. We pull the function pointer +
        // data pointer out of the slot BEFORE marking it Dead so
        // we don't reborrow the slot mid-call. The dfree itself
        // may transitively re-enter Ruby (rare but legal); the GC
        // is not re-entrant, so this is documented as a contract
        // violation if it ever happens — mirrors CRuby's own
        // gc-during-gc protection model.
        let mut live = 0usize;
        let mut pending_frees: Vec<(unsafe extern "C" fn(*mut std::ffi::c_void), *mut std::ffi::c_void)> =
            Vec::new();
        for i in 0..self.slots.len() {
            match &self.slots[i] {
                Slot::Live(_) => {
                    if self.marks[i] { live += 1; }
                    else {
                        if let Slot::Live(HeapObj::TypedData(d)) = &self.slots[i]
                            && let Some(f) = d.dfree {
                                pending_frees.push((f, d.data_ptr));
                            }
                        self.slots[i] = Slot::Dead;
                        self.free.push(i as u32);
                    }
                }
                Slot::Dead => {}
            }
        }
        self.live_count = live;
        // Post-sweep trigger threshold. Originally `live * 2 max
        // 1024` — sweeps every ~1k allocs from a small base, which
        // is fine for one-shot scripts but punishes long-running
        // alloc-and-discard loops (JSON round-trip, request
        // handlers re-parsing every POST body, etc.). `live * 4 max
        // 4096` cuts sweep count ~4× on those workloads; the heap-
        // memory cost is bounded by the next sweep at 4× the new
        // live-set size, not unbounded growth. Measured on the
        // json_bench round_trip: 44 µs/iter → 35 µs/iter, ~70 % of
        // the GC overhead recovered (the remaining 3 µs is the
        // sweep itself, which is mark-cost-proportional to the
        // larger live set and would need generational separation
        // to fix — out of scope here).
        //
        // Lower-bound tunable via `RUBYRS_GC_MIN_THRESHOLD` for
        // ratchet investigations (perf budget regressions, memory-
        // RSS budget regressions); when unset the 4096 default
        // applies. Embedders running untrusted scripts with tight
        // RSS budgets can dial back to the historic 1024 / live*2
        // by setting `RUBYRS_GC_MIN_THRESHOLD=1024` +
        // `RUBYRS_GC_GROWTH=2` (the env vars stay parse-time-
        // checked so worst case is a cache miss + atoi on each
        // sweep — cheap).
        let growth = std::env::var("RUBYRS_GC_GROWTH")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(4);
        let min_threshold = std::env::var("RUBYRS_GC_MIN_THRESHOLD")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(4096);
        self.next_gc = (live * growth).max(min_threshold);
        pending_frees
    }

    pub(crate) fn visit_value(v: &Value, marks: &mut [bool], worklist: &mut Vec<ObjId>) {
        match v {
            Value::Object(id) | Value::Array(id) | Value::Hash(id) | Value::Range(id) | Value::Block(id) | Value::BoundMethod(id) | Value::UnboundMethod(id) | Value::CurriedProc(id) => {
                let i = id.0 as usize;
                if !marks[i] {
                    marks[i] = true;
                    worklist.push(*id);
                }
            }
            #[cfg(feature = "bignum")]
            Value::BigInt(id) => {
                // Leaf: HeapObj::BigInt holds no nested Values, so we
                // just mark — pushing onto the worklist would only
                // make the sweep loop re-visit a slot it has nothing
                // to do for.
                let i = id.0 as usize;
                marks[i] = true;
            }
            // Same leaf shape as BigInt — `HeapObj::Rational` is a
            // `RationalRepr { num: i64, den: i64 }` with no nested
            // Value. Without this arm a live `Value::Rational` would
            // fall into the `_ => {}` catch-all and never mark its
            // backing slot, getting swept under stress_gc and
            // corrupting subsequent reads via `heap.rational(*id)`.
            Value::Rational(id) => {
                let i = id.0 as usize;
                marks[i] = true;
            }
            _ => {}
        }
    }
}

/// Escape `raw` for inclusion in a Ruby `String#inspect`/`#to_inspect`
/// representation, appending to `out` (caller wraps in the `"` quotes).
///
/// Matches CRuby's string-inspect rules:
/// - `\\`, `"` → `\\` / `\"`
/// - `\a` (0x07)      → `\a`
/// - `\b` (0x08)      → `\b`
/// - `\t` (0x09)      → `\t`
/// - `\n` (0x0A)      → `\n`
/// - `\v` (0x0B)      → `\v`
/// - `\f` (0x0C)      → `\f`
/// - `\r` (0x0D)      → `\r`
/// - `\e` (0x1B)      → `\e`
/// - other control bytes (0x00-0x06, 0x0E-0x1A, 0x1C-0x1F, 0x7F)
///   get `\u00NN` with uppercase hex - the CRuby UTF-8 default.
///   In particular the null byte renders as `\u0000`, NOT `\0`
///   (the previously-shipped rubyrs divergence this helper closes).
/// - everything else is pushed verbatim - printable ASCII + valid
///   UTF-8 multibyte chars stay as-is, same shape as CRuby.
pub(crate) fn inspect_escape_into(raw: &str, out: &mut String) {
    for c in raw.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"'  => out.push_str("\\\""),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x09' => out.push_str("\\t"),
            '\x0A' => out.push_str("\\n"),
            '\x0B' => out.push_str("\\v"),
            '\x0C' => out.push_str("\\f"),
            '\x0D' => out.push_str("\\r"),
            '\x1B' => out.push_str("\\e"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7F => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            _ => out.push(c),
        }
    }
}

/// Whether a Symbol's name renders as the bare `:name` form in
/// `Symbol#inspect` (vs. the quoted `:"..."` form). Mirrors CRuby's
/// `rb_str_symname_p`: bare identifiers (with `@`/`@@`/`$` prefixes
/// and `?`/`!`/`=` suffixes for method-name symbols) and operator
/// method names render bare. Anything else — empty, spaces, leading
/// digit, punctuation — needs quoting.
pub(crate) fn symbol_name_is_simple(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Operator method names that print bare.
    const OPS: &[&str] = &[
        "+", "-", "*", "/", "%", "**", "==", "===", "!=", "=~", "!~", "<", "<=", ">", ">=",
        "<=>", "<<", ">>", "&", "|", "^", "~", "!", "+@", "-@", "[]", "[]=", "`",
    ];
    if OPS.contains(&s) {
        return true;
    }
    // Strip a leading sigil (`@@`, `@`, `$`) for ivar/cvar/gvar names.
    let body = s
        .strip_prefix("@@")
        .or_else(|| s.strip_prefix('@'))
        .or_else(|| s.strip_prefix('$'))
        .unwrap_or(s);
    // Method-name symbols may carry one trailing `?`, `!`, or `=`.
    let core = body
        .strip_suffix('?')
        .or_else(|| body.strip_suffix('!'))
        .or_else(|| body.strip_suffix('='))
        .unwrap_or(body);
    if core.is_empty() {
        return false;
    }
    let mut chars = core.chars();
    // `core` is non-empty (checked above); match on the first char
    // rather than an infallible-pop to stay off the panic budget.
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Render a Symbol's name in `Symbol#inspect` form (`:name` or the
/// quoted `:"..."` form). Shared by `Value::to_inspect` and the
/// `sym_primitive` inspect arm so they can't drift.
pub(crate) fn symbol_inspect(name: &str) -> String {
    if symbol_name_is_simple(name) {
        format!(":{}", name)
    } else {
        let mut s = String::with_capacity(name.len() + 4);
        s.push_str(":\"");
        inspect_escape_into(name, &mut s);
        s.push('"');
        s
    }
}

impl Value {
    /// Build a `Value::Str` from anything stringy. Centralises the
    /// `Rc<RefCell<String>>` wrap so call sites don't repeat the
    /// boilerplate.
    pub fn new_str(s: impl Into<String>) -> Self {
        Value::Str(std::rc::Rc::new(crate::value::RStr::new(s.into())))
    }
    /// Binary-safe constructor — preserves bytes verbatim (no UTF-8 check).
    pub fn new_str_bytes(b: Vec<u8>) -> Self {
        Value::Str(std::rc::Rc::new(crate::value::RStr::from_bytes(b)))
    }

    pub(crate) fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// True when this `Value` carries an `ObjId` into the GC heap
    /// (i.e. mark/sweep is the lifetime authority for the payload).
    ///
    /// Useful for callers that hold a `Value` only via Rust locals
    /// and must decide whether to pin it before a potential GC
    /// safepoint. Immediates (`Int` / `Float` / `Bool` / `Nil` /
    /// `Sym`) and Rc-shared variants (`Str` / `Class` / `Regex`)
    /// return `false` — pinning them adds GC scan work without
    /// improving safety.
    ///
    /// Keep this list aligned with `Heap::visit_value` (heap.rs:521)
    /// whenever a new heap-slot `Value` variant is introduced; both
    /// have to agree on "is this slot in the heap" or pin-protection
    /// silently rots.
    ///
    /// Implementation note: both arms enumerate every variant
    /// explicitly (no `_ => ...` catch-all) so adding a new `Value`
    /// case is a compile error here, forcing the author to make a
    /// conscious decision about GC-trackability rather than silently
    /// defaulting to `false` (which would regress chunk / group_by /
    /// min_by / … key pins for the new variant).
    pub(crate) fn is_gc_heap_ref(&self) -> bool {
        match self {
            Value::Array(_)
            | Value::Hash(_)
            | Value::Object(_)
            | Value::Range(_)
            | Value::Block(_)
            | Value::BoundMethod(_)
            | Value::UnboundMethod(_)
            | Value::CurriedProc(_) => true,
            #[cfg(feature = "bignum")]
            Value::BigInt(_) => true,
            Value::Rational(_) => true,
            // Explicitly enumerate the non-heap variants so that
            // adding a new `Value` case forces an explicit decision
            // here rather than silently defaulting to `false`.
            Value::Int(_)
            | Value::Float(_)
            | Value::Str(_)
            | Value::Sym(_)
            | Value::Bool(_)
            | Value::Nil
            | Value::Class(_) => false,
            #[cfg(feature = "regex")]
            Value::Regex(_) => false,
        }
    }
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Integer",
            #[cfg(feature = "bignum")]
            Value::BigInt(_) => "Integer", // unified with Fixnum since CRuby 2.4
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::Sym(_) => "Symbol",
            Value::Bool(_) => "Boolean",
            Value::Nil => "NilClass",
            Value::Class(_) => "Class",
            Value::Object(_) => "Object",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
            Value::Range(_) => "Range",
            Value::Block(_) => "Proc", // block lives in heap now (P2-13); type name unchanged
            #[cfg(feature = "regex")]
            Value::Regex(_) => "Regexp",
            Value::BoundMethod(_) => "Method",
            Value::UnboundMethod(_) => "UnboundMethod",
            Value::CurriedProc(_) => "Proc",
            Value::Rational(_) => "Rational",
        }
    }
    pub(crate) fn to_display(&self, heap: &Heap, interner: &Interner) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            #[cfg(feature = "bignum")]
            Value::BigInt(id) => heap.bigint(*id).to_string(),
            Value::Float(f) => format_float(*f),
            Value::Str(s) => s.to_string_lossy(),
            Value::Sym(id) => interner.resolve(*id).to_string(),
            Value::Bool(true) => "true".into(),
            Value::Bool(false) => "false".into(),
            Value::Nil => "".into(),
            Value::Class(c) => c.name.clone(),
            // Use class_of so TypedData-backed Objects (L3-B) print
            // safely too — `heap.instance(*id)` would panic on
            // those slots (review #1).
            // `#<Foo>` shows the user-declared class; CRuby
            // doesn't surface the eigenclass here even when one
            // exists. Use `real_class_of` for the same reason
            // `Object#class` does.
            Value::Object(id) => format!("#<{}>", heap.real_class_of(*id).name),
            Value::Array(id) => {
                let a = heap.array(*id);
                let parts: Vec<String> = a.iter().map(|v| v.to_inspect(heap, interner)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Hash(id) => {
                let h = heap.hash(*id);
                let parts: Vec<String> = h.iter()
                    .map(|(k, v)| {
                        // CRuby 3.4+: Symbol keys render as `name: value`
                        // shorthand instead of `:name => value`. Every
                        // other key type uses the explicit hash-rocket
                        // form with spaces around `=>`.
                        //
                        // Symbol names that aren't valid bareword
                        // identifiers (contain a hyphen, space, or
                        // start with a digit) get wrapped in quotes
                        // — `{"X-Token": "abc"}` — matching CRuby's
                        // output. Bareword-safe shape is a name that
                        // starts with [a-zA-Z_] and continues with
                        // [a-zA-Z0-9_], optionally with a trailing
                        // `?` / `!` / `=` per method-name rules.
                        fn sym_needs_quotes(name: &str) -> bool {
                            let mut chars = name.chars();
                            let Some(first) = chars.next() else { return true };
                            if !first.is_ascii_alphabetic() && first != '_' { return true; }
                            let mut last = first;
                            for c in chars {
                                last = c;
                                if c.is_ascii_alphanumeric() || c == '_' { continue; }
                                // Trailing `?` / `!` / `=` are allowed only as
                                // the final char; if we see one mid-name it
                                // counts as needing quotes too.
                                if matches!(c, '?' | '!' | '=') { continue; }
                                return true;
                            }
                            // Mid-name `?` / `!` / `=` invalid — but we
                            // accepted them above; re-check last char rules:
                            // if `last` is one of those, it's fine (trailing);
                            // if interior, we already returned. OK.
                            let _ = last;
                            false
                        }
                        if let Value::Sym(sid) = k {
                            let name = interner.resolve(*sid);
                            if sym_needs_quotes(name) {
                                format!("\"{name}\": {}", v.to_inspect(heap, interner))
                            } else {
                                format!("{name}: {}", v.to_inspect(heap, interner))
                            }
                        } else {
                            format!("{} => {}", k.to_inspect(heap, interner), v.to_inspect(heap, interner))
                        }
                    })
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::Range(id) => {
                let r = heap.range(*id);
                let sep = if r.exclusive { "..." } else { ".." };
                format!("{}{}{}", r.begin.to_display(heap, interner), sep, r.end.to_display(heap, interner))
            }
            Value::Block(_) => "#<Proc>".into(),
            #[cfg(feature = "regex")]
            Value::Regex(r) => format!("(?-mix:{})", r.as_str()),
            Value::BoundMethod(_) => "#<Method>".into(),
            Value::UnboundMethod(_) => "#<UnboundMethod>".into(),
            Value::CurriedProc(_) => "#<Proc (curried)>".into(),
            // `Rational#to_s` — CRuby uses `"num/den"` (no parens);
            // inspect wraps in `(num/den)`. den is always positive
            // by the canonical-form invariant.
            Value::Rational(id) => {
                let r = heap.rational(*id);
                format!("{}/{}", r.num, r.den)
            }
        }
    }
    pub(crate) fn to_inspect(&self, heap: &Heap, interner: &Interner) -> String {
        match self {
            Value::Str(s) => {
                // Both `Array#inspect` (this path, via to_inspect on
                // each element) and `String#inspect` (the primitive
                // arm in vm/string.rs) share the same escape rules —
                // funnel both through the `inspect_escape_into`
                // helper so they can't drift apart.
                let raw = s.to_string_lossy();
                let mut out = String::with_capacity(raw.len() + 2);
                out.push('"');
                inspect_escape_into(&raw, &mut out);
                out.push('"');
                out
            },
            Value::Sym(id) => symbol_inspect(interner.resolve(*id)),
            Value::Nil => "nil".into(),
            // Range#inspect joins the endpoints via `#inspect`, not
            // `#to_s` — so String endpoints come out quoted
            // (`("a".."z").inspect == "\"a\"..\"z\""`). Endless /
            // beginless (`(1..)`, `(..5)`) render the missing
            // endpoint as empty, matching CRuby. The Range#to_s
            // arm in `to_display` above already uses `to_display`
            // on the endpoints, which naturally renders Strings
            // unquoted and Nil endpoints empty.
            Value::Range(id) => {
                let r = heap.range(*id);
                let sep = if r.exclusive { "..." } else { ".." };
                let endpoint = |v: &Value| -> String {
                    match v {
                        Value::Nil => String::new(),
                        _ => v.to_inspect(heap, interner),
                    }
                };
                format!("{}{}{}", endpoint(&r.begin), sep, endpoint(&r.end))
            }
            // `Rational#inspect` wraps the `num/den` display form
            // in parens to match CRuby (`Rational(1, 2).inspect ==
            // "(1/2)"`); `to_s` keeps the bare form via to_display.
            Value::Rational(id) => {
                let r = heap.rational(*id);
                format!("({}/{})", r.num, r.den)
            }
            _ => self.to_display(heap, interner),
        }
    }
    /// CRuby's `Object#eql?` — like `==` but WITHOUT cross-numeric-type
    /// coercion. `1.eql?(1.0)` is `false`; `1 == 1.0` is `true`.
    /// Used for Hash key collision / lookup so `{ 1.0 => :a, 1 => :b }`
    /// keeps both entries (CRuby semantics). Distinct from `ruby_eq`
    /// which is the `==` predicate used by Array#include?, the BinOp
    /// `==` opcode, etc.
    ///
    /// For non-numeric types this delegates to `ruby_eq` — they have
    /// no numeric-type-class to coerce across in the first place.
    /// Composite types (Array, Hash, Range) defer to the contained
    /// elements via this same `ruby_eql` so the strict semantics
    /// nest correctly: `[1.0] != [1]` under eql?, matching CRuby.
    ///
    /// Divergence ratcheted by PR #193's `divergence_hash_eql_keys`
    /// fixture; this method is the surface that retires it.
    pub(crate) fn ruby_eql(&self, other: &Value, heap: &Heap) -> bool {
        match (self, other) {
            // Float-vs-Float — `eql?` strictness PLUS a NaN
            // identity shortcut. CRuby's `Float#eql?(NaN, NaN)`
            // returns false (NaN != NaN even structurally),
            // but Array#uniq / Hash#uniq dedup AND Hash key
            // lookup (Hash#[], Hash#[]=, Hash#include?, key
            // collision check on insert) use an identity check
            // FIRST (same Float object short-circuits to
            // equal), and only fall back to `eql?` for
            // distinct objects. rubyrs's Float is a value type
            // with no identity, so distinct-but-bit-identical
            // NaN values are indistinguishable from "the same
            // NaN object" in CRuby. Treat them as eql? —
            // otherwise:
            //   - Hash#uniq / Array#uniq silently fail to
            //     dedupe the common `{a: nan, b: nan}.uniq`
            //     shape.
            //   - `h = {nan => 1}; h[nan]` returns nil (key
            //     lookup fails) instead of 1.
            //   - Set-like operations don't recognise NaN as
            //     a stored key.
            // Trade-off: distinct CRuby Float objects with
            // matching NaN bits would diverge here (we dedup,
            // CRuby doesn't), but rubyrs has no way to model
            // that distinction. For non-NaN floats, plain
            // `==` is identical to CRuby (handles ±0.0 etc.).
            (Value::Float(a), Value::Float(b)) => {
                if a.is_nan() && b.is_nan() {
                    a.to_bits() == b.to_bits()
                } else {
                    a == b
                }
            }
            // Numeric strictness: no Int↔Float or Int↔BigInt
            // coercion. Two values of DIFFERENT numeric type can
            // never be eql?, even when their `==` would be true.
            (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => false,
            #[cfg(feature = "bignum")]
            (Value::Int(_), Value::BigInt(_)) | (Value::BigInt(_), Value::Int(_)) => false,
            #[cfg(feature = "bignum")]
            (Value::Float(_), Value::BigInt(_)) | (Value::BigInt(_), Value::Float(_)) => false,
            // Phase C.2 — Rational cross-type strictness. The new
            // ruby_eq arms (heap.rs:~1268) make `Rational(1, 1) ==
            // 1` true via canonical i128 cross-multiply, but `eql?`
            // must remain type-strict per CRuby. Without these
            // explicit-false arms, ruby_eql falls through to
            // ruby_eq and `Rational(1, 1).eql?(1)` returns true —
            // breaking Hash#uniq / Array#uniq / Set semantics for
            // mixed numeric collections.
            (Value::Int(_), Value::Rational(_)) | (Value::Rational(_), Value::Int(_)) => false,
            (Value::Float(_), Value::Rational(_)) | (Value::Rational(_), Value::Float(_)) => false,
            #[cfg(feature = "bignum")]
            (Value::BigInt(_), Value::Rational(_)) | (Value::Rational(_), Value::BigInt(_)) => false,
            // Composites recurse via ruby_eql so the strictness
            // propagates: `[1] eql? [1.0]` is false.
            (Value::Array(a), Value::Array(b)) => {
                if a == b { return true; }
                let x = heap.array(*a); let y = heap.array(*b);
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.ruby_eql(q, heap))
            }
            (Value::Hash(a), Value::Hash(b)) => {
                if a == b { return true; }
                let x = heap.hash(*a); let y = heap.hash(*b);
                if x.len() != y.len() { return false; }
                x.iter().all(|(k, v)| {
                    y.iter().any(|(k2, v2)| k.ruby_eql(k2, heap) && v.ruby_eql(v2, heap))
                })
            }
            (Value::Range(a), Value::Range(b)) => {
                if a == b { return true; }
                let x = heap.range(*a); let y = heap.range(*b);
                x.exclusive == y.exclusive
                    && x.begin.ruby_eql(&y.begin, heap)
                    && x.end.ruby_eql(&y.end, heap)
            }
            // Same-type primitives + non-numeric paths reuse ruby_eq.
            _ => self.ruby_eq(other, heap),
        }
    }

    /// A hash code consistent with `ruby_eql`: whenever
    /// `a.ruby_eql(b, heap)` holds, `a.ruby_hash(heap) ==
    /// b.ruby_hash(heap)`. Backs the O(1) `HashObj` key index.
    ///
    /// Numeric strictness mirrors `ruby_eql` — `Int(5)` and
    /// `Float(5.0)` are NOT eql, so they live in different hash
    /// domains (distinct type tags). Object / Class keys hash by
    /// identity (ObjId / Rc pointer), matching `ruby_eq`'s
    /// identity comparison for those (rubyrs doesn't honour a
    /// user-defined `hash`/`eql?` pair for Hash keys — same blind
    /// spot the prior linear scan had).
    pub(crate) fn ruby_hash(&self, heap: &Heap) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        #[inline]
        fn mix(mut h: u64, bytes: &[u8]) -> u64 {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        }
        let h = FNV_OFFSET;
        match self {
            Value::Nil => mix(h, &[0]),
            Value::Bool(false) => mix(h, &[1]),
            Value::Bool(true) => mix(h, &[2]),
            Value::Int(n) => mix(mix(h, &[3]), &n.to_le_bytes()),
            Value::Float(f) => {
                // `-0.0`/`+0.0` are eql → normalise; NaN keeps its bits
                // (matching ruby_eql's NaN-bits identity).
                let bits = if *f == 0.0 { 0u64 } else { f.to_bits() };
                mix(mix(h, &[4]), &bits.to_le_bytes())
            }
            Value::Sym(s) => mix(mix(h, &[5]), &s.0.to_le_bytes()),
            Value::Str(rs) => mix(mix(h, &[6]), &rs.content.borrow()),
            Value::Object(id) => mix(mix(h, &[7]), &id.0.to_le_bytes()),
            Value::Class(c) => {
                mix(mix(h, &[8]), &(Rc::as_ptr(c) as usize as u64).to_le_bytes())
            }
            Value::Array(id) => {
                // Order-dependent (ruby_eql for Array is positional).
                let mut hh = mix(h, &[9]);
                for e in heap.array(*id).iter() {
                    hh = hh.wrapping_mul(FNV_PRIME) ^ e.ruby_hash(heap);
                }
                hh
            }
            Value::Hash(id) => {
                // Order-INdependent (ruby_eql for Hash ignores order):
                // XOR the per-pair contributions.
                let mut acc = 0u64;
                for (k, v) in heap.hash(*id).iter() {
                    acc ^= k
                        .ruby_hash(heap)
                        .wrapping_mul(31)
                        .wrapping_add(v.ruby_hash(heap));
                }
                mix(h, &[10]) ^ acc
            }
            Value::Range(id) => {
                let r = heap.range(*id);
                let hh = mix(h, &[11, r.exclusive as u8]);
                hh.wrapping_mul(FNV_PRIME) ^ r.begin.ruby_hash(heap).wrapping_add(
                    r.end.ruby_hash(heap).wrapping_mul(FNV_PRIME),
                )
            }
            Value::Rational(id) => {
                // `num`/`den` are BigInt under `bignum` and i64 without
                // it; hash the canonical (normalised) decimal form so
                // this compiles on both (and on wasm, where bignum is
                // off). Rationals are rare as Hash keys, so the
                // formatting cost is irrelevant.
                let r = heap.rational(*id);
                let hh = mix(h, &[12]);
                mix(mix(hh, r.num.to_string().as_bytes()), r.den.to_string().as_bytes())
            }
            #[cfg(feature = "bignum")]
            Value::BigInt(id) => mix(mix(h, &[13]), &heap.bigint(*id).to_signed_bytes_le()),
            // Procs / Methods / Blocks etc. aren't realistic Hash keys;
            // collapse to one bucket (still correct — identity-eql, and
            // they share the bucket so a linear ruby_eql scan resolves
            // any genuine collision).
            _ => mix(h, &[255]),
        }
    }

    pub(crate) fn ruby_eq(&self, other: &Value, heap: &Heap) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            // Numeric coercion: CRuby treats `5 == 5.0` as `true`.
            // Routes through `int_cmp_float_lossless` (numeric.rs)
            // so `|i| > 2^53` doesn't collapse onto the demoted
            // f64 bit pattern — same fix the BinOp `==` path got
            // in PR #237, mirrored here so `===` (via ruby_eq)
            // stays an alias of `==` for large integers.
            // `cmp == Some(Equal)` returns false for NaN
            // (helper returns None) — matches CRuby `5 == NaN`.
            (Value::Int(a), Value::Float(b)) => {
                crate::vm::int_cmp_float_lossless(*a, *b)
                    == Some(std::cmp::Ordering::Equal)
            }
            (Value::Float(a), Value::Int(b)) => {
                crate::vm::int_cmp_float_lossless(*b, *a)
                    == Some(std::cmp::Ordering::Equal)
            }
            (Value::Str(a), Value::Str(b)) => *a.borrow() == *b.borrow(),
            (Value::Sym(a), Value::Sym(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil, Value::Nil) => true,
            (Value::Object(a), Value::Object(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                if a == b { return true; }
                let x = heap.array(*a); let y = heap.array(*b);
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.ruby_eq(q, heap))
            }
            (Value::Hash(a), Value::Hash(b)) => {
                if a == b { return true; }
                let x = heap.hash(*a); let y = heap.hash(*b);
                if x.len() != y.len() { return false; }
                // Order-insensitive: for each (k, v) in `x`, find a
                // matching key in `y` with equal value. O(n*m) but
                // the lookup is unavoidable until we hash keys
                // properly (P3-class follow-up).
                x.iter().all(|(k, v)| {
                    y.iter().any(|(k2, v2)| k.ruby_eq(k2, heap) && v.ruby_eq(v2, heap))
                })
            }
            (Value::Range(a), Value::Range(b)) => {
                if a == b { return true; }
                let x = heap.range(*a); let y = heap.range(*b);
                x.exclusive == y.exclusive
                    && x.begin.ruby_eq(&y.begin, heap)
                    && x.end.ruby_eq(&y.end, heap)
            }
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            // BigInt × BigInt and BigInt ↔ Int — value equality so
            // Array#include?, Hash key matching, and the Object#==
            // fallback all see the same answer the BinOp == arm does
            // (the BinOp path goes through try_bigint_binop which
            // compares the underlying num_bigint::BigInt). Two
            // separately-allocated `2**64` BigInts must hash-equal
            // when treated as keys / collection members.
            #[cfg(feature = "bignum")]
            (Value::BigInt(a), Value::BigInt(b)) => {
                a == b || heap.bigint(*a) == heap.bigint(*b)
            }
            #[cfg(feature = "bignum")]
            (Value::BigInt(a), Value::Int(b)) => {
                heap.bigint(*a) == &num_bigint::BigInt::from(*b)
            }
            #[cfg(feature = "bignum")]
            (Value::Int(a), Value::BigInt(b)) => {
                &num_bigint::BigInt::from(*a) == heap.bigint(*b)
            }
            // BigInt × Float — lossless compare, mirroring the
            // BinOp `==` path (PR #230). Routes through the same
            // `bigint_equals_float_lossless` helper. Examples:
            //   `(2**64) === (2**64).to_f` → true (2^64 is exactly
            //     representable as f64; both sides denote the same
            //     integer value).
            //   `(2**64 + 1) === (2**64).to_f` → false (the LHS
            //     BigInt is 2^64 + 1; the RHS Float exactly denotes
            //     2^64 — so the integer values differ by 1).
            //   `(2**64) === 1.5` → false (fractional Float never
            //     equals an integer).
            //   BigInt × NaN / ±inf → false.
            // Without these arms, the comparison fell through to
            // `_ => false` since ruby_eq had no BigInt × Float
            // coverage — diverging from CRuby's `===` which
            // delegates to value `==`.
            #[cfg(feature = "bignum")]
            (Value::BigInt(a), Value::Float(b)) => {
                crate::vm::bigint_equals_float_lossless(heap.bigint(*a), *b)
            }
            #[cfg(feature = "bignum")]
            (Value::Float(a), Value::BigInt(b)) => {
                crate::vm::bigint_equals_float_lossless(heap.bigint(*b), *a)
            }
            // Phase C.1: structural equality for Rational. Safe to
            // wire now (independent of arithmetic) because the
            // gcd-normalize + sign-normalize at construction
            // guarantee canonical form — two Rationals representing
            // the same value always have identical (num, den) pairs.
            // Same-ObjId fast path mirrors the Array / Hash / Range /
            // BigInt arms above.
            (Value::Rational(a), Value::Rational(b)) => {
                if a == b { return true; }
                let x = heap.rational(*a);
                let y = heap.rational(*b);
                x.num == y.num && x.den == y.den
            }
            // Phase C.2: cross-type Rational × Integer / Float
            // equality. The BinOp `==` path goes through
            // `try_rational_binop` (canonical i128 cross-multiply
            // for Int, lossy f64 demote for Float), but
            // `send(:==, ...)` and the universal `Object#==`
            // fallback go through `ruby_eq` — without these arms
            // `1.send(:==, Rational(1, 1))` returned false despite
            // `1 == Rational(1, 1)` returning true. Mirrors the
            // BigInt × Float / BigInt × Int pattern above.
            // Canonical form means r.den > 0, so no sign-fixup is
            // needed on the cross-multiply.
            (Value::Rational(rid), Value::Int(n)) => {
                let r = heap.rational(*rid);
                rational_eq_int(r, *n)
            }
            (Value::Int(n), Value::Rational(rid)) => {
                let r = heap.rational(*rid);
                rational_eq_int(r, *n)
            }
            (Value::Rational(rid), Value::Float(f)) => {
                if !f.is_finite() { return false; }
                let r = heap.rational(*rid);
                rational_to_f64(r) == *f
            }
            (Value::Float(f), Value::Rational(rid)) => {
                if !f.is_finite() { return false; }
                let r = heap.rational(*rid);
                *f == rational_to_f64(r)
            }
            _ => false,
        }
    }
}

/// Format a `Value::Float` for `to_display` / `to_inspect`.
/// Rust's `{:?}` already preserves `.0` on whole numbers
/// (`5.0` → `"5.0"`) so common cases match CRuby for free.
/// Scientific notation for very large / small magnitudes is a
/// known divergence — Rust prints `1e16`, CRuby prints `1.0e+16`.
/// Restrict diff fixtures to the everyday range until P3-class
/// formatter work lands.
pub(crate) fn format_float(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    format!("{:?}", f)
}
