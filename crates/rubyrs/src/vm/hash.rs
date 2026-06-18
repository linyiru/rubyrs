//! `Hash` methods that need heap access. Mirrors CRuby's
//! `hash.c`. Dispatched from `Vm::collection_call`'s
//! `Value::Hash` arm.

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::{PinGuard, Vm};

/// Hash methods that mutate the receiver in place — CRuby raises
/// FrozenError on each when the receiver is frozen. Centralised so
/// the frozen guards in `hash_collection_call` (non-block forms) and
/// `collection_call_block` (block forms) stay in sync. (`[]=` covers
/// the explicit `h.[]=(k, v)` / `h.store`; the `h[k] = v` assignment
/// syntax routes through Op::CallAset / the `[]=` fast path, guarded
/// separately.)
pub(crate) fn is_hash_mutator(name: &str) -> bool {
    matches!(
        name,
        "[]=" | "store" | "delete" | "delete_if" | "reject!" | "select!"
            | "filter!" | "keep_if" | "clear" | "merge!" | "update" | "replace"
            | "shift" | "compact!" | "transform_values!" | "transform_keys!"
            | "compare_by_identity"
    )
}

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
        // Frozen guard (the Hash twin of array_collection_call's): every
        // mutating method raises FrozenError on a frozen Hash,
        // unconditionally. One central name-keyed check keeps all
        // mutators consistent. rack Headers freezes nothing, but
        // `{}.freeze` enforcement is core Ruby parity.
        if is_hash_mutator(name) && self.heap.hash_frozen(id) {
            let shown = self.inspect_value(&Value::Hash(id))?;
            return Err(self.trap(crate::error::RubyError::FrozenError {
                msg: format!("can't modify frozen Hash: {}", shown),
            }));
        }
        Ok(
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.hash(id).len() as i64)),
                    // `freeze` flips the frozen bit (chainable, returns
                    // self); `frozen?` reads it. Wrong-arity raises
                    // ArgumentError, matching CRuby.
                    ("freeze", []) => {
                        self.heap.set_hash_frozen(id);
                        Some(Value::Hash(id))
                    }
                    ("frozen?", []) => Some(Value::Bool(self.heap.hash_frozen(id))),
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
                    // `default=` — set (or nil-clear) the scalar
                    // default. CRuby semantics: assigning a scalar
                    // default replaces any default proc, so the
                    // block slot clears too. Returns the assigned
                    // value (assignment-expression contract).
                    // Discovery: minitest's summary reporter does
                    // `aggregate.default = []`.
                    ("default=", [v]) => {
                        let stored = if matches!(v, Value::Nil) { None } else { Some(v.clone()) };
                        self.heap.hash_set_default_value(id, stored);
                        self.heap.hash_set_default_block(id, None);
                        Some(v.clone())
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
                    // `default_proc = proc` / `= nil` — set (or clear)
                    // the missing-key default block. Returns the arg.
                    // Discovery: P3 Jekyll spike — jekyll's
                    // `merge_default_proc` copies one Hash's
                    // default_proc onto another.
                    ("default_proc=", [arg]) => {
                        match arg {
                            Value::Block(bid) => self.heap.hash_set_default_block(id, Some(*bid)),
                            Value::Nil => self.heap.hash_set_default_block(id, None),
                            _ => {
                                return Err(self.trap(RubyError::TypeError {
                                    msg: format!("no implicit conversion of {} into Proc", arg.type_name()),
                                }));
                            }
                        }
                        Some(arg.clone())
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
                        // O(1) indexed hit.
                        if let Some(pos) = self.heap.hash_index_lookup(id, k) {
                            return Ok(Some(self.heap.hash(id)[pos].1.clone()));
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
                    ("[]=", [k, v]) | ("store", [k, v]) => {
                        // P2-14c byte cap: only a key that isn't already
                        // present grows the table. The cap is unset in
                        // the common (CLI/embed) case, so skip the
                        // membership probe entirely then — `hash_insert`
                        // does its own single O(1) lookup.
                        if let Some(max) = self.max_value_bytes
                            && self.heap.hash_index_lookup(id, k).is_none()
                        {
                            let new_len = self.heap.hash(id).len().saturating_add(1);
                            if new_len.saturating_mul(std::mem::size_of::<(Value, Value)>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Hash []= would exceed {max} bytes"),
                                }));
                            }
                        }
                        // Index-maintaining insert: O(1) amortised, so
                        // building an N-key Hash is O(n), not O(n²).
                        self.heap.hash_insert(id, k.clone(), v.clone());
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
                        match self.heap.hash_index_lookup(id, k) {
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
                        Some(match self.heap.hash_index_lookup(id, k) {
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
                        Some(Value::Bool(self.heap.hash_index_lookup(id, k).is_some()))
                    }
                    // `h.assoc(key)` → [key, value] or nil. CRuby
                    // compares with ==; the index lookup uses eql?,
                    // identical for the string/symbol keys real
                    // callers (rack Headers#assoc supers here) use.
                    ("assoc", [k]) => {
                        let found = self.heap.hash_index_lookup(id, k)
                            .map(|pos| self.heap.hash(id)[pos].1.clone());
                        match found {
                            Some(v) => {
                                self.maybe_gc();
                                let nid = self.heap.alloc(HeapObj::Array(vec![k.clone(), v].into()));
                                Some(Value::Array(nid))
                            }
                            None => Some(Value::Nil),
                        }
                    }
                    // `h.rassoc(value)` → first [key, value] whose
                    // value == v (linear, insertion order), or nil.
                    ("rassoc", [val]) => {
                        let found = self.heap.hash(id).iter()
                            .find(|(_, v)| v.ruby_eq(val, &self.heap))
                            .map(|(k, v)| (k.clone(), v.clone()));
                        match found {
                            Some((k, v)) => {
                                self.maybe_gc();
                                let nid = self.heap.alloc(HeapObj::Array(vec![k, v].into()));
                                Some(Value::Array(nid))
                            }
                            None => Some(Value::Nil),
                        }
                    }
                    ("value?", [val]) | ("has_value?", [val]) => {
                        Some(Value::Bool(
                            self.heap.hash(id).iter().any(|(_, v)| v.ruby_eq(val, &self.heap)),
                        ))
                    }
                    // `h.shift` — removes and returns the FIRST pair
                    // (insertion order) as [key, value]; nil when
                    // empty (CRuby 3.x; the old default-return form
                    // was dropped in 3.2).
                    ("shift", []) => {
                        let first = {
                            let pairs = self.heap.hash_mut(id);
                            if pairs.is_empty() { None } else { Some(pairs.remove(0)) }
                        };
                        match first {
                            Some((k, v)) => {
                                self.maybe_gc();
                                let nid = self.heap.alloc(HeapObj::Array(vec![k, v].into()));
                                Some(Value::Array(nid))
                            }
                            None => Some(Value::Nil),
                        }
                    }
                    ("keys", []) => {
                        let keys: Vec<Value> = self.heap.hash(id).iter().map(|(k, _)| k.clone()).collect();
                        self.maybe_gc();
                        // check_alloc would need a `?`; collection_call returns Option,
                        // so we skip the cap check here. Embedders should set
                        // max_live with a small slack to account for these
                        // derived allocations.
                        let nid = self.heap.alloc(HeapObj::Array(keys.into()));
                        Some(Value::Array(nid))
                    }
                    ("values", []) => {
                        let vals: Vec<Value> = self.heap.hash(id).iter().map(|(_, v)| v.clone()).collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(vals.into()));
                        Some(Value::Array(nid))
                    }
                    // `h.values_at(*keys)` → the value for each key (a
                    // miss yields the hash's scalar default, else nil — a
                    // default PROC is not fired here, a minor divergence).
                    ("values_at", keys) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id)
                            .iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        let dflt = self.heap.hash_default_value(id);
                        let out: Vec<Value> = keys.iter().map(|key| {
                            pairs.iter()
                                .find(|(hk, _)| hk.ruby_eql(key, &self.heap))
                                .map(|(_, v)| v.clone())
                                .or_else(|| dflt.clone())
                                .unwrap_or(Value::Nil)
                        }).collect();
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out.into()));
                        Some(Value::Array(nid))
                    }
                    // `fetch_values(*keys)` — like `values_at` but raises
                    // KeyError on a missing key (no default fallback). rack
                    // Headers#fetch_values supers here with downcased keys.
                    ("fetch_values", keys) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id)
                            .iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        let mut out: Vec<Value> = Vec::with_capacity(keys.len());
                        for key in keys {
                            match pairs.iter().find(|(hk, _)| hk.ruby_eql(key, &self.heap)) {
                                Some((_, v)) => out.push(v.clone()),
                                None => {
                                    return Err(self.trap(RubyError::KeyError {
                                        msg: format!("key not found: {}",
                                            key.to_inspect(&self.heap, &self.interner)),
                                    }));
                                }
                            }
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out.into()));
                        Some(Value::Array(nid))
                    }
                    // `compare_by_identity` flips the bit and returns
                    // self (chainable). Identity-comparison semantics
                    // already hold for object/class/module/symbol keys
                    // in rubyrs's Hash; the flag is what
                    // `compare_by_identity?` reports and what dup/clone
                    // propagate. Primitive-value keys keep value
                    // semantics — documented Tier-1 divergence (see the
                    // `HashObj.by_identity` field). Frozen guard is via
                    // `is_hash_mutator` above (CRuby raises FrozenError).
                    // Motivating case: zeitwerk's `Zeitwerk::Cref::Map`
                    // (`@map = {}; @map.compare_by_identity`), keyed by
                    // Module objects.
                    ("compare_by_identity", []) => {
                        if let HeapObj::Hash(h) = self.heap.get(id) {
                            h.by_identity.set(true);
                        }
                        Some(Value::Hash(id))
                    }
                    ("compare_by_identity?", []) => Some(Value::Bool(
                        matches!(self.heap.get(id), HeapObj::Hash(h) if h.by_identity.get()),
                    )),
                    // `Hash#flatten(level = 1)` == `to_a.flatten(level)`:
                    // `to_a` is `[[k, v], ...]`, so level 1 spreads the
                    // pairs (`[k, v, k, v, ...]`) and leaves array VALUES
                    // nested; level 2 peels one more level. Equivalent to
                    // flattening each `[k, v]` pair at depth `level - 1`,
                    // which avoids allocating intermediate pair arrays.
                    ("flatten", many) if many.len() <= 1
                        && matches!(many.first(), None | Some(Value::Int(_))) => {
                        let level = match many.first() {
                            Some(Value::Int(n)) => *n,
                            _ => 1,
                        };
                        let inner_depth = if level < 0 { None } else { Some(level - 1) };
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id)
                            .iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        let mut out: Vec<Value> = Vec::with_capacity(pairs.len() * 2);
                        let mut changed = false;
                        let mut stack: Vec<ObjId> = Vec::new();
                        for (k, v) in pairs {
                            super::array::flatten_rec(
                                &self.heap, &[k, v], inner_depth,
                                &mut out, &mut changed, &mut stack);
                        }
                        self.maybe_gc();
                        let nid = self.heap.alloc(HeapObj::Array(out.into()));
                        Some(Value::Array(nid))
                    }
                    // No-block `each_key` / `each_value` → Enumerator; the
                    // block forms live in collection_call_block (iter.rs).
                    ("each_key", []) | ("each_value", []) => {
                        return self.make_enum_for(Value::Hash(id), name, vec![]).map(Some);
                    }
                    ("to_h", []) => Some(Value::Hash(id)),
                    // `Hash#to_hash` — explicit-conversion alias.
                    // Mirrors `String#to_str`: gems use
                    // `respond_to?(:to_hash)` as the duck-type
                    // probe to distinguish a real Hash from other
                    // values masquerading as options. Identical
                    // to `to_h` on a real Hash.
                    ("to_hash", []) => Some(Value::Hash(id)),
                    ("inspect", []) | ("to_s", []) => {
                        // M27 C1: CRuby's `Hash#to_s` is an alias of
                        // `Hash#inspect` (since 1.9). Both funnel through
                        // the cycle-safe, per-element `inspect`-dispatching
                        // renderer (`Vm::inspect_value`) so a self-
                        // referential hash renders `{...}` instead of
                        // overflowing the stack and custom / Exception
                        // values keep their real inspect.
                        let s = self.inspect_value(&Value::Hash(id))?;
                        // Seed encoding from the first KEY's inspect
                        // (CRuby's rule), promoting on non-ASCII bytes.
                        let seed = self.heap.hash(id).first().map(|(k, _)| k.clone()).unwrap_or(Value::Nil);
                        Some(self.tag_collection_inspect(&seed, s))
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
                        // merge sort over the pair list. We share
                        // the build path because both produce an
                        // Array<[k, v]>.
                        let mut pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        if name == "sort" {
                            match super::sort::merge_sort_by(&mut pairs, |a, b| {
                                match self.user_cmp(&a.0, &b.0) {
                                    Ok(Some(ord)) => Ok(ord),
                                    // Legacy decline on incomparable
                                    // keys — preserved as-is.
                                    Ok(None) => Err(super::sort::SortStop::Decline),
                                    Err(t) => Err(super::sort::SortStop::Trap(t)),
                                }
                            }) {
                                Ok(()) => {}
                                Err(super::sort::SortStop::Decline) => return Ok(None),
                                Err(super::sort::SortStop::Trap(t)) => return Err(t),
                                Err(_) => unreachable!("no comparator block in Hash#sort key sort"),
                            }
                        }
                        let nid = {
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id)); // source Hash
                            let mut pair_ids: Vec<Value> = Vec::with_capacity(pairs.len());
                            for (k, v) in pairs {
                                g.vm.maybe_gc();
                                let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                                g.pin(Value::Array(pid));
                                pair_ids.push(Value::Array(pid));
                            }
                            g.vm.maybe_gc();
                            g.vm.heap.alloc(HeapObj::Array(pair_ids.into()))
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
                        let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                        Some(Value::Array(pid))
                    }
                    // BigInt arg → RangeError, mirroring
                    // Array#first / #last at array.rs:511. A
                    // BigInt take-count is by construction larger
                    // than i64::MAX and can never be a meaningful
                    // size for a heap-bound collection.
                    #[cfg(feature = "bignum")]
                    ("first", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
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
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                            g.pin(Value::Array(pid));
                            pair_ids.push(Value::Array(pid));
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let aid = g.vm.heap.alloc(HeapObj::Array(pair_ids.into()));
                        Some(Value::Array(aid))
                    }
                    // Float coerce — CRuby truncates `first(2.5)` to
                    // 2. Self-recurse with the converted Int.
                    // Same pattern as Array#first / #last / #pop /
                    // #shift (PR #349).
                    ("first", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.hash_collection_call(id, name, &[Value::Int(n)]);
                    }
                    // Wrong-arity / non-Int catch-all. Was
                    // NoMethodError pre-fix — same lockstep
                    // contract violation pattern as the
                    // take/drop sweep in PR #340.
                    ("first", _) => {
                        return Err(self.arity_error_arg0_or_1_int(name, args));
                    }
                    // `h.take(n)` — returns the first n entries as
                    // Array<[k, v]>. Behaves like `first(n)`: caps
                    // at hash size, rejects negative n with
                    // ArgumentError, BigInt → RangeError. CRuby's
                    // Hash#take comes from Enumerable.
                    #[cfg(feature = "bignum")]
                    ("take", [Value::BigInt(_)]) | ("drop", [Value::BigInt(_)]) => {
                        return Err(self.trap(RubyError::RangeError {
                            msg: "bignum too big to convert into `long'".to_string(),
                        }));
                    }
                    // Float coerce — CRuby truncates `take(2.5)` to 2.
                    // Re-dispatch with the converted Int. Mirrors the
                    // each_slice/each_cons family from PR #338.
                    ("take" | "drop", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.hash_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("take", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(crate::error::RubyError::ArgumentError {
                                msg: "attempt to take negative size".to_string(),
                            }));
                        }
                        let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                        let take = n_usz.min(self.heap.hash(id).len());
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id)[..take].to_vec();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut pair_ids: Vec<Value> = Vec::with_capacity(take);
                        for (k, v) in pairs {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                            g.pin(Value::Array(pid));
                            pair_ids.push(Value::Array(pid));
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let aid = g.vm.heap.alloc(HeapObj::Array(pair_ids.into()));
                        Some(Value::Array(aid))
                    }
                    // `h.drop(n)` — returns entries AFTER the first n
                    // as Array<[k, v]>. Negative n raises
                    // ArgumentError; n ≥ size returns []. Mirrors
                    // Array#drop semantics.
                    ("drop", [Value::Int(n)]) => {
                        if *n < 0 {
                            return Err(self.trap(crate::error::RubyError::ArgumentError {
                                msg: "attempt to drop negative size".to_string(),
                            }));
                        }
                        let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                        let len = self.heap.hash(id).len();
                        let skip = n_usz.min(len);
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id)[skip..].to_vec();
                        let remain = pairs.len();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut pair_ids: Vec<Value> = Vec::with_capacity(remain);
                        for (k, v) in pairs {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                            g.pin(Value::Array(pid));
                            pair_ids.push(Value::Array(pid));
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let aid = g.vm.heap.alloc(HeapObj::Array(pair_ids.into()));
                        Some(Value::Array(aid))
                    }
                    // No-block `each` / `each_pair` / `each_with_index`
                    // (no args) returns an Enumerator — CRuby `enum.c`.
                    // The block forms live in `collection_call_block`
                    // (iter.rs); the Enumerator re-invokes them once
                    // driven, so `h.each_with_index.to_a`,
                    // `h.map { |k, v| ... }` (via each), etc. work.
                    ("each" | "each_pair" | "each_with_index", []) => {
                        return self.make_enum_for(Value::Hash(id), name, vec![]).map(Some);
                    }
                    // `h.transform_keys(mapping_hash)` (Ruby 2.5+, no
                    // block) — each key present in `mapping_hash` is
                    // replaced by its mapped value; keys absent from the
                    // mapping are kept unchanged. Last-wins on collision,
                    // preserving iteration order (CRuby).
                    ("transform_keys", [Value::Hash(mid)]) => {
                        let mid = *mid;
                        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        let mapping: Vec<(Value, Value)> = self.heap.hash(mid).clone();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        g.pin(Value::Hash(mid));
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let result_id = g.vm.heap.alloc(HeapObj::Hash(
                            crate::heap::HashObj::with_pairs(Vec::with_capacity(snapshot.len())),
                        ));
                        g.pin(Value::Hash(result_id));
                        for (k, v) in snapshot {
                            let new_key = mapping.iter()
                                .find(|(mk, _)| mk.ruby_eql(&k, &g.vm.heap))
                                .map(|(_, mv)| mv.clone())
                                .unwrap_or_else(|| k.clone());
                            let existing = g.vm.heap.hash(result_id).iter()
                                .position(|(k2, _)| k2.ruby_eql(&new_key, &g.vm.heap));
                            if let Some(p) = existing {
                                g.vm.heap.hash_mut(result_id)[p] = (new_key, v);
                            } else {
                                g.vm.heap.hash_mut(result_id).push((new_key, v));
                            }
                        }
                        Some(Value::Hash(result_id))
                    }
                    // Transform / filter Enumerable family with no block —
                    // returns an Enumerator (CRuby `enum.c`), re-invoking
                    // the block form (collection_call_block) once driven.
                    // Subset of the Array set: Hash has no min_by/max_by/
                    // reverse_each block form. Non-Enumerator no-block
                    // methods (sort_by-less sort, count, sum, …) excluded.
                    ("map" | "collect" | "select" | "filter" | "reject"
                        | "flat_map" | "collect_concat" | "filter_map"
                        | "find" | "detect" | "partition" | "group_by"
                        | "sort_by" | "transform_values" | "transform_keys", []) => {
                        return self.make_enum_for(Value::Hash(id), name, vec![]).map(Some);
                    }
                    // `h.each_with_object(memo)` with no block — Enumerator
                    // carrying the memo (block form at iter.rs). Hash has no
                    // min_by(n)/max_by(n) block form, so those stay a gap.
                    ("each_with_object", [_seed]) => {
                        return self.make_enum_for(Value::Hash(id), name, args.to_vec()).map(Some);
                    }
                    // `h.each_slice(n)` / `h.each_cons(n)` no-block return
                    // an Enumerator (CRuby `enum.c`); make_enum_for
                    // re-invokes the block form (iter.rs) once driven, so
                    // `h.each_slice(2).to_a` keeps the same shape (Array of
                    // slice/window Arrays of `[k, v]` pairs) while
                    // `.class` is now Enumerator. The size arg is validated
                    // eagerly (CRuby raises here, not on drive). Float
                    // coerce truncates per CRuby.
                    ("each_slice", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.hash_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("each_slice", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(crate::error::RubyError::ArgumentError {
                                msg: "invalid slice size".to_string(),
                            }));
                        }
                        return self.make_enum_for(Value::Hash(id), name, vec![Value::Int(*n)]).map(Some);
                    }
                    // Wrong-arity / non-Int for Hash#each_slice no-block form.
                    ("each_slice", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    ("each_cons", [Value::Float(f)]) => {
                        let n = self.float_to_int_arg(*f)?;
                        return self.hash_collection_call(id, name, &[Value::Int(n)]);
                    }
                    ("each_cons", [Value::Int(n)]) => {
                        if *n <= 0 {
                            return Err(self.trap(crate::error::RubyError::ArgumentError {
                                msg: "invalid size".to_string(),
                            }));
                        }
                        return self.make_enum_for(Value::Hash(id), name, vec![Value::Int(*n)]).map(Some);
                    }
                    // Wrong-arity / non-Int for Hash#each_cons no-block form.
                    ("each_cons", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
                    }
                    // `h.chunk_while(arg)` / `h.slice_when(arg)` without
                    // a block — arity guard mirrors Array's no-block arm
                    // and the block-form catch-all in iter.rs.
                    ("chunk_while" | "slice_when", many) if !many.is_empty() => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len()
                            ),
                        }));
                    }
                    // `h.find_index(target)` — Int insertion-order
                    // index of the first entry whose `[k, v]`
                    // pair `==` the target, or nil. CRuby's
                    // positional form on Hash (inherited from
                    // Enumerable). The block form lives in
                    // iter.rs.
                    ("find_index", [target]) => {
                        let target = target.clone();
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        for (i, (k, v)) in pairs.iter().enumerate() {
                            // Compare via a fresh [k, v] pair
                            // Array using ruby_eq. Allocating a
                            // throwaway pair per iter is the
                            // simplest path; the receiver pin
                            // happens implicitly because we
                            // never call maybe_gc inside the
                            // loop (ruby_eq is read-only).
                            let pid = self.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()].into()));
                            let pair = Value::Array(pid);
                            if pair.ruby_eq(&target, &self.heap) {
                                return Ok(Some(Value::Int(i as i64)));
                            }
                        }
                        Some(Value::Nil)
                    }
                    // `h.tally` (no block, no args) — returns a
                    // new Hash<[k, v], Int> counting each entry's
                    // pair. On a Hash receiver every pair is
                    // unique by definition (keys are eql?-unique),
                    // so every count is 1 — the behaviour is
                    // trivially Hash#each_with_index-shaped, but
                    // we still materialise the result Hash for
                    // CRuby parity (callers may chain
                    // `tally.values.sum` etc.).
                    ("tally", []) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let result_id = g.vm.heap.alloc(HeapObj::Hash(
                            crate::heap::HashObj::with_pairs(Vec::new())
                        ));
                        g.pin(Value::Hash(result_id));
                        for (k, v) in pairs {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                            // Each pair Array is unique per
                            // iteration (Hash keys eql?-unique
                            // by definition), so we always push
                            // a fresh entry with count = 1
                            // rather than re-scanning. After the
                            // push the pair is reachable via the
                            // pinned result Hash — no per-iter
                            // pin needed (would grow pinned-set
                            // O(n) for no benefit).
                            g.vm.heap.hash_mut(result_id).push((Value::Array(pid), Value::Int(1)));
                        }
                        Some(Value::Hash(result_id))
                    }
                    // `h.uniq` (no block) — returns all entries
                    // as Array<[k, v]>. Hash keys are already
                    // eql?-unique, so the result is trivially the
                    // pair list — but materialising the Array
                    // matches CRuby's surface (callers may
                    // chain `.size`, `.first`, etc.).
                    ("uniq", []) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        // Pre-alloc the result Array and pin it;
                        // direct-push each pair into it rather
                        // than accumulating in a Rust-local Vec
                        // + per-iter pinning each pair Array.
                        // Result Array roots all the pair Arrays
                        // through the GC walker, so pinned-set
                        // stays O(1) instead of O(n).
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let aid = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(pairs.len()).into()));
                        g.pin(Value::Array(aid));
                        for (k, v) in pairs {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                            g.vm.heap.array_mut(aid).push(Value::Array(pid));
                        }
                        Some(Value::Array(aid))
                    }
                    // `h.zip(*args)` — pairs each `[k, v]` entry
                    // with the corresponding element from each
                    // arg Array. Returns Array of `[pair,
                    // arg1_i, arg2_i, ...]`. Args shorter than
                    // the receiver fill with nil. With zero
                    // args, returns Array of `[[k, v]]`
                    // singletons. Only Array args are supported
                    // (Enumerator / Range args are Tier-2).
                    ("zip", args_slice) if args_slice.iter().all(|a| matches!(a, Value::Array(_))) => {
                        let receiver_pairs: Vec<(Value, Value)> = self.heap.hash(id).clone();
                        // Snapshot every arg Array's contents
                        // BEFORE the result-alloc loop so
                        // intermediate maybe_gc can't sweep
                        // them (each arg's ObjId is held only
                        // in args_slice, which is a Rust slice
                        // borrowed from caller).
                        let arg_lists: Vec<Vec<Value>> = args_slice.iter().map(|a| {
                            if let Value::Array(aid) = a {
                                self.heap.array(*aid).clone()
                            } else {
                                Vec::new()
                            }
                        }).collect();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        for a in args_slice {
                            g.pin(a.clone());
                        }
                        // Pre-alloc + pin the result Array;
                        // direct-push each tuple. Once the
                        // tuple is in the result Array it
                        // transitively roots its pair_id child
                        // too. Pinned-set stays O(1) instead of
                        // O(n) per receiver entry.
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let aid = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(receiver_pairs.len()).into()));
                        g.pin(Value::Array(aid));
                        for (i, (k, v)) in receiver_pairs.into_iter().enumerate() {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            // Build the per-entry tuple:
                            // [[k, v], arg1[i] || nil, arg2[i] || nil, ...]
                            let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                            // pair_id needs a brief pin only
                            // while we're allocating the
                            // tuple Array (one more maybe_gc
                            // window).
                            g.vm.pinned.push(Value::Array(pair_id));
                            let mut tuple: Vec<Value> = Vec::with_capacity(1 + arg_lists.len());
                            tuple.push(Value::Array(pair_id));
                            for list in &arg_lists {
                                tuple.push(list.get(i).cloned().unwrap_or(Value::Nil));
                            }
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let tid = g.vm.heap.alloc(HeapObj::Array(tuple.into()));
                            g.vm.pinned.pop();
                            // tid is now reachable via aid; no
                            // per-iter pin needed for either.
                            g.vm.heap.array_mut(aid).push(Value::Array(tid));
                        }
                        Some(Value::Array(aid))
                    }
                    // Wrong-arity for uniq — CRuby's no-block
                    // form takes no args; without this guard
                    // `h.uniq(1)` falls through to NoMethodError
                    // despite respond_to?(:uniq) returning true.
                    ("uniq", many) => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len(),
                            ),
                        }));
                    }
                    // Fallback for `zip` with a non-Array arg —
                    // matched after the typed `zip` arm above.
                    // CRuby coerces via `to_ary` / `each` for
                    // Enumerable args (Range / Enumerator);
                    // we restrict to Array in Tier 1, so anything
                    // else raises TypeError with a clear message
                    // rather than falling through to NoMethodError.
                    ("zip", _) => {
                        return Err(self.trap(crate::error::RubyError::TypeError {
                            msg: "Hash#zip in this subset only accepts Array arguments \
                                  (Range / Enumerator args are Tier-2)".to_string(),
                        }));
                    }
                    // `h.tally(target_hash)` — Ruby 2.7+
                    // accumulating form is out of subset.
                    // 1-arg form gets a specific "not
                    // supported" message; 2+ args get the
                    // standard wrong-arity shape so the
                    // diagnostic actually matches the input.
                    ("tally", many) => {
                        let msg = if many.len() == 1 {
                            "Hash#tally with an accumulating Hash argument is not \
                             supported in this subset (Ruby 2.7+ form)".to_string()
                        } else {
                            format!(
                                "wrong number of arguments (given {}, expected 0)",
                                many.len(),
                            )
                        };
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg,
                        }));
                    }
                    // Wrong-arity / non-Int catch-all for take / drop.
                    // Routes through `arity_error_arg1_int` so non-Int
                    // 1-arg surfaces as TypeError (CRuby parity) rather
                    // than the previous misleading "given 1, expected 1"
                    // ArgumentError that lumped both shapes together.
                    ("take" | "drop", _) => {
                        return Err(self.arity_error_arg1_int(name, args));
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
                        let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                        Some(Value::Array(pid))
                    }
                    ("dup", []) | ("clone", []) => {
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
                        // Preserve the Hash-subclass tag + ivars so
                        // `Conf[...].dup` / `.clone` stays a Conf with
                        // its instance state (CRuby copies both).
                        let class_tag = self.heap.hash_class_tag(id);
                        let ivars = self.heap.hash_ivars_clone(id);
                        // `clone` preserves the frozen bit; `dup` resets it.
                        let keep_frozen = name == "clone" && self.heap.hash_frozen(id);
                        // `compare_by_identity` survives BOTH dup and
                        // clone (CRuby copies the flag on each).
                        let by_identity = matches!(
                            self.heap.get(id),
                            crate::heap::HeapObj::Hash(h) if h.by_identity.get()
                        );
                        // `clone` also copies the per-instance singleton
                        // class (so `def h.foo` survives `h.clone`);
                        // `dup` drops it. CRuby: `h.dup.foo` → NoMethodError,
                        // `h.clone.foo` → works. (spec_headers#test_dup_and_clone)
                        let singleton_class = if name == "clone" {
                            if let crate::heap::HeapObj::Hash(h) = self.heap.get(id) {
                                h.singleton_class
                                    .as_ref()
                                    .map(|sc| std::rc::Rc::new(sc.shallow_copy()))
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let has_singleton = singleton_class.is_some();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        if let Some(bid) = default_block {
                            g.pin(Value::Block(bid));
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj {
                            pairs,
                            default_block: None,
                            default_value: None,
                            class_tag,
                            ivars,
                            index: None,
                            singleton_class,
                            frozen: std::cell::Cell::new(keep_frozen),
                            by_identity: std::cell::Cell::new(by_identity),
                        }));
                        if default_block.is_some() {
                            g.vm.heap.hash_set_default_block(nid, default_block);
                        }
                        if default_value.is_some() {
                            g.vm.heap.hash_set_default_value(nid, default_value);
                        }
                        if has_singleton {
                            // Keep the global gate + method-cache
                            // generation in sync, matching the
                            // `ensure_hash_singleton` install path —
                            // otherwise dispatch's fast path would
                            // skip the cloned eigenclass.
                            g.vm.any_hash_singletons = true;
                            g.vm.method_gen = g.vm.method_gen.wrapping_add(1);
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
                    // `h.merge!(other)` / `h.update(other)` — in-place
                    // counterpart of `merge`: keys in `other` overwrite
                    // self's (existing keys keep position, new keys
                    // append in other's order), mutating and returning
                    // self. `update` is CRuby's alias. The block-form
                    // conflict-resolver lives in `collection_call_block`
                    // (vm/iter.rs); this is the blockless path.
                    ("merge!", [Value::Hash(other)]) | ("update", [Value::Hash(other)]) => {
                        let extra: Vec<(Value, Value)> = self.heap.hash(*other).clone();
                        for (k, v) in extra {
                            let pos = self.heap.hash(id).iter()
                                .position(|(ek, _)| ek.ruby_eql(&k, &self.heap));
                            if let Some(p) = pos {
                                self.heap.hash_mut(id)[p].1 = v;
                            } else {
                                self.heap.hash_mut(id).push((k, v));
                            }
                        }
                        Some(Value::Hash(id))
                    }
                    // `h.replace(other)` — replace the receiver's
                    // contents wholesale with `other`'s pairs, in
                    // place, returning self. Core Ruby; ActiveSupport's
                    // `deep_transform_keys!` (and the deep symbolize/
                    // stringify bang methods built on it) rely on it.
                    ("replace", [Value::Hash(other)]) => {
                        let pairs: Vec<(Value, Value)> = self.heap.hash(*other).clone();
                        *self.heap.hash_mut(id) = pairs;
                        Some(Value::Hash(id))
                    }
                    // `Hash#clear` — remove all pairs, return self.
                    // Discovery: P3 Jekyll spike — Liquid's
                    // strainer.rb `global_filter` clears its filter
                    // cache.
                    ("clear", []) => {
                        self.heap.hash_mut(id).clear();
                        Some(Value::Hash(id))
                    }
                    ("delete", [k]) => {
                        // Index-aware delete (O(1) lookup; drops + lazily
                        // rebuilds the index since removal shifts later
                        // positions). Returns the removed value or nil.
                        Some(self.heap.hash_delete(id, k).unwrap_or(Value::Nil))
                    }
                    // `Hash#key(value)` — the first key whose value
                    // `==` the argument, or nil. Reverse of `[]`.
                    // Discovery: P3 Jekyll spike — log_adapter.rb's
                    // `LOG_LEVELS.key(writer.level)`.
                    ("key", [v]) => {
                        let found = self.heap.hash(id).iter()
                            .find(|(_, val)| val.ruby_eql(v, &self.heap))
                            .map(|(k, _)| k.clone());
                        Some(found.unwrap_or(Value::Nil))
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
                    _ => None,
                }
        )
    }
}
