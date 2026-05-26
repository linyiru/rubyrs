//! `Hash` methods that need heap access. Mirrors CRuby's
//! `hash.c`. Dispatched from `Vm::collection_call`'s
//! `Value::Hash` arm.

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::{PinGuard, Vm};

impl Vm {
    /// Hash#X methods that don't take a block. Block-form
    /// methods (each / map / sort_by / etc.) still live in
    /// `collection_call_block` until that gets factored out.
    pub(crate) fn hash_collection_call(
        &mut self,
        id: ObjId,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        Ok(
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.hash(id).len() as i64)),
                    // `freeze` / `frozen?` — same pattern as Array.
                    // No-ops; tilt's `EMPTY_HASH = {}.freeze` relies on
                    // freeze being chainable (returning the receiver).
                    // Wrong-arity raises ArgumentError, matching CRuby.
                    ("freeze", []) => Some(Value::Hash(id)),
                    ("frozen?", []) => Some(Value::Bool(false)),
                    ("freeze" | "frozen?", many) => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!("wrong number of arguments (given {}, expected 0)", many.len()),
                        }));
                    }
                    ("[]", [k]) => {
                        // Direct hit first.
                        {
                            let h = self.heap.hash(id);
                            for (key, val) in h {
                                if key.ruby_eq(k, &self.heap) {
                                    return Ok(Some(val.clone()));
                                }
                            }
                        }
                        // Missing key — invoke default-block if the
                        // Hash was built via `Hash.new { |h, k| ... }`.
                        // CRuby contract: block called with
                        // `(self_hash, key)`; its return value becomes
                        // the `[]` result. Common idiom is
                        // `Hash.new { |h, k| h[k] = [] }` — block
                        // mutates the Hash AND returns the value the
                        // caller sees.
                        if let Some(block_id) = self.heap.hash_default_block(id) {
                            let pre_frames = self.frames.len();
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id));
                            g.pin(k.clone());
                            // Pin the block too — it lives on the
                            // heap and could be swept across maybe_gc
                            // sites in invoke_block / dispatch_until.
                            g.pin(Value::Block(block_id));
                            g.vm.invoke_block(block_id, vec![Value::Hash(id), k.clone()])?;
                            g.vm.dispatch_until(pre_frames)?;
                            // Non-local return from inside the block
                            // (`def foo; h = Hash.new { return :early };
                            // h[:x]; end` → foo returns :early). The
                            // outer unwind machinery handles
                            // method_return; we propagate by leaving
                            // it set and returning Nil. The `[]` site
                            // never observes our Nil because the
                            // dispatch loop sees method_return first.
                            if g.vm.method_return.is_some() {
                                return Ok(Some(Value::Nil));
                            }
                            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                            // `break` from inside a stored Proc
                            // (which is what a Hash default-block
                            // is — not an iterator yield) is a
                            // LocalJumpError in CRuby: there's no
                            // loop body to break out of. Raise to
                            // match. Clear the flag first so the
                            // trap doesn't carry it into the outer
                            // unwind state.
                            if g.vm.break_signaled {
                                g.vm.break_signaled = false;
                                return Err(g.vm.trap(crate::error::RubyError::LocalJumpError {
                                    msg: "break from proc-closure".into(),
                                }));
                            }
                            return Ok(Some(r));
                        }
                        Some(Value::Nil)
                    }
                    ("[]=", [k, v]) => {
                        // Need a way to compare without borrowing heap while mutating.
                        // Snapshot positions first.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        // P2-14c byte cap: only a key that isn't
                        // already present grows the table. Update
                        // of an existing key is free (size-wise).
                        if pos.is_none() {
                            let new_len = self.heap.hash(id).len().saturating_add(1);
                            if let Some(max) = self.max_value_bytes
                                && new_len.saturating_mul(std::mem::size_of::<(Value, Value)>()) > max {
                                    return Err(self.trap(RubyError::ResourceExhausted {
                                        msg: format!("Hash []= would exceed {max} bytes"),
                                    }));
                                }
                        }
                        let h = self.heap.hash_mut(id);
                        if let Some(p) = pos {
                            h[p].1 = v.clone();
                        } else {
                            h.push((k.clone(), v.clone()));
                        }
                        Some(v.clone())
                    }
                    ("empty?", []) => Some(Value::Bool(self.heap.hash(id).is_empty())),
                    ("dig", keys) if !keys.is_empty() => {
                        // Walk the keys/indices, looking up at each
                        // step. Nil at any level short-circuits.
                        // Type-dispatch per step: Hash → ruby_eq
                        // lookup, Array → integer index. Other types
                        // would need `dig` defined; treat as nil.
                        let mut cur = Value::Hash(id);
                        for key in keys {
                            cur = self.dig_step(&cur, key)?;
                            if matches!(cur, Value::Nil) { break; }
                        }
                        Some(cur)
                    }
                    ("fetch", [k]) => {
                        // 1-arg fetch: return value or raise KeyError.
                        // The Trap is routed through the rescue
                        // machinery by `dispatch`, so a script
                        // `begin ... rescue KeyError => e; ... end`
                        // catches it like CRuby.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        match pos {
                            Some(p) => Some(self.heap.hash(id)[p].1.clone()),
                            None => {
                                return Err(self.trap(RubyError::KeyError {
                                    msg: format!("key not found: {}",
                                        k.to_inspect(&self.heap, &self.interner)),
                                }));
                            }
                        }
                    }
                    ("fetch", [k, default]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        Some(match pos {
                            Some(p) => self.heap.hash(id)[p].1.clone(),
                            None => default.clone(),
                        })
                    }
                    ("include?", [k]) | ("has_key?", [k]) | ("key?", [k]) | ("member?", [k]) => {
                        let h = self.heap.hash(id);
                        let hit = h.iter().any(|(key, _)| key.ruby_eq(k, &self.heap));
                        Some(Value::Bool(hit))
                    }
                    ("keys", []) => {
                        let keys: Vec<Value> = self.heap.hash(id).iter().map(|(k, _)| k.clone()).collect();
                        self.maybe_gc();
                        // check_alloc would need a `?`; collection_call returns Option,
                        // so we skip the cap check here. Embedders should set
                        // max_live with a small slack to account for these
                        // derived allocations.
                        let nid = self.heap.alloc(HeapObj::Array(keys));
                        Some(Value::Array(nid))
                    }
                    ("values", []) => {
                        let vals: Vec<Value> = self.heap.hash(id).iter().map(|(_, v)| v.clone()).collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(vals));
                        Some(Value::Array(nid))
                    }
                    ("to_h", []) => Some(Value::Hash(id)),
                    ("inspect", []) => {
                        let s = Value::Hash(id).to_inspect(&self.heap, &self.interner);
                        Some(Value::new_str(s))
                    }
                    ("to_a", []) | ("sort", []) => {
                        // Hash#to_a returns an Array of two-element Arrays.
                        // Each inner [k, v] is freshly heap-allocated; we
                        // need every inner Array kept alive as we
                        // accumulate, otherwise the next loop iter's
                        // `maybe_gc` will sweep the previous pair (it's
                        // only live via the Rust-local Vec, not via any
                        // GC root). Failing to pin produces slot-reuse
                        // cycles that explode `to_display`'s recursion.
                        //
                        // Hash#sort (no block) is just to_a sorted by
                        // key using <=> — handled below with an
                        // insertion sort over the pair list. We share
                        // the build path because both produce an
                        // Array<[k, v]>.
                        let mut pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        if name == "sort" {
                            let n = pairs.len();
                            for i in 1..n {
                                let mut j = i;
                                while j > 0 {
                                    let ord = {
                                        let a = pairs[j - 1].0.clone();
                                        let b = pairs[j].0.clone();
                                        self.user_cmp(&a, &b)?
                                    };
                                    match ord {
                                        None => return Ok(None),
                                        Some(std::cmp::Ordering::Greater) => {
                                            pairs.swap(j - 1, j);
                                            j -= 1;
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                        let nid = {
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id)); // source Hash
                            let mut pair_ids: Vec<Value> = Vec::with_capacity(pairs.len());
                            for (k, v) in pairs {
                                g.vm.maybe_gc();
                                let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                                g.pin(Value::Array(pid));
                                pair_ids.push(Value::Array(pid));
                            }
                            g.vm.maybe_gc();
                            g.vm.heap.alloc(HeapObj::Array(pair_ids))
                        };
                        Some(Value::Array(nid))
                    }
                    ("merge", [Value::Hash(other)]) => {
                        // CRuby: keys in `other` overwrite keys in `self`,
                        // and `other`'s key-order is appended after self's
                        // (existing keys retain their position). The
                        // result inherits the RECEIVER's default-block
                        // (`h.default_proc`), so
                        // `Hash.new { |h, k| h[k] = [] }.merge(x)[:y]`
                        // still auto-vivifies on the merged hash.
                        let mut out: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        let extra: Vec<(Value, Value)> = self.heap.hash(*other).clone();
                        for (k, v) in extra {
                            let pos = out.iter().position(|(ek, _)| ek.ruby_eq(&k, &self.heap));
                            if let Some(p) = pos {
                                out[p].1 = v;
                            } else {
                                out.push((k, v));
                            }
                        }
                        // GC rooting: snapshot the receiver's default-
                        // block BEFORE alloc. The receiver `id` arrived
                        // as a Rust-local ObjId from `do_call`'s recv-
                        // pop; it isn't on the stack / in a frame /
                        // pinned, so `maybe_gc` could sweep it AND
                        // anything reachable only through it (the
                        // default-block itself). That would leave us
                        // copying a freed-slot ObjId onto the new
                        // Hash. Pin both the receiver hash and (when
                        // present) the default-block across the alloc.
                        let default_block = self.heap.hash_default_block(id);
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        // Pin `*other` too — `extra` was a shallow clone
                        // of its pairs (ObjIds, not deep copies), so any
                        // nested heap children (Arrays / Strings /
                        // Hashes / etc.) inside `extra` are reachable
                        // ONLY through `*other`. Without this pin,
                        // maybe_gc could sweep `*other` plus its
                        // children, leaving the new merged Hash with
                        // dangling ObjIds. Caught under STRESS_GC by
                        // a probe like
                        // `h.merge({a: [1,2,3,4,5]})` in a tight loop.
                        g.pin(Value::Hash(*other));
                        if let Some(bid) = default_block {
                            g.pin(Value::Block(bid));
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(out)));
                        if default_block.is_some() {
                            g.vm.heap.hash_set_default_block(nid, default_block);
                        }
                        Some(Value::Hash(nid))
                    }
                    ("delete", [k]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        if let Some(p) = pos {
                            let removed = self.heap.hash_mut(id).remove(p).1;
                            Some(removed)
                        } else {
                            Some(Value::Nil)
                        }
                    }
                    ("invert", []) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).iter()
                            .map(|(k, v)| (v.clone(), k.clone()))
                            .collect();
                        // Later duplicates win for invert — same as CRuby:
                        // if two original values collide as inverted keys,
                        // the last one through wins. Dedup keeping latest.
                        let mut out: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
                        for (k, v) in pairs {
                            let pos = out.iter().position(|(ek, _)| ek.ruby_eq(&k, &self.heap));
                            if let Some(p) = pos { out[p].1 = v; } else { out.push((k, v)); }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(out)));
                        Some(Value::Hash(nid))
                    }
                    // `h.compact` — return a new Hash with nil-value
                    // entries removed. Non-mutating.
                    ("compact", []) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).iter()
                            .filter(|(_, v)| !matches!(v, Value::Nil))
                            .cloned()
                            .collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        Some(Value::Hash(nid))
                    }
                    // `h.compact!` — in-place compaction. Returns
                    // the receiver if any entries were dropped,
                    // `nil` if there were no nil-valued entries
                    // (matches CRuby's "nil unchanged" convention).
                    ("compact!", []) => {
                        let before = self.heap.hash(id).len();
                        self.heap.hash_mut(id).retain(|(_, v)| !matches!(v, Value::Nil));
                        let after = self.heap.hash(id).len();
                        Some(if before == after { Value::Nil } else { Value::Hash(id) })
                    }
                    // `h.except(*keys)` — return a new Hash with the
                    // listed keys removed. Non-mutating. Keys not
                    // present in the receiver are silently skipped.
                    ("except", keys) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).iter()
                            .filter(|(k, _)| !keys.iter().any(|x| x.ruby_eq(k, &self.heap)))
                            .cloned()
                            .collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        Some(Value::Hash(nid))
                    }
                    // `h.slice(*keys)` — return a new Hash with only
                    // the listed keys, in ARGUMENT order (matches
                    // CRuby — `{a:1,c:3}.slice(:c, :a)` is
                    // `{c:3, a:1}`). Missing keys are silently skipped.
                    ("slice", keys) => {
                        let mut pairs: Vec<(Value, Value)> = Vec::new();
                        for k in keys {
                            if let Some(pair) = self.heap.hash(id).iter()
                                .find(|(hk, _)| hk.ruby_eq(k, &self.heap)) {
                                pairs.push(pair.clone());
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        Some(Value::Hash(nid))
                    }
                    ("store", [k, v]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        let h = self.heap.hash_mut(id);
                        if let Some(p) = pos { h[p].1 = v.clone(); }
                        else { h.push((k.clone(), v.clone())); }
                        Some(v.clone())
                    }
                    _ => None,
                }
        )
    }
}
