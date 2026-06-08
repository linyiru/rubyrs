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

use super::{value_cmp_v_heap, PinGuard, Vm};

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
                    // `freeze` / `frozen?` — CRuby tracks an immutability
                    // bit per object; rubyrs doesn't model it. Both are
                    // no-ops that match CRuby's signature: `freeze`
                    // returns the receiver (chainable, used by tilt's
                    // `EMPTY_ARRAY = [].freeze` constant), `frozen?`
                    // returns false (we never freeze). Real enforcement
                    // is a documented gap in SUBSET.md. Wrong-arity
                    // still raises ArgumentError, matching CRuby, so
                    // caller bugs don't get misreported as missing
                    // methods by the no-recv fall-through.
                    ("freeze", []) => Some(Value::Array(id)),
                    ("frozen?", []) => Some(Value::Bool(false)),
                    // Array#<=> — element-wise lex compare; length is
                    // the tiebreaker when the common prefix is Equal.
                    // Returns nil when any element pair is incompara-
                    // ble (cross-type with no ordering). Delegates to
                    // `value_cmp_v_heap`, which recurses into nested
                    // Arrays so `[[1,2],[3,4]] <=> [[1,2],[3,5]]` works.
                    ("<=>", [Value::Array(other)]) => {
                        Some(match super::util::value_cmp_v_heap(
                            &Value::Array(id),
                            &Value::Array(*other),
                            &self.interner,
                            &self.heap,
                        ) {
                            Some(o) => Value::Int(o as i64),
                            None => Value::Nil,
                        })
                    }
                    ("<=>", [_]) => Some(Value::Nil),
                    // Wrong arity (0 or 2+ args) — CRuby raises
                    // ArgumentError here. The catch-all in the
                    // outer dispatcher would otherwise surface
                    // NoMethodError, which mis-reports a real
                    // caller bug as a missing method.
                    ("<=>", many) => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!("wrong number of arguments (given {}, expected 1)", many.len()),
                        }));
                    }
                    ("freeze" | "frozen?", many) => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!("wrong number of arguments (given {}, expected 0)", many.len()),
                        }));
                    }
                    // No-block `each` / `each_with_index` / `each_index`
                    // (no args) returns an Enumerator — CRuby `enum.c`.
                    // The block forms live in `collection_call_block`
                    // (iter.rs); now that a real Enumerator is modelled,
                    // `arr.each.to_a`, `arr.each_with_index.map { }`,
                    // `arr.each_index.select { }` work via enum_for
                    // re-invoking the block-form method when the
                    // Enumerator is finally driven with a block.
                    ("each" | "each_with_index" | "each_index", []) => {
                        return self.make_enum_for(Value::Array(id), name, vec![]).map(Some);
                    }
                    // The transform / filter Enumerable family returns an
                    // Enumerator when called with no block (CRuby `enum.c`):
                    // `arr.map`, `arr.select.with_index { }`, etc. The
                    // block forms live in collection_call_block (iter.rs);
                    // the Enumerator re-invokes them once driven. Only the
                    // no-arg shape is covered — `min_by(2)` etc. (arg, no
                    // block) stays a gap. Methods whose no-block form is
                    // NOT an Enumerator (sort / uniq / min / count / all?
                    // / inject / …) are deliberately excluded.
                    ("map" | "collect" | "select" | "filter" | "reject"
                        | "flat_map" | "collect_concat" | "filter_map"
                        | "find" | "detect" | "partition" | "group_by"
                        | "min_by" | "max_by" | "sort_by" | "reverse_each", []) => {
                        return self.make_enum_for(Value::Array(id), name, vec![]).map(Some);
                    }
                    // `Array#to_h` (no block) — build a Hash from an
                    // array of `[k, v]` pair Arrays. CRuby raises
                    // TypeError if an element isn't an Array and
                    // ArgumentError if a pair isn't length 2. Duplicate
                    // keys keep the first position with the last value
                    // (hash_insert semantics). The block form
                    // (`arr.to_h { |x| [k, v] }`) lives in
                    // collection_call_block (iter.rs).
                    ("to_h", []) => {
                        // Pin the receiver BEFORE `maybe_gc`: the source
                        // pair Arrays are reachable only via the receiver
                        // here (not the operand stack), so an unpinned
                        // GC would sweep them out from under the loop.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let src: Vec<Value> = g.vm.heap.array(id).clone();
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let hid = g.vm.heap.alloc(HeapObj::Hash(
                            crate::heap::HashObj::with_pairs(Vec::new()),
                        ));
                        g.pin(Value::Hash(hid));
                        for (i, el) in src.into_iter().enumerate() {
                            match el {
                                Value::Array(eid) => {
                                    let pair = g.vm.heap.array(eid);
                                    if pair.len() != 2 {
                                        let n = pair.len();
                                        return Err(g.vm.trap(crate::error::RubyError::ArgumentError {
                                            msg: format!(
                                                "wrong array length at {i} (expected 2, was {n})"
                                            ),
                                        }));
                                    }
                                    let k = pair[0].clone();
                                    let v = pair[1].clone();
                                    g.vm.heap.hash_insert(hid, k, v);
                                }
                                other => {
                                    return Err(g.vm.trap(crate::error::RubyError::TypeError {
                                        msg: format!(
                                            "wrong element type {} at {i} (expected array)",
                                            other.type_name()
                                        ),
                                    }));
                                }
                            }
                        }
                        drop(g);
                        Some(Value::Hash(hid))
                    }
                    // `Array#shift` — remove and return the first
                    // element; `nil` if empty. In-place mutation.
                    ("shift", []) => {
                        let a = self.heap.array_mut(id);
                        if a.is_empty() {
                            Some(Value::Nil)
                        } else {
                            Some(a.remove(0))
                        }
                    }
                    // `Array#shift(n)` — remove and return the
                    // first n elements as a new Array. Mirrors
                    // `Array#pop(n)` below. `n` larger than the
                    // array clamps to the array length; `n == 0`
                    // returns `[]`; empty array returns `[]`.
                    //
                    // GC discipline: do `maybe_gc` + `check_alloc`
                    // + `alloc` BEFORE the drain. Once drained,
                    // the elements live only in a Rust-local Vec
                    // — the source Array no longer references them
                    // — so a subsequent `maybe_gc` would sweep any
                    // heap-bearing element. Allocating the result
                    // Array first and synchronously moving drained
                    // values into the heap-owned Vec keeps the
                    // children rooted via the receiver pin until
                    // they land in the result Array.
                    ("shift", [Value::Int(n)]) => {
                        let n_i = *n;
                        if n_i < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "negative array size".into(),
                            }));
                        }
                        // wasm32 truncation guard — match the
                        // first(n)/last(n) pattern.
                        let n_usz = usize::try_from(n_i).unwrap_or(usize::MAX);
                        let arr_len = self.heap.array(id).len();
                        let take = n_usz.min(arr_len);
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let nid = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(take)));
                        let drained: Vec<Value> = g.vm.heap.array_mut(id).drain(0..take).collect();
                        g.vm.heap.array_mut(nid).extend(drained);
                        Some(Value::Array(nid))
                    }
                    #[cfg(feature = "bignum")]
                    ("shift", [Value::BigInt(_)]) => {
                        // Matches first(n)/last(n)'s BigInt arm —
                        // raise RangeError rather than fall to the
                        // wrong-arity catch-all (which would
                        // mis-report the arity).
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce — CRuby truncates `shift(2.5)`
                    // to 2 (Integer cast). Re-dispatch with the
                    // converted Int. Same pattern as the
                    // take/drop / each_slice family.
                    ("shift", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    // Wrong-arity / non-Int catch-all. Routed
                    // through `arity_error_arg0_or_1_int` so
                    // non-Int 1-arg surfaces as TypeError (CRuby
                    // parity) rather than the misleading
                    // "wrong number of arguments" message.
                    ("shift", _) => {
                        return Err(self.arity_error_arg0_or_1_int(name, args));
                    }
                    // `Array#pop` — remove and return the last
                    // element; `nil` if empty. In-place mutation.
                    ("pop", []) => {
                        let a = self.heap.array_mut(id);
                        Some(a.pop().unwrap_or(Value::Nil))
                    }
                    // `Array#pop(n)` — remove and return the last
                    // n elements as a new Array (in original
                    // order — `[1,2,3].pop(2) == [2, 3]`). Negative
                    // n raises ArgumentError; n exceeding array
                    // length clamps to the length. Same GC
                    // discipline as `shift(n)` above — alloc the
                    // result Array BEFORE the drain so drained
                    // children stay rooted via the receiver pin
                    // until they land in the heap-owned Vec.
                    ("pop", [Value::Int(n)]) => {
                        let n_i = *n;
                        if n_i < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "negative array size".into(),
                            }));
                        }
                        let n_usz = usize::try_from(n_i).unwrap_or(usize::MAX);
                        let arr_len = self.heap.array(id).len();
                        let take = n_usz.min(arr_len);
                        let split_at = arr_len - take;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let nid = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(take)));
                        let drained: Vec<Value> = g.vm.heap.array_mut(id).drain(split_at..).collect();
                        g.vm.heap.array_mut(nid).extend(drained);
                        Some(Value::Array(nid))
                    }
                    #[cfg(feature = "bignum")]
                    ("pop", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce — same as shift above.
                    ("pop", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("pop", _) => {
                        return Err(self.arity_error_arg0_or_1_int(name, args));
                    }
                    // `Array#delete(obj)` — value-based delete.
                    // Removes EVERY element equal to `obj` (using
                    // `==`, via `ruby_eq`), returns the last
                    // deleted element, or nil if `obj` wasn't
                    // found. In-place mutation.
                    //
                    // Motivating consumer: tilt's
                    // `local_extraction` at lib/tilt/template.rb:378
                    // calls `assignments.delete("locals =
                    // locals[:locals]")` to decide whether to
                    // re-append the `locals`-key assignment last.
                    //
                    // Divergence: the block form
                    // `arr.delete(obj) { yield-if-not-found }`
                    // reaches this arm (via the `first`/`last`-
                    // style delegation in `collection_call_block`)
                    // but rubyrs silently drops the block instead
                    // of yielding `obj` on no-match. CRuby returns
                    // the block's result on no-match; rubyrs
                    // returns nil.
                    ("delete", args) if args.len() != 1 => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 1)",
                                args.len()
                            ),
                        }));
                    }
                    ("delete", [needle]) => {
                        // Two-phase: walk the immutable view to
                        // collect indices that match (need the
                        // heap borrow for `ruby_eq`'s
                        // cross-reference resolution), then drain
                        // those indices via `array_mut`. Last
                        // matched element wins as the return
                        // value, mirroring CRuby.
                        let a = self.heap.array(id);
                        let hits: Vec<usize> = a
                            .iter()
                            .enumerate()
                            .filter(|(_, x)| x.ruby_eq(needle, &self.heap))
                            .map(|(i, _)| i)
                            .collect();
                        if hits.is_empty() {
                            Some(Value::Nil)
                        } else {
                            // CRuby returns the LAST matched element
                            // in array order. Since `==` can hold
                            // across distinct objects (e.g.
                            // `1 == 1.0`), this is the highest-index
                            // hit BEFORE removal — not whatever the
                            // last drop happens to surface.
                            let last_idx = *hits.last().unwrap();
                            let last = self.heap.array(id)[last_idx].clone();
                            // Single O(n) `retain` pass driven by a
                            // peekable iterator over the (ascending)
                            // hit indices, instead of N × `Vec::remove`
                            // (each of which shifts the tail and would
                            // make a many-hit delete O(n²)).
                            let a = self.heap.array_mut(id);
                            let mut hits_iter = hits.iter().copied().peekable();
                            let mut i = 0usize;
                            a.retain(|_| {
                                let drop = hits_iter.peek() == Some(&i);
                                if drop { hits_iter.next(); }
                                i += 1;
                                !drop
                            });
                            Some(last)
                        }
                    }
                    // `Array#replace(other)` — clear self and copy in
                    // other's contents. Returns self. CRuby raises
                    // TypeError when `other` isn't an Array (and the
                    // receiver isn't an Array subclass with `to_ary`);
                    // rubyrs ships the strict-Array shape and matches
                    // CRuby's TypeError message wording byte-for-byte
                    // for non-Array args. Same byte-cap pattern push
                    // / unshift use, sized at the OTHER's length
                    // because the new content is what determines the
                    // post-replace size.
                    //
                    // Self-replace (`a.replace(a)`) clones `b` into
                    // `tmp` BEFORE the truncate so the source view
                    // outlives the in-place clear; otherwise the
                    // `array_mut(a).clear()` would empty the
                    // borrow-via-id we'd read from next.
                    //
                    // Spec un-skip: array_first_spec.rb's "returns an
                    // array which is independent to the original
                    // when passed count" + the analogous block in
                    // array_last_spec.rb (PRs #133/#158 deferred
                    // them as `# skipped (method-not-implemented):`).
                    ("replace", [Value::Array(other_id)]) => {
                        let new_len = self.heap.array(*other_id).len();
                        if let Some(max) = self.max_value_bytes
                            && new_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array.replace would exceed {max} bytes"),
                                }));
                            }
                        // Snapshot the source contents first so the
                        // self-replace case (a.replace(a)) doesn't
                        // read from a buffer we then truncate.
                        let snapshot: Vec<Value> = self.heap.array(*other_id).clone();
                        let a = self.heap.array_mut(id);
                        a.clear();
                        a.extend(snapshot);
                        Some(Value::Array(id))
                    }
                    ("replace", [other]) => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "no implicit conversion of {} into Array",
                                other.type_name(),
                            ),
                        }));
                    }
                    ("replace", many) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 1)",
                                many.len()
                            ),
                        }));
                    }
                    // `Array#clear` — drop all elements in place,
                    // return the (now-empty) receiver. CRuby is
                    // O(1) modulo refcount drops; rubyrs is O(n)
                    // because the GC owns element liveness, but
                    // the observable shape (`a.equal?(a.clear)`)
                    // is the same.
                    ("clear", []) => {
                        self.heap.array_mut(id).clear();
                        Some(Value::Array(id))
                    }
                    ("clear", many) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len()
                            ),
                        }));
                    }
                    // `Array#find_index(target)` / `Array#index(target)`
                    // — Int index of the first element `==` the
                    // target, or nil. The block form lives in
                    // iter.rs; the no-arg-no-block form returns
                    // an Enumerator in CRuby (not implemented).
                    ("find_index", [target]) | ("index", [target]) => {
                        let target = target.clone();
                        let len = self.heap.array(id).len();
                        let mut found: Option<i64> = None;
                        for i in 0..len {
                            let el = self.heap.array(id)[i].clone();
                            if el.ruby_eq(&target, &self.heap) {
                                found = Some(i as i64);
                                break;
                            }
                        }
                        Some(match found {
                            Some(i) => Value::Int(i),
                            None => Value::Nil,
                        })
                    }
                    // `arr.find_index` / `arr.index` (no arg, no block)
                    // returns an Enumerator that yields each element to a
                    // block and reports the first truthy index. The block
                    // form lives in iter.rs; the Enumerator re-invokes it
                    // once driven (e.g. `arr.find_index.each { |x| ... }`).
                    ("find_index" | "index", []) => {
                        return self.make_enum_for(Value::Array(id), name, vec![]).map(Some);
                    }
                    ("find_index" | "index", many) if many.len() > 1 => {
                        // CRuby surface says `expected 0..1`
                        // because no-arg returns an Enumerator
                        // (not implemented here). We mirror the
                        // wording so rescue-by-message callers
                        // don't diverge.
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0..1)",
                                many.len()
                            ),
                        }));
                    }
                    // `Array#unshift(v)` / `prepend(v)` — insert at
                    // front, return receiver. Variadic in CRuby
                    // (`unshift(a, b, c)` inserts all at once in
                    // order); rubyrs accepts the single-arg form
                    // first (the common shape — `$LOAD_PATH.unshift
                    // dir` everywhere) and the variadic form via
                    // the next arm. Byte-cap enforcement mirrors
                    // `push`.
                    ("unshift", [v]) | ("prepend", [v]) => {
                        let new_len = self.heap.array(id).len().saturating_add(1);
                        if let Some(max) = self.max_value_bytes
                            && new_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array.unshift would exceed {max} bytes"),
                                }));
                            }
                        self.heap.array_mut(id).insert(0, v.clone());
                        Some(Value::Array(id))
                    }
                    ("unshift", many) | ("prepend", many) if !many.is_empty() => {
                        let new_len = self.heap.array(id).len().saturating_add(many.len());
                        if let Some(max) = self.max_value_bytes
                            && new_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array.unshift would exceed {max} bytes"),
                                }));
                            }
                        // CRuby semantics: unshift(a, b, c) leaves
                        // [a, b, c, ...rest] (args appear in
                        // call-order at the front). Splice in one
                        // shot rather than `insert` per element so
                        // the relative order is preserved.
                        let a = self.heap.array_mut(id);
                        let owned: Vec<Value> = many.to_vec();
                        a.splice(0..0, owned);
                        Some(Value::Array(id))
                    }
                    // `Array#insert(index, *objs)`. Non-negative index
                    // inserts the objects BEFORE that position (padding
                    // with nils if index > length); a negative index
                    // inserts AFTER the referenced element
                    // (`len + index + 1`). No objects → no-op. Returns
                    // self. Discovery: P3 Jekyll spike — liquid/jekyll
                    // splice into rendered-content arrays via `insert`.
                    ("insert", []) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "wrong number of arguments (given 0, expected 1+)".to_string(),
                        }));
                    }
                    ("insert", [Value::Int(idx), rest @ ..]) => {
                        if rest.is_empty() {
                            return Ok(Some(Value::Array(id)));
                        }
                        let len = self.heap.array(id).len() as i64;
                        // Negative index inserts after the element it
                        // names, so the position is `len + idx + 1`.
                        let pos = if *idx < 0 { len + *idx + 1 } else { *idx };
                        if pos < 0 {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!(
                                    "index {idx} too small for array; minimum: {}",
                                    -len - 1
                                ),
                            }));
                        }
                        let pos = pos as usize;
                        let new_len = self
                            .heap
                            .array(id)
                            .len()
                            .max(pos)
                            .saturating_add(rest.len());
                        // Absolute ceiling independent of the opt-in
                        // `max_value_bytes` cap: without it a bare
                        // `insert(huge_index, x)` on a default-config
                        // interpreter drives `Vec::resize` into an
                        // allocation that aborts the host process. CRuby
                        // raises `IndexError: index N too big` past its
                        // array-size limit; mirror that boundary.
                        const ARY_MAX: usize = (i64::MAX as usize) / std::mem::size_of::<Value>();
                        if new_len > ARY_MAX {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {idx} too big"),
                            }));
                        }
                        if let Some(max) = self.max_value_bytes
                            && new_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array.insert would exceed {max} bytes"),
                                }));
                            }
                        let owned: Vec<Value> = rest.to_vec();
                        let a = self.heap.array_mut(id);
                        if pos > a.len() {
                            a.resize(pos, Value::Nil);
                            a.extend(owned);
                        } else {
                            a.splice(pos..pos, owned);
                        }
                        Some(Value::Array(id))
                    }
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
                    // `a[start, length]` two-arg slice form. CRuby
                    // semantics (verified via `ruby -e`):
                    //   - negative `start` wraps from end; if still
                    //     negative after wrap → nil
                    //   - `start == len` returns `[]` (NOT nil — the
                    //     "boundary-zero-length-tail" rule)
                    //   - `start > len` → nil
                    //   - `length < 0` → nil
                    //   - `length` clamps at `len - start`
                    //   - returns a FRESH Array (CRuby's slice
                    //     contract — mutating the result doesn't
                    //     affect the original)
                    //
                    // Discovered as a missing surface during the
                    // AS-lite Tier D-narrow Duration#inspect work
                    // (commit f53bc4ee) — Oxford-comma formatting
                    // wanted `pieces[0..-2]` / `pieces[0, n - 1]`.
                    ("[]", [Value::Int(start), Value::Int(length)]) => {
                        let a = self.heap.array(id);
                        let len = a.len() as i64;
                        let s = if *start < 0 { len + *start } else { *start };
                        let l = *length;
                        if s < 0 || s > len || l < 0 {
                            Some(Value::Nil)
                        } else {
                            let end_idx = (s + l).min(len) as usize;
                            let slice: Vec<Value> = a[s as usize..end_idx].to_vec();
                            self.maybe_gc();
                            self.check_alloc()?;
                            let nid = self.heap.alloc(HeapObj::Array(slice));
                            Some(Value::Array(nid))
                        }
                    }
                    // `a[range]` Range-slice form. Handles full
                    // CRuby surface:
                    //   - Inclusive (`a..b`) AND exclusive (`a...b`)
                    //     bounds
                    //   - Beginless (`a[..3]`, begin == Nil) treats
                    //     begin as 0
                    //   - Endless (`a[2..]`, end == Nil) treats end
                    //     as `len - 1`
                    //   - Negative indices wrap from end
                    //   - Empty result when begin == len → []
                    //     (matching boundary rule from two-arg form)
                    //   - nil result when begin > len OR begin < 0
                    //     after wrap
                    //   - End past len is clamped to `len - 1`
                    ("[]", [Value::Range(rid)]) => {
                        let r = self.heap.range(*rid);
                        // Snapshot range bounds + exclusive flag
                        // before re-borrowing the receiver Array.
                        let r_begin = r.begin.clone();
                        let r_end = r.end.clone();
                        let r_exclusive = r.exclusive;
                        let a = self.heap.array(id);
                        let len = a.len() as i64;
                        let begin = match r_begin {
                            Value::Nil => 0,
                            Value::Int(b) => if b < 0 { len + b } else { b },
                            other => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into Integer", other.type_name()),
                            })),
                        };
                        // For endless ranges (`a[2..]` / `a[2...]`)
                        // the exclusive flag is a no-op — CRuby
                        // treats both as "from begin through the
                        // last element". Only apply the
                        // exclusive-end shift when end is an
                        // explicit Integer.
                        let end_idx = match r_end {
                            Value::Nil => len - 1,
                            Value::Int(e) => {
                                let resolved = if e < 0 { len + e } else { e };
                                if r_exclusive { resolved - 1 } else { resolved }
                            }
                            other => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into Integer", other.type_name()),
                            })),
                        };
                        if begin < 0 || begin > len {
                            Some(Value::Nil)
                        } else if begin == len {
                            self.maybe_gc();
                            self.check_alloc()?;
                            let nid = self.heap.alloc(HeapObj::Array(Vec::new()));
                            Some(Value::Array(nid))
                        } else {
                            let last = end_idx.min(len - 1);
                            let slice: Vec<Value> = if last < begin {
                                Vec::new()
                            } else {
                                a[begin as usize..=last as usize].to_vec()
                            };
                            self.maybe_gc();
                            self.check_alloc()?;
                            let nid = self.heap.alloc(HeapObj::Array(slice));
                            Some(Value::Array(nid))
                        }
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
                    // `a[start, length] = value` — splice assignment.
                    // CRuby semantics (verified via `ruby -e`):
                    //   - `value` Array: contents replace the slice
                    //   - `value` non-Array: wraps as single-element
                    //     replacement
                    //   - Negative `start` wraps from end; if still
                    //     too negative → IndexError ("index N too
                    //     small for array; minimum: -L")
                    //   - Negative `length` → IndexError
                    //     ("negative length (N)") — note this is
                    //     IndexError, NOT the nil-return of the read
                    //     form
                    //   - `start > len`: pad with Nil between current
                    //     len and start, then insert
                    //   - `length` clamps at `len - start`
                    //   - Returns the assigned `value` as-is (Ruby
                    //     `a[1, 2] = [9, 8]` expression value is the
                    //     [9, 8] Array, not the array's contents)
                    ("[]=", [Value::Int(start), Value::Int(length), v]) => {
                        let len = self.heap.array(id).len() as i64;
                        let s = if *start < 0 { len + *start } else { *start };
                        let l = *length;
                        if s < 0 {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!(
                                    "index {} too small for array; minimum: -{}",
                                    start, len,
                                ),
                            }));
                        }
                        if l < 0 {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("negative length ({})", l),
                            }));
                        }
                        // Snapshot the replacement values BEFORE
                        // mutably borrowing the receiver Array.
                        // Wrap non-Array values in a single-element
                        // Vec — CRuby's "[]= with non-Array value
                        // means replace with this single element".
                        let new_vals: Vec<Value> = match v {
                            Value::Array(vid) if *vid != id => {
                                self.heap.array(*vid).clone()
                            }
                            Value::Array(_) => {
                                // Aliasing: assigning a slice OF the
                                // same Array. Clone the snapshot to
                                // break the borrow before the splice.
                                self.heap.array(id).clone()
                            }
                            other => vec![other.clone()],
                        };
                        let s_u = s as usize;
                        let end_idx = ((s + l) as usize).min(self.heap.array(id).len());
                        // Same cap check as the 1-arg form. The
                        // post-splice length is start + new_vals.len()
                        // + (current_len - end_idx).
                        let cur_len = self.heap.array(id).len();
                        let tail_len = cur_len.saturating_sub(end_idx);
                        let needed_len = s_u
                            .saturating_add(new_vals.len())
                            .saturating_add(tail_len);
                        if let Some(max) = self.max_value_bytes
                            && needed_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array []= would exceed {max} bytes"),
                                }));
                            }
                        let a = self.heap.array_mut(id);
                        // Pad with Nil if start is past current end.
                        while a.len() < s_u { a.push(Value::Nil); }
                        let splice_end = end_idx.max(s_u).min(a.len());
                        a.splice(s_u..splice_end, new_vals);
                        Some(v.clone())
                    }
                    // `a[range] = value` — Range-form splice assignment.
                    // Same semantics as the two-arg form, with
                    // begin/end resolved from the Range (nil bounds,
                    // exclusive flag, negative wrapping all match
                    // the read-side `a[range]` arm above).
                    //
                    // CRuby quirk: `a[begin..end] = v` where begin > end
                    // (after wrap) is INSERT-at-begin without removing,
                    // not a no-op. E.g. `a = [1,2,3,4,5]; a[1..0] = [9, 9]`
                    // gives `[1, 9, 9, 2, 3, 4, 5]` (length 0 splice at
                    // idx 1). Our normalisation produces `length = 0`
                    // for that shape, which the two-arg semantics
                    // already handle correctly.
                    ("[]=", [Value::Range(rid), v]) => {
                        let r = self.heap.range(*rid);
                        let r_begin = r.begin.clone();
                        let r_end = r.end.clone();
                        let r_exclusive = r.exclusive;
                        let len = self.heap.array(id).len() as i64;
                        let begin = match r_begin {
                            Value::Nil => 0,
                            Value::Int(b) => if b < 0 { len + b } else { b },
                            other => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into Integer", other.type_name()),
                            })),
                        };
                        let end_idx = match r_end {
                            Value::Nil => len - 1,
                            Value::Int(e) => {
                                let resolved = if e < 0 { len + e } else { e };
                                if r_exclusive { resolved - 1 } else { resolved }
                            }
                            other => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into Integer", other.type_name()),
                            })),
                        };
                        if begin < 0 {
                            return Err(self.trap(RubyError::RangeError {
                                msg: format!("{}..{} out of range", begin, end_idx),
                            }));
                        }
                        // Derive the equivalent two-arg `length`:
                        // - If end_idx < begin: zero-length insert at
                        //   begin (CRuby's `a[1..0] = ...` insert
                        //   semantics).
                        // - Otherwise: `end_idx - begin + 1` covers
                        //   the inclusive range; over-len gets clamped
                        //   by the splice arm below.
                        let length = if end_idx < begin { 0 } else { end_idx - begin + 1 };
                        let new_vals: Vec<Value> = match v {
                            Value::Array(vid) if *vid != id => {
                                self.heap.array(*vid).clone()
                            }
                            Value::Array(_) => self.heap.array(id).clone(),
                            other => vec![other.clone()],
                        };
                        let s_u = begin as usize;
                        let cur_len = self.heap.array(id).len();
                        let end_clamp = ((begin + length) as usize).min(cur_len);
                        let tail_len = cur_len.saturating_sub(end_clamp);
                        let needed_len = s_u
                            .saturating_add(new_vals.len())
                            .saturating_add(tail_len);
                        if let Some(max) = self.max_value_bytes
                            && needed_len.saturating_mul(std::mem::size_of::<Value>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array []= would exceed {max} bytes"),
                                }));
                            }
                        let a = self.heap.array_mut(id);
                        while a.len() < s_u { a.push(Value::Nil); }
                        let splice_end = end_clamp.max(s_u).min(a.len());
                        a.splice(s_u..splice_end, new_vals);
                        Some(v.clone())
                    }
                    ("first", []) => Some(self.heap.array(id).first().cloned().unwrap_or(Value::Nil)),
                    // `arr.first(n)` / `arr.last(n)` — CRuby returns a
                    // new Array of up to `n` elements (capped at the
                    // receiver's length). `n == 0` is `[]`; `n < 0` is
                    // ArgumentError "negative array size".
                    //
                    // CRuby-divergences NOT introduced here but worth
                    // naming so a reader doesn't think the inconsistency
                    // came from this PR:
                    //
                    //   - Array#take / Array#drop (vm/array.rs ~770)
                    //     silently clamp negative `n` to 0 rather than
                    //     trapping. Array#first / #last now trap, on
                    //     purpose — matches CRuby's actual semantics for
                    //     the *Array* methods.
                    //   - Range#first(n) is missing entirely on closed
                    //     ranges (e.g. `(1..5).first(2)` → NoMethodError);
                    //     the only arm that exists is the *endless* one
                    //     at vm/range.rs:83, and that arm silently clamps
                    //     negative n via `(*n).max(0)` instead of
                    //     trapping. Both gaps are pre-existing. Tracked
                    //     in issue #143.
                    //
                    // The Range gap is tracked separately rather than
                    // bundled in this PR — fixing it touches a different
                    // file, a different (endless / closed) shape split,
                    // and a different semantic question (does Range
                    // materialise into an Array, or stream lazily).
                    ("first", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "negative array size".into(),
                            }));
                        }
                        // `*n as usize` would silently truncate on
                        // wasm32-wasip1 (usize is u32 there), turning
                        // an `arr.first(2**32)` request into
                        // `arr.first(0)`. `try_from` + `unwrap_or(MAX)`
                        // keeps the contract "n bigger than usize means
                        // n bigger than len means take the whole
                        // thing", which is what CRuby does. Native
                        // hosts (usize == u64) are unaffected because
                        // we already trapped negatives above.
                        let n = usize::try_from(*n).unwrap_or(usize::MAX);
                        // Pin the receiver across maybe_gc: same rationale
                        // as `take`/`drop` — the receiver Array has been
                        // popped from the operand stack before this match
                        // arm runs, and STRESS_GC would otherwise sweep
                        // it (and its children) between iter().take()
                        // and the alloc.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().take(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // BigInt count → RangeError. CRuby's wording:
                    // `bignum too big to convert into 'long'`. The
                    // `Value::Int(n)` arm above already caps at
                    // `usize::MAX` for in-range "n bigger than len",
                    // so the rationale for raising here is strictly
                    // "value won't fit a C long" — matches
                    // bignum.rs:1361's identical guard for
                    // `Integer#to_s(big_radix)`. Divergence was
                    // pinned by PR #193's
                    // `divergence_array_first_bignum` ratchet
                    // (retired in this PR alongside the fix);
                    // un-skipped spec block lives in
                    // `spec/ruby/array_first_spec.rb`.
                    #[cfg(feature = "bignum")]
                    ("first", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce — same pattern as pop/shift.
                    ("first", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    // Wrong-arity / non-Int catch-all for first.
                    // Previously fell through to NoMethodError
                    // despite respond_to? returning true.
                    ("first", _) => {
                        return Err(self.arity_error_arg0_or_1_int(name, args));
                    }
                    ("dig", keys) if !keys.is_empty() => {
                        let mut cur = Value::Array(id);
                        for key in keys {
                            cur = self.dig_step(&cur, key)?;
                            if matches!(cur, Value::Nil) { break; }
                        }
                        Some(cur)
                    }
                    ("last", []) => Some(self.heap.array(id).last().cloned().unwrap_or(Value::Nil)),
                    ("last", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "negative array size".into(),
                            }));
                        }
                        // Same wasm32 truncation guard as `first(n)`
                        // above; combined with the `saturating_sub`
                        // below, `n` beyond `usize::MAX` collapses to
                        // start == 0, i.e. return the whole array —
                        // matching CRuby's "n > len" semantics.
                        let n = usize::try_from(*n).unwrap_or(usize::MAX);
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let arr = g.vm.heap.array(id);
                        // `saturating_sub` handles `n >= len` cleanly —
                        // CRuby's `[1,2,3].last(5)` returns the full
                        // array, no error.
                        let start = arr.len().saturating_sub(n);
                        let out: Vec<Value> = arr[start..].to_vec();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // BigInt count → RangeError. Same shape as the
                    // `first` arm above; see the rationale there.
                    #[cfg(feature = "bignum")]
                    ("last", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce + catch-all — same pattern as first.
                    ("last", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("last", _) => {
                        return Err(self.arity_error_arg0_or_1_int(name, args));
                    }
                    ("empty?", []) => Some(Value::Bool(self.heap.array(id).is_empty())),
                    // `Array#any?` / `Array#all?` / `Array#none?` /
                    // `Array#one?` — no-block forms. CRuby's contract:
                    //   * `any?` — true iff at least one element is truthy
                    //   * `all?` — true iff every element is truthy
                    //   * `none?` — true iff no element is truthy
                    //   * `one?` — true iff exactly one element is truthy
                    // The block-form lives in iter.rs's
                    // `iter_array_filter` arm; this set covers the
                    // (no-block) Enumerable shape gems reach for
                    // (rack-protection's `parts.any?` is the
                    // motivating use case).
                    ("any?", []) => {
                        let a = self.heap.array(id);
                        Some(Value::Bool(a.iter().any(|x| x.is_truthy())))
                    }
                    ("all?", []) => {
                        let a = self.heap.array(id);
                        Some(Value::Bool(a.iter().all(|x| x.is_truthy())))
                    }
                    ("none?", []) => {
                        let a = self.heap.array(id);
                        Some(Value::Bool(!a.iter().any(|x| x.is_truthy())))
                    }
                    ("one?", []) => {
                        let a = self.heap.array(id);
                        Some(Value::Bool(a.iter().filter(|x| x.is_truthy()).count() == 1))
                    }
                    ("include?", [needle]) | ("member?", [needle]) => {
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
                        // GC rooting: `out` is a Rust-local Vec, NOT a
                        // GC root. Each iteration alloc's a sub-array
                        // and pushes its ObjId into `out`; under
                        // STRESS_GC=1 the next iteration's `maybe_gc`
                        // sweeps the prior sub-arrays (nothing roots
                        // them yet — the wrapping result Array isn't
                        // alloc'd until the loop ends). The reused
                        // slots then form a self-referential mess
                        // that overflows the stack at inspect time.
                        // Pin each sub-array via PinGuard; Drop pops
                        // them all once the function returns.
                        let mut g = PinGuard::new(self);
                        let mut out: Vec<Value> = Vec::new();
                        if n_take == 0 {
                            g.vm.maybe_gc();
                            let empty_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                            let v = Value::Array(empty_id);
                            g.pin(v.clone());
                            out.push(v);
                        } else if n_take > 0 && (n_take as usize) <= len {
                            let k = n_take as usize;
                            let mut idx: Vec<usize> = (0..k).collect();
                            loop {
                                let pick: Vec<Value> = idx.iter().map(|&i| snapshot[i].clone()).collect();
                                g.vm.maybe_gc();
                                let pid = g.vm.heap.alloc(HeapObj::Array(pick));
                                let v = Value::Array(pid);
                                g.pin(v.clone());
                                out.push(v);
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
                        g.vm.maybe_gc();
                        let result_id = g.vm.heap.alloc(HeapObj::Array(out));
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
                        // Same GC-rooting shape as `combination` above —
                        // pin each sub-Array as it's pushed into `out`
                        // so STRESS_GC=1 doesn't sweep it before the
                        // wrapping result Array is alloc'd.
                        let mut g = PinGuard::new(self);
                        let mut out: Vec<Value> = Vec::new();
                        if n_take == 0 {
                            g.vm.maybe_gc();
                            let empty_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                            let v = Value::Array(empty_id);
                            g.pin(v.clone());
                            out.push(v);
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
                                g.vm.maybe_gc();
                                let pid = g.vm.heap.alloc(HeapObj::Array(pick));
                                let v = Value::Array(pid);
                                g.pin(v.clone());
                                out.push(v);
                            }
                        }
                        g.vm.maybe_gc();
                        let result_id = g.vm.heap.alloc(HeapObj::Array(out));
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
                        let nid = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
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
                        // PinGuard the receiver Array for the whole
                        // loop: each apply_int_promote / try_bigint_binop
                        // call takes &mut self and may trigger
                        // maybe_gc inside bigint_to_value. The
                        // receiver Array is held in the `recv` local
                        // (already popped from stack by dispatch),
                        // so without an explicit pin the GC sweep
                        // can reclaim it between iterations, panicking
                        // the next array(id) call. Found via
                        // STRESS_GC=1 on the bignum fixture.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let kind = crate::bytecode::BinOpKind::Add;
                        let mut acc: Value = Value::Int(init);
                        let len = g.vm.heap.array(id).len();
                        for i in 0..len {
                            let v = g.vm.heap.array(id)[i].clone();
                            match (&acc, &v) {
                                (Value::Int(x), Value::Int(y)) => {
                                    acc = g.vm.apply_int_promote(kind, *x, *y)?;
                                }
                                _ => {
                                    // Either acc or v (or both) is
                                    // BigInt — try_bigint_binop handles
                                    // any Int/BigInt mix.
                                    #[cfg(feature = "bignum")]
                                    if let Some(next) = g.vm.try_bigint_binop(kind, &acc, &v)? {
                                        acc = next;
                                        continue;
                                    }
                                    return Ok(None);
                                }
                            }
                        }
                        Some(acc)
                    }
                    ("min", []) => {
                        let a = self.heap.array(id);
                        if a.is_empty() { return Ok(Some(Value::Nil)); }
                        let mut best = a[0].clone();
                        for v in &a[1..] {
                            match value_cmp_v_heap(v, &best, &self.interner, &self.heap) {
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
                            match value_cmp_v_heap(v, &best, &self.interner, &self.heap) {
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
                                    // Incomparable pair (no usable `<=>`):
                                    // CRuby raises ArgumentError, not the
                                    // NoMethodError the old `Ok(None)` bail
                                    // produced.
                                    None => {
                                        let t = g.vm.cmp_failed(&copy[j - 1], &copy[j]);
                                        return Err(t);
                                    }
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
                        if self.heap.array(id).is_empty() { return Ok(Some(Value::Nil)); }
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        // PinGuard the receiver Array: the Vec we
                        // clone below holds the Values by value, but
                        // any Value::BigInt element is just an ObjId
                        // — the actual BigInt heap slot is only kept
                        // live by the receiver Array's mark walk.
                        // Without this pin, maybe_gc inside
                        // apply_int_promote / bigint_to_value sweeps
                        // unreached BigInt slots, leaving the cloned
                        // Value::BigInt with dangling ObjIds. Found
                        // via STRESS_GC=1 on the bignum fixture.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let a = g.vm.heap.array(id).clone();
                        let mut acc = a[0].clone();
                        for v in &a[1..] {
                            // Int×Int fast path with overflow promotion;
                            // once `acc` becomes BigInt (or `v` is one),
                            // fall through to `try_bigint_binop` so the
                            // fold continues in arbitrary precision
                            // instead of bailing the whole primitive.
                            match (&acc, v) {
                                (Value::Int(x), Value::Int(y)) => {
                                    if matches!(kind, crate::bytecode::BinOpKind::Div | crate::bytecode::BinOpKind::Mod) && *y == 0 {
                                        return Err(g.vm.trap(RubyError::ZeroDivisionError {
                                            msg: "divided by 0".to_string(),
                                        }));
                                    }
                                    acc = g.vm.apply_int_promote(kind, *x, *y)?;
                                }
                                _ => {
                                    #[cfg(feature = "bignum")]
                                    if let Some(next) = g.vm.try_bigint_binop(kind, &acc, v)? {
                                        acc = next;
                                        continue;
                                    }
                                    return Ok(None);
                                }
                            }
                        }
                        Some(acc)
                    }
                    // `reduce(init, :op)` / `inject(init, :op)` — the
                    // two-arg form: fold every element starting from
                    // the explicit seed `init` (so an empty receiver
                    // returns `init`, not nil). Same numeric fast-path
                    // + BigInt-promotion + ZeroDivision shape as the
                    // single-symbol arm above.
                    ("inject", [init, Value::Sym(op_sym)]) | ("reduce", [init, Value::Sym(op_sym)]) => {
                        let op_name = self.interner.resolve(*op_sym).clone();
                        let kind = match crate::bytecode::BinOpKind::from_op_name(&op_name) { Some(k) => k, None => return Ok(None) };
                        let init_val = init.clone();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.pin(init_val.clone());
                        let a = g.vm.heap.array(id).clone();
                        let mut acc = init_val;
                        for v in &a {
                            match (&acc, v) {
                                (Value::Int(x), Value::Int(y)) => {
                                    if matches!(kind, crate::bytecode::BinOpKind::Div | crate::bytecode::BinOpKind::Mod) && *y == 0 {
                                        return Err(g.vm.trap(RubyError::ZeroDivisionError {
                                            msg: "divided by 0".to_string(),
                                        }));
                                    }
                                    acc = g.vm.apply_int_promote(kind, *x, *y)?;
                                }
                                _ => {
                                    #[cfg(feature = "bignum")]
                                    if let Some(next) = g.vm.try_bigint_binop(kind, &acc, v)? {
                                        acc = next;
                                        continue;
                                    }
                                    return Ok(None);
                                }
                            }
                        }
                        Some(acc)
                    }
                    ("to_a", []) => Some(Value::Array(id)),
                    // `arr.dup` / `arr.clone` — shallow copy. CRuby's
                    // `clone` also preserves the frozen flag; Tier-1
                    // Arrays don't model `freeze` beyond a no-op
                    // (see line 41 above where `freeze` returns the
                    // same id), so `dup` and `clone` are
                    // indistinguishable here. Closes TRY_RUNS
                    // pass-9.7d layer #26 — sinatra/base.rb:1534
                    // (`get` handler) does `@conditions = conditions.dup`
                    // to snapshot route conditions; without this arm
                    // it raised NoMethodError.
                    ("dup", []) | ("clone", []) => {
                        let src = self.heap.array(id).clone();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(src));
                        Some(Value::Array(nid))
                    }
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
                        // CRuby's Array#uniq dedupes via `eql?`
                        // (strict — no Int↔Float coercion), not
                        // `==`. Switched from ruby_eq to
                        // ruby_eql so `[1, 1.0].uniq` correctly
                        // returns [1, 1.0] (was [1]). Bit-
                        // identical NaN now dedupes too via the
                        // NaN identity shortcut in ruby_eql.
                        //
                        // GC discipline: pin the receiver Array
                        // before the maybe_gc + alloc — heap-ref
                        // elements collected into `out` (e.g.
                        // `[[1, 2], [3, 4]].uniq`) are held only
                        // in a Rust-local Vec and would dangle
                        // under STRESS_GC=1 between the loop and
                        // the result alloc. Pinning the receiver
                        // transitively roots all elements via
                        // the GC walker.
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if !out.iter().any(|x| x.ruby_eql(v, &self.heap)) {
                                out.push(v.clone());
                            }
                        }
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // Wrong-arity guard for uniq — CRuby's
                    // no-block form takes no positional args.
                    // Without this guard, `ary.uniq(1)` falls
                    // through every primitive arm and surfaces
                    // as NoMethodError despite
                    // respond_to?(:uniq) returning true.
                    ("uniq", many) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len(),
                            ),
                        }));
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
                        // PinGuard the receiver Array: `user_cmp` can now
                        // invoke a user-defined `<=>` (instance method, or
                        // a Class's `def self.<=>`), which may trigger
                        // maybe_gc. `copy`'s element ObjIds stay reachable
                        // via the pinned receiver's mark walk — mirrors the
                        // no-block `sort` arm above.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let mut copy = g.vm.heap.array(id).clone();
                        let n = copy.len();
                        for i in 1..n {
                            let mut j = i;
                            while j > 0 {
                                let ord = g.vm.user_cmp(&copy[j - 1], &copy[j])?;
                                match ord {
                                    None => {
                                        let t = g.vm.cmp_failed(&copy[j - 1], &copy[j]);
                                        return Err(t);
                                    }
                                    Some(std::cmp::Ordering::Greater) => {
                                        copy.swap(j - 1, j);
                                        j -= 1;
                                    }
                                    _ => break,
                                }
                            }
                        }
                        *g.vm.heap.array_mut(id) = copy;
                        Some(Value::Array(id))
                    }
                    ("uniq!", []) => {
                        // Mirror Array#uniq: dedup via ruby_eql
                        // (eql?, strict on numeric type) so the
                        // bang variant doesn't diverge from the
                        // non-bang form.
                        let src = self.heap.array(id).clone();
                        let mut out: Vec<Value> = Vec::with_capacity(src.len());
                        for v in &src {
                            if !out.iter().any(|x| x.ruby_eql(v, &self.heap)) {
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
                    // Symmetric wrong-arity guard for uniq!.
                    ("uniq!", many) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len(),
                            ),
                        }));
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
                    // Array#& — set intersection. CRuby: returns
                    // elements from `self` that ALSO appear in
                    // `other`, deduplicated, in the receiver's
                    // order. Used by sinatra-cors's
                    // `method_is_allowed?` to intersect the
                    // configured allow-methods with the route
                    // table's actual verbs.
                    ("&", [Value::Array(other)]) => {
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.pin(Value::Array(*other));
                        let src = g.vm.heap.array(id).clone();
                        let keep = g.vm.heap.array(*other).clone();
                        let mut out: Vec<Value> = Vec::new();
                        for v in src {
                            if keep.iter().any(|x| x.ruby_eq(&v, &g.vm.heap))
                                && !out.iter().any(|y| y.ruby_eq(&v, &g.vm.heap))
                            {
                                out.push(v);
                            }
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // Array#| — set union. CRuby: receiver's
                    // elements first (dedup'd) then `other`'s new
                    // elements. Companion to `&` / `-` set ops.
                    ("|", [Value::Array(other)]) => {
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        g.pin(Value::Array(*other));
                        let src = g.vm.heap.array(id).clone();
                        let add = g.vm.heap.array(*other).clone();
                        let mut out: Vec<Value> = Vec::new();
                        for v in src.iter().chain(add.iter()) {
                            if !out.iter().any(|y| y.ruby_eq(v, &g.vm.heap)) {
                                out.push(v.clone());
                            }
                        }
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
                    // BigInt arg — CRuby raises RangeError (the
                    // value is too large to fit in a C long).
                    // Mirrors Hash#take/#drop's BigInt arm at
                    // hash.rs:378. Without this arm, BigInt
                    // falls through to the take/drop catch-all
                    // below and renders as "no implicit conversion
                    // of Integer into Integer" — nonsensical
                    // because `type_name_for_coerce(BigInt)` is
                    // "Integer".
                    #[cfg(feature = "bignum")]
                    ("take", [Value::BigInt(_)]) | ("drop", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce — CRuby truncates `take(2.5)` to 2.
                    // Re-dispatch with the converted Int so the
                    // existing Int arm owns the rest of the logic.
                    // Same pattern as each_slice/each_cons family
                    // (PR #338).
                    ("take" | "drop", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("take", [Value::Int(n)]) => {
                        // Pin the receiver across maybe_gc: by the
                        // time we get here the receiver Array has
                        // been popped from the operand stack, so its
                        // children (the cloned ObjIds in `out`) have
                        // no GC root and STRESS_GC sweeps them.
                        if *n < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "attempt to take negative size".to_string(),
                            }));
                        }
                        let n = *n as usize;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().take(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    ("drop", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "attempt to drop negative size".to_string(),
                            }));
                        }
                        let n = *n as usize;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Array(id));
                        let out: Vec<Value> = g.vm.heap.array(id).iter().skip(n).cloned().collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(nid))
                    }
                    // Wrong-arity / non-Int for take/drop. Catches
                    // `arr.take`, `arr.take(2,3)`, `arr.take("2")`,
                    // `arr.take(nil)`, etc. — was NoMethodError
                    // pre-fix despite `respond_to?` returning true.
                    ("take" | "drop", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    // No-block `each_slice(n)` / `each_cons(n)` —
                    // CRuby returns an Enumerator we don't model;
                    // instead, return the Array of slices/windows
                    // directly. Calling `.to_a` on the result is
                    // a no-op, so the canonical
                    // `arr.each_slice(2).to_a` idiom still works.
                    // Float coerce — CRuby truncates 2.5 → 2.
                    // Re-dispatch with the converted Int. Same
                    // pattern across the 5 sibling no-block arms.
                    ("each_slice", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("each_slice", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("invalid slice size: {}", n),
                            }));
                        }
                        let n = usize::try_from(*n).unwrap_or(usize::MAX);
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
                    // Wrong-arity / non-Int for Array#each_slice
                    // no-block form (block-form gap mirrored by
                    // iter.rs catch-all).
                    ("each_slice", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    ("each_cons", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.array_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("each_cons", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("invalid size: {}", n),
                            }));
                        }
                        let n = usize::try_from(*n).unwrap_or(usize::MAX);
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
                    // Wrong-arity / non-Int for Array#each_cons no-block form.
                    ("each_cons", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    // `arr.chunk_while(arg)` without a block —
                    // CRuby returns an Enumerator on the
                    // no-block call but still raises
                    // ArgumentError when extra args are
                    // passed. rubyrs has no Enumerator stub
                    // for chunk_while; arity validation is the
                    // only thing left to do here. With block,
                    // dispatch goes through iter.rs where the
                    // block-form catch-all already handles
                    // wrong-arity.
                    ("chunk_while", many) if !many.is_empty() => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len()
                            ),
                        }));
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
                    // Array#product(*others) — Cartesian product.
                    // `[1,2].product([3,4])` =>
                    //   `[[1,3],[1,4],[2,3],[2,4]]`.
                    // With no args, returns `[[e]]` per element.
                    // If any factor is empty the result is `[]`.
                    // Rack 3's `rack/utils.rb:569`
                    //   `Hash[((100..199).to_a << 204 << 304).product([true])]`
                    // is the spike's surface — bytecode-loaded at
                    // module-init time, so the require chain fails
                    // without this primitive.
                    ("product", rest) => {
                        let mut factors: Vec<Vec<Value>> =
                            Vec::with_capacity(1 + rest.len());
                        factors.push(self.heap.array(id).clone());
                        for a in rest {
                            match a {
                                Value::Array(oid) => factors.push(self.heap.array(*oid).clone()),
                                _ => return Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "wrong argument type {} (must be Array)",
                                        a.type_name(),
                                    ),
                                })),
                            }
                        }
                        if factors.iter().any(|f| f.is_empty()) {
                            let nid = self.heap.alloc(HeapObj::Array(Vec::new()));
                            return Ok(Some(Value::Array(nid)));
                        }
                        let row_width = factors.len();
                        let total: usize = factors.iter()
                            .try_fold(1usize, |acc, f| acc.checked_mul(f.len()))
                            .ok_or_else(|| self.trap(RubyError::ResourceExhausted {
                                msg: "Array#product result size overflow".to_string(),
                            }))?;
                        if let Some(max) = self.max_value_bytes {
                            let projected = total
                                .saturating_mul(row_width)
                                .saturating_mul(std::mem::size_of::<Value>());
                            if projected > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Array#product would exceed {max} bytes"),
                                }));
                            }
                        }
                        // PinGuard the row Arrays — same shape as
                        // the zip implementation immediately above:
                        // freshly-alloc'd Arrays live in a Rust
                        // local Vec until the outer alloc, so the
                        // intermediate maybe_gc() (or a STRESS_GC
                        // sweep on every alloc) would otherwise
                        // collect them.
                        let nid = {
                            let mut g = PinGuard::new(self);
                            let mut out: Vec<Value> = Vec::with_capacity(total);
                            // Mixed-radix counter: indices[i]
                            // iterates 0..factors[i].len().
                            let mut indices = vec![0usize; row_width];
                            loop {
                                let mut row: Vec<Value> = Vec::with_capacity(row_width);
                                for (i, idx) in indices.iter().enumerate() {
                                    row.push(factors[i][*idx].clone());
                                }
                                let rid = g.vm.heap.alloc(HeapObj::Array(row));
                                let rv = Value::Array(rid);
                                g.pin(rv.clone());
                                out.push(rv);
                                // Increment from the LEAST-
                                // significant position (matches
                                // CRuby's iteration order:
                                // last factor varies fastest).
                                let mut k = row_width;
                                let mut carried = true;
                                while k > 0 && carried {
                                    k -= 1;
                                    indices[k] += 1;
                                    if indices[k] < factors[k].len() {
                                        carried = false;
                                    } else {
                                        indices[k] = 0;
                                    }
                                }
                                if carried { break; }
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
