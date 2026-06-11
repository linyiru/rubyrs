//! Shared sort engine for the `sort` / `sort!` / `sort_by` family.
//!
//! Every comparison may dispatch arbitrary Ruby (a user `<=>` or a
//! comparator block), so the comparator is a fallible `FnMut` and the
//! algorithm runs comparisons strictly sequentially — no `slice::sort`
//! because its closure would alias `&mut Vm` while the Vec borrow is
//! live (the constraint that originally forced the per-arm insertion
//! sorts).
//!
//! Bottom-up merge sort, O(n log n), stable. The insertion sorts it
//! replaces were O(n²): Jekyll's `SiteDrop#posts` reverse-sorts 1000
//! already-ascending Documents (`docs.sort { |a, b| b <=> a }`), which
//! the insertion sort turned into ~500k interpreted comparisons —
//! ~1.8B instructions, 31% of a no-layout jekyll-1k build.
//!
//! Stability contract: "take left unless strictly Greater", the same
//! tie behaviour as the old insertion sorts (CRuby's qsort is
//! unstable, so ties are unspecified there; keeping ours stable means
//! no fixture churn).
//!
//! GC contract: this module never allocates on the Ruby heap and
//! never calls `maybe_gc` itself; the scratch buffers hold `Clone`d
//! Values whose ObjIds the CALLER must keep rooted (PinGuard on the
//! receiver and/or elements) because the comparator can trigger GC.
//!
//! Error contract: on `Err` the input Vec is left UNTOUCHED — call
//! sites rely on this for `sort!` + `break` semantics (receiver
//! unmodified when the comparator block breaks out).

use std::cmp::Ordering;

use crate::error::Trap;
use crate::value::Value;

/// Why a sort stopped early. `Trap` propagates a Ruby exception;
/// `MethodReturn` / `Break` carry a comparator block's non-local
/// exits to the primitive arm (which maps them to its own return
/// protocol); `Decline` preserves the legacy `return Ok(None)`
/// bail of the Hash arms (incomparable keys fall through to the
/// generic dispatch path instead of raising).
pub(crate) enum SortStop {
    Trap(Trap),
    MethodReturn,
    Break(Value),
    Decline,
}

impl From<Trap> for SortStop {
    fn from(t: Trap) -> Self { SortStop::Trap(t) }
}

/// Stable merge sort with a fallible comparator. Generic over the
/// error type so the no-block arms can use `E = Trap` and `?`
/// directly, while block-comparator arms use `E = SortStop`.
///
/// Adaptive pre-pass: one O(n) scan detects already-sorted input and
/// returns without allocating scratch — `Collection#sort_docs!` in
/// Jekyll's read phase sorts docs that arrive in filename (= date)
/// order, and the old insertion sort handled that case in n-1
/// comparisons; this keeps that property.
pub(crate) fn merge_sort_by<T: Clone, E>(
    items: &mut Vec<T>,
    mut cmp: impl FnMut(&T, &T) -> Result<Ordering, E>,
) -> Result<(), E> {
    let n = items.len();
    if n < 2 {
        return Ok(());
    }
    let mut presorted = true;
    for i in 1..n {
        if cmp(&items[i - 1], &items[i])? == Ordering::Greater {
            presorted = false;
            break;
        }
    }
    if presorted {
        return Ok(());
    }
    // Ping-pong between two scratch buffers; commit to `items` only
    // on success (see the error contract above).
    let mut src: Vec<T> = items.clone();
    let mut dst: Vec<T> = items.clone();
    let mut width = 1usize;
    while width < n {
        let mut start = 0usize;
        while start < n {
            let mid = (start + width).min(n);
            let end = (start + 2 * width).min(n);
            let (mut l, mut r, mut o) = (start, mid, start);
            while l < mid && r < end {
                if cmp(&src[l], &src[r])? == Ordering::Greater {
                    dst[o] = src[r].clone();
                    r += 1;
                } else {
                    dst[o] = src[l].clone();
                    l += 1;
                }
                o += 1;
            }
            while l < mid {
                dst[o] = src[l].clone();
                l += 1;
                o += 1;
            }
            while r < end {
                dst[o] = src[r].clone();
                r += 1;
                o += 1;
            }
            start = end;
        }
        std::mem::swap(&mut src, &mut dst);
        width *= 2;
    }
    *items = src;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_cmp(a: &i64, b: &i64) -> Result<Ordering, ()> {
        Ok(a.cmp(b))
    }

    #[test]
    fn sorts_reverse_input() {
        let mut v: Vec<i64> = (1..=100).rev().collect();
        merge_sort_by(&mut v, ok_cmp).unwrap();
        assert_eq!(v, (1..=100).collect::<Vec<i64>>());
    }

    #[test]
    fn sorts_random_like_input() {
        // Deterministic pseudo-shuffle (no Math.random in tests):
        // multiplicative stride over a prime-sized domain.
        let mut v: Vec<i64> = (0..997).map(|i| (i * 263) % 997).collect();
        merge_sort_by(&mut v, ok_cmp).unwrap();
        assert_eq!(v, (0..997).collect::<Vec<i64>>());
    }

    #[test]
    fn presorted_input_costs_n_minus_1_comparisons() {
        let mut v: Vec<i64> = (1..=1000).collect();
        let mut count = 0usize;
        merge_sort_by(&mut v, |a, b| {
            count += 1;
            Ok::<_, ()>(a.cmp(b))
        })
        .unwrap();
        assert_eq!(count, 999);
    }

    #[test]
    fn reverse_input_is_n_log_n_not_quadratic() {
        let n = 1024i64;
        let mut v: Vec<i64> = (1..=n).rev().collect();
        let mut count = 0usize;
        merge_sort_by(&mut v, |a, b| {
            count += 1;
            Ok::<_, ()>(a.cmp(b))
        })
        .unwrap();
        // n log2 n = 10240; insertion sort would need ~524k. Allow
        // headroom for the failed pre-pass + uneven merges.
        assert!(count < 12000, "comparison count {count} suggests a quadratic sort");
    }

    #[test]
    fn stable_on_equal_keys() {
        // Pairs sorted by .0 only; .1 records original order.
        let mut v: Vec<(i64, usize)> = vec![(2, 0), (1, 1), (2, 2), (1, 3), (2, 4), (1, 5)];
        merge_sort_by(&mut v, |a, b| Ok::<_, ()>(a.0.cmp(&b.0))).unwrap();
        assert_eq!(v, vec![(1, 1), (1, 3), (1, 5), (2, 0), (2, 2), (2, 4)]);
    }

    #[test]
    fn error_leaves_items_untouched() {
        let original = vec![3i64, 1, 2, 5, 4];
        let mut v = original.clone();
        let mut budget = 3usize;
        let r = merge_sort_by(&mut v, |a, b| {
            if budget == 0 {
                return Err("stop");
            }
            budget -= 1;
            Ok(a.cmp(b))
        });
        assert!(r.is_err());
        assert_eq!(v, original);
    }
}
