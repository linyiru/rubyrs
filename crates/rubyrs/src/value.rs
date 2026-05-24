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
    Str(Rc<str>),
    Sym(SymId),
    Bool(bool),
    Nil,
    Class(Rc<Class>),
    Object(ObjId),
    Array(ObjId),
    Hash(ObjId),
    Range(ObjId),
    Block(Rc<BlockHandle>),
}

#[derive(Debug)]
pub struct BlockHandle {
    pub(crate) proto_idx: usize,
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
