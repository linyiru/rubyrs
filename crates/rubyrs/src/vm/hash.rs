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
            | "compare_by_identity" | "rehash"
    )
}

impl Vm {
    /// Resolve a USER-defined `hash` / `eql?` method for a Hash key, or `None`
    /// for the builtin. Only object-shaped keys (instances and Class/Module
    /// objects) can override these; primitives always use the fast heap path.
    /// Used so a Hash whose keys override `hash`/`eql?` matches CRuby (which
    /// calls `key.hash` on insert and `key.eql?` to disambiguate collisions).
    pub(crate) fn key_user_method(
        &self,
        key: &Value,
        name_id: crate::intern::SymId,
    ) -> Option<std::rc::Rc<crate::value::Method>> {
        match key {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, name_id)
            }
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id),
            _ => None,
        }
    }

    /// `true` if the key overrides `hash` or `eql?` — such keys can't use the
    /// identity-based heap index and must be compared via Ruby dispatch.
    /// `hash_sym`/`eql_sym` are interned once by the caller.
    pub(crate) fn key_needs_ruby_hash(
        &self,
        key: &Value,
        hash_sym: crate::intern::SymId,
        eql_sym: crate::intern::SymId,
    ) -> bool {
        if !matches!(key, Value::Object(_) | Value::Class(_)) {
            return false;
        }
        self.key_user_method(key, hash_sym).is_some()
            || self.key_user_method(key, eql_sym).is_some()
    }

    /// A `compare_by_identity` Hash keys strictly on object identity and NEVER
    /// calls `hash`/`eql?` — zeitwerk's Cref::Map relies on this to store
    /// non-hashable module objects as keys. Such Hashes must always use the fast
    /// identity path even when the key overrides `hash`/`eql?`.
    pub(crate) fn hash_is_by_identity(&self, id: ObjId) -> bool {
        matches!(self.heap.get(id), HeapObj::Hash(h) if h.by_identity.get())
    }

    /// Synchronously invoke a resolved method and return its result (propagating
    /// a raise as a `Trap`). The `invoke_method` + `dispatch_until` pattern.
    pub(crate) fn call_resolved_method(
        &mut self,
        m: std::rc::Rc<crate::value::Method>,
        recv: Value,
        args: Vec<Value>,
    ) -> Result<Value, Trap> {
        let pre_frames = self.frames.len();
        let mut g = PinGuard::new(self);
        g.pin(recv.clone());
        for a in &args {
            g.pin(a.clone());
        }
        g.vm.invoke_method(m, recv, args)?;
        g.vm.dispatch_until(pre_frames)?;
        Ok(g.vm.stack.pop().unwrap_or(Value::Nil))
    }

    /// Ruby-level key equality for keys that override `hash`/`eql?`: `a.eql?(b)`.
    /// Falls back to the fast `ruby_eql` when `a` has no user `eql?`.
    pub(crate) fn keys_ruby_eql(
        &mut self,
        a: &Value,
        b: &Value,
        eql_sym: crate::intern::SymId,
    ) -> Result<bool, Trap> {
        if let Some(m) = self.key_user_method(a, eql_sym) {
            let r = self.call_resolved_method(m, a.clone(), vec![b.clone()])?;
            return Ok(!matches!(r, Value::Nil | Value::Bool(false)));
        }
        Ok(a.ruby_eql(b, &self.heap))
    }

    /// Call a key's user `hash` override and reduce it to an i64 bucket key.
    /// A non-Integer result folds to 0 (still correct: `eql?` disambiguates
    /// within a bucket — just less selective for those rare keys). Also serves
    /// CRuby's arity-raise on a bad `hash` override. Caller guarantees `key`
    /// has a user `hash` method (checked via `key_needs_ruby_hash`).
    fn key_ruby_hash(&mut self, key: &Value, hash_sym: crate::intern::SymId) -> Result<i64, Trap> {
        if let Some(m) = self.key_user_method(key, hash_sym) {
            let r = self.call_resolved_method(m, key.clone(), vec![])?;
            return Ok(match r { Value::Int(n) => n, _ => 0 });
        }
        Ok(0)
    }

    /// Build Hash `id`'s user-key index if absent: bucket every user-`hash`/
    /// `eql?` key by its Ruby `#hash`. Native keys interleaved in a mixed Hash
    /// are skipped (they use the identity-based heap `index`). O(n) dispatches,
    /// done once and reused until a delete/`hash_mut` invalidates it.
    fn ensure_user_index(
        &mut self,
        id: ObjId,
        hash_sym: crate::intern::SymId,
        eql_sym: crate::intern::SymId,
    ) -> Result<(), Trap> {
        if matches!(self.heap.get(id), HeapObj::Hash(h) if h.user_index().is_some()) {
            return Ok(());
        }
        let n = self.heap.hash(id).len();
        let mut idx: crate::intern::FxHashMap<i64, Vec<u32>> = crate::intern::FxHashMap::default();
        let pg = PinGuard::new(self);
        for i in 0..n {
            let k = match pg.vm.heap.get(id) {
                HeapObj::Hash(h) if i < h.pairs.len() => h.pairs[i].0.clone(),
                _ => break,
            };
            if pg.vm.key_needs_ruby_hash(&k, hash_sym, eql_sym) {
                let hv = pg.vm.key_ruby_hash(&k, hash_sym)?;
                idx.entry(hv).or_default().push(i as u32);
            }
        }
        pg.vm.heap.hash_obj_mut(id).extras_mut().user_index = Some(idx);
        Ok(())
    }

    /// `eql?`-scan the positions in `id`'s user-index bucket for `hv`, returning
    /// the pair index of the first key equal to `key` (or None). O(bucket-size)
    /// — usually 1. Assumes `ensure_user_index` has run.
    fn vm_hash_find_bucketed(
        &mut self,
        id: ObjId,
        key: &Value,
        hv: i64,
        eql_sym: crate::intern::SymId,
    ) -> Result<Option<usize>, Trap> {
        let bucket: Vec<u32> = match self.heap.get(id) {
            HeapObj::Hash(h) => h
                .user_index()
                .and_then(|ui| ui.get(&hv))
                .cloned()
                .unwrap_or_default(),
            _ => return Ok(None),
        };
        if bucket.is_empty() {
            return Ok(None);
        }
        let mut pg = PinGuard::new(self);
        pg.pin(key.clone());
        for &pos in &bucket {
            let existing = match pg.vm.heap.get(id) {
                HeapObj::Hash(h) if (pos as usize) < h.pairs.len() => h.pairs[pos as usize].0.clone(),
                _ => continue,
            };
            if pg.vm.keys_ruby_eql(key, &existing, eql_sym)? {
                return Ok(Some(pos as usize));
            }
        }
        Ok(None)
    }

    /// One-`key.hash`-dispatch locate: returns `(hv, pos)` where `hv` is the
    /// key's Ruby `#hash` (`Some` only for user-`hash`/`eql?` keys — callers
    /// thread it into `vm_hash_append` so a full upsert costs exactly ONE
    /// hash dispatch, CRuby's per-op contract) and `pos` is the pair index
    /// of the eql?-equal stored key, if any. Plain / `compare_by_identity`
    /// keys take the identity-index path with `hv = None`.
    pub(crate) fn vm_hash_locate(
        &mut self,
        id: ObjId,
        key: &Value,
        hash_sym: crate::intern::SymId,
        eql_sym: crate::intern::SymId,
    ) -> Result<(Option<i64>, Option<usize>), Trap> {
        if self.hash_is_by_identity(id) || !self.key_needs_ruby_hash(key, hash_sym, eql_sym) {
            return Ok((None, self.heap.hash_index_lookup(id, key)));
        }
        // Pin the query key: `ensure_user_index`/`key_ruby_hash` dispatch user
        // `hash` methods that allocate → can GC, and the query key (freshly
        // built, e.g. `h[Key.new(...)]`) may be reachable only through this
        // borrow. Without the pin, a rebuild (after a delete cleared the index)
        // sweeps it → `class_of` on a dead slot.
        let mut pg = PinGuard::new(self);
        pg.pin(key.clone());
        pg.vm.ensure_user_index(id, hash_sym, eql_sym)?;
        let hv = pg.vm.key_ruby_hash(key, hash_sym)?;
        let pos = pg.vm.vm_hash_find_bucketed(id, key, hv, eql_sym)?;
        Ok((Some(hv), pos))
    }

    /// Append a NOT-PRESENT pair (caller established absence via
    /// `vm_hash_locate`), maintaining whichever index tracks the key:
    /// `hv = Some` threads the located user-index bucket (no second hash
    /// dispatch); plain keys keep the identity index live. Append never
    /// shifts existing positions, so both indexes stay valid.
    pub(crate) fn vm_hash_append(&mut self, id: ObjId, key: Value, val: Value, hv: Option<i64>) {
        match hv {
            Some(hv) => {
                let h = self.heap.hash_obj_mut(id);
                let pos = h.pairs.len() as u32;
                h.pairs.push((key, val));
                if let Some(ui) = h.user_index_mut() {
                    ui.entry(hv).or_default().push(pos);
                }
            }
            None => self.heap.hash_append_new(id, key, val),
        }
    }

    /// Find the index of `key` in Hash `id`, honoring a user-defined `hash`/
    /// `eql?` on the key. Ordinary keys use the fast identity-based heap index
    /// (zero overhead). User-hash keys use the bucketed `user_index` (Ruby
    /// `#hash` → positions), so this is O(1)-amortized instead of an O(n)
    /// `eql?` scan — RuboCop's `add_offense` dedup (`Set#add?` over Range keys)
    /// was O(offenses²) without it. Used by `[]` / `fetch` / `key?` / `delete`
    /// / `assoc` / `values_at` / `slice` / `dig` (and every other lookup-shaped
    /// entry point) so a Hash with hash/eql-overriding keys matches CRuby.
    pub(crate) fn vm_hash_find(&mut self, id: ObjId, key: &Value) -> Result<Option<usize>, Trap> {
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        Ok(self.vm_hash_locate(id, key, hash_sym, eql_sym)?.1)
    }

    /// Insert `key`→`val`, honoring user-defined `hash`/`eql?`: calls `key.hash`
    /// (so a wrong-arity override raises, like CRuby) and finds an existing
    /// entry via `eql?`. Ordinary keys take the fast index-maintaining heap
    /// path. User keys are stored in the pairs Vec only (the identity index
    /// can't hold them), found henceforth via `vm_hash_find`. On update the
    /// ORIGINAL key object keeps its position and only the value is replaced
    /// (CRuby `rb_hash_aset`). Every inserting entry point funnels here —
    /// `[]=` / `store` / merge family / `Hash[]` / `to_h` / `invert` /
    /// `transform_keys` / Marshal load — so the dedup semantics stay in one
    /// place.
    pub(crate) fn vm_hash_insert(
        &mut self,
        id: ObjId,
        key: Value,
        val: Value,
    ) -> Result<Option<Value>, Trap> {
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        self.vm_hash_insert_syms(id, key, val, hash_sym, eql_sym)
    }

    /// `vm_hash_insert` with the syms pre-interned — the loop form for the
    /// merge family (one interner probe per call site, not per key).
    pub(crate) fn vm_hash_insert_syms(
        &mut self,
        id: ObjId,
        key: Value,
        val: Value,
        hash_sym: crate::intern::SymId,
        eql_sym: crate::intern::SymId,
    ) -> Result<Option<Value>, Trap> {
        if self.hash_is_by_identity(id) || !self.key_needs_ruby_hash(&key, hash_sym, eql_sym) {
            return Ok(self.heap.hash_insert(id, key, val));
        }
        // Compute `key.hash` ONCE (also does CRuby's arity-raise), then find +
        // insert within that bucket — O(1)-amortized, not an O(n) eql? scan.
        // Pin key + val: the index build / hash dispatch can GC, and the owned
        // `key`/`val` locals aren't GC roots until pushed into the (rooted)
        // pairs (see the vm_hash_locate note).
        let mut pg = PinGuard::new(self);
        pg.pin(key.clone());
        pg.pin(val.clone());
        let (hv, pos) = pg.vm.vm_hash_locate(id, &key, hash_sym, eql_sym)?;
        match pos {
            Some(i) => {
                let h = pg.vm.heap.hash_obj_mut(id);
                Ok(Some(std::mem::replace(&mut h.pairs[i].1, val)))
            }
            None => {
                pg.vm.vm_hash_append(id, key, val, hv);
                Ok(None)
            }
        }
    }

    /// True when Hash `id` holds at least one key that overrides
    /// `hash`/`eql?` (and the Hash is not `compare_by_identity`) — the gate
    /// for VM-dispatched Hash equality. Plain hashes keep the zero-dispatch
    /// native `ruby_eq` path.
    pub(crate) fn hash_has_user_keys(&mut self, id: ObjId) -> bool {
        if self.hash_is_by_identity(id) {
            return false;
        }
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        let h = self.heap.hash(id);
        h.iter().any(|(k, _)| self.key_needs_ruby_hash(k, hash_sym, eql_sym))
    }

    /// VM-aware Hash equality for hashes involving user-`hash`/`eql?` keys —
    /// CRuby's `rb_hash_equal`: same size, and every `[k, v]` of `a` found in
    /// `b` by KEY (hash/eql? honored via `vm_hash_find`) with a matching
    /// value (`==` semantics via `ruby_eq`; `eql?` semantics when `strict`).
    /// Callers gate on `hash_has_user_keys` so plain hashes keep the native
    /// `ruby_eq` compare byte-for-byte.
    pub(crate) fn vm_hash_eq(&mut self, a: ObjId, b: ObjId, strict: bool) -> Result<bool, Trap> {
        if a == b {
            return Ok(true);
        }
        if self.heap.hash(a).len() != self.heap.hash(b).len() {
            return Ok(false);
        }
        // CRuby: non-empty hashes whose compare_by_identity flags differ
        // are never equal (same rule as the native ruby_eq/ruby_eql arms).
        if !self.heap.hash(a).is_empty()
            && self.hash_is_by_identity(a) != self.hash_is_by_identity(b)
        {
            return Ok(false);
        }
        let pairs: Vec<(Value, Value)> = self.heap.hash(a).to_vec();
        let mut g = PinGuard::new(self);
        g.pin(Value::Hash(a));
        g.pin(Value::Hash(b));
        for (k, v) in pairs {
            match g.vm.vm_hash_find(b, &k)? {
                Some(pos) => {
                    let ov = g.vm.heap.hash(b)[pos].1.clone();
                    let eq = if strict {
                        v.ruby_eql(&ov, &g.vm.heap)
                    } else {
                        v.ruby_eq(&ov, &g.vm.heap)
                    };
                    if !eq {
                        return Ok(false);
                    }
                }
                None => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Dedup a literal's key/value pair buffer in place — the CRuby aset
    /// semantics shared by `{k => v, ...}` literals (`op_new_hash`) and the
    /// `Hash[...]` constructor: FIRST key position kept, LAST value wins.
    /// Keys overriding `hash`/`eql?` get CRuby's insert contract — `key.hash`
    /// dispatched once per key (a wrong-arity override raises here), `eql?`
    /// deciding the dedup. Plain pairs keep the zero-dispatch pairwise scan.
    pub(crate) fn hash_literal_dedup(
        &mut self,
        pairs: &mut crate::heap::PairsBuf,
    ) -> Result<(), Trap> {
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        let has_user = pairs
            .iter()
            .any(|(k, _)| self.key_needs_ruby_hash(k, hash_sym, eql_sym));
        if has_user {
            // Pin the pairs across the dispatches (they may live only in
            // this Rust-local buffer), then call each user key's `hash` and
            // dedup with Ruby equality.
            let mut g = PinGuard::new(self);
            for (k, v) in pairs.iter() {
                g.pin(k.clone());
                g.pin(v.clone());
            }
            for i in 0..pairs.len() {
                let k = pairs[i].0.clone();
                if let Some(m) = g.vm.key_user_method(&k, hash_sym) {
                    g.vm.call_resolved_method(m, k, vec![])?;
                }
            }
            let mut i = 0;
            while i < pairs.len() {
                let mut j = i + 1;
                while j < pairs.len() {
                    let (ki, kj) = (pairs[i].0.clone(), pairs[j].0.clone());
                    if g.vm.keys_ruby_eql(&ki, &kj, eql_sym)? {
                        pairs[i].1 = pairs[j].1.clone();
                        pairs.remove(j);
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
        } else {
            // Hash-prefiltered pairwise dedup: `ruby_hash` is consistent
            // with `ruby_eql` (equal keys hash equal), so keys with
            // different hashes skip the (comparatively costly) content
            // `ruby_eql` — the all-distinct literal (the overwhelmingly
            // common shape) does n hashes + cheap u64 compares and zero
            // eql calls.
            let mut hashes: smallvec::SmallVec<[u64; crate::heap::HASH_INLINE_PAIRS]> =
                pairs.iter().map(|(k, _)| k.ruby_hash(&self.heap)).collect();
            let mut i = 0;
            while i < pairs.len() {
                let mut j = i + 1;
                while j < pairs.len() {
                    if hashes[j] == hashes[i]
                        && pairs[j].0.ruby_eql(&pairs[i].0, &self.heap)
                    {
                        pairs[i].1 = pairs[j].1.clone();
                        pairs.remove(j);
                        hashes.remove(j);
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
        }
        Ok(())
    }

    /// Position of the first key in `keys` equal to `key`, honoring user
    /// `eql?` when `key` overrides `hash`/`eql?`. For result builders whose
    /// accumulator is a scratch Vec, not a live Hash (`group_by` buckets,
    /// `transform_keys!`). Linear; the plain path is the old `ruby_eql`
    /// scan unchanged. Caller must keep `keys` values GC-rooted (pinned or
    /// reachable) — the `eql?` dispatch can collect.
    pub(crate) fn find_key_ruby(
        &mut self,
        keys: &[Value],
        key: &Value,
    ) -> Result<Option<usize>, Trap> {
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        if !self.key_needs_ruby_hash(key, hash_sym, eql_sym) {
            return Ok(keys.iter().position(|k| k.ruby_eql(key, &self.heap)));
        }
        for (i, k) in keys.iter().enumerate() {
            let k = k.clone();
            if self.keys_ruby_eql(key, &k, eql_sym)? {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// Cheap gate for the Op-level Hash fast paths: a key overriding
    /// `hash`/`eql?` can't use the identity index, so the fast path must defer
    /// to the (vm-aware) slow path. Primitives short-circuit with no interning.
    pub(crate) fn hash_key_needs_slow(&mut self, key: &Value) -> bool {
        if !matches!(key, Value::Object(_) | Value::Class(_)) {
            return false;
        }
        let hs = self.interner.intern("hash");
        let es = self.interner.intern("eql?");
        self.key_needs_ruby_hash(key, hs, es)
    }

    /// Delete `key`, honoring user `hash`/`eql?` (eql? scan). Ordinary keys use
    /// the fast heap delete.
    pub(crate) fn vm_hash_delete(&mut self, id: ObjId, key: &Value) -> Result<Option<Value>, Trap> {
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        if self.hash_is_by_identity(id) || !self.key_needs_ruby_hash(key, hash_sym, eql_sym) {
            return Ok(self.heap.hash_delete(id, key));
        }
        match self.vm_hash_find(id, key)? {
            Some(i) => {
                let h = self.heap.hash_obj_mut(id);
                let (_, v) = h.pairs.remove(i);
                // positions shifted — drop both indexes, rebuilt lazily
                h.clear_indexes();
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    /// Resolve the operand of a Hash comparison (`< <= > >=`) to a Hash
    /// ObjId — CRuby's `rb_to_hash_type`: a Hash passes through, an
    /// object with `to_hash` converts (result must be a Hash), anything
    /// else raises TypeError `no implicit conversion of X into Hash`
    /// (nil/true/false render as their literals, objects as their class
    /// name — same shape as CRuby's convert-type message).
    fn hash_cmp_operand(&mut self, other: &Value) -> Result<ObjId, Trap> {
        if let Value::Hash(oid) = other {
            return Ok(*oid);
        }
        // `to_hash` duck conversion — the Kernel#Hash shape (kernel.rs).
        let mid = self.interner.intern("to_hash");
        let m = match self.class_of(other) {
            Value::Class(cls) => self.lookup_method_uncached(&cls, mid),
            _ => None,
        };
        if let Some(m) = m {
            let r = self.call_resolved_method(m, other.clone(), vec![])?;
            if let Value::Hash(oid) = r {
                return Ok(oid);
            }
        }
        let tn = match other {
            Value::Object(oid) => {
                crate::value::class_display_name(&self.heap.class_of(*oid))
            }
            v => super::numeric::type_name_for_coerce(v).to_string(),
        };
        Err(self.trap(RubyError::TypeError {
            msg: format!("no implicit conversion of {} into Hash", tn),
        }))
    }

    /// Pairwise subset test for Hash comparison: every `[k, v]` of `sub`
    /// is present in `sup` — the key found with Hash-lookup semantics
    /// (`vm_hash_find`, honoring user `hash`/`eql?`), the value compared
    /// with `==` (`ruby_eq`, the same equality `rassoc`/`value?` use).
    /// Both hashes are pinned across the walk: `vm_hash_find` may run
    /// Ruby-level `eql?`/`hash` which can allocate and GC, and neither
    /// receiver nor operand is rooted here (both arrived as popped
    /// Rust-local ObjIds from do_call).
    fn vm_hash_pairs_subset(&mut self, sub: ObjId, sup: ObjId) -> Result<bool, Trap> {
        let pairs: Vec<(Value, Value)> = self.heap.hash(sub).to_vec();
        let mut g = PinGuard::new(self);
        g.pin(Value::Hash(sub));
        g.pin(Value::Hash(sup));
        for (k, v) in pairs {
            match g.vm.vm_hash_find(sup, &k)? {
                Some(pos) => {
                    let other_v = g.vm.heap.hash(sup)[pos].1.clone();
                    if !v.ruby_eq(&other_v, &g.vm.heap) {
                        return Ok(false);
                    }
                }
                None => return Ok(false),
            }
        }
        Ok(true)
    }

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
                    // (set via `Hash.new(value)`) or nil.
                    ("default", []) => {
                        Some(self.heap.hash_default_value(id).unwrap_or(Value::Nil))
                    }
                    // 1-arg `h.default(key)`: a default_proc (block) is
                    // invoked with `(self, key)`; otherwise the scalar
                    // default (or nil) is returned, ignoring the key.
                    // Mirrors the `[]` lookup-miss default-block path
                    // below. Sinatra's `IndifferentHash#default(*args)`
                    // maps keys then `super(*args)` here (test_default_block:
                    // `IH.new { |h, k| h[k] = k.upcase }.default(:a) == "A"`).
                    ("default", [key]) => {
                        if let Some(block_id) = self.heap.hash_default_block(id) {
                            let key = key.clone();
                            let pre_frames = self.frames.len();
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id));
                            g.pin(key.clone());
                            g.pin(Value::Block(block_id));
                            match g.vm.step_block(block_id, vec![Value::Hash(id), key], pre_frames)? {
                                crate::vm::iter::BlockStep::MethodReturn => Some(Value::Nil),
                                crate::vm::iter::BlockStep::Break(_) => {
                                    return Err(g.vm.trap(crate::error::RubyError::LocalJumpError {
                                        msg: "break from proc-closure".into(),
                                    }));
                                }
                                crate::vm::iter::BlockStep::Value(r) => Some(r),
                            }
                        } else {
                            Some(self.heap.hash_default_value(id).unwrap_or(Value::Nil))
                        }
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
                        // O(1) indexed hit (or eql?-aware for user keys).
                        if let Some(pos) = self.vm_hash_find(id, k)? {
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
                            && self.vm_hash_find(id, k)?.is_none()
                        {
                            let new_len = self.heap.hash(id).len().saturating_add(1);
                            if new_len.saturating_mul(std::mem::size_of::<(Value, Value)>()) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("Hash []= would exceed {max} bytes"),
                                }));
                            }
                        }
                        // Index-maintaining insert (or eql?-aware for keys
                        // overriding hash/eql).
                        self.vm_hash_insert(id, k.clone(), v.clone())?;
                        Some(v.clone())
                    }
                    ("empty?", []) => Some(Value::Bool(self.heap.hash(id).is_empty())),
                    ("dig", keys) if !keys.is_empty() => {
                        // Walk the keys/indices, looking up at each
                        // step. Nil at any level short-circuits.
                        // `dig_step` dispatches `.dig` on a nested
                        // subclass / user object (so a nested
                        // IndifferentHash re-converts its key) and
                        // raises TypeError on a non-diggable value.
                        let mut cur = Value::Hash(id);
                        for (i, key) in keys.iter().enumerate() {
                            cur = self.dig_step(&cur, key, i > 0)?;
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
                        match self.vm_hash_find(id, k)? {
                            Some(p) => Some(self.heap.hash(id)[p].1.clone()),
                            None => {
                                // VM-aware inspect — CRuby renders the key
                                // via its (possibly user-defined) `inspect`.
                                let shown = self.inspect_value(k)?;
                                return Err(self.trap(RubyError::KeyError {
                                    msg: format!("key not found: {shown}"),
                                }));
                            }
                        }
                    }
                    ("fetch", [k, default]) => {
                        Some(match self.vm_hash_find(id, k)? {
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
                        Some(Value::Bool(self.vm_hash_find(id, k)?.is_some()))
                    }
                    // `h.assoc(key)` → [key, value] or nil. CRuby
                    // compares with ==; the index lookup uses eql?,
                    // identical for the string/symbol keys real
                    // callers (rack Headers#assoc supers here) use.
                    ("assoc", [k]) => {
                        // CRuby assoc returns the STORED pair (probed:
                        // `{-0.0 => 2}.assoc(0.0)` is `[-0.0, 2]`), not the
                        // argument key.
                        let found = self.vm_hash_find(id, k)?
                            .map(|pos| self.heap.hash(id)[pos].clone());
                        match found {
                            Some((sk, v)) => {
                                let mut g = PinGuard::new(self);
                                if sk.is_gc_heap_ref() { g.pin(sk.clone()); }
                                if v.is_gc_heap_ref() { g.pin(v.clone()); }
                                g.vm.maybe_gc();
                                let nid = g.vm.heap.alloc(HeapObj::Array(vec![sk, v].into()));
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
                                // The pair was just REMOVED from the hash and is
                                // held only in these Rust locals; the receiver
                                // hash is off-stack (not a root) and no longer
                                // contains them, so pinning it wouldn't help.
                                // Pin k and v directly across maybe_gc + alloc
                                // (else they're swept → dangling result slots).
                                let mut g = PinGuard::new(self);
                                g.pin(k.clone());
                                g.pin(v.clone());
                                g.vm.maybe_gc();
                                let nid = g.vm.heap.alloc(HeapObj::Array(vec![k, v].into()));
                                Some(Value::Array(nid))
                            }
                            None => Some(Value::Nil),
                        }
                    }
                    ("keys", []) => {
                        let keys: Vec<Value> = self.heap.hash(id).iter().map(|(k, _)| k.clone()).collect();
                        // The receiver Hash was popped off the operand stack at
                        // the do_call boundary, so it is no longer a GC root.
                        // Under STRESS_GC `maybe_gc` would sweep it AND every key
                        // it holds (e.g. Set#divide's inner-Set keys reached via
                        // `@hash.keys`), recycling their slots into the result
                        // Array's alloc → dangling ObjIds. Pin across the alloc.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        g.vm.maybe_gc();
                        // check_alloc would need a `?`; collection_call returns Option,
                        // so we skip the cap check here. Embedders should set
                        // max_live with a small slack to account for these
                        // derived allocations.
                        let nid = g.vm.heap.alloc(HeapObj::Array(keys.into()));
                        Some(Value::Array(nid))
                    }
                    ("values", []) => {
                        let vals: Vec<Value> = self.heap.hash(id).iter().map(|(_, v)| v.clone()).collect();
                        // Same hazard as `keys`: pin the popped receiver across
                        // maybe_gc/alloc (Set#divide's `classify().values` keeps
                        // the inner Sets reachable only through these values).
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(vals.into()));
                        Some(Value::Array(nid))
                    }
                    // `h.values_at(*keys)` → the value for each key (a
                    // miss yields the hash's scalar default, else nil — a
                    // default PROC is not fired here, a minor divergence).
                    ("values_at", keys) => {
                        // CRuby values_at is an aref per key (rb_hash_aref):
                        // full `[]` semantics — user `hash`/`eql?` honored,
                        // and a miss consults the scalar default AND the
                        // default proc (probed: `Hash.new { "p" }
                        // .values_at(:x)` is `["p"]`). Re-dispatch through
                        // the canonical `[]` arm per key so the two can't
                        // drift. Pin the receiver + accumulated values (the
                        // `[]` default-block / user-hash dispatch can GC).
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut out: Vec<Value> = Vec::with_capacity(keys.len());
                        for key in keys {
                            let v = g.vm
                                .hash_collection_call(id, "[]", &[key.clone()])?
                                .unwrap_or(Value::Nil);
                            if v.is_gc_heap_ref() { g.pin(v.clone()); }
                            out.push(v);
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out.into()));
                        Some(Value::Array(nid))
                    }
                    // `fetch_values(*keys)` — like `values_at` but raises
                    // KeyError on a missing key (no default fallback). rack
                    // Headers#fetch_values supers here with downcased keys.
                    ("fetch_values", keys) => {
                        // Same user-key-aware lookup as `values_at`, raising
                        // KeyError on any miss (no default fallback).
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut out: Vec<Value> = Vec::with_capacity(keys.len());
                        for key in keys {
                            match g.vm.vm_hash_find(id, key)? {
                                Some(p) => {
                                    let v = g.vm.heap.hash(id)[p].1.clone();
                                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                                    out.push(v);
                                }
                                None => {
                                    // VM-aware inspect: CRuby's KeyError
                                    // message renders the key via its own
                                    // (possibly user-defined) `inspect`.
                                    let shown = g.vm.inspect_value(key)?;
                                    return Err(g.vm.trap(RubyError::KeyError {
                                        msg: format!("key not found: {shown}"),
                                    }));
                                }
                            }
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Array(out.into()));
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
                    // `h < other` / `<=` / `>` / `>=` — proper/improper
                    // subset comparison (CRuby hash.c rb_hash_lt/le/gt/ge):
                    // `a <= b` iff every [key, value] pair of `a` is in `b`
                    // (key matched with Hash-lookup semantics — user
                    // `hash`/`eql?` honored via vm_hash_find — value
                    // compared with `==`); `<` additionally requires `a` to
                    // be strictly smaller. `>` / `>=` mirror with the sides
                    // swapped. A non-Hash argument goes through implicit
                    // `to_hash` conversion, TypeError otherwise (CRuby's
                    // rb_to_hash_type). Motivating consumer: rubocop 1.88's
                    // `Options#invalid_arguments_for_parallel` compares the
                    // parsed flag hash with `>` on every multi-file run.
                    ("<" | "<=" | ">" | ">=", [other]) => {
                        let oid = self.hash_cmp_operand(other)?;
                        let (sub, sup) = if matches!(name, "<" | "<=") { (id, oid) } else { (oid, id) };
                        let sub_len = self.heap.hash(sub).len();
                        let sup_len = self.heap.hash(sup).len();
                        let strict = matches!(name, "<" | ">");
                        let ok = if sub_len > sup_len || (strict && sub_len == sup_len) {
                            false
                        } else {
                            self.vm_hash_pairs_subset(sub, sup)?
                        };
                        Some(Value::Bool(ok))
                    }
                    ("<" | "<=" | ">" | ">=", many) => {
                        return Err(self.trap(crate::error::RubyError::ArgumentError {
                            msg: format!("wrong number of arguments (given {}, expected 1)", many.len()),
                        }));
                    }
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
                        let mut pairs: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                            if k.is_gc_heap_ref() { g.pin(k.clone()); }
                            if v.is_gc_heap_ref() { g.pin(v.clone()); }
                            // Mapping lookup honors user `hash`/`eql?` on the
                            // key (`vm_hash_find` on the live mapping Hash);
                            // the result upsert goes through `vm_hash_insert`
                            // so eql?-equal NEW keys collapse last-value-wins
                            // with the FIRST key object kept (CRuby aset).
                            let new_key = match g.vm.vm_hash_find(mid, &k)? {
                                Some(p) => g.vm.heap.hash(mid)[p].1.clone(),
                                None => k.clone(),
                            };
                            g.vm.vm_hash_insert(result_id, new_key, v)?;
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
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                        let receiver_pairs: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                        let pairs = self.heap.hash(id).to_vec();
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
                        let pairs: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
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
                                h.singleton_class()
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
                        let nid = {
                            let mut nh = crate::heap::HashObj::with_pairs_tagged(pairs, class_tag);
                            if !ivars.is_empty() {
                                nh.extras_mut().ivars = ivars;
                            }
                            if singleton_class.is_some() {
                                nh.extras_mut().singleton_class = singleton_class;
                            }
                            nh.frozen.set(keep_frozen);
                            nh.by_identity.set(by_identity);
                            g.vm.heap.alloc(HeapObj::Hash(nh))
                        };
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
                    ("merge", others) => {
                        // CRuby 3.0+: `merge` takes ZERO OR MORE hash-like args, applied
                        // left-to-right — keys in a later arg overwrite earlier ones, key
                        // order appends after self's (existing keys retain position). No args
                        // returns a copy of self. The result inherits the RECEIVER's
                        // default-block (`h.default_proc`) + subclass. A NON-Hash arg is
                        // coerced via `to_hash` (CRuby behaviour — e.g. RuboCop's `Config`
                        // responds to `to_hash`, so `default_config.merge(config)` works); an
                        // arg with no `to_hash` raises TypeError. (Block-form merge lives in
                        // iter.rs.) dry-types builds BOOLEAN_MAP with `EMPTY_HASH.merge(trues,
                        // falses)`.
                        //
                        // GC rooting: pin the receiver + every arg up front so a mid-merge GC
                        // — including one inside a re-entrant `to_hash` call — can't sweep
                        // them (or the pair ObjIds reachable only through them). Pin each
                        // coerced result too.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        for ov in others {
                            g.pin(ov.clone());
                        }
                        // Resolve each arg to a source hash's pairs (coercing non-Hashes).
                        let mut sources: Vec<Vec<(Value, Value)>> = Vec::with_capacity(others.len());
                        for ov in others {
                            let hid = match ov {
                                Value::Hash(o) => *o,
                                other => {
                                    let to_hash = g.vm.interner.intern("to_hash");
                                    if !g.vm.responds_to(other, to_hash, false) {
                                        return Err(g.vm.trap(RubyError::TypeError {
                                            msg: format!(
                                                "no implicit conversion of {} into Hash",
                                                other.type_name()
                                            ),
                                        }));
                                    }
                                    // Re-entrant `other.to_hash` — `do_call` pushes the
                                    // frame; `dispatch_until` drives the nested loop to
                                    // completion (user methods don't run synchronously
                                    // otherwise), leaving the result on the stack.
                                    let pre = g.vm.frames.len();
                                    g.vm.stack.push(other.clone());
                                    g.vm.do_call(to_hash, 0, false, u32::MAX)?;
                                    g.vm.dispatch_until(pre)?;
                                    match g.vm.stack.pop() {
                                        Some(Value::Hash(h)) => {
                                            g.pin(Value::Hash(h));
                                            h
                                        }
                                        _ => {
                                            return Err(g.vm.trap(RubyError::TypeError {
                                                msg: format!(
                                                    "can't convert {0} to Hash ({0}#to_hash gives a non-Hash)",
                                                    other.type_name()
                                                ),
                                            }));
                                        }
                                    }
                                }
                            };
                            sources.push(g.vm.heap.hash(hid).to_vec());
                        }
                        // CRuby merge = dup self, then aset each source pair —
                        // build the result Hash FIRST and upsert into it via
                        // `vm_hash_insert_syms`, which honors a user
                        // `hash`/`eql?` on the inserted key (existing key
                        // object + position kept, value replaced). A
                        // compare_by_identity RECEIVER keys by identity, so
                        // its source pairs take the identity insert (the
                        // fresh result doesn't carry the flag yet).
                        let default_block = g.vm.heap.hash_default_block(id);
                        if let Some(bid) = default_block {
                            g.pin(Value::Block(bid));
                        }
                        let out: Vec<(Value, Value)> = g.vm.heap.hash(id).to_vec();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(out)));
                        g.pin(Value::Hash(nid));
                        let hash_sym = g.vm.interner.intern("hash");
                        let eql_sym = g.vm.interner.intern("eql?");
                        let receiver_cbi = g.vm.hash_is_by_identity(id);
                        // Pin the snapshotted source pairs: a user `hash`
                        // dispatch inside the insert can GC, and an evil
                        // override could unroot them from the (pinned)
                        // source hashes mid-merge.
                        for extra in &sources {
                            for (k, v) in extra {
                                if k.is_gc_heap_ref() { g.pin(k.clone()); }
                                if v.is_gc_heap_ref() { g.pin(v.clone()); }
                            }
                        }
                        for extra in sources {
                            for (k, v) in extra {
                                if receiver_cbi {
                                    g.vm.heap.hash_insert(nid, k, v);
                                } else {
                                    g.vm.vm_hash_insert_syms(nid, k, v, hash_sym, eql_sym)?;
                                }
                            }
                        }
                        if default_block.is_some() {
                            g.vm.heap.hash_set_default_block(nid, default_block);
                        }
                        // Preserve the receiver's subclass (CRuby: merge returns an instance of
                        // the receiver's class — Sinatra's IndifferentHash#merge stays indifferent).
                        if let Some(tag) = g.vm.heap.hash_class_tag(id) {
                            g.vm.heap.hash_set_class_tag(nid, Some(tag));
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
                        // Per-pair `vm_hash_insert_syms`: honors a user
                        // `hash`/`eql?` on the inserted key (CRuby updates the
                        // existing entry in place — original key object +
                        // position kept — instead of appending a duplicate);
                        // plain keys route through the identity-index insert,
                        // same semantics as the old linear scan. Pin the
                        // receiver, source and snapshotted pairs — the user
                        // `hash` dispatch can GC.
                        let other = *other;
                        let extra: Vec<(Value, Value)> = self.heap.hash(other).to_vec();
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        g.pin(Value::Hash(other));
                        for (k, v) in &extra {
                            if k.is_gc_heap_ref() { g.pin(k.clone()); }
                            if v.is_gc_heap_ref() { g.pin(v.clone()); }
                        }
                        let hash_sym = g.vm.interner.intern("hash");
                        let eql_sym = g.vm.interner.intern("eql?");
                        for (k, v) in extra {
                            g.vm.vm_hash_insert_syms(id, k, v, hash_sym, eql_sym)?;
                        }
                        Some(Value::Hash(id))
                    }
                    // `h.replace(other)` — replace the receiver's
                    // contents wholesale with `other`'s pairs, in
                    // place, returning self. Core Ruby; ActiveSupport's
                    // `deep_transform_keys!` (and the deep symbolize/
                    // stringify bang methods built on it) rely on it.
                    ("replace", [Value::Hash(other)]) => {
                        let other = *other;
                        let pairs: Vec<(Value, Value)> = self.heap.hash(other).to_vec();
                        *self.heap.hash_mut(id) = pairs.into();
                        // CRuby rb_hash_replace also copies (or clears) the
                        // OTHER hash's default — value or proc — and its
                        // compare_by_identity flag (probed on 3.4: a
                        // `Hash.new(:D)` replaced with a defaultless hash
                        // reports `default == nil`, and vice versa).
                        let dv = self.heap.hash_default_value(other);
                        let db = self.heap.hash_default_block(other);
                        self.heap.hash_set_default_value(id, dv);
                        self.heap.hash_set_default_block(id, db);
                        let cbi = self.hash_is_by_identity(other);
                        if let HeapObj::Hash(h) = self.heap.get(id) {
                            h.by_identity.set(cbi);
                        }
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
                    // `Hash#rehash` — CRuby recomputes every key's stored
                    // hash after in-place key mutation. rubyrs stores no
                    // per-pair hash (small hashes content-scan; the lazy
                    // index rebuilds from live content), so the observable
                    // effect to reproduce (probed on CRuby 3.4, see the
                    // hash_rehash fixture) is the DEDUP: keys that have
                    // BECOME eql? collapse — the FIRST key object keeps
                    // its position, the LAST value wins — and both lookup
                    // indexes are rebuilt. Frozen guard via
                    // `is_hash_mutator`. User `hash`/`eql?` keys are
                    // honored through the same Ruby-dispatch compare the
                    // literal-dedup path (`op_new_hash`) uses.
                    ("rehash", []) => {
                        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).to_vec();
                        let hash_sym = self.interner.intern("hash");
                        let eql_sym = self.interner.intern("eql?");
                        let by_identity = self.hash_is_by_identity(id);
                        let has_user = !by_identity
                            && snapshot
                                .iter()
                                .any(|(k, _)| self.key_needs_ruby_hash(k, hash_sym, eql_sym));
                        let mut pairs = snapshot;
                        if has_user {
                            // Pin across the eql? dispatches (they can GC and
                            // the snapshot is off the rooted heap copy).
                            let mut g = crate::vm::PinGuard::new(self);
                            for (k, v) in &pairs {
                                g.pin(k.clone());
                                g.pin(v.clone());
                            }
                            let mut i = 0;
                            while i < pairs.len() {
                                let mut j = i + 1;
                                while j < pairs.len() {
                                    let (ki, kj) = (pairs[i].0.clone(), pairs[j].0.clone());
                                    if g.vm.keys_ruby_eql(&ki, &kj, eql_sym)? {
                                        pairs[i].1 = pairs[j].1.clone();
                                        pairs.remove(j);
                                    } else {
                                        j += 1;
                                    }
                                }
                                i += 1;
                            }
                        } else {
                            let mut i = 0;
                            while i < pairs.len() {
                                let mut j = i + 1;
                                while j < pairs.len() {
                                    if pairs[j].0.ruby_eql(&pairs[i].0, &self.heap) {
                                        pairs[i].1 = pairs[j].1.clone();
                                        pairs.remove(j);
                                    } else {
                                        j += 1;
                                    }
                                }
                                i += 1;
                            }
                        }
                        // hash_mut clears both indexes — the rebuild-from-
                        // live-content that IS rubyrs's rehash.
                        *self.heap.hash_mut(id) = pairs.into();
                        Some(Value::Hash(id))
                    }
                    ("delete", [k]) => {
                        // Index-aware delete (O(1) lookup; drops + lazily
                        // rebuilds the index since removal shifts later
                        // positions). Returns the removed value or nil.
                        Some(self.vm_hash_delete(id, k)?.unwrap_or(Value::Nil))
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
                        // the last one through wins (first KEY object kept —
                        // aset semantics). VALUES become keys here, so a
                        // value overriding `hash`/`eql?` needs the
                        // user-aware insert path; plain values keep the old
                        // native dedup unchanged.
                        let hash_sym = self.interner.intern("hash");
                        let eql_sym = self.interner.intern("eql?");
                        let has_user = pairs
                            .iter()
                            .any(|(k, _)| self.key_needs_ruby_hash(k, hash_sym, eql_sym));
                        if has_user {
                            let mut g = PinGuard::new(self);
                            g.pin(Value::Hash(id));
                            for (k, v) in &pairs {
                                if k.is_gc_heap_ref() { g.pin(k.clone()); }
                                if v.is_gc_heap_ref() { g.pin(v.clone()); }
                            }
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let nid = g.vm.heap.alloc(HeapObj::Hash(
                                crate::heap::HashObj::with_pairs(Vec::with_capacity(pairs.len())),
                            ));
                            g.pin(Value::Hash(nid));
                            for (k, v) in pairs {
                                g.vm.vm_hash_insert_syms(nid, k, v, hash_sym, eql_sym)?;
                            }
                            return Ok(Some(Value::Hash(nid)));
                        }
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
                        // Resolve each argument key to its pair position via
                        // `vm_hash_find` (user `hash`/`eql?` honored — CRuby
                        // deletes each listed key from a dup), then filter by
                        // position.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut excluded: Vec<usize> = Vec::with_capacity(keys.len());
                        for k in keys {
                            if let Some(p) = g.vm.vm_hash_find(id, k)? {
                                excluded.push(p);
                            }
                        }
                        let pairs: Vec<(Value, Value)> = g.vm.heap.hash(id).iter()
                            .enumerate()
                            .filter(|(i, _)| !excluded.contains(i))
                            .map(|(_, pair)| pair.clone())
                            .collect();
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        Some(Value::Hash(nid))
                    }
                    // `h.slice(*keys)` — return a new Hash with only
                    // the listed keys, in ARGUMENT order (matches
                    // CRuby — `{a:1,c:3}.slice(:c, :a)` is
                    // `{c:3, a:1}`). Missing keys are silently skipped.
                    ("slice", keys) => {
                        // `vm_hash_find` per argument key — user
                        // `hash`/`eql?` honored; plain keys keep the
                        // identity path.
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Hash(id));
                        let mut pairs: Vec<(Value, Value)> = Vec::new();
                        let mut taken: Vec<usize> = Vec::with_capacity(keys.len());
                        for k in keys {
                            if let Some(p) = g.vm.vm_hash_find(id, k)?
                                && !taken.contains(&p)
                            {
                                taken.push(p);
                                // CRuby slice asets the ARGUMENT key into the
                                // result (probed: `{-0.0 => 2}.slice(0.0)` is
                                // `{0.0 => 2}`), not the stored key.
                                let v = g.vm.heap.hash(id)[p].1.clone();
                                if k.is_gc_heap_ref() { g.pin(k.clone()); }
                                if v.is_gc_heap_ref() { g.pin(v.clone()); }
                                pairs.push((k.clone(), v));
                            }
                        }
                        g.vm.maybe_gc();
                        let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
                        Some(Value::Hash(nid))
                    }
                    _ => None,
                }
        )
    }
}
