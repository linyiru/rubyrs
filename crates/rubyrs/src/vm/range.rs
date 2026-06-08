//! `Range` methods that need heap access. Mirrors CRuby's
//! `range.c`. Dispatched from `Vm::collection_call`'s
//! `Value::Range` arm.

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::{PinGuard, Vm};

impl Vm {
    pub(crate) fn range_collection_call(
        &mut self,
        id: ObjId,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        Ok({
                let (b, e, excl) = {
                    let r = self.heap.range(id);
                    (r.begin.clone(), r.end.clone(), r.exclusive)
                };
                // Endless / beginless variants — work for a handful
                // of methods that don't actually need both endpoints
                // to be known Ints. The strict (Int, Int) match
                // below handles everything else and bails to
                // NoMethodError for partial ranges.
                let begin_int = if let Value::Int(a) = &b { Some(*a) } else { None };
                let end_int = if let Value::Int(c) = &e { Some(*c) } else { None };
                // Float-bounded or mixed Int/Float numeric Range
                // `include?` / `member?` / `cover?` — sinatra-param's
                // `validate!(param, in: 0.0..10.0)` calls
                // `range.include?(param)` where the range bounds
                // and param can be Int OR Float. Coerce both
                // sides to f64 for comparison; this matches
                // CRuby's `Range#include?` semantics on numeric
                // bounds.
                let to_f = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Int(n) => Some(*n as f64),
                        Value::Float(f) => Some(*f),
                        _ => None,
                    }
                };
                if matches!(name, "include?" | "member?" | "cover?")
                    && args.len() == 1
                    && (matches!(&b, Value::Float(_)) || matches!(&e, Value::Float(_)))
                    && let (Some(bf), Some(ef), Some(arg)) = (to_f(&b), to_f(&e), to_f(&args[0]))
                {
                    let lo_ok = arg >= bf;
                    let hi_ok = if excl { arg < ef } else { arg <= ef };
                    return Ok(Some(Value::Bool(lo_ok && hi_ok)));
                }
                // No-block transform/filter/iteration → Enumerator
                // (CRuby `enum.c`), re-invoking the block form once
                // driven. Mirrors the Array/Hash no-block wiring; works
                // for any finite Range (Int- or String-bounded) since
                // make_enum_for only defers — the block form (direct or
                // via the Range Enumerable fallback in iter.rs) handles
                // the bounds when the Enumerator is actually driven.
                if args.is_empty() && matches!(name,
                    "each" | "map" | "collect" | "select" | "filter"
                    | "reject" | "find" | "detect" | "each_with_index"
                    | "partition" | "group_by" | "min_by" | "max_by"
                    | "sort_by"
                ) {
                    return Ok(Some(self.make_enum_for(Value::Range(id), name, vec![])?));
                }
                if begin_int.is_none() || end_int.is_none() {
                    // String-endpoint Range support: `('a'..'z').to_a`,
                    // `.size`, `.include?("c")`, etc. driven by
                    // String#succ. Iteration bounded by `len(end)`
                    // to avoid running away when succ produces a
                    // longer string than the upper endpoint.
                    if let (Value::Str(sb), Value::Str(se)) = (&b, &e) {
                        let start = sb.to_string_lossy();
                        let stop = se.to_string_lossy();
                        match (name, args) {
                            ("to_a", []) | ("sort", []) => {
                                let mut out: Vec<Value> = Vec::new();
                                let mut cur = start;
                                loop {
                                    let done = if excl { cur >= stop } else { cur > stop };
                                    if done { break; }
                                    out.push(Value::new_str(cur.clone()));
                                    let next = super::string::str_succ(&cur);
                                    if next.len() > stop.len() { break; }
                                    cur = next;
                                }
                                self.maybe_gc();
                                let nid = self.heap.alloc(HeapObj::Array(out));
                                return Ok(Some(Value::Array(nid)));
                            }
                            // CRuby Range#size is nil for non-numeric
                            // endpoints; use `count` for an actual count.
                            ("size", []) => return Ok(Some(Value::Nil)),
                            ("count", []) => {
                                let mut n: i64 = 0;
                                let mut cur = start;
                                loop {
                                    let done = if excl { cur >= stop } else { cur > stop };
                                    if done { break; }
                                    n += 1;
                                    let next = super::string::str_succ(&cur);
                                    if next.len() > stop.len() { break; }
                                    cur = next;
                                }
                                return Ok(Some(Value::Int(n)));
                            }
                            ("include?", [Value::Str(needle)]) | ("member?", [Value::Str(needle)]) | ("cover?", [Value::Str(needle)]) => {
                                let n = needle.to_string_lossy();
                                let lo_ok = n >= start;
                                let hi_ok = if excl { n < stop } else { n <= stop };
                                return Ok(Some(Value::Bool(lo_ok && hi_ok)));
                            }
                            _ => {}
                        }
                    }
                    match (name, args) {
                        ("begin", []) | ("first", []) | ("min", []) => return Ok(Some(b.clone())),
                        ("end", []) | ("last", []) | ("max", []) => return Ok(Some(e.clone())),
                        // CRuby quirk: beginless `(..e).first(...)`
                        // raises the same RangeError regardless of
                        // arg shape — beginless precedence ALWAYS
                        // wins over arity/type checks. Add this
                        // guard BEFORE the per-shape arms below so
                        // (..5).first("x"), .first(1, 2),
                        // .first(2.5), .first(NaN), .first(big) all
                        // raise the beginless RangeError instead of
                        // TypeError / ArgumentError / RangeError
                        // ("Inf out of range"). Note: the 0-arg
                        // `("first", [])` arm above still returns
                        // `b.clone()` (= Nil) for beginless — that's
                        // a separate pre-existing divergence from
                        // CRuby and out of scope here.
                        ("first", many) if !many.is_empty() && matches!(&b, Value::Nil) => {
                            let _ = many;
                            return Err(self.trap(RubyError::RangeError {
                                msg: "cannot get the first element of beginless range".into(),
                            }));
                        }
                        // BigInt arg — CRuby raises RangeError
                        // even on endless `(1..)`. Mirrors the
                        // Int+Int branch's BigInt arm.
                        #[cfg(feature = "bignum")]
                        ("first", [Value::BigInt(_)]) => {
                            return Err(self.trap(RubyError::RangeError {
                                msg: "bignum too big to convert into `long'".to_string(),
                            }));
                        }
                        // Float coerce — same pattern as the
                        // Int+Int branch (PR #351). Self-recurse
                        // with the converted Int so the existing
                        // Int arm below owns the rest of the
                        // logic (negative-n guard / endless walk;
                        // beginless was already short-circuited
                        // above).
                        ("first", [Value::Float(f)]) => {
                            let n = self.float_to_int_arg(*f)?;
                            return self.range_collection_call(id, name, &[Value::Int(n)]);
                        }
                        // Multi-arg for partial-range `first` —
                        // same CRuby "expected 1" wording as the
                        // Int+Int branch.
                        ("first", many) if many.len() > 1 => {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!(
                                    "wrong number of arguments (given {}, expected 1)",
                                    many.len()
                                ),
                            }));
                        }
                        ("first", [Value::Int(n)]) => {
                            // Three reachable cases in this partial-
                            // range branch:
                            //   1. Truly beginless `(..e)`:
                            //      `b == Value::Nil` → CRuby raises
                            //      `RangeError: cannot get the first
                            //      element of beginless range`,
                            //      regardless of n's sign. The
                            //      beginless check has to come BEFORE
                            //      the negative-n guard so
                            //      `(..5).first(-1)` produces
                            //      RangeError, not ArgumentError.
                            //   2. Endless `(b..)` with Int begin:
                            //      walk Ints from `bi`. Negative n is
                            //      ArgumentError per CRuby (and per
                            //      #140's Array policy).
                            //   3. Non-Int begin (BigInt, etc.) with
                            //      missing/non-Int end: not
                            //      implemented here; return
                            //      NoMethodError via `Ok(None)`.
                            //      Implementing BigInt iteration is
                            //      tracked in #143 follow-ups; until
                            //      then, an honest NoMethodError is
                            //      preferable to a misleading
                            //      "beginless range" RangeError.
                            //
                            // Case 1 first.
                            if matches!(&b, Value::Nil) {
                                return Err(self.trap(RubyError::RangeError {
                                    msg: "cannot get the first element of beginless range".into(),
                                }));
                            }
                            // Then negative-n. Past this point we know
                            // begin is non-Nil; either Int (case 2)
                            // or non-Int non-Nil (case 3).
                            if *n < 0 {
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: "negative array size (or size too big)".into(),
                                }));
                            }
                            if let Some(bi) = begin_int {
                                // `usize::try_from(*n).unwrap_or(MAX)`
                                // is the wasm32-safe shape from #140:
                                // i64 → usize would truncate large
                                // positives on a 32-bit usize host.
                                // Capacity is bounded by `n` so very
                                // large requests still try to alloc
                                // the full vec — that's a memory
                                // cost the caller asked for, matching
                                // the existing endless `step`/`to_a`
                                // patterns.
                                let n = usize::try_from(*n).unwrap_or(usize::MAX);
                                let mut out: Vec<Value> = Vec::with_capacity(n);
                                let mut v = bi;
                                for _ in 0..n {
                                    out.push(Value::Int(v));
                                    v = v.saturating_add(1);
                                }
                                self.maybe_gc();
                                let nid = self.heap.alloc(HeapObj::Array(out));
                                return Ok(Some(Value::Array(nid)));
                            }
                            // Case 3: non-Int non-Nil begin (e.g.
                            // BigInt-bounded range). Fall through to
                            // the outer `_ => return Ok(None)` which
                            // produces NoMethodError. This matches
                            // the pre-#146 behaviour for the same
                            // inputs.
                            return Ok(None);
                        }
                        // Non-Int 1-arg catch-all for partial-range
                        // `first`. Without this, endless `(1..).first("x")`
                        // fell to NoMethodError despite `respond_to?`
                        // returning true — same lockstep violation
                        // PR #351 fixed for the Int+Int branch.
                        // Placed AFTER the Int arm so the Int success
                        // path is unchanged.
                        ("first", _) => {
                            return Err(self.arity_error_arg0_or_1_int(name, args));
                        }
                        ("cover?", [Value::Int(v)]) => {
                            let lo_ok = match begin_int { Some(lo) => *v >= lo, None => true };
                            let hi_ok = match end_int {
                                Some(hi) => if excl { *v < hi } else { *v <= hi },
                                None => true,
                            };
                            return Ok(Some(Value::Bool(lo_ok && hi_ok)));
                        }
                        // `r.cover?(other_range)` — true iff the
                        // other range is fully contained. CRuby
                        // checks both endpoints; we mirror that
                        // for the closed-Int case (the only Range
                        // shape we model).
                        ("cover?", [Value::Range(other_id)]) => {
                            let other = self.heap.range(*other_id);
                            let other_excl = other.exclusive;
                            let (ob, oe) = match (&other.begin, &other.end) {
                                (Value::Int(b), Value::Int(e)) => (*b, *e),
                                _ => return Ok(None),
                            };
                            // CRuby: empty sub-ranges do NOT cover.
                            let empty = if other_excl { ob >= oe } else { ob > oe };
                            if empty { return Ok(Some(Value::Bool(false))); }
                            let other_min = ob;
                            let other_max = if other_excl { oe - 1 } else { oe };
                            let lo_ok = match begin_int { Some(lo) => other_min >= lo, None => true };
                            let hi_ok = match end_int {
                                Some(hi) => if excl { other_max < hi } else { other_max <= hi },
                                None => true,
                            };
                            return Ok(Some(Value::Bool(lo_ok && hi_ok)));
                        }
                        ("include?", [Value::Int(v)]) | ("member?", [Value::Int(v)]) => {
                            let lo_ok = match begin_int { Some(lo) => *v >= lo, None => true };
                            let hi_ok = match end_int {
                                Some(hi) => if excl { *v < hi } else { *v <= hi },
                                None => true,
                            };
                            return Ok(Some(Value::Bool(lo_ok && hi_ok)));
                        }
                        ("exclude_end?", []) => return Ok(Some(Value::Bool(excl))),
                        // each_slice / each_cons on non-Int (e.g.
                        // Str+Str) ranges: lookup.rs:646 lists both
                        // as `respond_to? = true` for any Range, so
                        // falling through to NoMethodError would
                        // contradict the lockstep contract at
                        // lookup.rs:756. Raise RuntimeError with an
                        // explicit "not yet implemented" message —
                        // same fallback as the zero-arg find_index
                        // path at array.rs:357 (PR #308 cycle 3).
                        ("each_slice", [Value::Int(_)]) | ("each_cons", [Value::Int(_)]) => {
                            return Err(self.trap(RubyError::RuntimeError {
                                msg: format!(
                                    "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                                ),
                            }));
                        }
                        _ => return Ok(None),
                    }
                }
                let (bi, ei) = (begin_int.unwrap(), end_int.unwrap());
                // count uses checked arithmetic — `ei - bi` overflows
                // for ranges like `(i64::MIN..i64::MAX)`; treat any
                // overflow as a size of 0 (matches the "empty" semantic
                // that the rest of this match already returns for
                // bi > end_inc). Pre-cycle-14 used `ei - bi + 1`
                // unchecked, panicking in debug builds.
                let count = if excl {
                    ei.checked_sub(bi).map(|d| d.max(0)).unwrap_or(0)
                } else {
                    ei.checked_sub(bi).and_then(|d| d.checked_add(1)).map(|d| d.max(0)).unwrap_or(0)
                };
                match (name, args) {
                    ("begin", []) | ("first", []) | ("min", []) => Some(b.clone()),
                    ("end", []) | ("last", []) => Some(e.clone()),
                    // `(b..e).first(n)` / `(b..e).last(n)` — materialise
                    // the slice as a fresh Array. Both refuse negative
                    // `n` with ArgumentError, matching CRuby's
                    // `Array#first/last(n)` policy that #140 mirrored.
                    // For first/last on the empty range (b > e) the
                    // `count == 0` short-circuit returns []. CRuby
                    // uses slightly different wording for the two
                    // sides ("negative array size (or size too big)"
                    // for first vs "negative array size" for last);
                    // matching that exactly so a diff_cruby fixture
                    // can lock both error paths.
                    //
                    // Tracked in #143 alongside the endless-range
                    // negative-n fix above.
                    ("first", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "negative array size (or size too big)".into(),
                            }));
                        }
                        // Cap `n` at `count` so a request bigger than
                        // the range size doesn't try to alloc Vec for
                        // billions of elements. `count` is already
                        // computed safely (saturating) above.
                        let n_taken = (*n).min(count);
                        let n_safe = usize::try_from(n_taken).unwrap_or(usize::MAX);
                        let mut elems: Vec<Value> = Vec::with_capacity(n_safe);
                        let mut v = bi;
                        for _ in 0..n_safe {
                            elems.push(Value::Int(v));
                            v = v.saturating_add(1);
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(nid))
                    }
                    ("last", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "negative array size".into(),
                            }));
                        }
                        // Same `count`-capping rationale as `first(n)`
                        // above. The slice starts at
                        // `bi + (count - n_taken)` and walks `n_taken`
                        // ints upward.
                        //
                        // Earlier shape computed `start` as
                        // `end_inc.saturating_sub(n_taken).saturating_add(1)`,
                        // which had an off-by-one at the i64::MIN
                        // boundary: with `bi == ei == i64::MIN` and
                        // `n_taken == 1`, `i64::MIN - 1` saturates to
                        // `i64::MIN`, the `+ 1` then gives `i64::MIN + 1`,
                        // and the result was `[i64::MIN + 1]` instead
                        // of `[i64::MIN]`. The `bi + (count - n_taken)`
                        // form is safe: `count - n_taken ≥ 0` by the
                        // `n_taken = n.min(count)` cap, and
                        // `bi + (count - n_taken) ≤ bi + count = ei + (1 or 0)`,
                        // which fits in i64 as long as `ei` itself
                        // does. `saturating_add` is paranoia in case
                        // a future change pushes the bound.
                        let n_taken = (*n).min(count);
                        let n_safe = usize::try_from(n_taken).unwrap_or(usize::MAX);
                        let mut elems: Vec<Value> = Vec::with_capacity(n_safe);
                        if count == 0 {
                            let nid = self.heap.alloc(HeapObj::Array(elems));
                            return Ok(Some(Value::Array(nid)));
                        }
                        let start = bi.saturating_add(count.saturating_sub(n_taken));
                        let mut v = start;
                        for _ in 0..n_safe {
                            elems.push(Value::Int(v));
                            v = v.saturating_add(1);
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(nid))
                    }
                    // BigInt arg — CRuby raises RangeError (the
                    // value is too large to fit in a C long).
                    // Mirrors Array#first/#last's BigInt arms and
                    // Hash#first BigInt arm. Without this arm,
                    // BigInt falls into the catch-all and routes
                    // through `arity_error_arg0_or_1_int`, which
                    // renders BigInt's `type_name_for_coerce`
                    // ("Integer") as the nonsensical
                    // "Integer into Integer" TypeError.
                    #[cfg(feature = "bignum")]
                    ("first", [Value::BigInt(_)]) | ("last", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce + catch-all for Range#first /
                    // Range#last (Int+Int branch). Same pattern as
                    // Array#first/#last (PR #349) and Hash#first
                    // above — `first(2.5)` truncates to 2; non-Int
                    // 1-arg raises TypeError instead of NoMethodError.
                    ("first" | "last", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.range_collection_call(id, name, &[Value::Int(n)]);
                    }
                    // CRuby quirk: Range#first / #last use
                    // "expected 1" for multi-arg (even though
                    // 0-arg is also valid), while Array uses
                    // "expected 0..1". Match CRuby's exact wording
                    // by handling multi-arg before the helper —
                    // the helper's catch-all then only fires for
                    // the 1-non-Int case where the TypeError
                    // wording is the same across receivers.
                    ("first" | "last", many) if many.len() > 1 => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 1)",
                                many.len()
                            ),
                        }));
                    }
                    ("first" | "last", _) => {
                        return Err(self.arity_error_arg0_or_1_int(name, args));
                    }
                    ("max", []) => Some(if excl {
                        // ei - 1 overflows when ei == i64::MIN
                        // (e.g. `(-2**63...-2**63).max`); treat as
                        // empty range → nil, matching the
                        // empty-range semantic that Range#sum/inject
                        // already use for the same i64-edge case.
                        match ei.checked_sub(1) {
                            Some(v) => Value::Int(v),
                            None => Value::Nil,
                        }
                    } else { e.clone() }),
                    ("size", []) | ("length", []) | ("count", []) => Some(Value::Int(count)),
                    ("exclude_end?", []) => Some(Value::Bool(excl)),
                    ("include?", [Value::Int(v)]) | ("member?", [Value::Int(v)]) => {
                        let in_r = if excl { *v >= bi && *v < ei } else { *v >= bi && *v <= ei };
                        Some(Value::Bool(in_r))
                    }
                    ("cover?", [Value::Int(v)]) => {
                        let in_r = if excl { *v >= bi && *v < ei } else { *v >= bi && *v <= ei };
                        Some(Value::Bool(in_r))
                    }
                    // BigInt arg with Int bounds: BigInt is always
                    // outside the Int-bounded range UNLESS the
                    // BigInt happens to fit i64 (in which case
                    // bigint_to_value would have demoted it). So
                    // any reachable Value::BigInt arg here is
                    // outside the range — return false. (The
                    // BigInt-bound branch below handles the
                    // BigInt-bound case.)
                    #[cfg(feature = "bignum")]
                    ("include?", [Value::BigInt(_)]) | ("member?", [Value::BigInt(_)]) | ("cover?", [Value::BigInt(_)]) => {
                        Some(Value::Bool(false))
                    }
                    // `r.cover?(other_range)` — true iff the
                    // other range is fully within self. For Int
                    // bounds both sides; mismatched-type endpoints
                    // fall through (None → NoMethodError).
                    ("cover?", [Value::Range(other_id)]) => {
                        let other = self.heap.range(*other_id);
                        let other_excl = other.exclusive;
                        let (ob, oe) = match (&other.begin, &other.end) {
                            (Value::Int(b), Value::Int(e)) => (*b, *e),
                            _ => return Ok(None),
                        };
                        // CRuby: empty sub-ranges do NOT cover —
                        // `(1..10).cover?(8...8)` is false. Empty
                        // means begin >= end (excl) or begin > end
                        // (inclusive).
                        let empty = if other_excl { ob >= oe } else { ob > oe };
                        if empty { return Ok(Some(Value::Bool(false))); }
                        let other_min = ob;
                        let other_max = if other_excl { oe - 1 } else { oe };
                        let lo_ok = other_min >= bi;
                        let hi_ok = if excl { other_max < ei } else { other_max <= ei };
                        Some(Value::Bool(lo_ok && hi_ok))
                    }
                    ("to_a", []) | ("sort", []) => {
                        // `sort` with no block on a Range is just
                        // `to_a` — the underlying sequence is
                        // already non-decreasing for Int bounds.
                        // Descending (bi > ei) is an empty range
                        // in CRuby; we render it as `[]` to match.
                        let mut elems = Vec::with_capacity(count.max(0) as usize);
                        let end_inclusive = if excl { ei - 1 } else { ei };
                        for v in bi..=end_inclusive { elems.push(Value::Int(v)); }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(nid))
                    }
                    // `r.each_slice(n)` / `r.each_cons(n)` —
                    // no-block (Enumerator) forms. CRuby returns
                    // an Enumerator; rubyrs returns the
                    // materialised Array of slices/windows
                    // directly, matching the Array / Hash family
                    // (Enumerator-stub strategy). `.to_a` on
                    // either is a no-op vs forced materialisation
                    // — same shape. Block forms in iter.rs.
                    ("each_slice", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.range_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("each_slice", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("invalid slice size: {}", n),
                            }));
                        }
                        let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                        // Exclusive end at i64::MIN means an empty
                        // range (`min...min`). `saturating_sub(1)`
                        // would underflow to `min` and make the
                        // loop yield once; checked_sub maps it to
                        // an early-return empty Array. Same
                        // pattern as the Range#sum arm.
                        let end_inc = if excl {
                            match ei.checked_sub(1) {
                                Some(v) => v,
                                None => {
                                    self.maybe_gc();
                                    self.check_alloc()?;
                                    let oid = self.heap.alloc(HeapObj::Array(Vec::new()));
                                    return Ok(Some(Value::Array(oid)));
                                }
                            }
                        } else { ei };
                        // Pin each freshly-allocated slice id as
                        // we build the outer chunks Vec — the Vec
                        // is a Rust local, not a GC root, so any
                        // intervening `maybe_gc()` between alloc
                        // and the final outer alloc could sweep
                        // earlier slice ids. PinGuard's Drop
                        // releases them on every exit path.
                        let mut g = PinGuard::new(self);
                        let mut chunks: Vec<Value> = Vec::new();
                        let mut current: Vec<Value> = Vec::with_capacity(n_usz.min(64));
                        let mut i = bi;
                        while i <= end_inc {
                            current.push(Value::Int(i));
                            if current.len() == n_usz {
                                g.vm.maybe_gc();
                                g.vm.check_alloc()?;
                                let cid = g.vm.heap.alloc(HeapObj::Array(std::mem::take(&mut current)));
                                g.pin(Value::Array(cid));
                                chunks.push(Value::Array(cid));
                                current = Vec::with_capacity(n_usz.min(64));
                            }
                            if i == end_inc { break; }
                            i += 1;
                        }
                        if !current.is_empty() {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let cid = g.vm.heap.alloc(HeapObj::Array(current));
                            g.pin(Value::Array(cid));
                            chunks.push(Value::Array(cid));
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let oid = g.vm.heap.alloc(HeapObj::Array(chunks));
                        Some(Value::Array(oid))
                    }
                    // Wrong-arity / non-Int for Range#each_slice no-block form.
                    ("each_slice", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    ("each_cons", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.range_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("each_cons", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("invalid size: {}", n),
                            }));
                        }
                        let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                        // See each_slice arm above — checked_sub
                        // for the exclusive-end-at-i64::MIN edge.
                        let end_inc = if excl {
                            match ei.checked_sub(1) {
                                Some(v) => v,
                                None => {
                                    self.maybe_gc();
                                    self.check_alloc()?;
                                    let oid = self.heap.alloc(HeapObj::Array(Vec::new()));
                                    return Ok(Some(Value::Array(oid)));
                                }
                            }
                        } else { ei };
                        // Early-return empty when range_len < n
                        // — no windows can be yielded; avoid the
                        // O(range_len) scan + buffering. Overflow
                        // on `end_inc - bi + 1` is treated as
                        // "len is huge, don't early-return".
                        let too_short = if bi > end_inc {
                            true
                        } else {
                            match end_inc.checked_sub(bi).and_then(|d| d.checked_add(1)) {
                                Some(len) => len < *n,
                                None => false,
                            }
                        };
                        if too_short {
                            self.maybe_gc();
                            self.check_alloc()?;
                            let oid = self.heap.alloc(HeapObj::Array(Vec::new()));
                            return Ok(Some(Value::Array(oid)));
                        }
                        let mut g = PinGuard::new(self);
                        let mut windows: Vec<Value> = Vec::new();
                        let mut buf: std::collections::VecDeque<Value> =
                            std::collections::VecDeque::with_capacity(n_usz.min(64));
                        let mut i = bi;
                        while i <= end_inc {
                            if buf.len() == n_usz { buf.pop_front(); }
                            buf.push_back(Value::Int(i));
                            if buf.len() == n_usz {
                                g.vm.maybe_gc();
                                g.vm.check_alloc()?;
                                let win: Vec<Value> = buf.iter().cloned().collect();
                                let wid = g.vm.heap.alloc(HeapObj::Array(win));
                                g.pin(Value::Array(wid));
                                windows.push(Value::Array(wid));
                            }
                            if i == end_inc { break; }
                            i += 1;
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let oid = g.vm.heap.alloc(HeapObj::Array(windows));
                        Some(Value::Array(oid))
                    }
                    // Wrong-arity / non-Int for Range#each_cons no-block form.
                    ("each_cons", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    // `r.chunk_while(arg)` / `r.slice_when(arg)` without
                    // a block — arity guard mirrors Array's no-block arm
                    // and the block-form catch-all in iter.rs.
                    ("chunk_while" | "slice_when", many) if !many.is_empty() => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len()
                            ),
                        }));
                    }
                    // Range#step(n) without a block returns a
                    // step-arithmetic Array. The block form is
                    // covered separately in collection_call_block.
                    ("step", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("step can't be {}", n),
                            }));
                        }
                        let end_inc = if excl { ei - 1 } else { ei };
                        let mut elems: Vec<Value> = Vec::new();
                        let mut v = bi;
                        while v <= end_inc {
                            elems.push(Value::Int(v));
                            v = v.saturating_add(*n);
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(nid))
                    }
                    ("sum", []) | ("sum", [Value::Int(_)]) => {
                        let init = match args { [Value::Int(n)] => *n, _ => 0 };
                        // ei - 1 can overflow when ei == i64::MIN
                        // (e.g. `(-2**63...-2**63)` — exclusive end
                        // at the minimum). Treat that as an empty
                        // range, same as `bi > end_inc`.
                        let end_inc = if excl {
                            match ei.checked_sub(1) {
                                Some(v) => v,
                                None => return Ok(Some(Value::Int(init))),
                            }
                        } else { ei };
                        if bi > end_inc { return Ok(Some(Value::Int(init))); }
                        // n * (bi + end_inc) / 2 closed form. The
                        // i64 fast path computes EVERY step with
                        // checked_* (including `n = end_inc - bi + 1`,
                        // which can overflow on extremely wide ranges
                        // like `i64::MIN..i64::MAX`). On any overflow
                        // we fall through to the BigInt branch (or
                        // the wrapping legacy arm with bignum off).
                        // The BigInt branch computes everything from
                        // the endpoints in BigInt space — never
                        // touches the i64 `n` — so a fast-path
                        // overflow can't carry a wrapped value into
                        // the precise calculation.
                        if let Some(total) = (|| -> Option<i64> {
                            let n_i64 = end_inc.checked_sub(bi)?.checked_add(1)?;
                            let sum = bi.checked_add(end_inc)?;
                            let prod = n_i64.checked_mul(sum)?;
                            init.checked_add(prod / 2)
                        })() {
                            return Ok(Some(Value::Int(total)));
                        }
                        #[cfg(feature = "bignum")]
                        {
                            use num_bigint::BigInt;
                            let big_bi = BigInt::from(bi);
                            let big_end = BigInt::from(end_inc);
                            // n = end_inc - bi + 1, computed in BigInt
                            // so we don't inherit the i64 overflow from
                            // the fast path above.
                            let big_n = &big_end - &big_bi + 1;
                            let big_sum = &big_bi + &big_end;
                            let big_total = BigInt::from(init) + (big_n * big_sum) / 2;
                            return Ok(Some(self.bigint_to_value(big_total)?));
                        }
                        #[cfg(not(feature = "bignum"))]
                        {
                            let n = end_inc.wrapping_sub(bi).wrapping_add(1);
                            let s = n.wrapping_mul(bi.wrapping_add(end_inc)) / 2;
                            Some(Value::Int(init.wrapping_add(s)))
                        }
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        // ei - 1 can overflow when ei == i64::MIN
                        // (exclusive end at the minimum); empty range.
                        let end_inc = if excl {
                            match ei.checked_sub(1) {
                                Some(v) => v,
                                None => return Ok(Some(Value::Nil)),
                            }
                        } else { ei };
                        if bi > end_inc { return Ok(Some(Value::Nil)); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        let mut acc = Value::Int(bi);
                        // bi + 1 can overflow when bi == i64::MAX
                        // (e.g. `(i64::MAX..i64::MAX).inject(:+)`).
                        // After the empty-range early returns above
                        // we know bi <= end_inc, but bi == i64::MAX
                        // still hits this. Treat that as a singleton:
                        // acc already holds the only element.
                        let mut i = match bi.checked_add(1) {
                            Some(v) => v,
                            None => return Ok(Some(acc)),
                        };
                        // Singleton inclusive range (e.g. `(1..1)`):
                        // bi == end_inc, so i = bi + 1 > end_inc and
                        // the loop body must NOT run. Without this
                        // guard the loop runs anyway (i starts > end_inc
                        // but the `if i == end_inc break` never fires),
                        // incrementing i forever and hanging the host.
                        // Real bug found by Copilot cycle 12.
                        if i > end_inc { return Ok(Some(acc)); }
                        // Single shared increment site. The BigInt
                        // arm previously had its own `i += 1; continue;`
                        // which double-incremented; now both arms
                        // share the bottom-of-loop step. The
                        // increment uses checked_add so a fold over
                        // a range ending at i64::MAX terminates
                        // cleanly instead of wrapping into an
                        // infinite loop.
                        loop {
                            match &acc {
                                Value::Int(x) => {
                                    if matches!(kind, crate::bytecode::BinOpKind::Div | crate::bytecode::BinOpKind::Mod) && i == 0 {
                                        return Err(self.trap(RubyError::ZeroDivisionError {
                                            msg: "divided by 0".to_string(),
                                        }));
                                    }
                                    acc = self.apply_int_promote(kind, *x, i)?;
                                }
                                _ => {
                                    #[cfg(feature = "bignum")]
                                    if let Some(next) = self.try_bigint_binop(kind, &acc, &Value::Int(i))? {
                                        acc = next;
                                    } else {
                                        return Ok(None);
                                    }
                                    #[cfg(not(feature = "bignum"))]
                                    return Ok(None);
                                }
                            }
                            if i == end_inc { break; }
                            i = match i.checked_add(1) {
                                Some(v) => v,
                                None => break, // overflow at end of range
                            };
                        }
                        Some(acc)
                    }
                    _ => None,
                }
        })
    }
}
