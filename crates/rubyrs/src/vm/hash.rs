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
                    // `default` no-arg returns the scalar default
                    // (set via `Hash.new(value)`) or nil. CRuby's
                    // 1-arg form `h.default(key)` invokes the
                    // default_proc with the key — that variant is
                    // out of subset (needs the step_block scaffold
                    // and pin discipline; the `[]` arm above
                    // already routes the lookup-miss path).
                    ("default", []) => {
                        Some(self.heap.hash_default_value(id).unwrap_or(Value::Nil))
                    }
                    // `default_proc` returns the Block value (CRuby
                    // returns it as a Proc; rubyrs's Value::Block
                    // resolves `.class` to "Proc", so the surface
                    // matches). Nil if the Hash wasn't built via
                    // `Hash.new { ... }`.
                    ("default_proc", []) => {
                        Some(match self.heap.hash_default_block(id) {
                            Some(bid) => Value::Block(bid),
                            None => Value::Nil,
                        })
                    }
                    // `any?` no-block — true iff non-empty. The
                    // with-block form goes through iter.rs's
                    // `iter_hash_filter` Any mode.
                    ("any?", []) => {
                        Some(Value::Bool(!self.heap.hash(id).is_empty()))
                    }
                    // `count` no-arg returns the pair count as Int.
                    // With-block form is in iter.rs (mirrors
                    // `Array#count` block).
                    ("count", []) => Some(Value::Int(self.heap.hash(id).len() as i64)),
                    ("[]", [k]) => {
                        // Direct hit first.
                        {
                            let h = self.heap.hash(id);
                            for (key, val) in h {
                                if key.ruby_eql(k, &self.heap) {
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
                        // Scalar default (set by `Hash.new(value)`)
                        // is checked BEFORE the block — but only one
                        // of the two can be set at allocation time
                        // (CRuby refuses both, and the Hash.new
                        // intercept enforces that). Returned as-is,
                        // NOT cached: `h[:missing]` returns the
                        // default but doesn't add `:missing` to the
                        // pairs.
                        if let Some(v) = self.heap.hash_default_value(id) {
                            return Ok(Some(v));
                        }
                        if let Some(block_id) = self.heap.hash_default_block(id) {
                            let pre_frames = self.frames.len();
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id));
                            g.pin(k.clone());
                            // Pin the block too — it lives on the
                            // heap and could be swept across maybe_gc
                            // sites inside step_block / dispatch_until.
                            g.pin(Value::Block(block_id));
                            // Reuse the iter.rs step_block helper
                            // (#151) for the PIN-INVOKE-DISPATCH-CHECK
                            // boilerplate. Stored-block semantics
                            // diverge from iterator-yield only at the
                            // Break arm: a Hash default-block is a
                            // stored Proc, not an iterator yield, so
                            // there's no loop body to break out of
                            // and CRuby raises LocalJumpError. The
                            // step_block helper leaves break_signaled
                            // cleared by the time it returns Break(_),
                            // so the trap doesn't carry the flag.
                            match g.vm.step_block(block_id, vec![Value::Hash(id), k.clone()], pre_frames)? {
                                crate::vm::iter::BlockStep::MethodReturn => {
                                    // Non-local return propagates via
                                    // method_return staying set; the
                                    // `[]` site itself never observes
                                    // our Nil because the dispatch
                                    // loop sees method_return first.
                                    return Ok(Some(Value::Nil));
                                }
                                crate::vm::iter::BlockStep::Break(_) => {
                                    return Err(g.vm.trap(crate::error::RubyError::LocalJumpError {
                                        msg: "break from proc-closure".into(),
                                    }));
                                }
                                crate::vm::iter::BlockStep::Value(r) => {
                                    return Ok(Some(r));
                                }
                            }
                        }
                        Some(Value::Nil)
                    }
                    ("[]=", [k, v]) => {
                        // Need a way to compare without borrowing heap while mutating.
                        // Snapshot positions first.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eql(k, &self.heap));
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
                        // Type-dispatch per step: Hash → ruby_eql
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
                            .position(|(key, _)| key.ruby_eql(k, &self.heap));
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
                            .position(|(key, _)| key.ruby_eql(k, &self.heap));
                        Some(match pos {
                            Some(p) => self.heap.hash(id)[p].1.clone(),
                            None => default.clone(),
                        })
                    }
                    // Wrong-arity raises ArgumentError, matching CRuby.
                    // Previously a `fetch(...)` with 0 or 3+ args
                    // matched none of the arms in this `match`,
                    // `hash_collection_call` returned `Ok(None)`, and
                    // `do_call` surfaced `NoMethodError: undefined
                    // method 'fetch' for Hash` — divergence ratcheted
                    // by PR #193's `divergence_hash_fetch_arity`
                    // fixture (retired in this PR). This catch-all
                    // sits AFTER the 1-arg and 2-arg arms so they
                    // still take precedence; only 0-arg and 3+-arg
                    // shapes reach here.
                    ("fetch", many) => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!("wrong number of arguments (given {}, expected 1..2)", many.len()),
                        }));
                    }
                    ("include?", [k]) | ("has_key?", [k]) | ("key?", [k]) | ("member?", [k]) => {
                        let h = self.heap.hash(id);
                        let hit = h.iter().any(|(key, _)| key.ruby_eql(k, &self.heap));
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
                    // `h.first` — returns the first `[k, v]` pair Array
                    // (or nil on empty). `h.first(n)` — returns the
                    // first n pairs as Array<[k, v]>. Mirrors
                    // Array#first; insertion order is the Hash's
                    // canonical iteration order.
                    // `h.one?` (no block) — true iff the Hash
                    // has exactly one entry. Every Hash entry is
                    // truthy (a `[k, v]` pair), so the no-block
                    // Enumerable shape collapses to a size check.
                    // Block form lives in iter.rs.
                    ("one?", []) => Some(Value::Bool(self.heap.hash(id).len() == 1)),
                    ("first", []) => {
                        let pairs = self.heap.hash(id);
                        if pairs.is_empty() { return Ok(Some(Value::Nil)); }
                        let (k, v) = pairs[0].clone();
                        // Pin the receiver + the chosen k/v across
                        // maybe_gc / check_alloc / alloc — without
                        // an explicit pin the receiver-id from
                        // do_call's recv-pop is held only in a
                        // Rust local, and any heap-ref child of
                        // k/v could be swept under STRESS_GC.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        if k.is_gc_heap_ref() { g.pin(k.clone()); }
                        if v.is_gc_heap_ref() { g.pin(v.clone()); }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                        Some(Value::Array(pid))
                    }
                    ("first", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(crate::error::RubyError::ArgumentError {
                                msg: "attempt to take negative size".to_string(),
                            }));
                        }
                        // Convert via try_from + usize::MAX
                        // saturation (mirrors Array#first(n) at
                        // array.rs:483) so a huge `n` on a 32-bit
                        // target (wasm32) still falls through to
                        // "take all" rather than truncating.
                        let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                        let take = n_usz.min(self.heap.hash(id).len());
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id)[..take].to_vec();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut pair_ids: Vec<Value> = Vec::with_capacity(take);
                        for (k, v) in pairs {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                            g.pin(Value::Array(pid));
                            pair_ids.push(Value::Array(pid));
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let aid = g.vm.heap.alloc(HeapObj::Array(pair_ids));
                        Some(Value::Array(aid))
                    }
                    // `h.min` / `h.max` (no block) — find min/max
                    // entry via lexicographic compare on the
                    // `[k, v]` pair (key first, value tiebreaker).
                    // Returns nil on empty Hash. The pair is
                    // materialised as a fresh `[k, v]` Array. Block
                    // form (`h.min { |a, b| ... }`) is out of subset.
                    //
                    // Comparison is done inline via two
                    // `value_cmp_v_heap` calls per step (key
                    // first, value if keys equal) instead of
                    // materialising a throwaway pair Array per
                    // pairwise compare — avoids O(n) heap
                    // allocations and the corresponding
                    // max_live pressure.
                    ("min", []) | ("max", []) => {
                        let pairs = self.heap.hash(id).clone();
                        if pairs.is_empty() { return Ok(Some(Value::Nil)); }
                        let want_max = name == "max";
                        let mut best_idx = 0usize;
                        for i in 1..pairs.len() {
                            let ord = {
                                let (ak, av) = (&pairs[best_idx].0, &pairs[best_idx].1);
                                let (bk, bv) = (&pairs[i].0, &pairs[i].1);
                                let k_ord = crate::vm::value_cmp_v_heap(
                                    ak, bk, &self.interner, &self.heap,
                                );
                                match k_ord {
                                    Some(std::cmp::Ordering::Equal) => {
                                        crate::vm::value_cmp_v_heap(
                                            av, bv, &self.interner, &self.heap,
                                        )
                                    }
                                    other => other,
                                }
                            };
                            let take_b = match ord {
                                Some(std::cmp::Ordering::Less) => want_max,
                                Some(std::cmp::Ordering::Greater) => !want_max,
                                Some(std::cmp::Ordering::Equal) => false,
                                None => return Ok(None),
                            };
                            if take_b { best_idx = i; }
                        }
                        let (k, v) = pairs[best_idx].clone();
                        // Pin receiver + winning k/v across the
                        // final alloc — receiver is held only in
                        // the Rust local from do_call's recv-pop,
                        // and heap-ref k/v could otherwise be
                        // swept by maybe_gc under STRESS_GC.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        if k.is_gc_heap_ref() { g.pin(k.clone()); }
                        if v.is_gc_heap_ref() { g.pin(v.clone()); }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                        Some(Value::Array(pid))
                    }
                    ("dup", []) => {
                        // Shallow copy: clones the pair vector and
                        // re-allocates a new Hash heap slot. Pair
                        // Values are copied by ObjId (children
                        // remain shared with the receiver — matches
                        // CRuby `Hash#dup` semantics where mutations
                        // on the dup don't propagate, but mutations
                        // on shared nested Arrays/Hashes/Strings do.
                        //
                        // Both `default_proc` (block form) and the
                        // scalar default (set via `Hash.new(val)`)
                        // carry over — missing-key lookup consults
                        // `hash_default_value` first, so dropping it
                        // would silently change semantics on the dup.
                        // Pin receiver + block (when present) across
                        // alloc — same GC-rooting concern as `merge`
                        // since the receiver `id` is a Rust-local
                        // from `do_call`'s recv-pop. The scalar
                        // default Value is captured by-value before
                        // the alloc, so it doesn't need an extra
                        // pin (heap-ObjId children of it are
                        // reachable through the receiver pin).
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        let default_block = self.heap.hash_default_block(id);
                        let default_value = self.heap.hash_default_value(id);
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        if let Some(bid) = default_block {
                            g.pin(Value::Block(bid));
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        if default_block.is_some() {
                            g.vm.heap.hash_set_default_block(nid, default_block);
                        }
                        if default_value.is_some() {
                            g.vm.heap.hash_set_default_value(nid, default_value);
                        }
                        Some(Value::Hash(nid))
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
                            let pos = out.iter().position(|(ek, _)| ek.ruby_eql(&k, &self.heap));
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
                            .position(|(key, _)| key.ruby_eql(k, &self.heap));
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
                            let pos = out.iter().position(|(ek, _)| ek.ruby_eql(&k, &self.heap));
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
                            .filter(|(k, _)| !keys.iter().any(|x| x.ruby_eql(k, &self.heap)))
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
                                .find(|(hk, _)| hk.ruby_eql(k, &self.heap)) {
                                pairs.push(pair.clone());
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        Some(Value::Hash(nid))
                    }
                    ("store", [k, v]) => {
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eql(k, &self.heap));
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
