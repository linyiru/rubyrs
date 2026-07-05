use std::rc::Rc;

use crate::intern::Interner;
use crate::value::{BlockHandle, Class, Instance, ObjId, Value};

// ---------- GC Heap ----------

pub(crate) enum HeapObj {
    Instance(Instance),
    Array(ArrayObj),
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
    /// `Box`ed: `FiberObject` is ~472 bytes (it inlines a suspended
    /// execution snapshot), vs the next-largest HeapObj variant at
    /// ~136. Left unboxed it would size EVERY heap slot to 480 bytes
    /// — even Strings/Arrays/Hashes, even when no fiber is alive (a
    /// Rust enum is sized to its largest variant). Boxing keeps the
    /// rare, heavyweight fiber state off-slab so the common slot is
    /// ~136 bytes (3.5× smaller slab + GC-walk stride). Fibers are
    /// rare and already expensive, so the extra indirection is noise.
    #[cfg(feature = "_fiber")]
    Fiber(Box<crate::vm::fiber::FiberObject>),
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

/// Heap-side of `Value::Array`. Mirrors `HashObj`'s subclass
/// support: `class StringRegister < Array` (rouge's python lexer)
/// allocates a REAL Array — so every Array primitive dispatches on
/// its instances — carrying the actual class in `class_tag` for
/// `obj.class` / `is_a?` / user-override lookup, plus `ivars` for
/// `@foo` set in subclass methods. Plain `[]` literals pay one
/// `None` + one empty-map (no allocation) over the previous bare
/// `Vec<Value>`; the enum's size is already set by the larger
/// `HashObj` variant, so the layout cost is zero. `Deref` to
/// `Vec<Value>` keeps the ~200 existing element-access sites
/// compiling unchanged.
pub(crate) struct ArrayObj {
    pub(crate) elems: Vec<Value>,
    /// `Some(c)` for instances of a user subclass of Array;
    /// `None` for plain literals. `Rc<Class>` is not GC-managed,
    /// so no marking needed.
    pub(crate) class_tag: Option<Rc<Class>>,
    /// Instance variables for subclass instances. Values are
    /// GC-marked alongside `elems`.
    pub(crate) ivars: crate::intern::FxHashMap<crate::intern::SymId, Value>,
    /// CRuby's per-object frozen bit. `false` by default; `freeze`
    /// flips it and every mutating method then raises FrozenError.
    /// `clone` copies it (CRuby semantics); `dup` resets it to false.
    pub(crate) frozen: std::cell::Cell<bool>,
}

impl ArrayObj {
    pub(crate) fn plain(elems: Vec<Value>) -> Self {
        Self {
            elems,
            class_tag: None,
            ivars: crate::intern::FxHashMap::default(),
            frozen: std::cell::Cell::new(false),
        }
    }
}

impl From<Vec<Value>> for ArrayObj {
    fn from(elems: Vec<Value>) -> Self {
        Self::plain(elems)
    }
}

impl std::ops::Deref for ArrayObj {
    type Target = Vec<Value>;
    fn deref(&self) -> &Vec<Value> {
        &self.elems
    }
}

impl std::ops::DerefMut for ArrayObj {
    fn deref_mut(&mut self) -> &mut Vec<Value> {
        &mut self.elems
    }
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
///
/// ## Record-shape layout (2026-07 small-hash campaign)
///
/// The overwhelming majority of live Hashes are small "records" —
/// JSON objects, kwargs, option hashes — that never carry a default,
/// a subclass tag, ivars, an eigenclass, or (below the
/// `HASH_INDEX_MIN` threshold) an index. The old flat struct made
/// every such Hash pay for all of those slots anyway: `HashObj` was
/// 168 bytes, which (being the largest `HeapObj` variant) also set
/// the heap-slot size for EVERY object on the heap. Two changes,
/// profiled on the JSON `parse_sids` workload (200 tiny record
/// hashes per parse):
///
///   1. The cold tail lives behind `extras: Option<Box<HashExtras>>`
///      — `None` until a default/tag/ivar/index/eigenclass is first
///      set, so record hashes allocate and sweep 40-ish bytes of
///      cold-slot bookkeeping instead of 144.
///   2. `pairs` is a `SmallVec` with `HASH_INLINE_PAIRS` inline
///      slots (CRuby's `ar_table` analogue: entries embedded in the
///      RHash slot, no separate table allocation): hashes at or
///      below the inline cap pay NO pairs allocation at all — and
///      correspondingly no free at sweep, which the parse_sids
///      profile showed dominating GC time.
///
/// Both changes together keep `size_of::<HashObj>()` at or below
/// `Instance`'s 128 bytes, so the shared heap-slot size DROPS from
/// 168 to 136 bytes (pinned by the `heap_layout_sizes` test).
pub(crate) struct HashObj {
    pub(crate) pairs: PairsBuf,
    /// Cold tail (defaults / subclass tag / ivars / indexes /
    /// eigenclass) — `None` for record-shaped hashes. Private so
    /// every consumer goes through the accessors below, which
    /// preserve the lazy-allocation invariant.
    extras: Option<Box<HashExtras>>,
    /// CRuby's per-object frozen bit (the Hash twin of
    /// `ArrayObj.frozen`). `freeze` sets it; every mutating method
    /// then raises FrozenError. `clone` copies it; `dup` resets it.
    pub(crate) frozen: std::cell::Cell<bool>,
    /// `Hash#compare_by_identity` flag. When set, CRuby compares keys
    /// by `equal?` (object identity) instead of `eql?`/`hash`. In
    /// rubyrs's Tier-1 Hash model, object/class/module/symbol keys
    /// ALREADY compare by identity (their `ruby_eql`/`ruby_hash` are
    /// pointer/ id based), so the realistic identity-map use — keys
    /// that are Module/Class objects, e.g. zeitwerk's
    /// `Zeitwerk::Cref::Map` — behaves correctly with the flag alone.
    /// Primitive-value keys (String/Array contents) keep value
    /// semantics here, a documented divergence (full identity hashing
    /// would need an object-id-keyed index threaded through the hot
    /// path). `compare_by_identity?` reflects this bit; `clone`/`dup`
    /// preserve it (CRuby copies the flag on both).
    pub(crate) by_identity: std::cell::Cell<bool>,
}

/// Inline `pairs` capacity — the `ar_table` analogue's embed limit.
/// 3 keeps `HashObj` (120 B) under `Instance` (128 B) so the shared
/// heap-slot size is set by Instance, not Hash; 4 would push the
/// slot from 136 to 160 bytes for every object on the heap. Records
/// with more pairs spill to the heap exactly like the old `Vec`
/// (one allocation), so nothing gets slower past the cap.
pub(crate) const HASH_INLINE_PAIRS: usize = 3;

/// The Hash pairs buffer: insertion-ordered key/value pairs, inline
/// up to [`HASH_INLINE_PAIRS`]. Grows to a heap buffer past that —
/// all `Vec` mutators used by the VM (`push` / `insert` / `remove` /
/// `retain` / `drain` / `truncate` / `sort_by`) exist on `SmallVec`
/// with identical semantics.
pub(crate) type PairsBuf = smallvec::SmallVec<[(Value, Value); HASH_INLINE_PAIRS]>;

/// The cold tail of a Hash — every slot that record-shaped hashes
/// (JSON objects, kwargs, option hashes) never touch. Boxed behind
/// `HashObj::extras`, allocated lazily by the first setter that
/// needs it. Field semantics are unchanged from the pre-2026-07
/// flat layout; see each field's comment.
#[derive(Default)]
pub(crate) struct HashExtras {
    /// `Hash.new { |h, k| ... }` default block (a `Value::Block` id).
    /// GC walks it; `Hash#[]` invokes it with `(self_hash, key)` on
    /// a missing key.
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
    pub(crate) ivars: crate::intern::FxHashMap<crate::intern::SymId, Value>,
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
    /// O(1) index for keys that override Ruby-level `hash`/`eql?` (e.g.
    /// `Parser::Source::Range`, AST nodes). The native `index` above can't
    /// hold them — its `ruby_hash` is identity-based, so value-equal-but-
    /// distinct objects land in different buckets and never dedup. This maps
    /// the key's RUBY `#hash` (an i64; non-Integer results fold to 0) → the
    /// `pairs` positions that hash there, so `vm_hash_find`/`insert` only
    /// `eql?`-compare within one bucket instead of scanning all pairs. Built
    /// and maintained at the VM level (`ensure_user_index`) since computing the
    /// hash needs method dispatch. `None` = not built / invalidated (a delete
    /// shifts positions). Only u32 offsets, so the GC never marks it. Without
    /// it, RuboCop's `add_offense` dedup (`Set#add?` over Range keys) was
    /// O(offenses²) — ~30s on a 7617-offense file.
    pub(crate) user_index: Option<crate::intern::FxHashMap<i64, Vec<u32>>>,
    /// Per-instance eigenclass — `def h.method_missing` /
    /// `h.define_singleton_method` (the openstruct-over-Hash
    /// pattern; minitest's ValueMonad tests). `None` for the
    /// overwhelming majority of Hashes; dispatch only consults it
    /// behind the VM-level `any_hash_singletons` gate so plain
    /// Hash traffic pays nothing.
    pub(crate) singleton_class: Option<Rc<Class>>,
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
    /// Deep copy for the post-preamble baseline snapshot
    /// (`Heap::capture_baseline_contents`): owns fresh `pairs` /
    /// `ivars` buffers so post-capture mutations of the live Hash
    /// can't reach the snapshot. The lazily-rebuilt caches
    /// (`index`, `user_index`) are dropped rather than cloned —
    /// they hold only positional offsets and re-derive on first
    /// use. `class_tag` / `singleton_class` share their `Rc`s:
    /// classes are snapshot-restored separately
    /// (`ClassStateSnapshot`), and a baseline eigenclass's inner
    /// RefCell state is a documented residual (see
    /// `Heap::capture_baseline_contents`).
    pub(crate) fn baseline_deep_clone(&self) -> HashObj {
        HashObj {
            pairs: self.pairs.clone(),
            extras: self.extras.as_ref().map(|e| {
                Box::new(HashExtras {
                    default_block: e.default_block,
                    default_value: e.default_value.clone(),
                    class_tag: e.class_tag.clone(),
                    ivars: e.ivars.clone(),
                    index: None,
                    user_index: None,
                    singleton_class: e.singleton_class.clone(),
                })
            }),
            frozen: std::cell::Cell::new(self.frozen.get()),
            by_identity: std::cell::Cell::new(self.by_identity.get()),
        }
    }

    /// Plain-Hash constructor. Accepts a `Vec` (every pre-existing
    /// caller) or a `PairsBuf` built directly (the hot JSON/YAML
    /// visitors — building inline avoids a Vec alloc + free for
    /// ≤ `HASH_INLINE_PAIRS` records).
    pub(crate) fn with_pairs(pairs: impl Into<PairsBuf>) -> Self {
        Self {
            pairs: pairs.into(),
            extras: None,
            frozen: std::cell::Cell::new(false),
            by_identity: std::cell::Cell::new(false),
        }
    }

    /// Subclass-instance constructor (`class M < Hash; M.new`):
    /// `with_pairs` + the class tag in one step.
    pub(crate) fn with_pairs_tagged(
        pairs: impl Into<PairsBuf>,
        class_tag: Option<Rc<Class>>,
    ) -> Self {
        let mut h = Self::with_pairs(pairs);
        if class_tag.is_some() {
            h.extras_mut().class_tag = class_tag;
        }
        h
    }

    /// Cold-tail view (`None` = pristine record-shaped Hash).
    #[inline]
    pub(crate) fn extras(&self) -> Option<&HashExtras> {
        self.extras.as_deref()
    }

    /// Mutable cold tail, allocating it on first use. Callers that
    /// only want to CLEAR state should prefer the `Option`-aware
    /// helpers (`clear_indexes`, …) so a pristine Hash stays
    /// extras-free.
    #[inline]
    pub(crate) fn extras_mut(&mut self) -> &mut HashExtras {
        self.extras.get_or_insert_with(Box::default)
    }

