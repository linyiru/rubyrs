use std::collections::HashMap;
use std::rc::Rc;

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
    map: HashMap<Rc<str>, SymId>,
    vec: Vec<Rc<str>>,
}

impl Interner {
    pub(crate) fn new() -> Self {
        Interner { map: HashMap::new(), vec: Vec::new() }
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
