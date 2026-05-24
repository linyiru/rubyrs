use std::rc::Rc;

use crate::value::{Instance, ObjId, Value};

// ---------- GC Heap ----------

pub(crate) enum HeapObj {
    Instance(Instance),
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>), // insertion-ordered, linear lookup (PoC)
}

pub(crate) enum Slot {
    Live(HeapObj),
    Dead,
}

pub(crate) struct Heap {
    pub(crate) slots: Vec<Slot>,
    pub(crate) marks: Vec<bool>,
    pub(crate) free: Vec<u32>,
    pub(crate) live_count: usize,
    pub(crate) next_gc: usize,
}

impl Heap {
    pub(crate) fn new() -> Self {
        Heap { slots: vec![], marks: vec![], free: vec![], live_count: 0, next_gc: 1024 }
    }
    pub(crate) fn alloc(&mut self, obj: HeapObj) -> ObjId {
        self.live_count += 1;
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = Slot::Live(obj);
            self.marks[i as usize] = false;
            return ObjId(i);
        }
        let i = self.slots.len() as u32;
        self.slots.push(Slot::Live(obj));
        self.marks.push(false);
        ObjId(i)
    }
    pub(crate) fn get(&self, id: ObjId) -> &HeapObj {
        match &self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("ICE: use-after-free ObjId({})", id.0),
        }
    }
    pub(crate) fn get_mut(&mut self, id: ObjId) -> &mut HeapObj {
        match &mut self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("ICE: use-after-free ObjId({})", id.0),
        }
    }
    pub(crate) fn instance(&self, id: ObjId) -> &Instance {
        if let HeapObj::Instance(i) = self.get(id) { i } else { panic!("ICE: heap slot is not an Instance") }
    }
    pub(crate) fn instance_mut(&mut self, id: ObjId) -> &mut Instance {
        if let HeapObj::Instance(i) = self.get_mut(id) { i } else { panic!("ICE: heap slot is not an Instance") }
    }
    pub(crate) fn array(&self, id: ObjId) -> &Vec<Value> {
        if let HeapObj::Array(a) = self.get(id) { a } else { panic!("ICE: heap slot is not an Array") }
    }
    pub(crate) fn array_mut(&mut self, id: ObjId) -> &mut Vec<Value> {
        if let HeapObj::Array(a) = self.get_mut(id) { a } else { panic!("ICE: heap slot is not an Array") }
    }
    pub(crate) fn hash(&self, id: ObjId) -> &Vec<(Value, Value)> {
        if let HeapObj::Hash(h) = self.get(id) { h } else { panic!("ICE: heap slot is not a Hash") }
    }
    pub(crate) fn hash_mut(&mut self, id: ObjId) -> &mut Vec<(Value, Value)> {
        if let HeapObj::Hash(h) = self.get_mut(id) { h } else { panic!("ICE: heap slot is not a Hash") }
    }
    pub(crate) fn should_gc(&self) -> bool { self.live_count >= self.next_gc }

    pub(crate) fn collect(&mut self, roots: &[Value]) {
        for m in self.marks.iter_mut() { *m = false; }
        let mut worklist: Vec<ObjId> = Vec::new();
        for v in roots { Heap::visit_value(v, &mut self.marks, &mut worklist); }
        while let Some(id) = worklist.pop() {
            let children: Vec<Value> = match &self.slots[id.0 as usize] {
                Slot::Live(HeapObj::Instance(i)) => i.ivars.values().cloned().collect(),
                Slot::Live(HeapObj::Array(a)) => a.clone(),
                Slot::Live(HeapObj::Hash(h)) => {
                    let mut v = Vec::with_capacity(h.len() * 2);
                    for (k, val) in h { v.push(k.clone()); v.push(val.clone()); }
                    v
                }
                _ => vec![],
            };
            for v in &children { Heap::visit_value(v, &mut self.marks, &mut worklist); }
        }
        let mut live = 0usize;
        for i in 0..self.slots.len() {
            match &self.slots[i] {
                Slot::Live(_) => {
                    if self.marks[i] { live += 1; }
                    else {
                        self.slots[i] = Slot::Dead;
                        self.free.push(i as u32);
                    }
                }
                Slot::Dead => {}
            }
        }
        self.live_count = live;
        self.next_gc = (live * 2).max(1024);
    }

    pub(crate) fn visit_value(v: &Value, marks: &mut [bool], worklist: &mut Vec<ObjId>) {
        match v {
            Value::Object(id) | Value::Array(id) | Value::Hash(id) => {
                let i = id.0 as usize;
                if !marks[i] {
                    marks[i] = true;
                    worklist.push(*id);
                }
            }
            Value::Block(b) => {
                // Walk captured locals; also walk self_val
                let snapshot: Vec<Value> = b.captured.borrow().iter().cloned().collect();
                for v in &snapshot { Heap::visit_value(v, marks, worklist); }
                Heap::visit_value(&b.self_val.clone(), marks, worklist);
            }
            _ => {}
        }
    }
}

impl Value {
    pub(crate) fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Integer",
            Value::Str(_) => "String",
            Value::Sym(_) => "Symbol",
            Value::Bool(_) => "Boolean",
            Value::Nil => "NilClass",
            Value::Class(_) => "Class",
            Value::Object(_) => "Object",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
            Value::Block(_) => "Proc",
        }
    }
    pub(crate) fn to_display(&self, heap: &Heap) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Str(s) => (**s).clone(),
            Value::Sym(s) => (**s).clone(),
            Value::Bool(true) => "true".into(),
            Value::Bool(false) => "false".into(),
            Value::Nil => "".into(),
            Value::Class(c) => c.name.clone(),
            Value::Object(id) => format!("#<{}>", heap.instance(*id).class.name),
            Value::Array(id) => {
                let a = heap.array(*id);
                let parts: Vec<String> = a.iter().map(|v| v.to_inspect(heap)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Hash(id) => {
                let h = heap.hash(*id);
                let parts: Vec<String> = h.iter()
                    .map(|(k, v)| format!("{}=>{}", k.to_inspect(heap), v.to_inspect(heap)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::Block(_) => "#<Proc>".into(),
        }
    }
    pub(crate) fn to_inspect(&self, heap: &Heap) -> String {
        match self {
            Value::Str(s) => format!("\"{}\"", s),
            Value::Sym(s) => format!(":{}", s),
            Value::Nil => "nil".into(),
            _ => self.to_display(heap),
        }
    }
    pub(crate) fn ruby_eq(&self, other: &Value, heap: &Heap) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => **a == **b,
            (Value::Sym(a), Value::Sym(b)) => Rc::ptr_eq(a, b) || **a == **b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil, Value::Nil) => true,
            (Value::Object(a), Value::Object(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                if a == b { return true; }
                let x = heap.array(*a); let y = heap.array(*b);
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.ruby_eq(q, heap))
            }
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}
