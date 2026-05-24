use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::intern::SymId;

// ---------- Values ----------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ObjId(pub(crate) u32);

#[derive(Clone)]
pub(crate) enum Value {
    Int(i64),
    Str(Rc<str>),
    Sym(SymId),
    Bool(bool),
    Nil,
    Class(Rc<Class>),
    Object(ObjId),
    Array(ObjId),
    Hash(ObjId),
    Block(Rc<BlockHandle>),
}

pub(crate) struct BlockHandle {
    pub(crate) proto_idx: usize,
    pub(crate) captured: Rc<RefCell<Vec<Value>>>,
    pub(crate) self_val: Value,
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
}

pub(crate) struct Class {
    pub(crate) name: String,
    pub(crate) methods: RefCell<HashMap<SymId, Rc<Method>>>,
}

pub(crate) struct Instance {
    pub(crate) class: Rc<Class>,
    pub(crate) ivars: HashMap<SymId, Value>,
}

pub(crate) struct Method {
    pub(crate) params: Vec<String>,
    pub(crate) proto_idx: usize,
}
