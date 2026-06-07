use std::rc::Rc;

/// rustc-hash-style `FxHasher` — a fast, non-cryptographic hasher for the
/// integer `SymId` keys the method / ivar / const tables use on the dispatch
/// hot path. The std default `SipHash` is cryptographic (DoS-resistant) but
/// overkill for these internal, non-attacker-controlled integer keys, and
/// showed up at ~6% self-time in the call-path profile. Not used for the
/// user-facing `Hash` (that keeps its own FNV-consistent path in heap.rs).
#[derive(Default)]
pub(crate) struct FxHasher {
    hash: u64,
}

impl FxHasher {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    #[inline]
    fn add(&mut self, i: u64) {
        self.hash = (self.hash.rotate_left(5) ^ i).wrapping_mul(Self::K);
    }
}

impl std::hash::Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(b as u64);
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// A `HashMap` keyed with [`FxHasher`] — for the internal SymId/Rc<str> tables.
pub(crate) type FxHashMap<K, V> =
    std::collections::HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

/// Opaque token identifying a string in the [`Interner`]. Equality is a
/// single u32 compare, which is what makes Ruby `Symbol#==`, method-dispatch
/// hash keys, and IVar lookups O(1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymId(pub(crate) u32);

/// Global string-intern table. Compile-time strings (method names, ivar
/// names, class names, string literals) live here as `Rc<str>`; each unique
/// content gets one [`SymId`]. The same table is shared by every Proto in
/// a Vm — it replaces the per-Proto `strings: Vec<String>` we used before.
pub(crate) struct Interner {
    map: FxHashMap<Rc<str>, SymId>,
    vec: Vec<Rc<str>>,
}

impl Interner {
    pub(crate) fn new() -> Self {
        Interner { map: FxHashMap::default(), vec: Vec::new() }
    }

    pub(crate) fn intern(&mut self, s: &str) -> SymId {
        if let Some(&id) = self.map.get(s) { return id; }
        let id = SymId(self.vec.len() as u32);
        let rc: Rc<str> = Rc::from(s);
        self.vec.push(rc.clone());
        self.map.insert(rc, id);
        id
    }

    pub(crate) fn resolve(&self, id: SymId) -> &Rc<str> {
        &self.vec[id.0 as usize]
    }

    /// Current number of interned symbols. The interner ID space is
    /// `[0, len())`. Used by `Vm` to enforce `Config::max_symbols`
    /// before interning a fresh string.
    pub(crate) fn len(&self) -> usize { self.vec.len() }

    /// Is `s` already interned? Lets cap-checking callers
    /// distinguish "would create a new symbol" (count against cap)
    /// from "lookup existing" (always allowed). Reuses the same
    /// hash probe `intern` would do.
    pub(crate) fn contains(&self, s: &str) -> bool {
        self.map.contains_key(s)
    }

    /// Discard every symbol with id `>= keep_len`, leaving the
    /// interner in the state it was at when `len() == keep_len`.
    /// Powers `Runtime::reset` — user-interned symbols from a
    /// prior eval are dropped so the next eval starts from the
    /// post-preamble baseline.
    ///
    /// Any SymId held by user code with `id.0 >= keep_len` is
    /// invalid after this call. The Runtime API caller is
    /// responsible for not retaining stale SymIds across reset
    /// (fuzz / per-request embedders discard user code on
    /// reset, so this isn't observable to them).
    pub(crate) fn truncate_to(&mut self, keep_len: usize) {
        if keep_len >= self.vec.len() {
            return;
        }
        for stale in self.vec.drain(keep_len..) {
            self.map.remove(&*stale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher;

    #[test]
    fn fxhasher_covers_all_write_widths() {
        // Every Hasher::write_* width + the byte path; deterministic, and
        // distinct inputs don't collapse to the same digest.
        let mut a = FxHasher::default();
        a.write_u8(1);
        a.write_u16(2);
        a.write_u32(3);
        a.write_u64(4);
        a.write_usize(5);
        a.write(b"rubyrs");
        let h1 = a.finish();

        let mut b = FxHasher::default();
        b.write_u8(1);
        b.write_u16(2);
        b.write_u32(3);
        b.write_u64(4);
        b.write_usize(5);
        b.write(b"rubyrs");
        assert_eq!(h1, b.finish(), "FxHasher is deterministic");

        let mut c = FxHasher::default();
        c.write_u32(99);
        assert_ne!(h1, c.finish(), "different input → different digest");
    }

    #[test]
    fn fxhashmap_round_trips_symid_keys() {
        let mut m: FxHashMap<SymId, i32> = FxHashMap::default();
        m.insert(SymId(1), 10);
        m.insert(SymId(2), 20);
        assert_eq!(m.get(&SymId(1)), Some(&10));
        assert_eq!(m.get(&SymId(2)), Some(&20));
        assert_eq!(m.get(&SymId(3)), None);
    }
}
