//! `Range` methods that need heap access. Mirrors CRuby's
//! `range.c`. Dispatched from `Vm::collection_call`'s
//! `Value::Range` arm.

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::Vm;

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
                            ("include?", [Value::Str(needle)]) | ("cover?", [Value::Str(needle)]) => {
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
                        ("first", [Value::Int(n)]) => {
                            // Endless (1..) supports first(n);
                            // beginless (..n) doesn't (no anchor
                            // for "first").
                            if let Some(bi) = begin_int {
                                let n = (*n).max(0);
                                let mut out: Vec<Value> = Vec::with_capacity(n as usize);
                                let mut v = bi;
                                for _ in 0..n {
                                    out.push(Value::Int(v));
                                    v = v.saturating_add(1);
                                }
                                self.maybe_gc();
                                let nid = self.heap.alloc(HeapObj::Array(out));
                                return Ok(Some(Value::Array(nid)));
                            }
                            return Ok(None);
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
                        ("include?", [Value::Int(v)]) => {
                            let lo_ok = match begin_int { Some(lo) => *v >= lo, None => true };
                            let hi_ok = match end_int {
                                Some(hi) => if excl { *v < hi } else { *v <= hi },
                                None => true,
                            };
                            return Ok(Some(Value::Bool(lo_ok && hi_ok)));
                        }
                        ("exclude_end?", []) => return Ok(Some(Value::Bool(excl))),
                        _ => return Ok(None),
                    }
                }
                let (bi, ei) = (begin_int.unwrap(), end_int.unwrap());
                let count = if excl { (ei - bi).max(0) } else { (ei - bi + 1).max(0) };
                match (name, args) {
                    ("begin", []) | ("first", []) | ("min", []) => Some(b.clone()),
                    ("end", []) | ("last", []) => Some(e.clone()),
                    ("max", []) => Some(if excl { Value::Int(ei - 1) } else { e.clone() }),
                    ("size", []) | ("length", []) | ("count", []) => Some(Value::Int(count)),
                    ("exclude_end?", []) => Some(Value::Bool(excl)),
                    ("include?", [Value::Int(v)]) => {
                        let in_r = if excl { *v >= bi && *v < ei } else { *v >= bi && *v <= ei };
                        Some(Value::Bool(in_r))
                    }
                    ("cover?", [Value::Int(v)]) => {
                        let in_r = if excl { *v >= bi && *v < ei } else { *v >= bi && *v <= ei };
                        Some(Value::Bool(in_r))
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
                        let end_inc = if excl { ei - 1 } else { ei };
                        if bi > end_inc { return Ok(Some(Value::Int(init))); }
                        let n = end_inc - bi + 1;
                        let s = n.wrapping_mul(bi.wrapping_add(end_inc)) / 2;
                        Some(Value::Int(init.wrapping_add(s)))
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        let end_inc = if excl { ei - 1 } else { ei };
                        if bi > end_inc { return Ok(Some(Value::Nil)); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        let mut acc = Value::Int(bi);
                        let mut i = bi + 1;
                        while i <= end_inc {
                            // Same overflow-promotion shape as
                            // Array#inject: once `acc` becomes BigInt
                            // (e.g. `(1..30).inject(:*)`), fall through
                            // to `try_bigint_binop` so the fold
                            // continues in arbitrary precision instead
                            // of bailing the whole primitive.
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
                                        i += 1;
                                        continue;
                                    }
                                    return Ok(None);
                                }
                            }
                            i += 1;
                        }
                        Some(acc)
                    }
                    _ => None,
                }
        })
    }
}
