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
pub(crate) fn value_cmp_v_heap(
    a: &Value,
    b: &Value,
    interner: &Interner,
    heap: &crate::heap::Heap,
) -> Option<std::cmp::Ordering> {
    let _ = heap; // referenced only under `feature = "bignum"`
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
