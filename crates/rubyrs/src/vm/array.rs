//! `Array` methods that need heap access. Mirrors CRuby's
//! `array.c`. Dispatched from `Vm::collection_call`'s
//! `Value::Array` arm.
//!
//! Currently only the no-block methods (everything in
//! `collection_call`'s Array arm). The block-form Array methods
//! (`each` / `map` / `partition` / etc.) still live inline in
//! `collection_call_block`; they'll move to this file in a
//! follow-up cut once their iterator-driver helpers are
//! similarly grouped.

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::{value_cmp_v, PinGuard, Vm};

impl Vm {
    /// Array#X methods that don't take a block. Returns
    /// `Ok(Some(v))` on a hit; `Ok(None)` on miss so the caller
    /// falls through to the universal `equal?` / `==` etc. arms.
    pub(crate) fn array_collection_call(
        &mut self,
        id: ObjId,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        Ok(
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.array(id).len() as i64)),
                    ("push", [v]) | ("<<", [v]) => {
                        // P2-14c: refuse a push that would make this
                        // Array's storage exceed the per-value byte
                        // cap. We size in bytes-of-Value because that's
                        // what the host actually pays for in RAM.
                        let new_len = self.heap.array(id).len().saturating_add(1);
                        if let Some(max) = self.max_value_bytes
                            && new_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array.push would exceed {max} bytes"),
                                }));
                            }
                        self.heap.array_mut(id).push(v.clone());
                        Some(Value::Array(id))
                    }
                    ("[]", [Value::Int(i)]) => {
                        let a = self.heap.array(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                        Some(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                    }
                    // Internal helpers for multi-write splat
                    // destructuring (`a, *r, b = arr`).
                    //
                    // `__mw_splat(start, post)` returns the
                    // middle slice as a fresh Array; underflow
                    // (`len < start + post`) yields `[]`.
                    //
                    // `__mw_get(i, post)` returns `self[i]` if a
                    // pre-splat position truly has an element to
                    // claim once the post-splat slots reserve
                    // theirs (`i < len - post`); otherwise nil.
                    // Without this guard, `a, *m, b = [1]` would
                    // wrongly bind `a = 1` instead of `nil`.
                    ("__mw_splat", [Value::Int(start), Value::Int(post)]) => {
                        let a = self.heap.array(id);
                        let len = a.len() as i64;
                        let s = (*start).max(0).min(len);
                        let p = (*post).max(0).min((len - s).max(0));
                        let slice_len = (len - s - p).max(0) as usize;
                        let s = s as usize;
                        let slice: Vec<Value> = a[s..s + slice_len].to_vec();
                        if let Some(max) = self.max_value_bytes
                            && slice.len().saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("multi-write splat would exceed {max} bytes"),
                                }));
                            }
                        self.maybe_gc();
                        let new_id = self.heap.alloc(HeapObj::Array(slice));
                        Some(Value::Array(new_id))
                    }
                    // `__mw_post(j, pre_count, post_count)` —
                    // returns the value for the `j`th post-splat
                    // target (0-indexed from the left of the
                    // post group). CRuby's rule:
                    // `post_start = max(pre_count, len - post_count)`,
                    // then `post[j] = arr[post_start + j]` (OOB → nil).
                    // This pins post-targets to indices >= pre_count
                    // (so pre never gets overwritten) while
                    // sliding them rightward when the array is
                    // long enough to give all post slots their
                    // natural "from the end" positions.
                    ("__mw_post", [Value::Int(j), Value::Int(pre_n), Value::Int(post_n)]) => {
                        let a = self.heap.array(id);
                        let len = a.len() as i64;
                        let pre = (*pre_n).max(0);
                        let post = (*post_n).max(0);
                        let post_start = pre.max(len - post);
                        let idx = post_start + *j;
                        if idx < 0 {
                            Some(Value::Nil)
                        } else {
                            Some(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                        }
                    }
                    ("[]=", [Value::Int(i), v]) => {
                        let a = self.heap.array_mut(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i } as usize;
                        // Same cap check as `push` — `[]=` past the
                        // end pads with `nil` and so can grow the
                        // backing Vec without bound.
                        let needed_len = idx.saturating_add(1).max(a.len());
                        if let Some(max) = self.max_value_bytes
                            && needed_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array []= would exceed {max} bytes"),
                                }));
                            }
                        let a = self.heap.array_mut(id);
                        while a.len() <= idx { a.push(Value::Nil); }
                        a[idx] = v.clone();
                        Some(v.clone())
                    }
                    ("first", []) => Some(self.heap.array(id).first().cloned().unwrap_or(Value::Nil)),
                    ("dig", keys) if !keys.is_empty() => {
                        let mut cur = Value::Array(id);
                        for key in keys {
                            cur = self.dig_step(&cur, key)?;
                            if matches!(cur, Value::Nil) { break; }
                        }
                        Some(cur)
                    }
                    ("last", []) => Some(self.heap.array(id).last().cloned().unwrap_or(Value::Nil)),
                    ("empty?", []) => Some(Value::Bool(self.heap.array(id).is_empty())),
                    ("include?", [needle]) => {
                        let a = self.heap.array(id);
                        let hit = a.iter().any(|x| x.ruby_eq(needle, &self.heap));
                        Some(Value::Bool(hit))
                    }
                    // `arr.pack(format)` — binary packing, inverse
                    // of `String#unpack`. Same directive subset:
                    // C/c, n/N, v/V, q/Q, a/A/Z. Documented
                    // divergence: exotic specs (m, U, w, f/d/e/E)
                    // raise ArgumentError.
                    ("pack", [Value::Str(fmt)]) => {
                        let snapshot: Vec<Value> = self.heap.array(id).clone();
                        let fmt_str = fmt.to_string_lossy();
                        let bytes = super::string::pack_values(&snapshot, &fmt_str)
                            .map_err(|m| self.trap(RubyError::ArgumentError { msg: m }))?;
                        Some(Value::new_str_bytes(bytes))
                    }
                    // `arr.assoc(needle)` — first sub-Array whose
                    // `[0]` equals `needle`; nil if none. Sub-Array
                    // is returned by reference (CRuby returns the
                    // same Array; we clone the Value but its inner
                    // ObjId points at the same heap slot).
                    ("assoc", [needle]) => {
                        let snapshot: Vec<Value> = self.heap.array(id).clone();
                        for v in snapshot {
                            if let Value::Array(sub_id) = v {
                                let sub = self.heap.array(sub_id);
                                if let Some(first) = sub.first()
                                    && first.ruby_eq(needle, &self.heap) {
                                        return Ok(Some(Value::Array(sub_id)));
                                }
                            }
                        }
                        Some(Value::Nil)
                    }
                    // `arr.rassoc(needle)` — first sub-Array whose
                    // `[1]` equals `needle`. Same shape as assoc;
                    // skips non-Array elements.
                    ("rassoc", [needle]) => {
                        let snapshot: Vec<Value> = self.heap.array(id).clone();
                        for v in snapshot {
                            if let Value::Array(sub_id) = v {
                                let sub = self.heap.array(sub_id);
                                if sub.len() >= 2
                                    && sub[1].ruby_eq(needle, &self.heap) {
                                        return Ok(Some(Value::Array(sub_id)));
                                }
                            }
                        }
                        Some(Value::Nil)
                    }
                    // `arr.combination(n)` — every n-element subset
                    // in lexicographic order. `n == 0` → `[[]]`;
                    // `n > len` → `[]`. Non-block form returns the
                    // materialised Array of Arrays (no Enumerator
                    // in the subset).
                    ("combination", [Value::Int(n)]) => {
                        let n_take = *n;
                        let snapshot: Vec<Value> = self.heap.array(id).clone();
                        let len = snapshot.len();
                        let mut out: Vec<Value> = Vec::new();
                        if n_take == 0 {
                            self.maybe_gc();
                            let empty_id = self.heap.alloc(HeapObj::Array(Vec::new()));
                            out.push(Value::Array(empty_id));
                        } else if n_take > 0 && (n_take as usize) <= len {
                            let k = n_take as usize;
                            let mut idx: Vec<usize> = (0..k).collect();
                            loop {
                                let pick: Vec<Value> = idx.iter().map(|&i| snapshot[i].clone()).collect();
                                self.maybe_gc();
                                let pid = self.heap.alloc(HeapObj::Array(pick));
                                out.push(Value::Array(pid));
                                // Advance idx like CRuby (rightmost
                                // that can still advance bumps; tail
                                // resets to consecutive).
                                let mut i = k;
                                while i > 0 {
                                    i -= 1;
                                    if idx[i] < len - (k - i) {
                                        idx[i] += 1;
                                        for j in (i + 1)..k { idx[j] = idx[j - 1] + 1; }
                                        break;
                                    }
                                    if i == 0 { i = k; break; }  // exhausted
                                }
                                if i == k { break; }
                            }
                        }
                        self.maybe_gc();
                        let result_id = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(result_id))
                    }
                    // `arr.permutation` / `arr.permutation(n)` — every
                    // n-element ordered arrangement. Defaults to
                    // length (full permutations). Edge cases match
                    // CRuby: `n == 0` → `[[]]`; `n > len` → `[]`.
                    ("permutation", []) | ("permutation", [Value::Int(_)]) => {
                        let snapshot: Vec<Value> = self.heap.array(id).clone();
                        let len = snapshot.len();
                        let n_take = match args {
                            [Value::Int(n)] => *n,
                            _ => len as i64,
                        };
                        let mut out: Vec<Value> = Vec::new();
                        if n_take == 0 {
                            self.maybe_gc();
                            let empty_id = self.heap.alloc(HeapObj::Array(Vec::new()));
                            out.push(Value::Array(empty_id));
                        } else if n_take > 0 && (n_take as usize) <= len {
                            let k = n_take as usize;
                            // Recursive lexicographic enumeration —
                            // pick index sets without repetition,
                            // each in source order.
                            let indices: Vec<usize> = (0..len).collect();
                            let mut current: Vec<usize> = Vec::with_capacity(k);
                            let mut used = vec![false; len];
                            fn rec(
                                indices: &[usize], used: &mut [bool],
                                current: &mut Vec<usize>, k: usize,
                                // `_snapshot` is only forwarded through
                                // recursion — kept to preserve the
                                // call-site shape; clippy needs the
                                // underscore prefix to skip its
                                // only-used-in-recursion lint.
                                _snapshot: &[Value], out: &mut Vec<Vec<usize>>,
                            ) {
                                if current.len() == k {
                                    out.push(current.clone());
                                    return;
                                }
                                for &i in indices {
                                    if used[i] { continue; }
                                    used[i] = true;
                                    current.push(i);
                                    rec(indices, used, current, k, _snapshot, out);
                                    current.pop();
                                    used[i] = false;
                                }
                            }
                            let mut perms: Vec<Vec<usize>> = Vec::new();
                            rec(&indices, &mut used, &mut current, k, &snapshot, &mut perms);
                            for p in perms {
                                let pick: Vec<Value> = p.into_iter().map(|i| snapshot[i].clone()).collect();
                                self.maybe_gc();
                                let pid = self.heap.alloc(HeapObj::Array(pick));
                                out.push(Value::Array(pid));
                            }
                        }
                        self.maybe_gc();
                        let result_id = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(result_id))
                    }
                    // `arr.tally` — count occurrences into a Hash
                    // keyed by element value, value=occurrence
                    // count. Preserves first-appearance insertion
                    // order. Pure value-equality via `ruby_eq`.
                    ("tally", []) => {
                        let snapshot: Vec<Value> = self.heap.array(id).clone();
                        let mut pairs: Vec<(Value, Value)> = Vec::new();
                        for v in snapshot {
                            let pos = pairs.iter()
                                .position(|(k, _)| k.ruby_eq(&v, &self.heap));
                            if let Some(p) = pos {
                                if let Value::Int(n) = pairs[p].1 {
                                    pairs[p].1 = Value::Int(n + 1);
                                }
                            } else {
                                pairs.push((v, Value::Int(1)));
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(pairs));
                        Some(Value::Hash(nid))
                    }
                    ("count", []) => Some(Value::Int(self.heap.array(id).len() as i64)),
                    ("count", [needle]) => {
                        let a = self.heap.array(id);
                        let n = a.iter().filter(|x| x.ruby_eq(needle, &self.heap)).count();
                        Some(Value::Int(n as i64))
                    }
                    ("sum", []) | ("sum", [Value::Int(_)]) => {
                        let init = match args { [Value::Int(n)] => *n, _ => 0 };
                        let a = self.heap.array(id);
                        let mut s: i64 = init;
                        for v in a {
                            match v {
                                Value::Int(n) => s = s.wrapping_add(*n),
                                _ => return Ok(None),
                            }
                        }
                        Some(Value::Int(s))
                    }
                    ("min", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v(v, &best, &self.interner) {
                                Some(std::cmp::Ordering::Less) => best = v.clone(),
                                Some(_) => {}
                                None => return Ok(None),
                            }
                        }
                        Some(best)
                    }
                    ("max", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v(v, &best, &self.interner) {
                                Some(std::cmp::Ordering::Greater) => best = v.clone(),
                                Some(_) => {}
                                None => return Ok(None),
                            }
                        }
                        Some(best)
                    }
                    ("sort", []) => {
                        // Insertion sort with synchronous dispatch
                        // through `user_cmp`. O(n²) but correctness-
                        // critical: we can't use Rust's sort_by here
                        // because invoking a user method during the
                        // comparison closure would alias `&mut Vm`
                        // while the Vec borrow is live. For arrays
                        // of built-in types the fast path stays
                        // value_cmp_v; user-classed elements go
                        // through their `<=>` method via user_cmp.
                        //
                        // PinGuard wraps the entire impl: `copy` is
                        // a Rust local with the receiver's element
                        // ObjIds, NOT on `vm.stack`/`vm.pinned`. The
                        // `user_cmp` call may invoke a user `<=>`
                        // method which can trigger `maybe_gc` → sweeps
                        // copy's contents → next access panics with
                        // ICE use-after-free. Pin the receiver Array
                        // so its children stay reachable via the GC
                        // mark walk.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let mut copy: Vec<Value> = g.vm.heap.array(id).clone();
                        let n = copy.len();
                        for i in 1..n {
                            let mut j = i;
                            while j > 0 {
                                let ord = g.vm.user_cmp(&copy[j - 1], &copy[j])?;
                                match ord {
                                    None => return Ok(None),
                                    Some(std::cmp::Ordering::Greater) => {
                                        copy.swap(j - 1, j);
                                        j -= 1;
                                    }
                                    _ => break,
                                }
                            }
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(copy));
                        Some(Value::Array(nid))
                    }
                    ("inject", [Value::Sym(op_sym)]) | ("reduce", [Value::Sym(op_sym)]) => {
                        let a = self.heap.array(id).clone();
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        let mut acc = a[0].clone();
                        for v in &a[1..] {
                            match (&acc, v) {
                                (Value::Int(x), Value::Int(y)) => {
                                    if matches!(kind, crate::bytecode::BinOpKind::Div | crate::bytecode::BinOpKind::Mod) && *y == 0 {
                                        return Err(self.trap(RubyError::ZeroDivisionError {
                                            msg: "divided by 0".to_string(),
                                        }));
                                    }
                                    acc = kind.apply_int(*x, *y);
                                }
                                _ => return Ok(None),
                            }
                        }
                        Some(acc)
                    }
                    ("to_a", []) => Some(Value::Array(id)),
                    ("inspect", []) => {
                        let s = Value::Array(id).to_inspect(&self.heap, &self.interner);
                        Some(Value::new_str(s))
                    }
                    ("reverse", []) => {
                        let rev: Vec<Value> = self.heap.array(id).iter().rev().cloned().collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(rev));
                        Some(Value::Array(nid))
                    }
                    ("uniq", []) => {
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if !out.iter().any(|x| x.ruby_eq(v, &self.heap)) {
                                out.push(v.clone());
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("compact", []) => {
                        let out: Vec<Value> = self.heap.array(id).iter()
                            .filter(|v| !matches!(v, Value::Nil))
                            .cloned()
                            .collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // In-place bang variants. Mutate the receiver
                    // and return either self (always for `sort!`)
                    // or self/nil depending on whether anything
                    // actually changed (matching CRuby for
                    // `uniq!` / `compact!` / `flatten!`).
                    ("sort!", []) => {
                        let mut copy = self.heap.array(id).clone();
                        let n = copy.len();
                        for i in 1..n {
                            let mut j = i;
                            while j > 0 {
                                let ord = self.user_cmp(&copy[j - 1], &copy[j])?;
                                match ord {
                                    None => return Ok(None),
                                    Some(std::cmp::Ordering::Greater) => {
                                        copy.swap(j - 1, j);
                                        j -= 1;
                                    }
                                    _ => break,
                                }
                            }
                        }
                        *self.heap.array_mut(id) = copy;
                        Some(Value::Array(id))
                    }
                    ("uniq!", []) => {
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if !out.iter().any(|x| x.ruby_eq(v, &self.heap)) {
                                out.push(v.clone());
                            }
                        }
                        if out.len() == src.len() {
                            // Nothing deduped — CRuby returns nil.
                            Some(Value::Nil)
                        } else {
                            *self.heap.array_mut(id) = out;
                            Some(Value::Array(id))
                        }
                    }
                    ("compact!", []) => {
                        let src = self.heap.array(id).clone();
                        let out: Vec<Value> = src.iter()
                            .filter(|v| !matches!(v, Value::Nil))
                            .cloned()
                            .collect();
                        if out.len() == src.len() {
                            Some(Value::Nil)
                        } else {
                            *self.heap.array_mut(id) = out;
                            Some(Value::Array(id))
                        }
                    }
                    ("flatten!", []) => {
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        let mut changed = false;
                        for v in &src {
                            if let Value::Array(inner) = v {
                                changed = true;
                                for x in self.heap.array(*inner) { out.push(x.clone()); }
                            } else {
                                out.push(v.clone());
                            }
                        }
                        if !changed {
                            Some(Value::Nil)
                        } else {
                            *self.heap.array_mut(id) = out;
                            Some(Value::Array(id))
                        }
                    }
                    ("reverse!", []) => {
                        self.heap.array_mut(id).reverse();
                        Some(Value::Array(id))
                    }
                    ("flatten", []) => {
                        // Depth-1 flatten — same as CRuby's default `flatten(1)`
                        // is recursive; ours stops at depth 1 to match the
                        // CRuby behaviour we exercise in fixtures. Document
                        // unbounded recursion as a follow-up if needed.
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if let Value::Array(inner) = v {
                                for x in self.heap.array(*inner) { out.push(x.clone()); }
                            } else {
                                out.push(v.clone());
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("join", []) => {
                        let parts: Vec<String> = self.heap.array(id).iter()
                            .map(|v| v.to_display(&self.heap, &self.interner))
                            .collect();
                        Some(Value::new_str(parts.join("")))
                    }
                    ("join", [Value::Str(sep)]) => {
                        let parts: Vec<String> = self.heap.array(id).iter()
                            .map(|v| v.to_display(&self.heap, &self.interner))
                            .collect();
                        Some(Value::new_str(parts.join(sep.to_string_lossy().as_str())))
                    }
                    ("+", [Value::Array(other)]) => {
                        // Pin both source Arrays across maybe_gc — by the
                        // time we get here the receiver has been popped
                        // from the operand stack (by `do_call`'s drain
                        // path), and the rhs is held only in `do_call`'s
                        // local `args: Vec<Value>` which is handed through
                        // `collection_call` → here as the `args: &[Value]`
                        // slice. Heap-typed elements
                        // inside either Array (e.g. a trailing kwargs
                        // Hash in a mixed-splat call expansion like
                        // `f(*arr, c: 100)`) would otherwise have no
                        // GC root and STRESS_GC sweeps them — the new
                        // alloc reuses their slots, and dispatch later
                        // panics with "heap slot is not a Hash".
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.pin(Value::Array(*other));
                        let mut out: Vec<Value> = g.vm.heap.array(id).clone();
                        let extra: Vec<Value> = g.vm.heap.array(*other).clone();
                        out.extend(extra);
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("-", [Value::Array(other)]) => {
                        // Same root-hole pattern as `+` above —
                        // pin both source Arrays before maybe_gc.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.pin(Value::Array(*other));
                        let src = g.vm.heap.array(id).clone();
                        let exclude = g.vm.heap.array(*other).clone();
                        let out: Vec<Value> = src.into_iter()
                            .filter(|v| !exclude.iter().any(|x| x.ruby_eq(v, &g.vm.heap)))
                            .collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("concat", [Value::Array(other)]) => {
                        // In-place: extend self with other's elements, return self.
                        let extra: Vec<Value> = self.heap.array(*other).clone();
                        self.heap.array_mut(id).extend(extra);
                        Some(Value::Array(id))
                    }
                    ("take", [Value::Int(n)]) => {
                        // Pin the receiver across maybe_gc: by the
                        // time we get here the receiver Array has
                        // been popped from the operand stack, so its
                        // children (the cloned ObjIds in `out`) have
                        // no GC root and STRESS_GC sweeps them.
                        let n = (*n).max(0) as usize;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().take(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("drop", [Value::Int(n)]) => {
                        let n = (*n).max(0) as usize;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().skip(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // No-block `each_slice(n)` / `each_cons(n)` —
                    // CRuby returns an Enumerator we don't model;
                    // instead, return the Array of slices/windows
                    // directly. Calling `.to_a` on the result is
                    // a no-op, so the canonical
                    // `arr.each_slice(2).to_a` idiom still works.
                    ("each_slice", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("invalid slice size: {}", n),
                            }));
                        }
                        let n = *n as usize;
                        let src: Vec<Value> = self.heap.array(id).clone();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let mut chunks: Vec<Value> = Vec::new();
                        for chunk in src.chunks(n) {
                            g.vm.maybe_gc();
                            let cid = g.vm.heap.alloc(HeapObj::Array(chunk.to_vec()));
                            g.pin(Value::Array(cid));
                            chunks.push(Value::Array(cid));
                        }
                        g.vm.maybe_gc();
                        let oid = g.vm.heap.alloc(HeapObj::Array(chunks));
                        Some(Value::Array(oid))
                    }
                    ("each_cons", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("invalid size: {}", n),
                            }));
                        }
                        let n = *n as usize;
                        let src: Vec<Value> = self.heap.array(id).clone();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let mut windows: Vec<Value> = Vec::new();
                        if src.len() >= n {
                            for win in src.windows(n) {
                                g.vm.maybe_gc();
                                let wid = g.vm.heap.alloc(HeapObj::Array(win.to_vec()));
                                g.pin(Value::Array(wid));
                                windows.push(Value::Array(wid));
                            }
                        }
                        g.vm.maybe_gc();
                        let oid = g.vm.heap.alloc(HeapObj::Array(windows));
                        Some(Value::Array(oid))
                    }
                    // `zip` — pairs each element of `self` with the
                    // same-index element of each Array argument.
                    // Result length is the receiver's length. Shorter
                    // arguments pad with `nil`; longer arguments are
                    // truncated. Block form (`zip { ... }` for side
                    // effects, returning nil) is not supported — use
                    // `.zip(...).each` instead.
                    ("zip", rest) => {
                        let mut others: Vec<Vec<Value>> = Vec::with_capacity(rest.len());
                        for a in rest {
                            match a {
                                Value::Array(oid) => others.push(self.heap.array(*oid).clone()),
                                _ => return Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "wrong argument type {} (must respond to :to_ary)",
                                        a.type_name(),
                                    ),
                                })),
                            }
                        }
                        let base = self.heap.array(id).clone();
                        let row_width = 1 + others.len();
                        if let Some(max) = self.max_value_bytes {
                            let projected = base.len()
                                .saturating_mul(row_width)
                                .saturating_mul(std::mem::size_of::<Value>());
                            if projected > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array#zip would exceed {max} bytes"),
                                }));
                            }
                        }
                        // PinGuard the freshly-alloc'd row Arrays: their
                        // ObjIds live in a Rust local `out` Vec, NOT on
                        // `vm.stack` / `vm.pinned`, so the explicit
                        // `maybe_gc()` after the loop (or a STRESS_GC
                        // gc on every alloc) would sweep them and the
                        // outer `heap.alloc(out)` would then panic with
                        // `ICE: use-after-free`. Same shape as L1.5 P0-A.
                        let nid = {
                            let mut g = PinGuard::new(self);
                            let mut out: Vec<Value> = Vec::with_capacity(base.len());
                            for (i, v) in base.iter().enumerate() {
                                let mut row: Vec<Value> = Vec::with_capacity(row_width);
                                row.push(v.clone());
                                for o in &others {
                                    row.push(o.get(i).cloned().unwrap_or(Value::Nil));
                                }
                                let rid = g.vm.heap.alloc(HeapObj::Array(row));
                                let rv = Value::Array(rid);
                                g.pin(rv.clone());
                                out.push(rv);
                            }
                            g.vm.maybe_gc();
                            g.vm.heap.alloc(HeapObj::Array(out))
                        };
                        Some(Value::Array(nid))
                    }
                    _ => None,
                }
    )
    }
}