    #[inline]
    pub(crate) fn default_block(&self) -> Option<ObjId> {
        self.extras.as_ref().and_then(|e| e.default_block)
    }
    #[inline]
    pub(crate) fn default_value(&self) -> Option<&Value> {
        self.extras.as_ref().and_then(|e| e.default_value.as_ref())
    }
    /// `true` when `Hash#[]` on a missing key would NOT just return
    /// nil (default value or default proc present). Consumed by the
    /// jit-native Hash helpers; dead code elsewhere by design.
    #[cfg_attr(not(feature = "jit-native"), allow(dead_code))]
    #[inline]
    pub(crate) fn has_default(&self) -> bool {
        self.extras
            .as_ref()
            .is_some_and(|e| e.default_block.is_some() || e.default_value.is_some())
    }
    #[inline]
    pub(crate) fn class_tag(&self) -> Option<&Rc<Class>> {
        self.extras.as_ref().and_then(|e| e.class_tag.as_ref())
    }
    #[inline]
    pub(crate) fn singleton_class(&self) -> Option<&Rc<Class>> {
        self.extras.as_ref().and_then(|e| e.singleton_class.as_ref())
    }
    /// Subclass ivar table; `None` when no ivar was ever set (the
    /// borrow-free read path).
    #[inline]
    pub(crate) fn ivars(&self) -> Option<&crate::intern::FxHashMap<crate::intern::SymId, Value>> {
        self.extras.as_ref().map(|e| &e.ivars)
    }
    #[inline]
    pub(crate) fn index(&self) -> Option<&HashIndex> {
        self.extras.as_ref().and_then(|e| e.index.as_ref())
    }
    #[inline]
    pub(crate) fn user_index(
        &self,
    ) -> Option<&crate::intern::FxHashMap<i64, Vec<u32>>> {
        self.extras.as_ref().and_then(|e| e.user_index.as_ref())
    }
    /// Mutable user-key index view WITHOUT allocating extras — `None`
    /// when the index was never built (callers that only maintain an
    /// existing index must not force a Box onto a pristine Hash).
    #[inline]
    pub(crate) fn user_index_mut(
        &mut self,
    ) -> Option<&mut crate::intern::FxHashMap<i64, Vec<u32>>> {
        self.extras.as_deref_mut().and_then(|e| e.user_index.as_mut())
    }
    /// Drop both key indexes (pairs mutated in a way they can't
    /// track). No-op — and no extras allocation — on a pristine Hash.
    #[inline]
    pub(crate) fn clear_indexes(&mut self) {
        if let Some(e) = self.extras.as_deref_mut() {
            e.index = None;
            e.user_index = None;
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
    /// The GC trigger's minimum window (the floor in
    /// `next_gc = (live * growth).max(floor)`), adapted per sweep by the
    /// cost-proportional controller in `collect`'s epilogue (see the
    /// comment there for the model + measurements). Captured/restored by
    /// `Runtime`'s PostPreambleSnapshot so `reset()` rewinds it with the
    /// rest of the GC bookkeeping.
    pub(crate) gc_floor: usize,
    /// `RUBYRS_GC_MIN_THRESHOLD` parsed once at construction. `Some(n)`
    /// pins `gc_floor` to `n` and disables the adaptive controller.
    pub(crate) floor_override: Option<usize>,
    /// Previous sweep's measured wall time in µs — the controller raises
    /// the floor on `min(current, last)`, so a single anomalously slow
    /// sweep (scheduler blip) cannot inflate the window; two consecutive
    /// expensive sweeps are required. Snapshot-captured like `gc_floor`.
    pub(crate) last_sweep_us: u64,
    /// Generational GC step 1 (groundwork — does not change GC behaviour yet):
    /// `old[i] == true` once slot `i`'s object has survived a collection. New
    /// allocations are young (`false`). A future minor GC will skip re-walking
    /// old objects; `remembered` is the write-barrier's record of old objects
    /// mutated since the last collection (the only way an old object can come to
    /// reference a young one), so the minor GC still finds young objects held
    /// only by an old one. The barrier lives in the single `get_mut` mutation
    /// chokepoint, so it cannot be bypassed.
    pub(crate) old: Vec<bool>,
    pub(crate) remembered: Vec<u32>,
    /// Minor collections since the last major (full) one. A minor never sweeps
    /// old objects, so old garbage accumulates until a periodic major reclaims
    /// it; this counter triggers that major.
    pub(crate) minors_since_major: u32,
    /// Slots allocated since the last collection — the YOUNG region. A minor GC
    /// resets + sweeps ONLY these (O(young)), instead of scanning every slot
    /// (O(heap)); old objects keep their mark from the last collection. Cleared
    /// each collection (survivors are promoted to old, the rest freed).
    pub(crate) young_slots: Vec<u32>,
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
    /// The registered `Fiber` class, cached so `class_of` /
    /// `real_class_of` can report it for the class-less
    /// `HeapObj::Fiber` slots (a fiber handle behaves as a `Fiber`
    /// instance everywhere — `fib.class`, `is_a?`, `==`, hashing —
    /// not just on the method-dispatch path). Set once after the
    /// preamble defines `Fiber`; `None` only during early boot
    /// (before any fiber can exist).
    #[cfg(feature = "_fiber")]
    pub(crate) fiber_class: Option<std::rc::Rc<crate::value::Class>>,
    /// ADR 0035 Phase 2 — JIT-inline class guard. A table of class pointers PARALLEL to
    /// `slots` (always the same length): the value `class_ptr_of` returns — the singleton
    /// class if present, else the class — as a raw `usize`, or `0` for objects that bear no
    /// dispatchable class (Array/Hash/Range/… and `Fiber` before its class is cached), whose
    /// callers fall back to the slab walk. The native JIT will read this with an inline load
    /// instead of an extern-C `class_ptr_of` call (Phase 3). Maintained at the two points
    /// that set an object's effective class — `alloc` and `ensure_singleton_class`. A swept
    /// slot's entry goes stale but is never read (`class_ptr_of` on a dead oid panics) and is
    /// overwritten on the next `alloc` into that slot.
    #[cfg(feature = "jit-native")]
    pub(crate) class_ptrs: Vec<usize>,
    /// ADR 0035 Phase 3 — the stable-addressed view the native JIT bakes at compile time and
    /// loads `class_ptrs`'s live base through at run time. Kept in step with `class_ptrs` in
    /// `alloc` (the `Vec` reallocates; this `Box`'s heap address does not).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_view: Box<crate::jit_native::JitObjView>,
}

impl Heap {
    pub(crate) fn new() -> Self {
        // `RUBYRS_GC_MIN_THRESHOLD=N` pins the floor to N and DISABLES the
        // adaptive controller (an explicit override is a statement of
        // intent — perf/RSS ratchet investigations want a fixed knob, not
        // a moving one). Read once at heap construction; the sweep path
        // no longer re-reads the environment.
        let floor_override = std::env::var("RUBYRS_GC_MIN_THRESHOLD")
            .ok()
            .and_then(|s| s.parse::<usize>().ok());
        let gc_floor = floor_override.unwrap_or(Self::GC_FLOOR_MIN);
        Heap {
            slots: vec![],
            marks: vec![],
            free: vec![],
            old: vec![],
            remembered: vec![],
            minors_since_major: 0,
            young_slots: vec![],
            live_count: 0,
            // Cold-start trigger matches the current floor so preamble
            // load + first eval see the same budget the steady-state
            // sweep settles on. Starts at the LOW floor: the boot
            // preamble allocates only a few dozen heap slots (classes
            // and methods are `Rc`, not heap objects), so a low first
            // window costs at most one cheap early sweep, while a HIGH
            // first window is a pure ~28k-slot RSS tax on any small-
            // live-set program. Load-heavy programs (`require "rubocop"`)
            // raise the floor within two sweeps — see the controller in
            // `collect`'s epilogue.
            next_gc: gc_floor,
            gc_floor,
            floor_override,
            last_sweep_us: 0,
            max_live: None,
            #[cfg(feature = "_fiber")]
            fiber_alloc_count: 0,
            #[cfg(feature = "_fiber")]
            fiber_class: None,
            #[cfg(feature = "jit-native")]
            class_ptrs: vec![],
            #[cfg(feature = "jit-native")]
            jit_view: Box::new(crate::jit_native::JitObjView {
                class_ptrs: std::ptr::null(),
                class_ptrs_len: 0,
            }),
        }
    }

    /// ADR 0035 Phase 3 — the stable address of the JIT view, baked into compiled code.
    #[cfg(feature = "jit-native")]
    pub(crate) fn jit_view_addr(&self) -> i64 {
        &*self.jit_view as *const crate::jit_native::JitObjView as i64
    }

    /// ADR 0035 Phase 2 — the class pointer `class_ptr_of` would return for `obj`, computed
    /// from the object alone (used to populate `class_ptrs` at `alloc`). `0` for objects with
    /// no dispatchable class, and for a `Fiber` whose class isn't cached yet (the caller then
    /// falls back to the slab walk, where `class_ptr_of` resolves it lazily).
    #[cfg(feature = "jit-native")]
    fn class_ptr_of_obj(&self, obj: &HeapObj) -> usize {
        match obj {
            HeapObj::Instance(i) => match &i.singleton_class {
                Some(sc) => Rc::as_ptr(sc) as usize,
                None => Rc::as_ptr(&i.class) as usize,
            },
            HeapObj::TypedData(d) => Rc::as_ptr(&d.class) as usize,
            #[cfg(feature = "_fiber")]
            HeapObj::Fiber(_) => self
                .fiber_class
                .as_ref()
                .map(|c| Rc::as_ptr(c) as usize)
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Resolve a `HeapObj::Fiber` slot's class to the cached `Fiber`
    /// class. Panics if the class wasn't cached yet — a boot-ordering
    /// bug, since no fiber handle can exist before the preamble runs.
    #[cfg(feature = "_fiber")]
    fn fiber_class(&self) -> std::rc::Rc<crate::value::Class> {
        self.fiber_class
            .clone()
            .expect("ICE: Fiber class not cached before a fiber slot was inspected")
    }
    pub(crate) fn alloc(&mut self, obj: HeapObj) -> ObjId {
        self.live_count += 1;
        #[cfg(feature = "jit-native")]
        let cls = self.class_ptr_of_obj(&obj); // before `obj` is moved into the slot
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = Slot::Live(obj);
            self.marks[i as usize] = false;
            self.old[i as usize] = false; // a freshly (re)allocated object is young
            self.young_slots.push(i);
            #[cfg(feature = "jit-native")]
            {
                self.class_ptrs[i as usize] = cls;
            }
            return ObjId(i);
        }
        let i = self.slots.len() as u32;
        self.slots.push(Slot::Live(obj));
        self.marks.push(false);
        self.old.push(false);
        self.young_slots.push(i);
        #[cfg(feature = "jit-native")]
        {
            debug_assert_eq!(self.class_ptrs.len(), i as usize);
            self.class_ptrs.push(cls);
            // The push may have reallocated `class_ptrs` — refresh the baked-address view to
            // its live base. (The free-list reuse path above writes in place, no realloc, so
            // the base is unchanged there and needs no refresh.)
            self.jit_view.class_ptrs = self.class_ptrs.as_ptr();
            self.jit_view.class_ptrs_len = self.class_ptrs.len();
        }
        ObjId(i)
    }
    pub(crate) fn get(&self, id: ObjId) -> &HeapObj {
        match &self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("ICE: use-after-free ObjId({})", id.0),
        }
    }
    /// Non-panicking liveness probe. `true` iff the slot currently
    /// holds a `Live` object. Used by side-tables keyed on `ObjId`
    /// (e.g. `Vm::binding_locals`) to prune entries for objects that
    /// have been swept — before their slot is recycled by a later
    /// `alloc`, which would otherwise alias the stale entry.
    pub(crate) fn is_live(&self, id: ObjId) -> bool {
        matches!(self.slots.get(id.0 as usize), Some(Slot::Live(_)))
    }
    pub(crate) fn get_mut(&mut self, id: ObjId) -> &mut HeapObj {
        // Generational write barrier (the single mutation chokepoint): if this
        // object is OLD, record it — the mutation about to happen may store a
        // young object into it, and the minor GC must then scan it to keep that
        // young object alive. Conservative (records even mutations that don't
        // create an old→young edge), which is always SAFE — never a missed edge.
        let idx = id.0 as usize;
        if idx < self.old.len() && self.old[idx] {
            self.remembered.push(id.0);
        }
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
            #[cfg(feature = "_fiber")]
            HeapObj::Fiber(_) => self.fiber_class(),
            _ => panic!("ICE: class_of called on non-Object slot"),
        }
    }
    /// Non-panicking `class_of` for the dispatch entry paths: a
    /// `Value::Object` can wrap a heap slot with no Ruby class
    /// behind it (`HeapObj::Fiber` — `__rubyrs_fiber_current` /
    /// `__rubyrs_fiber_new` hand these out). Dispatch falls through
    /// to the universal primitive arms (`nil?` / `==` / `to_s`)
    /// instead of ICE-ing in the user-method lookup; methods with
    /// no universal arm surface NoMethodError (decline-loudly).
    /// The receiver's class as a raw POINTER (`Rc::as_ptr`), WITHOUT cloning the
    /// `Rc<Class>` — for hot inline-cache class guards (the JIT's obj-call /
    /// getter-array / value-is-class primitives), where `try_class_of`'s refcount
    /// inc+drop per element is pure overhead (only the pointer identity is needed).
    /// `None` for a non-Object slot (same cases as `try_class_of`).
    ///
    /// jit-native-gated: every caller lives in `jit_native.rs` (the
    /// obj-call / getter-array / value-is-class PIC guards). Lift the
    /// gate if an interpreter-side consumer appears — the
    /// `not(jit-native)` slab arm below is already written for it.
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn class_ptr_of(&self, id: ObjId) -> Option<usize> {
        // ADR 0035 Phase 2 — dogfood the JIT class table: serve from it when it holds a
        // class (nonzero), falling back to the slab walk otherwise (non-class objects, and a
        // Fiber before its class is cached → table 0). The debug-assert proves the table
        // stays in sync with the slab-derived value on every live read, so Phase 3's inline
        // codegen can trust it. (A swept slot is never reached — `get` panics on a dead oid.)
        #[cfg(feature = "jit-native")]
        {
            let tbl = self.class_ptrs[id.0 as usize];
            if tbl != 0 {
                // A nonzero entry must match the slab exactly (catches a stale/wrong class).
                // A zero entry means "no cached class" — fall back to the slab, which also
                // resolves a Fiber whose class was cached only after the Fiber was allocated
                // (so a 0 entry vs a `Some` slab is intended, not a desync, and is not asserted).
                debug_assert_eq!(
                    Some(tbl),
                    self.class_ptr_of_slab(id),
                    "ADR 0035 class_ptrs desync at oid {}",
                    id.0
                );
                return Some(tbl);
            }
            return self.class_ptr_of_slab(id);
        }
        #[cfg(not(feature = "jit-native"))]
        self.class_ptr_of_slab(id)
    }
    /// Deep-copy the MUTABLE content of every live slot below
    /// `upto` whose variant supports it — the post-preamble
    /// baseline image `Runtime::reset()` restores. This closes the
    /// residual 87d2f1d1 documented and deferred: heap truncation
    /// cannot rewind IN-PLACE mutations of preamble-era objects, so
    /// user code appending to a preamble-reachable container
    /// (`Thread`'s `@coop_threads`/`@coop_runq` registries were the
    /// fuzz-soak find: `__coop_register` pushes the user's Thread
    /// object into a preamble-era Array) left dangling ObjIds into
    /// the truncated region — the next collection's mark walk then
    /// indexed `marks[]` out of bounds (heap.rs `visit_value`,
    /// "len is 40 but the index is 40").
    ///
    /// Variant coverage:
    ///   - `Instance` / `Array` / `Hash` / `Range` — deep-copied
    ///     (owned buffers; `Rc<Class>` tags shared — classes are
    ///     restored separately via `ClassStateSnapshot`).
    ///   - `BigInt` / `Rational` — SKIPPED: immutable after
    ///     construction, nothing to rewind.
    ///   - `Block` / `TypedData` / `Fiber` — SKIPPED (documented
    ///     residual): captured cells / native state can't be
    ///     deep-copied. A baseline Block whose captured cell is
    ///     mutated to hold a user object can still dangle; no
    ///     preamble installs such state today.
    pub(crate) fn capture_baseline_contents(&self, upto: usize) -> Vec<(u32, HeapObj)> {
        let mut out = Vec::new();
        for (i, slot) in self.slots.iter().enumerate().take(upto) {
            if let Slot::Live(obj) = slot
                && let Some(copy) = Self::baseline_clone(obj)
            {
                out.push((i as u32, copy));
            }
        }
        out
    }

    /// Rewind every captured baseline slot to its capture-time
    /// content (fresh deep copy per call — the saved image stays
    /// pristine across resets). Also covers two below-high-water
    /// corruption shapes the truncation can't reach: a baseline
    /// slot swept mid-eval (Dead now, but the restored bookkeeping
    /// counts it live) is re-materialised, and one recycled into a
    /// USER object is overwritten back to the baseline object.
    /// Keeps the ADR-0035 `class_ptrs` table in step per slot (a
    /// user eval may have promoted an entry to an eigenclass
    /// pointer that no longer exists after the rewind).
    pub(crate) fn restore_baseline_contents(&mut self, saved: &[(u32, HeapObj)]) {
        for (i, obj) in saved {
            let idx = *i as usize;
            let Some(fresh) = Self::baseline_clone(obj) else {
                continue;
            };
            self.slots[idx] = Slot::Live(fresh);
            #[cfg(feature = "jit-native")]
            {
                self.class_ptrs[idx] =
                    self.class_ptr_of_slab(ObjId(*i)).unwrap_or(0);
            }
        }
    }

    /// The deep-copy behind `capture_baseline_contents` /
    /// `restore_baseline_contents`; `None` = variant not covered
    /// (immutable or un-copyable — see the capture doc).
    fn baseline_clone(obj: &HeapObj) -> Option<HeapObj> {
        Some(match obj {
            HeapObj::Instance(inst) => HeapObj::Instance(crate::value::Instance {
                class: inst.class.clone(),
                ivars: inst.ivars.clone(),
                singleton_class: inst.singleton_class.clone(),
                frozen: std::cell::Cell::new(inst.frozen.get()),
            }),
            HeapObj::Array(a) => HeapObj::Array(ArrayObj {
                elems: a.elems.clone(),
                class_tag: a.class_tag.clone(),
                ivars: a.ivars.clone(),
                frozen: std::cell::Cell::new(a.frozen.get()),
            }),
            HeapObj::Hash(h) => HeapObj::Hash(h.baseline_deep_clone()),
            HeapObj::Range(r) => HeapObj::Range(r.clone()),
            _ => return None,
        })
    }

    /// The slab-walk class pointer (the pre-ADR-0035 implementation): `class_ptr_of`'s
    /// source of truth, kept as the fallback + the debug-assert oracle for the table.
    #[cfg(feature = "jit-native")]
    fn class_ptr_of_slab(&self, id: ObjId) -> Option<usize> {
        match self.get(id) {
            HeapObj::Instance(i) => Some(match &i.singleton_class {
                Some(sc) => Rc::as_ptr(sc) as usize,
                None => Rc::as_ptr(&i.class) as usize,
            }),
            HeapObj::TypedData(d) => Some(Rc::as_ptr(&d.class) as usize),
            #[cfg(feature = "_fiber")]
            HeapObj::Fiber(_) => Some(Rc::as_ptr(&self.fiber_class()) as usize),
            _ => None,
        }
    }
    pub(crate) fn try_class_of(&self, id: ObjId) -> Option<Rc<crate::value::Class>> {
        match self.get(id) {
            HeapObj::Instance(i) => Some(match &i.singleton_class {
                Some(sc) => sc.clone(),
                None => i.class.clone(),
            }),
            HeapObj::TypedData(d) => Some(d.class.clone()),
            #[cfg(feature = "_fiber")]
            HeapObj::Fiber(_) => Some(self.fiber_class()),
            _ => None,
        }
    }
    /// Original class — what `Object#class` returns to script code
    /// (CRuby skips the eigenclass when reporting). Same shape as
    /// `class_of` but doesn't substitute the singleton class.
    pub(crate) fn real_class_of(&self, id: ObjId) -> Rc<crate::value::Class> {
        match self.get(id) {
            HeapObj::Instance(i) => i.class.clone(),
            HeapObj::TypedData(d) => d.class.clone(),
            #[cfg(feature = "_fiber")]
            HeapObj::Fiber(_) => self.fiber_class(),
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
            #[cfg(feature = "_fiber")]
            Slot::Live(HeapObj::Fiber(_)) => self.fiber_class.clone(),
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
        let inst = self.instance_mut(id);
        if let Some(sc) = &inst.singleton_class {
            return sc.clone();
        }
        let original = inst.class.clone();
        let sc = Rc::new(crate::value::Class {
            name: format!("#<Class:#<{}>>", original.name),
            is_module: false,
            ivars: RefCell::new(crate::intern::FxHashMap::default()),
            methods: RefCell::new(crate::intern::FxHashMap::default()),
            // Eigenclasses have no per-class singleton-method
            // table of their own — `def self.foo` (master's
            // class-level singletons) doesn't apply to a
            // synthetic singleton class. Keep this empty so
            // dispatch sites that walk the chain don't break.
            singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
            superclass: RefCell::new(Some(original)),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            singleton_includes: RefCell::new(Vec::new()),
            singleton_view: RefCell::new(None),
            singleton_target: RefCell::new(None),
            undefed: RefCell::new(crate::intern::FxHashSet::default()),
            anon_serial: std::cell::Cell::new(0),
            ivar_shape: std::cell::RefCell::new(crate::value::IvarShape::default()),
                    class_vars: RefCell::new(crate::intern::FxHashMap::default()),
            consts: RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: RefCell::new(None),
            class_tag: None,
            frozen: std::cell::Cell::new(false),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        });
        inst.singleton_class = Some(sc.clone());
        // ADR 0035 Phase 2 — the object's effective class is now the singleton; keep the
        // JIT class table in step (else a cached PIC guard would read the stale base class).
        #[cfg(feature = "jit-native")]
        {
            self.class_ptrs[id.0 as usize] = Rc::as_ptr(&sc) as usize;
        }
        sc
    }
    pub(crate) fn instance_mut(&mut self, id: ObjId) -> &mut Instance {
        if let HeapObj::Instance(i) = self.get_mut(id) { i } else { panic!("ICE: heap slot is not an Instance") }
    }
    pub(crate) fn array(&self, id: ObjId) -> &Vec<Value> {
        if let HeapObj::Array(a) = self.get(id) { &a.elems } else { panic!("ICE: heap slot is not an Array") }
    }
    pub(crate) fn array_mut(&mut self, id: ObjId) -> &mut Vec<Value> {
        if let HeapObj::Array(a) = self.get_mut(id) { &mut a.elems } else { panic!("ICE: heap slot is not an Array") }
    }
    /// Subclass tag for an Array instance (`None` for plain arrays) —
    /// the Array twin of `hash_class_tag`.
    pub(crate) fn array_class_tag(&self, id: ObjId) -> Option<Rc<Class>> {
        if let HeapObj::Array(a) = self.get(id) { a.class_tag.clone() } else { None }
    }
    /// CRuby's frozen bit for an Array. `freeze` sets it; every
    /// mutating method then raises FrozenError.
    pub(crate) fn array_frozen(&self, id: ObjId) -> bool {
        if let HeapObj::Array(a) = self.get(id) { a.frozen.get() } else { false }
    }
    pub(crate) fn set_array_frozen(&self, id: ObjId) {
        if let HeapObj::Array(a) = self.get(id) { a.frozen.set(true); }
    }
    /// CRuby's frozen bit for a Hash (the Hash twin of `array_frozen`).
    pub(crate) fn hash_frozen(&self, id: ObjId) -> bool {
        if let HeapObj::Hash(h) = self.get(id) { h.frozen.get() } else { false }
    }
    pub(crate) fn set_hash_frozen(&self, id: ObjId) {
        if let HeapObj::Hash(h) = self.get(id) { h.frozen.set(true); }
    }
    pub(crate) fn hash(&self, id: ObjId) -> &[(Value, Value)] {
        if let HeapObj::Hash(h) = self.get(id) { &h.pairs } else { panic!("ICE: heap slot is not a Hash") }
    }
    pub(crate) fn hash_mut(&mut self, id: ObjId) -> &mut PairsBuf {
        // A caller taking `&mut pairs` may insert / delete / reorder
        // entries the index can't track, so invalidate it — the next
        // indexed lookup rebuilds it lazily. Single-key fast paths use
        // `hash_insert` / `hash_delete` instead, which keep the index
        // live (so building a Hash stays O(1) per key, not O(n²)).
        if let HeapObj::Hash(h) = self.get_mut(id) {
            h.clear_indexes();
            &mut h.pairs
        } else {
            panic!("ICE: heap slot is not a Hash")
        }
    }
    pub(crate) fn hash_obj_mut(&mut self, id: ObjId) -> &mut HashObj {
        if let HeapObj::Hash(h) = self.get_mut(id) { h } else { panic!("ICE: heap slot is not a Hash") }
    }
    /// Build the key index (`ruby_hash(key)` → positions) for a Hash
    /// large enough to benefit. Below `HASH_INDEX_MIN` entries the index
    /// is left `None` and callers fall back to a linear `ruby_eql` scan:
    /// for a handful of keys that scan beats allocating and probing a
    /// HashMap (most Jekyll hashes are this small, so building the index
    /// eagerly was a net allocation regression), and it stays
    /// deterministic — a cached content-hash index silently, and
    /// order-dependently, misses a key mutated in place, whereas the
    /// linear scan always compares live key content.
    fn ensure_hash_index(&mut self, id: ObjId) {
        if let HeapObj::Hash(h) = self.get(id) {
            if h.index().is_some() { return; }
        } else {
            panic!("ICE: heap slot is not a Hash");
        }
        const HASH_INDEX_MIN: usize = 16;
        let n = self.hash(id).len();
        if n < HASH_INDEX_MIN {
            return;
        }
        let mut map = HashIndex::with_capacity_and_hasher(n, Default::default());
        for i in 0..n {
            let kh = self.hash(id)[i].0.ruby_hash(self);
            map.entry(kh).or_default().push(i as u32);
        }
        self.hash_obj_mut(id).extras_mut().index = Some(map);
    }
    /// O(1)-amortised position of `key` in the Hash, or `None`. Uses the
    /// key index for large Hashes; small ones (no index) fall back to a
    /// linear `ruby_eql` scan — the old behaviour, restored below the
    /// `ensure_hash_index` threshold.
    pub(crate) fn hash_index_lookup(&mut self, id: ObjId, key: &Value) -> Option<usize> {
        self.ensure_hash_index(id);
        let kh = key.ruby_hash(self);
        if let HeapObj::Hash(h) = self.get(id) {
            match h.index() {
                Some(m) => {
                    if let Some(cands) = m.get(&kh) {
                        for &i in cands {
                            if h.pairs[i as usize].0.ruby_eql(key, self) {
                                return Some(i as usize);
                            }
                        }
                    }
                }
                None => {
                    for i in 0..h.pairs.len() {
                        if h.pairs[i].0.ruby_eql(key, self) {
                            return Some(i);
                        }
                    }
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
            match h.index() {
                Some(m) => m.get(&kh).and_then(|cands| {
                    cands
                        .iter()
                        .copied()
                        .find(|&i| h.pairs[i as usize].0.ruby_eql(&key, self))
                        .map(|i| i as usize)
                }),
                // Small hash (no index): linear ruby_eql scan.
                None => (0..h.pairs.len()).find(|&i| h.pairs[i].0.ruby_eql(&key, self)),
            }
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
                if let Some(m) = h.extras.as_deref_mut().and_then(|e| e.index.as_mut()) {
                    m.entry(kh).or_default().push(new_i);
                }
                None
            }
        }
    }
    /// Append a pair the caller has already established is NOT present
    /// (the push arm of `hash_insert`, for VM callers that did their own
    /// lookup — `Vm::vm_hash_append`). Keeps the identity index live;
    /// append never shifts existing positions.
    pub(crate) fn hash_append_new(&mut self, id: ObjId, key: Value, val: Value) {
        let kh = key.ruby_hash(self);
        if let HeapObj::Hash(h) = self.get_mut(id) {
            let new_i = h.pairs.len() as u32;
            h.pairs.push((key, val));
            if let Some(m) = h.extras.as_deref_mut().and_then(|e| e.index.as_mut()) {
                m.entry(kh).or_default().push(new_i);
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
        // positions shifted — both indexes invalidated
        h.clear_indexes();
        Some(v)
    }
    /// The user Hash-subclass this Hash is an instance of, if any
    /// (`class M < Hash; end; M.new` → `Some(M)`). `None` for plain
    /// `{}` / `Hash.new`.
    pub(crate) fn hash_class_tag(&self, id: ObjId) -> Option<Rc<Class>> {
        if let HeapObj::Hash(h) = self.get(id) { h.class_tag().cloned() } else { None }
    }
    /// Stamp a subclass tag onto a Hash — used by non-mutating builders
    /// (`merge`, `select`, …) so the result keeps the receiver's class
    /// (CRuby: `IndifferentHash.new.merge(x).class == IndifferentHash`).
    pub(crate) fn hash_set_class_tag(&mut self, id: ObjId, tag: Option<Rc<Class>>) {
        if let HeapObj::Hash(h) = self.get_mut(id)
            && (tag.is_some() || h.extras().is_some())
        {
            h.extras_mut().class_tag = tag;
        }
    }
    /// Read `@name` ivar off a (subclass) Hash; `None` if unset.
    /// Array twin of `hash_ivar_get` / `hash_ivar_set`.
    pub(crate) fn array_ivar_get(&self, id: ObjId, name: crate::intern::SymId) -> Option<Value> {
        if let HeapObj::Array(a) = self.get(id) { a.ivars.get(&name).cloned() } else { None }
    }
    pub(crate) fn array_ivar_set(&mut self, id: ObjId, name: crate::intern::SymId, v: Value) {
        if let HeapObj::Array(a) = self.get_mut(id) {
            a.ivars.insert(name, v);
        }
    }
    pub(crate) fn hash_ivar_get(&self, id: ObjId, name: crate::intern::SymId) -> Option<Value> {
        if let HeapObj::Hash(h) = self.get(id) {
            h.ivars().and_then(|iv| iv.get(&name).cloned())
        } else { None }
    }
    /// Set `@name` ivar on a (subclass) Hash.
    pub(crate) fn hash_ivar_set(&mut self, id: ObjId, name: crate::intern::SymId, v: Value) {
        if let HeapObj::Hash(h) = self.get_mut(id) { h.extras_mut().ivars.insert(name, v); }
    }
    /// Clone a (subclass) Hash's full ivar table — used by dup/clone.
    /// Array twin of `hash_ivars_clone`.
    pub(crate) fn array_ivars_clone(&self, id: ObjId) -> crate::intern::FxHashMap<crate::intern::SymId, Value> {
        if let HeapObj::Array(a) = self.get(id) { a.ivars.clone() } else { crate::intern::FxHashMap::default() }
    }
    pub(crate) fn hash_ivars_clone(&self, id: ObjId) -> crate::intern::FxHashMap<crate::intern::SymId, Value> {
        if let HeapObj::Hash(h) = self.get(id) {
            h.ivars().cloned().unwrap_or_default()
        } else { crate::intern::FxHashMap::default() }
    }
    /// Delete `@name` off a (subclass or reflection-carrying) Array,
    /// returning the removed value (`None` when unset) — the
    /// `Object#remove_instance_variable` backend for Array values.
    pub(crate) fn array_ivar_remove(&mut self, id: ObjId, name: crate::intern::SymId) -> Option<Value> {
        if let HeapObj::Array(a) = self.get_mut(id) { a.ivars.remove(&name) } else { None }
    }
    /// Hash twin of `array_ivar_remove`.
    pub(crate) fn hash_ivar_remove(&mut self, id: ObjId, name: crate::intern::SymId) -> Option<Value> {
        if let HeapObj::Hash(h) = self.get_mut(id) {
            match h.extras.as_deref_mut() {
                Some(e) => e.ivars.remove(&name),
                None => None,
            }
        } else { None }
    }
    /// Default-value block stored alongside the Hash by `Hash.new {
    /// |h, k| ... }`. None for hash literals (`{}`) and the common
    /// `Hash.new` no-arg form. `Hash#[]` checks this slot when the
    /// key is missing — if present, invokes the block with `(self,
    /// key)` and returns the result. Mirrors CRuby's `default_proc`
    /// semantics, narrowed to the common shape (no static default
    /// value yet, no `default=` assignment — both are deferred gaps).
    pub(crate) fn hash_default_block(&self, id: ObjId) -> Option<ObjId> {
        if let HeapObj::Hash(h) = self.get(id) { h.default_block() }
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
        if let HeapObj::Hash(h) = self.get_mut(id) {
            if block.is_some() || h.extras().is_some() {
                h.extras_mut().default_block = block;
            }
        }
        else { panic!("ICE: heap slot is not a Hash (hash_set_default_block)") }
    }
    /// Scalar default — set by `Hash.new(default)`. Returned as-is
    /// on missing-key lookup. Cloned on read to avoid sharing a
    /// `&Value` into a method that's about to mutate the heap.
    /// Panics on type mismatch, consistent with `hash()`.
    pub(crate) fn hash_default_value(&self, id: ObjId) -> Option<Value> {
        if let HeapObj::Hash(h) = self.get(id) { h.default_value().cloned() }
        else { panic!("ICE: heap slot is not a Hash (hash_default_value)") }
    }
    pub(crate) fn hash_set_default_value(&mut self, id: ObjId, value: Option<Value>) {
        if let HeapObj::Hash(h) = self.get_mut(id) {
            if value.is_some() || h.extras().is_some() {
                h.extras_mut().default_value = value;
            }
        }
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
        self.alloc(HeapObj::Fiber(Box::new(crate::vm::fiber::FiberObject::new(body_block))))
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
    pub(crate) fn block_mut(&mut self, id: ObjId) -> &mut BlockHandle {
        if let HeapObj::Block(b) = self.get_mut(id) { b } else { panic!("ICE: heap slot is not a Block") }
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

    /// Minor collections between majors (see `collect`'s
    /// minor/major split). Hoisted from a `collect`-local const so
    /// `schedule_major` can reference it.
    pub(crate) const MAJOR_EVERY: u32 = 8;

    /// Bounds of the adaptive GC floor (the min window in
    /// `next_gc = (live * growth).max(floor)`). MIN is the historical
    /// 4096 default (small-live-set churn stays at a ~4k-slot heap);
    /// MAX is the 32768 the RuboCop require investigation landed on
    /// (a3073e5f) — kept as the ceiling so the adaptive floor can never
    /// exceed what that campaign budgeted for.
    pub(crate) const GC_FLOOR_MIN: usize = 4096;
    pub(crate) const GC_FLOOR_MAX: usize = 32768;
    /// The controller's gain: one µs of measured sweep cost buys this
    /// many allocations of trigger window. Calibrated on the two
    /// extremes (2026-07 probe, see collect's epilogue comment):
    /// gc_churn's ~65µs sweeps × 16 = ~1k → clamps to GC_FLOOR_MIN;
    /// post-`require "rubocop"` sweeps ≥ ~2000µs × 16 → clamps to
    /// GC_FLOOR_MAX. Equivalent to targeting ~5% GC time on a mutator
    /// that allocates every ~1.3µs (the measured require-phase rate).
    pub(crate) const GC_FLOOR_ALLOCS_PER_SWEEP_US: u64 = 16;

    /// The cost-proportional floor for a sweep that took `cost_us`:
    /// window ∝ sweep cost, clamped to [GC_FLOOR_MIN, GC_FLOOR_MAX].
    /// Pure so the unit tests can pin the curve without timing games.
    pub(crate) fn adaptive_floor(cost_us: u64) -> usize {
        let target = cost_us
            .saturating_mul(Self::GC_FLOOR_ALLOCS_PER_SWEEP_US)
            // Clamp in u64 BEFORE the usize cast: on 32-bit targets
            // (wasm32-wasip1) a multi-minute sweep would truncate —
            // direction-safe (decays to MIN) but the exact form
            // costs nothing.
            .min(Self::GC_FLOOR_MAX as u64);
        (target as usize).clamp(Self::GC_FLOOR_MIN, Self::GC_FLOOR_MAX)
    }

    /// Make the NEXT collection take the major (full-heap) path.
    /// Used by `Runtime`'s post-preamble snapshot capture to force
    /// a garbage-free baseline (see `Vm::gc_now`).
    pub(crate) fn schedule_major(&mut self) {
        self.minors_since_major = Self::MAJOR_EVERY - 1;
    }

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
    /// `t0` is the collection's TRUE start — callers take it BEFORE
    /// gathering roots (`Vm::gc_now`'s gather walks every class table
    /// and is a large share of a loaded program's per-collection fixed
    /// cost). The adaptive-floor controller in the epilogue sizes the
    /// trigger window from `t0.elapsed()`; timing only the mark+sweep
    /// underestimated a post-`require "rubocop"` collection ~2× and
    /// left the controller oscillating below the top clamp.
    pub(crate) fn collect(
        &mut self,
        roots: &[Value],
        t0: std::time::Instant,
    ) -> Vec<(unsafe extern "C" fn(*mut std::ffi::c_void), *mut std::ffi::c_void)> {
        // `RUBYRS_GC_STATS=1` probe state — captured up-front so the
        // per-sweep stderr line (printed in the epilogue, after the
        // next_gc calculation) can report the sweep's inputs: what
        // the trigger saw (`live_before`), how much was allocated
        // since the last collection (`young`), and wall time.
        let stats = std::env::var_os("RUBYRS_GC_STATS").is_some();
        let stats_live_before = self.live_count;
        let stats_young_alloc = self.young_slots.len();
        // Generational GC step 2: MINOR vs MAJOR. A minor pre-marks every OLD
        // object live (so `visit_value`'s "already marked → skip" naturally
        // avoids re-walking the stable old graph) and resets only YOUNG marks;
        // it then force-walks the `remembered` old objects (the only ones that
        // can hold a young object) to keep their young children alive. A major
        // (every `MAJOR_EVERY` collections, or while there is no old gen yet)
        // resets ALL marks and walks the whole heap, reclaiming old garbage.
        let minor = self.minors_since_major + 1 < Self::MAJOR_EVERY;
        if minor {
            self.minors_since_major += 1;
            // Reset ONLY the young region's marks (O(young)); old objects RETAIN
            // their mark (live) from the last collection — that retention is
            // exactly what lets `visit_value` skip re-walking the old graph
            // (an already-marked object is not re-queued). The first-ever
            // collection has every slot young, so this degenerates to a full
            // reset, matching a major.
            for &yi in &self.young_slots {
                self.marks[yi as usize] = false;
            }
        } else {
            self.minors_since_major = 0;
            for m in self.marks.iter_mut() { *m = false; }
        }
        let mut worklist: Vec<ObjId> = Vec::new();
        // Classes whose ivar/method graph has already been walked via an
        // instance's `class` field this cycle — visit each at most once
        // regardless of instance count (named classes are also covered
        // by the Vm root scan; this set bounds the redundant re-walk).
        let mut seen_inst_classes: crate::intern::FxHashSet<*const Class> =
            crate::intern::FxHashSet::default();
        // Locals-CELL content scans visited this cycle (Block handle
        // captures / closure captures / fiber-snapshot frames). Cells
        // are shared heavily (every handle minted from one loop shares
        // its scope cells; ~40 rubocop-ast `*_type?` closures share one
        // class-body cell) -- dedup bounds the scan to once per cell per
        // cycle. Sound: no Ruby code runs mid-collection, so contents
        // are stable.
        let mut seen_cells: crate::intern::FxHashSet<usize> =
            crate::intern::FxHashSet::default();
        for v in roots { Heap::visit_value(v, &mut self.marks, &mut worklist); }
        // Minor: force-walk the remembered old objects so a young object held
        // ONLY by an old one (via a post-promotion mutation) is still marked.
        // They're pre-marked live, so push their ids directly to walk children.
        if minor {
            self.remembered.sort_unstable();
            self.remembered.dedup();
            for &rid in &self.remembered {
                worklist.push(ObjId(rid));
            }
        }
        // Mark phase: iterate each greyed object's children in place.
        // The previous impl `let children: Vec<Value> = ...clone()` per
        // pop step turned every mark visit into a full copy of the
        // container's contents — quadratic on a heap that's mostly one
        // large Array. Split-borrow `self.slots` (read) vs `self.marks`
        // (write) on disjoint fields lets us walk references directly.
        while let Some(id) = worklist.pop() {
            match &self.slots[id.0 as usize] {
                Slot::Live(HeapObj::Instance(inst)) => {
                    for v in inst.ivars.values_raw() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Mark the instance's CLASS graph. An ANONYMOUS class
                    // (`Struct.new(:x).new(...)`, a `Class.new` instance)
                    // is reachable ONLY through its instances — the Vm
                    // root scan iterates `Vm.classes` (named only), so
                    // such a class's heap-backed ivars (e.g. a Struct's
                    // `@__struct_attrs` members Array) were swept while
                    // the instance still consulted them on every method
                    // call — a use-after-free at ObjId-reuse time
                    // (`Struct.new(:x).new(v)` under GC pressure; the
                    // `Value::Class` arm follows the superclass chain so
                    // `class Foo < Struct.new(...)` is covered too). Dedup
                    // per cycle so N instances of one class walk it once.
                    let cls = inst.class.clone();
                    if seen_inst_classes.insert(Rc::as_ptr(&cls)) {
                        Heap::visit_value(&crate::value::Value::Class(cls), &mut self.marks, &mut worklist);
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
                                cl.each_capture_cell(|cell| {
                                    if seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize) {
                                        for v in cell.borrow().iter() {
                                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                                        }
                                    }
                                });
                                // A closure method that captures an enclosing
                                // `yield` block (`define_singleton_method(:m) {
                                // yield }` — the on_teardown idiom) holds that
                                // block ONLY via captured_yield_block. Without
                                // tracing it, the block is swept once the
                                // defining scope returns, and the singleton
                                // method later yields to a dead slot
                                // (use-after-free). The Vm root-walker traces
                                // it for `Vm.classes` methods (gc.rs); instance
                                // eigenclasses need it here.
                                if let Some(b) = cl.captured_yield_block {
                                    Heap::visit_value(&crate::value::Value::Block(b), &mut self.marks, &mut worklist);
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
                    for v in &a.elems {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Subclass instance variables hold Values too;
                    // empty (no iteration) for plain arrays.
                    for v in a.ivars.values() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                Slot::Live(HeapObj::Hash(h)) => {
                    for (k, v) in &h.pairs {
                        Heap::visit_value(k, &mut self.marks, &mut worklist);
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Cold tail: absent on record-shaped hashes, so
                    // the common case skips this whole block.
                    let Some(ex) = h.extras() else { continue };
                    // Default-block is a heap-managed Block; without
                    // a mark walk it would be swept while the Hash
                    // still references it via `Hash.new { ... }`.
                    if let Some(blk_id) = ex.default_block
                        && !self.marks[blk_id.0 as usize]
                    {
                        self.marks[blk_id.0 as usize] = true;
                        worklist.push(blk_id);
                    }
                    // Scalar default — set by `Hash.new(default)`.
                    // May itself reference the heap (e.g. a default
                    // String or Array); walk via the usual Value
                    // visitor.
                    if let Some(v) = &ex.default_value {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Hash-subclass instance variables.
                    for v in ex.ivars.values() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    // Per-instance eigenclass (def h.x): closure
                    // captures + ivars, same walk as the Instance
                    // arm above — this eigenclass is reachable
                    // ONLY through the Hash slot.
                    if let Some(sc) = &ex.singleton_class {
                        for m in sc.methods.borrow().values() {
                            if let Some(cl) = &m.closure {
                                cl.each_capture_cell(|cell| {
                                    if seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize) {
                                        for v in cell.borrow().iter() {
                                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                                        }
                                    }
                                });
                            }
                        }
                        for v in sc.ivars.borrow().values() {
                            Heap::visit_value(v, &mut self.marks, &mut worklist);
                        }
                    }
                }
                Slot::Live(HeapObj::Range(r)) => {
                    Heap::visit_value(&r.begin, &mut self.marks, &mut worklist);
                    Heap::visit_value(&r.end, &mut self.marks, &mut worklist);
                }
                Slot::Live(HeapObj::Block(bh)) => {
                    // Walk captured locals (shared Rc<RefCell> with
                    // any frame currently executing this block, but
                    // immutably borrowed only here) — `captured` PLUS
                    // every outer-chain cell (an ORIGINAL binding
                    // cell whose defining frame popped may be
                    // reachable only through this handle) — and the
                    // block's `self_val`. The visit_value calls do
                    // not recurse — they mark + worklist-push only —
                    // so each RefCell borrow stays scoped to one
                    // `each_capture_cell` callback and can't conflict
                    // with itself.
                    bh.each_capture_cell(|cell| {
                        if seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize) {
                            for v in cell.borrow().iter() {
                                Heap::visit_value(v, &mut self.marks, &mut worklist);
                            }
                        }
                    });
                    Heap::visit_value(&bh.self_val, &mut self.marks, &mut worklist);
                    // The yield-block this block forwards to (escaped
                    // closure case): nothing else roots it once the
                    // defining method has returned, so mark it here.
                    if let Some(yb) = bh.captured_yield_block {
                        Heap::visit_value(&Value::Block(yb), &mut self.marks, &mut worklist);
                    }
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
                        cl.each_capture_cell(|cell| {
                            if seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize) {
                                for v in cell.borrow().iter() {
                                    Heap::visit_value(v, &mut self.marks, &mut worklist);
                                }
                            }
                        });
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
                        cl.each_capture_cell(|cell| {
                            if seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize) {
                                for v in cell.borrow().iter() {
                                    Heap::visit_value(v, &mut self.marks, &mut worklist);
                                }
                            }
                        });
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
                    // `Locals::Stack` frame slots live in the
                    // snapshot's swapped-out arena.
                    for v in snap.locals_arena.iter() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    for frame in &snap.frames {
                        if let Some(rc) = frame.locals.as_shared()
                            && seen_cells.insert(std::rc::Rc::as_ptr(rc) as usize)
                        {
                            let locals = rc.borrow();
                            for v in locals.iter() {
                                Heap::visit_value(v, &mut self.marks, &mut worklist);
                            }
                        }
                        // Capture-routing cells — a suspended fiber's
                        // block frame may hold the only path to an
                        // ORIGINAL binding cell (mirrors the live-
                        // frame root walk in vm/gc.rs).
                        if let Some(cell) = &frame.outer_cell
                            && seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize)
                        {
                            for v in cell.borrow().iter() {
                                Heap::visit_value(v, &mut self.marks, &mut worklist);
                            }
                        }
                        if let Some(chain) = &frame.outer_rest {
                            for (cell, _) in chain.iter() {
                                if seen_cells.insert(std::rc::Rc::as_ptr(cell) as usize) {
                                    for v in cell.borrow().iter() {
                                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                                    }
                                }
                            }
                        }
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
        let mut pending_frees: Vec<(unsafe extern "C" fn(*mut std::ffi::c_void), *mut std::ffi::c_void)> =
            Vec::new();
        if minor {
            // MINOR sweep: scan ONLY the young region (O(young)). Old objects are
            // untouched — their retained mark keeps them live; any old garbage
            // waits for the next major. Each young slot is `Live` (nothing frees
            // between collections), so it is either promoted or reclaimed here.
            let mut young_freed = 0usize;
            for k in 0..self.young_slots.len() {
                let i = self.young_slots[k] as usize;
                if let Slot::Live(_) = &self.slots[i] {
                    if self.marks[i] {
                        self.old[i] = true; // young survivor → promoted to old
                    } else {
                        if let Slot::Live(HeapObj::TypedData(d)) = &self.slots[i]
                            && let Some(f) = d.dfree {
                                pending_frees.push((f, d.data_ptr));
                            }
                        self.slots[i] = Slot::Dead;
                        self.free.push(i as u32);
                        self.old[i] = false;
                        young_freed += 1;
                    }
                }
            }
            self.live_count -= young_freed;
        } else {
            // MAJOR sweep: full scan, recomputing the live set and reclaiming
            // old garbage that minors never touch.
            let mut live = 0usize;
            for i in 0..self.slots.len() {
                match &self.slots[i] {
                    Slot::Live(_) => {
                        if self.marks[i] {
                            live += 1;
                            self.old[i] = true; // a survivor is promoted to old
                        } else {
                            if let Slot::Live(HeapObj::TypedData(d)) = &self.slots[i]
                                && let Some(f) = d.dfree {
                                    pending_frees.push((f, d.data_ptr));
                                }
                            self.slots[i] = Slot::Dead;
                            self.free.push(i as u32);
                            self.old[i] = false;
                        }
                    }
                    Slot::Dead => {}
                }
            }
            self.live_count = live;
        }
        let live = self.live_count;
        // Probe: exact young-survivor count (marks are final and young_slots
        // still populated here; a surviving young slot's mark is true, a
        // freed one's is false — for minors AND majors). Gated on the env
        // knob so the normal sweep path pays nothing.
        let stats_young_surv = if stats {
            self.young_slots.iter().filter(|&&i| self.marks[i as usize]).count()
        } else {
            0
        };
        // The young region has been consumed (survivors promoted, rest freed) and
        // the remembered old→young edges honoured by this collection. Reset both.
        self.young_slots.clear();
        self.remembered.clear();
        // Post-sweep trigger threshold. History: originally
        // `live * 2 max 1024` (sweeps every ~1k allocs — punishes
        // alloc-and-discard loops); bumped to `live * 4 max 4096`
        // when json_bench round_trip showed 27 % GC overhead
        // (44 µs/iter → 40, matching Oj). Re-measured 2026-06-10
        // on the current binary: growth 4 vs 2 is NOISE-level on
        // both json round_trip (7446 vs 7469 µs/iter, 0.3 %) and
        // mm_bench (0.62-0.63 s both) — the workloads grew and
        // per-sweep cost shrank, diluting the old 4× rationale.
        // Meanwhile growth=4 lets garbage pile to 4× the live set
        // between sweeps: on the jekyll liquid-1k build that's
        // +4.7MB peak RSS (90.1 → 85.4MB at growth=2) for zero
        // wall benefit. So the default is `live * 2 max FLOOR`.
        // growth=1 is NOT viable — every post-sweep allocation
        // immediately re-crosses the threshold and the build
        // degenerates to O(n²) sweeping (41 s vs 0.84 s).
        //
        // Both knobs stay env-tunable (`RUBYRS_GC_GROWTH`,
        // `RUBYRS_GC_MIN_THRESHOLD`) for perf/RSS ratchet
        // investigations; setting the MIN_THRESHOLD one pins the
        // floor and disables the adaptive controller below.
        let growth = std::env::var("RUBYRS_GC_GROWTH")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(2);
        // FLOOR history: a static 4096 punished `require "rubocop"` with a
        // collection storm (a3073e5f measured require −28% at 32768); the
        // static 32768 that fixed it taxed every small-live-set churn
        // program ~+6MB peak RSS (gc_churn 11.7 → 17.9MB — the window is
        // pure garbage held between sweeps, ~250B/slot of malloc'd
        // content). The 2026-07 RUBYRS_GC_STATS probe showed BOTH
        // workloads are low-survival churn over a small live set — what
        // separates them is PER-SWEEP COST: a require-phase sweep costs
        // ~1.1ms at the same young size where gc_churn's costs 65µs (17×),
        // because mark/root traversal scales with loaded program
        // complexity (class graphs, constants, cells), not heap size. So
        // survival/live-count heuristics are the wrong denominator; the
        // window that amortises an expensive sweep is proportional to the
        // sweep's own cost. Controller: floor = clamp(K × min(this sweep,
        // last sweep) µs); the min() makes a raise require two consecutive
        // expensive sweeps (a lone scheduler blip cannot buy a 28k-slot
        // RSS window) while one cheap sweep decays it immediately.
        // Measured fixed points (fast x86/arm64, 2026-07): gc_churn ⇒
        // GC_FLOOR_MIN clamp, post-rubocop-require ⇒ GC_FLOOR_MAX clamp.
        // HONEST STABILITY MARGIN (adversarial verify, 2026-07-05):
        // churn sweep cost is ~85% WINDOW-PROPORTIONAL (~22.5ns/slot
        // default build), so the update W ← K·c(W) has loop gain
        // K·a ≈ 0.36 — the MIN fixed point stays stable only while
        // the machine is within ~2.8× (default) / ~4.6× (mimalloc) of
        // the calibration box; slower machines (Pi-class) or
        // heavy-object churn (many strings per iteration) converge to
        // the MAX clamp instead, i.e. the RSS win is shape- and
        // speed-dependent. BOUNDED WORST CASE: pinning at MAX is
        // exactly the old static-32768 behaviour — never worse than
        // the pre-adaptive heap, and RUBYRS_GC_MIN_THRESHOLD remains
        // the manual override for such hosts.
        // STRESS_GC forces collection on every alloc regardless of
        // `next_gc`, so the controller is semantically inert under it.
        let sweep_us = t0.elapsed().as_micros() as u64;
        let min_threshold = if let Some(f) = self.floor_override {
            f
        } else {
            self.gc_floor = Self::adaptive_floor(sweep_us.min(self.last_sweep_us));
            self.gc_floor
        };
        self.last_sweep_us = sweep_us;
        self.next_gc = (live * growth).max(min_threshold);
        // `RUBYRS_GC_STATS=1`: per-sweep shape line on stderr (debug knob
        // in the `RUBYRS_IC_STATS` shape). Historical use: RSS attribution
        // on the jekyll liquid-1k build (slot array small, gap lived in
        // malloc'd content → lazy-regex fix). Enriched 2026-07 for the GC
        // floor-decay investigation: sweep kind, trigger-time live, the
        // window's allocation count + survivors, and sweep wall time —
        // enough to reconstruct WHERE a floor change adds/removes sweeps
        // and what each cost.
        if stats {
            eprintln!(
                "gc_stats: kind={} live_before={} live={} young={} young_surv={} next_gc={} floor={} us={} slots={} cap={} free={}",
                if minor { "minor" } else { "major" },
                stats_live_before,
                live,
                stats_young_alloc,
                stats_young_surv,
                self.next_gc,
                self.gc_floor,
                sweep_us,
                self.slots.len(),
                self.slots.capacity(),
                self.free.len()
            );
        }
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
            // A class is `Rc<Class>` and owns no GC slot of its own, but
            // its instance variables hold real heap Values — e.g. an
            // anonymous `Struct`'s `@__struct_attrs` Array. The root scan
            // (vm/gc.rs) descends into classes bound to a constant,
            // global, or `self.classes`, but a class reachable ONLY
            // through a container (an Array/Hash constant, an instance or
            // class ivar, a local, a frame slot) was never reached here,
            // so its ivars were swept mid-use — a use-after-free. Walk
            // the class graph iteratively (classes can hold classes; the
            // `seen` set guards cycles and keeps this non-recursive) and
            // mark each heap-backed ivar. Found by /code-review; the
            // constant/global root scan fixed only the direct-binding
            // case. (Matches that scan's ivars-only scope.)
            Value::Class(c) => {
                let mut stack: Vec<Rc<Class>> = vec![c.clone()];
                let mut seen: Vec<*const Class> = vec![Rc::as_ptr(c)];
                // Shared visit for every Value reachable from a
                // class's tables: nested classes feed the cycle-
                // guarded stack, everything else goes through the
                // normal heap visitor.
                let mut touch = |v: &Value, stack: &mut Vec<Rc<Class>>, seen: &mut Vec<*const Class>| {
                    if let Value::Class(d) = v {
                        let p = Rc::as_ptr(d);
                        if !seen.contains(&p) {
                            seen.push(p);
                            stack.push(d.clone());
                        }
                    } else {
                        Heap::visit_value(v, marks, worklist);
                    }
                };
                while let Some(cls) = stack.pop() {
                    for iv in cls.ivars.borrow().values() {
                        touch(iv, &mut stack, &mut seen);
                    }
                    // Closure-method captures (define_method &block)
                    // hold heap Values — for an ANONYMOUS class
                    // (e.g. minitest's describe-generated Spec
                    // subclasses, reachable only through the
                    // @@runnables registry) this arm is the ONLY
                    // mark path; the Vm root walk that handles
                    // registered classes iterates `Vm.classes` and
                    // never sees them. Without this, a `before`/`it`
                    // block captured by a define_method'd setup was
                    // swept and instance_exec'd post-free (minitest
                    // spec suite ICE under normal GC pressure).
                    for m in cls.methods.borrow().values() {
                        if let Some(cl) = &m.closure {
                            cl.each_capture_cell(|cell| {
                                for v in cell.borrow().iter() {
                                    touch(v, &mut stack, &mut seen);
                                }
                            });
                        }
                    }
                    for m in cls.singleton_methods.borrow().values() {
                        if let Some(cl) = &m.closure {
                            cl.each_capture_cell(|cell| {
                                for v in cell.borrow().iter() {
                                    touch(v, &mut stack, &mut seen);
                                }
                            });
                        }
                    }
                    // Class variables + per-class consts can hold
                    // heap Values on anonymous classes too (the
                    // registered-class walk covers named ones).
                    for v in cls.class_vars.borrow().values() {
                        touch(v, &mut stack, &mut seen);
                    }
                    for v in cls.consts.borrow().values() {
                        touch(v, &mut stack, &mut seen);
                    }
                    // Descend the SUPERCLASS chain: a named class can
                    // inherit from an anonymous generated one whose
                    // tables nobody else reaches — rack's
                    // `class MimePart < Struct.new(:body, ...)` keeps
                    // `@__struct_attrs` (the members Array) as an ivar
                    // on the anonymous parent. Without this hop the
                    // Array is swept while every MimePart instance
                    // still consults it (use-after-free at
                    // ObjId-reuse time; rack's multipart suite was
                    // the reproducer).
                    if let Some(sup) = cls.superclass.borrow().clone() {
                        let p = Rc::as_ptr(&sup);
                        if !seen.contains(&p) {
                            seen.push(p);
                            stack.push(sup);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Byte-level inspect escape for BINARY-tagged strings: CRuby
/// renders every non-ASCII byte as `\xNN` (even bytes that would
/// form valid UTF-8), while ASCII bytes get the same short-escape
/// treatment as the char path. E1 slice 3.
/// Inspect escape for REGISTRY-tagged strings: CRuby renders each
/// valid character of a non-UTF-8 multi-byte encoding as
/// `\x{HEXBYTES}` (single-byte encodings keep the plain `\xNN`
/// form), ASCII printables verbatim. Broken sequences fall back to
/// per-byte escapes via the caller.
#[cfg(feature = "_encoding_full")]
pub(crate) fn inspect_escape_chunks_into(chunks: &[Vec<u8>], out: &mut String) {
    for chunk in chunks {
        if chunk.len() == 1 {
            inspect_escape_bytes_into(chunk, out);
        } else {
            use std::fmt::Write as _;
            out.push_str("\\x{");
            for b in chunk {
                let _ = write!(out, "{b:02X}");
            }
            out.push('}');
        }
    }
}

pub(crate) fn inspect_escape_bytes_into(bytes: &[u8], out: &mut String) {
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x09 => out.push_str("\\t"),
            0x0A => out.push_str("\\n"),
            0x0B => out.push_str("\\v"),
            0x0C => out.push_str("\\f"),
            0x0D => out.push_str("\\r"),
            0x1B => out.push_str("\\e"),
            0x20..=0x7E => out.push(b as char),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\x{b:02X}");
            }
        }
    }
}
/// Escape a UTF-8-TAGGED byte buffer that may contain invalid
/// sequences: valid runs take the normal char escapes, invalid
/// bytes render as `\xNN` — CRuby's `"\xB6".inspect` shape.
/// (The lossy-decode path replaced them with U+FFFD, which broke
/// minitest's mu_pp encoding header comparisons for bad-UTF-8
/// fixtures.)
pub(crate) fn inspect_escape_utf8_bytes_into(bytes: &[u8], out: &mut String) {
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                inspect_escape_into(valid, out);
                break;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                // SAFETY: from_utf8 just validated this prefix.
                inspect_escape_into(unsafe { std::str::from_utf8_unchecked(valid) }, out);
                let bad_len = e.error_len().unwrap_or(after.len()).max(1);
                inspect_escape_bytes_into(&after[..bad_len.min(after.len())], out);
                if bad_len >= after.len() {
                    break;
                }
                rest = &after[bad_len..];
            }
        }
    }
}


/// Ruby `String#inspect` of an `RStr` (quoted + escaped, encoding-
/// aware). Shared by `Value::to_inspect`'s Str arm and the String
/// FrozenError messages so the two can't drift — CRuby's FrozenError
/// renders the receiver's inspect (`"y"`), not its raw bytes.
pub(crate) fn rstr_inspect(s: &crate::value::RStr) -> String {
    let mut out = String::new();
    out.push('"');
    match s.encoding.get() {
        crate::value::EncodingTag::Binary => {
            inspect_escape_bytes_into(&s.content.borrow(), &mut out);
        }
        #[cfg(feature = "_encoding_full")]
        crate::value::EncodingTag::Other(idx) => {
            let b = s.content.borrow();
            match crate::encoding_full::char_chunks(idx, &b) {
                Some(chunks) => inspect_escape_chunks_into(&chunks, &mut out),
                None => inspect_escape_bytes_into(&b, &mut out),
            }
        }
        #[cfg(not(feature = "_encoding_full"))]
        crate::value::EncodingTag::Other(_) => {
            inspect_escape_bytes_into(&s.content.borrow(), &mut out);
        }
        _ => {
            let b = s.content.borrow();
            if std::str::from_utf8(&b).is_ok() {
                inspect_escape_into(&s.to_string_lossy(), &mut out);
            } else {
                inspect_escape_utf8_bytes_into(&b, &mut out);
            }
        }
    }
    out.push('"');
    out
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

/// CRuby 3.4+ hash inspect: a Symbol key renders as `name: value`
/// shorthand only when `name` is a bareword-safe identifier — starts
/// with `[a-zA-Z_]`, continues with `[a-zA-Z0-9_]`, optionally a single
/// trailing `?` / `!` / `=`. Anything else (hyphen, space, leading
/// digit, empty) is quoted: `"X-Token": value`. Shared by `to_display`
/// and the cycle-safe `Vm::inspect_value` so the two can't drift.
pub(crate) fn sym_needs_quotes(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return true };
    if !first.is_ascii_alphabetic() && first != '_' { return true; }
    for c in chars {
        if c.is_ascii_alphanumeric() || c == '_' { continue; }
        // Trailing `?` / `!` / `=` are allowed only as the final char;
        // a mid-name occurrence still needs quotes. (We accept it here
        // and the rare interior case is a benign over-shorthand —
        // matches the prior inline behaviour exactly.)
        if matches!(c, '?' | '!' | '=') { continue; }
        return true;
    }
    false
}

impl Value {
    /// Build a `Value::Str` from anything stringy. Centralises the
    /// `Rc<RefCell<String>>` wrap so call sites don't repeat the
    /// boilerplate.
    pub fn new_str(s: impl Into<String>) -> Self {
        Value::Str(std::rc::Rc::new(crate::value::RStr::new(s.into())))
    }
    /// Binary-safe constructor — preserves bytes verbatim (no UTF-8 check).
    /// Bytes tagged ASCII-8BIT (CRuby BINARY) — see
    /// `RStr::from_bytes_binary` for the caller contract.
    pub fn new_str_bytes_binary(b: Vec<u8>) -> Self {
        Value::Str(std::rc::Rc::new(crate::value::RStr::from_bytes_binary(b)))
    }
    pub fn new_str_bytes(b: Vec<u8>) -> Self {
        Value::Str(std::rc::Rc::new(crate::value::RStr::from_bytes(b)))
    }
    /// US-ASCII-tagged string — for numeric `to_s`/`inspect` output,
    /// which CRuby builds as US-ASCII (the content is ASCII by
    /// construction). Caller must pass ASCII-only content.
    pub fn new_str_us_ascii(s: impl Into<String>) -> Self {
        Value::Str(std::rc::Rc::new(crate::value::RStr::new_us_ascii(s.into())))
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
    /// The value-name CRuby prints in "no implicit conversion of X
    /// into Y" TypeError messages (rb_builtin_class_name): the nil /
    /// true / false singletons render as their LITERAL spelling
    /// ("no implicit conversion of nil into String"), not their
    /// class name — verified vs CRuby 3.4.1. Everything else is the
    /// plain `type_name()`.
    ///
    /// Adopted across the "no implicit conversion" format sites
    /// (String/Array/Hash/Integer targets — each op category probed
    /// vs CRuby 3.4.1 rather than batch-edited blind). Sites where
    /// CRuby genuinely prints the CLASS name (`File.join`'s
    /// user-class path, `Hash()`'s "can't convert TrueClass into
    /// Hash", `File.utime`'s "can't convert X into time") keep
    /// their class-name helpers instead.
    pub(crate) fn conv_type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
            _ => self.type_name(),
        }
    }
    /// CRuby's rb_num2long-shaped Integer-coercion TypeError message:
    /// nil gets the distinct lowercase "no implicit conversion from
    /// nil to integer" wording (numeric.c `rb_num2long` special-cases
    /// nil BEFORE falling back to `rb_to_int`); everything else gets
    /// "no implicit conversion of X into Integer" with
    /// `conv_type_name`'s value-word singleton spelling.
    ///
    /// NOT every Integer-coercing op is num2long-shaped: ops that
    /// call `rb_to_int` directly say "of nil into Integer" instead
    /// (`1 << nil`, `Integer#digits`, `Integer#allbits?`,
    /// `File.chmod`) — those sites format with `conv_type_name`
    /// directly. Probed per op category vs CRuby 3.4.1
    /// (`Array.new(nil)` / `"abc".byteslice(nil)` /
    /// `sprintf("%*d", nil, 5)` / `1.to_s(nil)` → "from nil to
    /// integer"; `1 << nil` / `123.digits(nil)` → "of nil into
    /// Integer").
    pub(crate) fn num2int_conv_msg(&self) -> String {
        match self {
            Value::Nil => "no implicit conversion from nil to integer".to_string(),
            _ => format!("no implicit conversion of {} into Integer", self.conv_type_name()),
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
            Value::Class(c) => crate::value::class_display_name(c),
            // Use class_of so TypedData-backed Objects (L3-B) print
            // safely too — `heap.instance(*id)` would panic on
            // those slots (review #1).
            // `#<Foo>` shows the user-declared class; CRuby
            // doesn't surface the eigenclass here even when one
            // exists. Use `real_class_of` for the same reason
            // `Object#class` does.
            Value::Object(id) => {
                let cls = heap.real_class_of(*id);
                // CRuby's default Object#to_s/#inspect carries the
                // address (`#<Foo:0x0000...>`); ours carries the
                // deterministic object_id encoding instead (ADR
                // 0017 — same value `.object_id`/explicit
                // `.inspect` report). minitest's mu_pp normalizes
                // the hex away, but its "No visible difference"
                // diff messages only trigger when the form HAS a
                // hex field to normalize.
                let oid = crate::vm::dispatch::object_id_for(self);
                if cls.effective_name().is_some() {
                    format!("#<{}:0x{:016x}>", cls.name, oid)
                } else {
                    let cd = crate::value::class_display_name(&cls);
                    format!("#<{}:0x{:016x}>", cd, oid)
                }
            }
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
            // `Array#inspect` (this path, per element) and
            // `String#inspect` (the primitive arm in vm/string.rs) plus
            // the FrozenError messages all funnel through `rstr_inspect`
            // so the escape rules can't drift.
            Value::Str(s) => rstr_inspect(s),
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
            // Default Object#inspect: `#<Foo:0xID @a=1, @b="x">`
            // — the ivar tail is what minitest's hex-diff messages
            // key on (its absence made unequal-but-same-shape
            // objects "No visible difference"). Divergences from
            // CRuby (documented): ivars render NAME-SORTED (the
            // backing FxHashMap has no insertion order), and an
            // Object-valued ivar prints its to_display short form
            // instead of recursing (no cycle guard at this layer —
            // a self-referential ivar would otherwise overflow).
            Value::Object(id) => {
                let head = self.to_display(heap, interner);
                let inst_ivars = match heap.get(*id) {
                    HeapObj::Instance(inst) => Some(inst.ivar_pairs()),
                    _ => None,
                };
                match inst_ivars {
                    Some(iv) if !iv.is_empty() => {
                        let mut pairs: Vec<(String, String)> = iv
                            .into_iter()
                            .map(|(k, v)| {
                                let val = match v {
                                    Value::Object(_) => v.to_display(heap, interner),
                                    _ => v.to_inspect(heap, interner),
                                };
                                (interner.resolve(k).to_string(), val)
                            })
                            .collect();
                        pairs.sort_by(|a, b| a.0.cmp(&b.0));
                        let body: Vec<String> = pairs
                            .into_iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect();
                        // head is "#<Foo:0xID>" — splice the ivars
                        // before the closing '>'.
                        format!("{} {}>", &head[..head.len() - 1], body.join(", "))
                    }
                    _ => head,
                }
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
    /// True when exactly one of the two Hashes is `compare_by_identity` —
    /// CRuby's rb_hash_equal refuses to equate non-empty hashes across the
    /// flag (shared by the `ruby_eq` / `ruby_eql` Hash arms below).
    fn hash_cbi_flags_differ_impl(heap: &Heap, a: ObjId, b: ObjId) -> bool {
        let ca = matches!(heap.get(a), HeapObj::Hash(h) if h.by_identity.get());
        let cb = matches!(heap.get(b), HeapObj::Hash(h) if h.by_identity.get());
        ca != cb
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
                // CRuby: NON-empty hashes whose compare_by_identity flags
                // differ are never equal (empty pairs compare equal before
                // the flag matters) — probed on 3.4.
                if !x.is_empty() && Self::hash_cbi_flags_differ_impl(heap, *a, *b) {
                    return false;
                }
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
            Value::Str(rs) => {
                // Cached-hash fast path: `StrCell` stores the
                // last-computed all-ASCII content hash and clears it
                // on every `borrow_mut()` (the only mutation door).
                // Only ASCII content caches — see below, the
                // non-ASCII hash mixes the encoding tag, which can
                // change without a content mutation
                // (`force_encoding`).
                let cached = rs.content.cached_hash();
                if cached != 0 {
                    return cached;
                }
                // E1 slice 2: equal-by-== strings must hash equal,
                // and `==` is tag-sensitive only for non-ASCII
                // bytes — so the tag joins the hash exactly when
                // the content is non-ASCII. The ascii test is one
                // extra pass over bytes already being hashed; the
                // common all-ASCII path mixes nothing extra.
                let b = rs.content.borrow();
                let h2 = mix(mix(h, &[6]), &b);
                if b.iter().all(|&x| x < 0x80) {
                    rs.content.set_cached_hash(h2);
                    // Read back through the setter's 0→1 remap so
                    // cached and uncached probes agree on the value.
                    rs.content.cached_hash()
                } else {
                    let tag = match rs.encoding.get() {
                        crate::value::EncodingTag::Binary => 0u8,
                        crate::value::EncodingTag::Utf8 => 1,
                        crate::value::EncodingTag::UsAscii => 2,
                        crate::value::EncodingTag::Other(n) => 3u8.wrapping_add(n),
                    };
                    mix(h2, &[tag])
                }
            }
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
        #[cfg(feature = "regex")]
        if let (Value::Regex(a), Value::Regex(b)) = (self, other) {
            // Source + flags equality (CRuby Regexp#==), not Rc
            // identity — Array#include? over matcher tables
            // (minitest's register_spec_type) relies on it.
            return a.as_str() == b.as_str() && a.options() == b.options();
        }
        // Procs/methods compare by identity (same heap slot) —
        // the same matcher table holds Proc entries too.
        if let (Value::Block(a), Value::Block(b)) = (self, other) {
            return a == b;
        }
        if let (Value::BoundMethod(a), Value::BoundMethod(b)) = (self, other) {
            return a == b;
        }
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
            // E1 slice 2: byte equality AND tag compatibility.
            // Equal bytes share ascii-only-ness, so the cross-tag
            // case reduces to "tags equal OR the (shared) bytes are
            // pure ASCII" — `"abc" == "abc".b` is true (CRuby),
            // `"é" == "é".b` is false. The ascii scan only runs on
            // the rare tag-mismatch path.
            (Value::Str(a), Value::Str(b)) => {
                let ab = a.content.borrow();
                let bb = b.content.borrow();
                *ab == *bb
                    && (a.encoding.get() == b.encoding.get()
                        || ab.iter().all(|&x| x < 0x80))
            }
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
                // CRuby: NON-empty hashes whose compare_by_identity flags
                // differ are never == (empty pairs compare equal before
                // the flag matters) — probed on 3.4.
                if !x.is_empty() && Self::hash_cbi_flags_differ_impl(heap, *a, *b) {
                    return false;
                }
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
    // CRuby's `Float#to_s` (== inspect): shortest round-trip decimal,
    // rendered in FIXED notation when the decimal point lands in
    // `-3..=15` (i.e. CRuby's `decpt > -4 && decpt <= DBL_DIG`), and
    // SCIENTIFIC (`D.DDDe±EE`) otherwise — mantissa always carries a
    // fractional digit, the exponent is always signed and ≥2 digits.
    // Rust's `{:e}` yields the same shortest digit string (Ryū) as
    // CRuby's dtoa, already in `D.DDDe±exp` form, so we reshape from it.
    let sign = if f.is_sign_negative() { "-" } else { "" };
    let abs = f.abs();
    if abs == 0.0 {
        return format!("{sign}0.0");
    }
    let sci = format!("{abs:e}"); // e.g. "1e20", "1.5e20", "3.14e0"
    // `{:e}` always contains 'e' and a parseable exponent; the `else`
    // arms are unreachable in practice but keep this panic-free.
    let Some((mantissa, exp_str)) = sci.split_once('e') else {
        return format!("{f:?}");
    };
    let Ok(exp) = exp_str.parse::<i32>() else {
        return format!("{f:?}");
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1; // # of digits before the decimal point
    let body = if !(-3..=15).contains(&decpt) {
        // scientific — reuse `{:e}`'s mantissa (it already split at the
        // first digit); just guarantee a fractional digit + format exp.
        let m = if mantissa.contains('.') {
            mantissa.to_string()
        } else {
            format!("{mantissa}.0")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{m}e{esign}{:02}", exp.abs())
    } else if decpt <= 0 {
        // 0.00…digits
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt as usize >= digits.len() {
        // digits then trailing zeros, then ".0"
        format!("{}{}.0", digits, "0".repeat(decpt as usize - digits.len()))
    } else {
        // decimal point falls inside the digit string
        let (a, b) = digits.split_at(decpt as usize);
        format!("{a}.{b}")
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layout guard for the record-shape Hash representation (2026-07
    /// small-hash campaign). Before the campaign every heap slot was
    /// 168 bytes because the flat `HashObj` (cold tail inline) was the
    /// largest `HeapObj` variant. With the cold tail boxed behind
    /// `extras` and `HASH_INLINE_PAIRS` = 3 inline pairs, `HashObj`
    /// stays at or under `Instance`'s 128 bytes and the shared slot
    /// size drops to 136. If a future field pushes either bound, this
    /// fails loudly — grow deliberately (it costs bytes on EVERY heap
    /// object), don't let it drift.
    #[test]
    fn heap_layout_sizes() {
        use std::mem::size_of;
        eprintln!("size Slot           = {}", size_of::<Slot>());
        eprintln!("size HeapObj        = {}", size_of::<HeapObj>());
        eprintln!("size HashObj        = {}", size_of::<HashObj>());
        eprintln!("size ArrayObj       = {}", size_of::<ArrayObj>());
        eprintln!("size Instance       = {}", size_of::<Instance>());
        eprintln!("size Value          = {}", size_of::<Value>());
        assert!(
            size_of::<HashObj>() <= size_of::<Instance>(),
            "HashObj ({}) outgrew Instance ({}) — it would set the heap-slot size again",
            size_of::<HashObj>(),
            size_of::<Instance>()
        );
        assert!(
            size_of::<Slot>() <= 136,
            "heap slot grew past 136 bytes ({}) — every live object pays this",
            size_of::<Slot>()
        );
        // An inline-capacity record hash must not carry a pairs heap
        // buffer (the ar_table-analogue invariant this campaign ships).
        let h = HashObj::with_pairs(
            (0..HASH_INLINE_PAIRS as i64)
                .map(|i| (Value::Int(i), Value::Int(i)))
                .collect::<PairsBuf>(),
        );
        assert!(!h.pairs.spilled(), "inline-cap pairs must stay inline");
        assert!(h.extras().is_none(), "plain with_pairs must not allocate extras");
    }

    /// ArrayObj plumbing (ADR 0020 / Array-subclass work): plain
    /// construction carries no tag, Deref reaches the elems, the
    /// ivar + tag helpers answer None-shaped defaults on plain
    /// arrays and round-trip on tagged ones, and the GC walk marks
    /// subclass ivars (a value reachable ONLY through an Array
    /// ivar must survive a collect rooted at that array).
    /// Hash per-instance eigenclass GC: a closure-method capture
    /// and an eigenclass ivar reachable ONLY through the Hash's
    /// singleton_class slot must survive a collect rooted at the
    /// Hash (the dispatch-level fixture can't force a collection
    /// deterministically; this pins the mark-walk arm directly).
    #[test]
    fn hash_singleton_class_gc_walk() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        let mut heap = Heap::new();
        let captured = heap.alloc(HeapObj::Array(vec![Value::Int(42)].into()));
        let ivar_val = heap.alloc(HeapObj::Array(vec![Value::Int(7)].into()));
        let sc = Rc::new(crate::value::Class {
            name: "#<Class:#<Hash>>".to_string(),
            is_module: false,
            ivars: RefCell::new(crate::intern::FxHashMap::default()),
            methods: RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
            superclass: RefCell::new(None),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            singleton_includes: RefCell::new(Vec::new()),
            singleton_view: RefCell::new(None),
            singleton_target: RefCell::new(None),
            undefed: RefCell::new(crate::intern::FxHashSet::default()),
            anon_serial: Cell::new(0),
            ivar_shape: std::cell::RefCell::new(crate::value::IvarShape::default()),
            class_vars: RefCell::new(crate::intern::FxHashMap::default()),
            consts: RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: RefCell::new(None),
            class_tag: None,
            frozen: std::cell::Cell::new(false),
            #[cfg(feature = "cext")]
            cext_alloc_func: Cell::new(None),
        });
        let m = Rc::new(crate::value::Method {
            params: vec![],
            proto_idx: 0,
            fixed_arity: None,
            defining_class: Some(Rc::downgrade(&sc)),
            visibility: Cell::new(crate::value::Visibility::Public),
            closure: Some(crate::value::MethodClosure {
                captured: Rc::new(RefCell::new(vec![Value::Array(captured)])),
                param_start: 0,
                n_params: 0,
                captured_yield_block: None,
                outer_chain: None,
                creator_start: 0,
            }),
            builtin: None,
            original_name: None,
        });
        sc.methods.borrow_mut().insert(crate::intern::SymId(3), m);
        sc.ivars.borrow_mut().insert(crate::intern::SymId(4), Value::Array(ivar_val));
        let mut hobj = HashObj::with_pairs(Vec::new());
        hobj.extras_mut().singleton_class = Some(sc);
        let h = heap.alloc(HeapObj::Hash(hobj));
        let roots = vec![Value::Hash(h)];
        let _ = heap.collect(&roots, std::time::Instant::now());
        // Both eigenclass-reachable slots survive.
        assert!(matches!(heap.array(captured)[0], Value::Int(42)));
        assert!(matches!(heap.array(ivar_val)[0], Value::Int(7)));
        // An unreachable slot is swept (sanity that collect ran).
        let orphan = heap.alloc(HeapObj::Array(vec![].into()));
        let _ = heap.collect(&roots, std::time::Instant::now());
        assert!(matches!(heap.slots[orphan.0 as usize], Slot::Dead));
    }

    /// The adaptive-floor controller's transfer curve (pure — no
    /// timing): proportional band between the clamps, saturating
    /// multiply at the top. Endpoints match the 2026-07 probe data:
    /// gc_churn-shaped sweeps (~65µs) clamp to MIN, post-rubocop-
    /// require sweeps (≥2048µs) clamp to MAX.
    #[test]
    fn adaptive_floor_curve() {
        assert_eq!(Heap::adaptive_floor(0), Heap::GC_FLOOR_MIN);
        assert_eq!(Heap::adaptive_floor(65), Heap::GC_FLOOR_MIN);
        assert_eq!(Heap::adaptive_floor(256), Heap::GC_FLOOR_MIN); // 256×16 = 4096 exactly
        assert_eq!(Heap::adaptive_floor(300), 4800); // proportional band
        assert_eq!(Heap::adaptive_floor(2048), Heap::GC_FLOOR_MAX); // 2048×16 = 32768 exactly
        assert_eq!(Heap::adaptive_floor(50_000), Heap::GC_FLOOR_MAX);
        assert_eq!(Heap::adaptive_floor(u64::MAX), Heap::GC_FLOOR_MAX); // saturating_mul
    }

    /// A raise requires TWO consecutive expensive sweeps: with a cheap
    /// last-sweep history, even a slow current sweep (deliberately not
    /// simulated — min() takes the 0 history regardless of what this
    /// collect measures) cannot move the floor. Deterministic under any
    /// scheduler behaviour.
    #[test]
    fn floor_raise_needs_two_expensive_sweeps() {
        let mut heap = Heap::new();
        heap.floor_override = None;
        heap.gc_floor = Heap::GC_FLOOR_MIN;
        heap.last_sweep_us = 0; // cheap history wins the min()
        let _ = heap.collect(&[], std::time::Instant::now());
        assert_eq!(heap.gc_floor, Heap::GC_FLOOR_MIN);
        assert_eq!(heap.next_gc, Heap::GC_FLOOR_MIN); // live 0 → floor
    }

    /// ONE cheap sweep decays a raised floor (RSS-eager direction).
    /// The empty-heap collect below takes single-digit µs; retry a few
    /// times so a scheduler blip landing exactly inside one collect
    /// can't flake the assert.
    #[test]
    fn floor_decays_on_one_cheap_sweep() {
        let mut heap = Heap::new();
        heap.floor_override = None;
        let decayed = (0..5).any(|_| {
            heap.gc_floor = Heap::GC_FLOOR_MAX;
            heap.last_sweep_us = 10_000; // expensive history
            let _ = heap.collect(&[], std::time::Instant::now());
            heap.gc_floor == Heap::GC_FLOOR_MIN
        });
        assert!(decayed, "a cheap sweep must decay the floor to MIN");
    }

    /// `RUBYRS_GC_MIN_THRESHOLD` (parsed into `floor_override` at
    /// construction) pins the trigger floor and freezes the adaptive
    /// state — an explicit override disables adaptivity.
    #[test]
    fn floor_override_pins_threshold_and_disables_controller() {
        let mut heap = Heap::new();
        heap.floor_override = Some(12_345);
        heap.gc_floor = 7; // sentinel: controller must not touch it
        let _ = heap.collect(&[], std::time::Instant::now());
        assert_eq!(heap.next_gc, 12_345);
        assert_eq!(heap.gc_floor, 7);
    }

    #[test]
    fn array_obj_tag_and_ivars() {
        let mut heap = Heap::new();
        let plain = heap.alloc(HeapObj::Array(vec![Value::Int(1)].into()));
        assert!(heap.array_class_tag(plain).is_none());
        assert!(heap.array_ivar_get(plain, crate::intern::SymId(7)).is_none());
        assert_eq!(heap.array(plain).len(), 1);
        assert!(heap.array_ivars_clone(plain).is_empty());

        // Tagged array with an ivar holding the ONLY reference to
        // another heap value.
        let inner = heap.alloc(HeapObj::Array(Vec::new().into()));
        let tagged = heap.alloc(HeapObj::Array(ArrayObj {
            elems: vec![Value::Int(2)],
            class_tag: None, // tag is Rc<Class>; None here — tag
            // round-trip is covered end-to-end by the
            // array_subclass diff fixture.
            ivars: {
                let mut m = crate::intern::FxHashMap::default();
                m.insert(crate::intern::SymId(3), Value::Array(inner));
                m
            },
            frozen: std::cell::Cell::new(false),
        }));
        assert!(matches!(
            heap.array_ivar_get(tagged, crate::intern::SymId(3)),
            Some(Value::Array(id)) if id == inner
        ));
        heap.array_ivar_set(tagged, crate::intern::SymId(4), Value::Int(9));
        assert_eq!(heap.array_ivars_clone(tagged).len(), 2);

        // GC: root only the tagged array — `inner` must survive
        // through the ivar edge.
        let _ = heap.collect(&[Value::Array(tagged)], std::time::Instant::now());
        assert!(matches!(heap.get(inner), HeapObj::Array(_)));
        // Drop the root: inner becomes garbage.
        let _ = heap.collect(&[], std::time::Instant::now());
    }

    /// `inspect_escape_bytes_into` — every escape arm (the byte
    /// route only runs for BINARY-tagged strings, so the rarer
    /// control escapes need a unit caller for the coverage
    /// ratchet — and deserve one anyway).
    #[test]
    fn inspect_escape_utf8_bytes_mixed_runs() {
        // valid prefix + invalid byte + valid suffix
        let mut out = String::new();
        inspect_escape_utf8_bytes_into(b"ok\xFF!", &mut out);
        assert_eq!(out, "ok\\xFF!");
        // lone invalid byte
        let mut out = String::new();
        inspect_escape_utf8_bytes_into(b"\xB6", &mut out);
        assert_eq!(out, "\\xB6");
        // invalid byte then escape-needing char
        let mut out = String::new();
        inspect_escape_utf8_bytes_into(b"\xB6A\nB", &mut out);
        assert_eq!(out, "\\xB6A\\nB");
        // fully valid passes through the char path
        let mut out = String::new();
        inspect_escape_utf8_bytes_into("h\u{e9}llo".as_bytes(), &mut out);
        assert_eq!(out, "h\u{e9}llo");
        // truncated multibyte at end (error_len None)
        let mut out = String::new();
        inspect_escape_utf8_bytes_into(b"a\xE2\x82", &mut out);
        assert_eq!(out, "a\\xE2\\x82");
    }

    #[test]
    fn inspect_escape_bytes_all_arms() {
        let mut out = String::new();
        inspect_escape_bytes_into(
            b"\\\"\x07\x08\t\n\x0B\x0C\r\x1B a~\x00\x7F\xFF",
            &mut out,
        );
        assert_eq!(
            out,
            "\\\\\\\"\\a\\b\\t\\n\\v\\f\\r\\e a~\\x00\\x7F\\xFF"
        );
    }

    /// `enc_compat` truth table + `display` names — every branch
    /// (the coverage ratchet flagged the two rare arms after
    /// slice 2 landed them production-side only).
    #[test]
    fn enc_compat_table() {
        use crate::value::{enc_compat, EncodingTag as T};
        // same tag
        assert_eq!(enc_compat(T::Utf8, b"\xc3\xa9", T::Utf8, b"\xc3\xa9"), Some(T::Utf8));
        // both ascii → receiver
        assert_eq!(enc_compat(T::Utf8, b"a", T::Binary, b"b"), Some(T::Utf8));
        // ascii receiver, non-ascii arg → arg
        assert_eq!(enc_compat(T::Utf8, b"a", T::Binary, b"\xff"), Some(T::Binary));
        // non-ascii receiver, ascii arg → receiver
        assert_eq!(enc_compat(T::Binary, b"\xff", T::Utf8, b"a"), Some(T::Binary));
        // both non-ascii, different tags → incompatible
        assert_eq!(enc_compat(T::Utf8, b"\xc3\xa9", T::Binary, b"\xff"), None);
        // display names (incl. the dual-name BINARY + the Tier 2
        // placeholder)
        assert_eq!(T::Utf8.display(), "UTF-8");
        assert_eq!(T::UsAscii.display(), "US-ASCII");
        assert_eq!(T::Binary.display(), "BINARY (ASCII-8BIT)");
        // Index 200 is unregistered in every build; with
        // _encoding_full, low indices resolve registry names.
        assert_eq!(T::Other(200).display(), "OTHER");
        #[cfg(feature = "_encoding_full")]
        assert_eq!(T::Other(3).display(), "KOI8-R");
    }

    /// `from_bytes_binary` tags Binary; `from_bytes`/`new` tag Utf8
    /// (E1 step-1 contract — semantics consume the tag later).
    #[test]
    fn rstr_encoding_tags() {
        use crate::value::{EncodingTag, RStr};
        assert_eq!(RStr::new("x".to_string()).encoding.get(), EncodingTag::Utf8);
        assert_eq!(RStr::from_bytes(vec![0xff]).encoding.get(), EncodingTag::Utf8);
        assert_eq!(
            RStr::from_bytes_binary(vec![0xff]).encoding.get(),
            EncodingTag::Binary
        );
        let v = Value::new_str_bytes_binary(vec![1, 2]);
        if let Value::Str(s) = v {
            assert_eq!(s.encoding.get(), EncodingTag::Binary);
            assert_eq!(s.content.borrow().len(), 2);
        } else {
            panic!("not a Str");
        }
    }

    /// ADR 0035 Phase 3a — the baked-address `JitObjView` must keep pointing at the live
    /// `class_ptrs` base across reallocations, its `len` must track, and its OWN address must
    /// stay put (that is the whole point of the `Box`: the JIT bakes it once).
    #[cfg(feature = "jit-native")]
    #[test]
    fn jit_view_tracks_class_ptrs_across_realloc() {
        let mut heap = Heap::new();
        let view_addr = heap.jit_view_addr();
        // Allocate enough to force several Vec growths.
        for _ in 0..5000 {
            let _ = heap.alloc(HeapObj::Hash(HashObj::with_pairs(vec![])));
            assert_eq!(heap.class_ptrs.len(), heap.slots.len(), "table length tracks slots");
            assert_eq!(
                heap.jit_view.class_ptrs,
                heap.class_ptrs.as_ptr(),
                "view base refreshed to the live class_ptrs after (possible) realloc"
            );
            assert_eq!(heap.jit_view.class_ptrs_len, heap.class_ptrs.len(), "view len tracks");
            assert_eq!(heap.jit_view_addr(), view_addr, "view address is stable (Box)");
        }
    }
}
