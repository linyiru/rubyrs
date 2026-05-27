//! Small shared helpers used across multiple `vm/` submodules
//! — comparison ordering, default-Nil vec construction, visibility-
//! modifier name parsing. Each is too small to deserve its own
//! file but doesn't belong with any of the per-type modules.

use crate::intern::Interner;
use crate::value::{Value, Visibility};

/// Ordering for built-in aggregation methods (`min` / `max` /
/// `sort`). Only homogeneous Int / Str / Sym arrays are supported
/// at this entry point — BigInt-aware comparison uses
/// `value_cmp_v_heap` since BigInt operands need heap access.
/// Other shapes return `None` so the caller can fall through to
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

/// BigInt-aware ordering. Resolves `Value::BigInt(id)` against the
/// heap and supports mixed `Int↔BigInt` comparisons; falls back to
/// `value_cmp_v` for non-BigInt cases. Used by the aggregators
/// (`Array#min/#max/#sort` and friends) so a fold-promoted BigInt
/// element doesn't make the whole builtin return `None` and
/// surface NoMethodError. The extra `heap` parameter is the only
/// difference from `value_cmp_v` — call sites that have a `Heap`
/// in scope should prefer this one.
/// Recursion-depth ceiling for the Array×Array arm. Sized to
/// keep the worst-case nested-Array walk well within Rust's
/// default 8 MiB thread stack: empirically the dispatch frame
/// for `value_cmp_v_heap` is ~1 KiB, and crashes were observed
/// at ~100K nesting (PR #219 review). 2_000 is two orders of
/// magnitude smaller than that crash threshold and two orders
/// of magnitude larger than any realistic spec fixture. Past
/// the ceiling we return `None` rather than aborting; the
/// caller surfaces this as nil (`Array#<=>`) or NoMethodError
/// (`Array#sort` / `min` / `max`) — a soft failure that's
/// catchable, vs the previous Rust-level `stack overflow,
/// aborting` (uncatchable process abort).
const ARRAY_CMP_MAX_DEPTH: usize = 2_000;

pub(crate) fn value_cmp_v_heap(
    a: &Value,
    b: &Value,
    interner: &Interner,
    heap: &crate::heap::Heap,
) -> Option<std::cmp::Ordering> {
    value_cmp_v_heap_inner(a, b, interner, heap, 0)
}

fn value_cmp_v_heap_inner(
    a: &Value,
    b: &Value,
    interner: &Interner,
    heap: &crate::heap::Heap,
    depth: usize,
) -> Option<std::cmp::Ordering> {
    #[cfg(feature = "bignum")]
    {
        use num_bigint::BigInt;
        match (a, b) {
            (Value::BigInt(x), Value::BigInt(y)) => {
                return Some(heap.bigint(*x).cmp(heap.bigint(*y)));
            }
            (Value::Int(x), Value::BigInt(y)) => {
                return Some(BigInt::from(*x).cmp(heap.bigint(*y)));
            }
            (Value::BigInt(x), Value::Int(y)) => {
                return Some(heap.bigint(*x).cmp(&BigInt::from(*y)));
            }
            _ => {}
        }
    }
    // Numeric coercion arms — Float×Float and the mixed
    // Int↔Float pairs. `value_cmp_v` itself doesn't cover
    // numerics other than Int×Int (it's the homogeneous-
    // aggregator entrypoint), so without these arms `Array#<=>`
    // returns nil on any pair that crosses numeric types even
    // though `Float#<=>(Integer)` is implemented at the
    // primitive level. `partial_cmp` on NaN returns None →
    // propagates upward, matching CRuby `[Float::NAN] <=>
    // [Float::NAN] == nil`. The Int→f64 cast loses precision
    // beyond 2^53 but matches CRuby's `Integer#<=>(Float)`
    // semantics (CRuby's `Float#<=>(Integer)` also coerces
    // through f64 for the same reason).
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => return x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => return (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => return x.partial_cmp(&(*y as f64)),
        _ => {}
    }
    // Array#<=> element-wise lex compare. Length is the
    // tiebreaker only when all common-prefix pairs are Equal —
    // matches CRuby `[1,2] <=> [1,2,3] == -1`. If any pair is
    // incomparable (cross-type without ordering), the whole
    // comparison is incomparable — propagate None upward, mirror-
    // ing CRuby's `[1,2] <=> [1,"x"] == nil`. Recursing into
    // `value_cmp_v_heap` means nested Arrays-of-Arrays compose
    // automatically.
    if let (Value::Array(xa), Value::Array(xb)) = (a, b) {
        // Self-comparison short-circuit: when both sides reference
        // the same Array heap slot, the result is Equal without
        // recursing into the pairs. Without this, a self-cycle
        // (`a = []; a << a; a <=> a`) recurses into a[0] vs a[0]
        // → same slot → ... and overflows the stack. CRuby uses a
        // per-thread recursion-tracking table to detect arbitrary
        // mutual cycles; rubyrs catches the common direct case
        // here. Deeper mutual cycles (`a << b; b << a; a <=> b`)
        // remain a gap — bounded by the depth ceiling below
        // rather than abort'ed.
        if *xa == *xb {
            return Some(std::cmp::Ordering::Equal);
        }
        // Depth ceiling against non-cyclic deep nesting (PR #219
        // review). Beyond this we return None rather than letting
        // Rust abort with `stack overflow, aborting`. Soft-fails
        // to nil at the Array#<=> caller — catchable from Ruby,
        // vs the previous uncatchable process abort.
        if depth >= ARRAY_CMP_MAX_DEPTH {
            return None;
        }
        let av = heap.array(*xa);
        let bv = heap.array(*xb);
        let common = av.len().min(bv.len());
        for i in 0..common {
            match value_cmp_v_heap_inner(&av[i], &bv[i], interner, heap, depth + 1) {
                Some(std::cmp::Ordering::Equal) => continue,
                Some(ord) => return Some(ord),
                None => return None,
            }
        }
        return Some(av.len().cmp(&bv.len()));
    }
    let _ = heap; // unused without bignum + when neither arm above fires
    value_cmp_v(a, b, interner)
}

pub(crate) fn vec_nil(n: usize) -> Vec<Value> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n { v.push(Value::Nil); }
    v
}

pub(crate) fn visibility_from_name(name: &str) -> Option<Visibility> {
    match name {
        "private" => Some(Visibility::Private),
        "protected" => Some(Visibility::Protected),
        "public" => Some(Visibility::Public),
        _ => None,
    }
}
