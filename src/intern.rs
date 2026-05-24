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
}
