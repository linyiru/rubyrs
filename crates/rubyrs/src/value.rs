use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::intern::SymId;

// ---------- Values ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjId(pub(crate) u32);

#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    /// 64-bit float. Mixed arithmetic with Int promotes the Int
    /// (CRuby's "Float wins on mix" rule). Equality across the
    /// numeric types coerces too — `5 == 5.0` is `true`.
    Float(f64),
    Str(Rc<str>),
    Sym(SymId),
    Bool(bool),
    Nil,
    Class(Rc<Class>),
    Object(ObjId),
    Array(ObjId),
    Hash(ObjId),
    Range(ObjId),
    /// `Proc`-flavoured block value. Heap-managed since P2-13 —
    /// before that this was `Block(Rc<BlockHandle>)`, which formed
    /// an Rc cycle whenever a block's `captured` slots held the
    /// block itself (e.g. `p = proc { p }` patterns common in
    /// callback DSLs). Now the BlockHandle lives in a heap slot
    /// and is mark-swept like Array/Hash/Range.
    Block(ObjId),
}

#[derive(Debug)]
pub struct BlockHandle {
    pub(crate) proto_idx: usize,
    /// Shared with the frame the block executes in: when
    /// `Vm::invoke_block` pushes a frame for this block, the frame
    /// borrows the SAME `Rc<RefCell<Vec<Value>>>`, so writes to
    /// outer-frame variables inside the block are visible to
    /// subsequent invocations. The Rc here is shared frame-wise,
    /// not as a back-edge for ownership of the BlockHandle itself
    /// — that's the heap slot's job.
    pub(crate) captured: Rc<RefCell<Vec<Value>>>,
    pub(crate) self_val: Value,
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
}

#[derive(Debug)]
pub struct Class {
    pub(crate) name: String,
    pub(crate) methods: RefCell<HashMap<SymId, Rc<Method>>>,
    /// Parent class for method lookup. `None` only for the implicit root
    /// (Object); every user-defined class has a superclass (defaulting to
    /// Object if not specified).
    pub(crate) superclass: RefCell<Option<Rc<Class>>>,
}

#[derive(Debug)]
pub struct Instance {
    pub(crate) class: Rc<Class>,
    pub(crate) ivars: HashMap<SymId, Value>,
}

#[derive(Debug)]
pub struct Method {
    pub(crate) params: Vec<String>,
    pub(crate) proto_idx: usize,
}
