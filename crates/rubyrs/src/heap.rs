use std::rc::Rc;

use crate::intern::Interner;
use crate::value::{BlockHandle, Instance, ObjId, Value};

// ---------- GC Heap ----------

pub(crate) enum HeapObj {
    Instance(Instance),
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>),
    Range(RangeObj),
    /// A `proc { ... }` value. Lives in the heap (P2-13) so blocks
    /// participate in mark-sweep — earlier `Rc<BlockHandle>` form
    /// cycled whenever a block's `captured` held the block itself.
    Block(BlockHandle),
}

/// A Ruby Range. For our subset, both endpoints must be `Value::Int`.
#[derive(Clone)]
pub(crate) struct RangeObj {
    pub(crate) begin: Value,
    pub(crate) end: Value,
    pub(crate) exclusive: bool,
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
    /// When `Some(n)`, the runtime refuses to allocate past `n` live
    /// objects; the caller traps with `ResourceExhausted`. Hosts running
    /// untrusted scripts should set this; default (None) is unlimited.
    pub(crate) max_live: Option<usize>,
}

impl Heap {
    pub(crate) fn new() -> Self {
        Heap { slots: vec![], marks: vec![], free: vec![], live_count: 0, next_gc: 1024, max_live: None }
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
    pub(crate) fn range(&self, id: ObjId) -> &RangeObj {
        if let HeapObj::Range(r) = self.get(id) { r } else { panic!("ICE: heap slot is not a Range") }
    }
    pub(crate) fn block(&self, id: ObjId) -> &BlockHandle {
        if let HeapObj::Block(b) = self.get(id) { b } else { panic!("ICE: heap slot is not a Block") }
    }
    pub(crate) fn should_gc(&self) -> bool { self.live_count >= self.next_gc }

    pub(crate) fn collect(&mut self, roots: &[Value]) {
        for m in self.marks.iter_mut() { *m = false; }
        let mut worklist: Vec<ObjId> = Vec::new();
        for v in roots { Heap::visit_value(v, &mut self.marks, &mut worklist); }
        // Mark phase: iterate each greyed object's children in place.
        // The previous impl `let children: Vec<Value> = ...clone()` per
        // pop step turned every mark visit into a full copy of the
        // container's contents — quadratic on a heap that's mostly one
        // large Array. Split-borrow `self.slots` (read) vs `self.marks`
        // (write) on disjoint fields lets us walk references directly.
        while let Some(id) = worklist.pop() {
            match &self.slots[id.0 as usize] {
                Slot::Live(HeapObj::Instance(inst)) => {
                    for v in inst.ivars.values() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                Slot::Live(HeapObj::Array(a)) => {
                    for v in a {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                Slot::Live(HeapObj::Hash(h)) => {
                    for (k, v) in h {
                        Heap::visit_value(k, &mut self.marks, &mut worklist);
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                }
                Slot::Live(HeapObj::Range(r)) => {
                    Heap::visit_value(&r.begin, &mut self.marks, &mut worklist);
                    Heap::visit_value(&r.end, &mut self.marks, &mut worklist);
                }
                Slot::Live(HeapObj::Block(bh)) => {
                    // Walk captured locals (shared Rc<RefCell> with
                    // any frame currently executing this block, but
                    // immutably borrowed only here) and the block's
                    // `self_val`. The visit_value calls do not
                    // recurse — they mark + worklist-push only —
                    // so the RefCell borrow stays scoped to this
                    // arm and can't conflict with itself.
                    let captured = bh.captured.borrow();
                    for v in captured.iter() {
                        Heap::visit_value(v, &mut self.marks, &mut worklist);
                    }
                    drop(captured);
                    Heap::visit_value(&bh.self_val, &mut self.marks, &mut worklist);
                }
                _ => {}
            }
        }
        // Sweep phase: unchanged from before.
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
            Value::Object(id) | Value::Array(id) | Value::Hash(id) | Value::Range(id) | Value::Block(id) => {
                let i = id.0 as usize;
                if !marks[i] {
                    marks[i] = true;
                    worklist.push(*id);
                }
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
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::Sym(_) => "Symbol",
            Value::Bool(_) => "Boolean",
            Value::Nil => "NilClass",
            Value::Class(_) => "Class",
            Value::Object(_) => "Object",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
            Value::Range(_) => "Range",
            Value::Block(_) => "Proc", // block lives in heap now (P2-13); type name unchanged
        }
    }
    pub(crate) fn to_display(&self, heap: &Heap, interner: &Interner) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Str(s) => s.to_string(),
            Value::Sym(id) => interner.resolve(*id).to_string(),
            Value::Bool(true) => "true".into(),
            Value::Bool(false) => "false".into(),
            Value::Nil => "".into(),
            Value::Class(c) => c.name.clone(),
            Value::Object(id) => format!("#<{}>", heap.instance(*id).class.name),
            Value::Array(id) => {
                let a = heap.array(*id);
                let parts: Vec<String> = a.iter().map(|v| v.to_inspect(heap, interner)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Hash(id) => {
                let h = heap.hash(*id);
                let parts: Vec<String> = h.iter()
                    .map(|(k, v)| {
                        // CRuby 3.4+: Symbol keys render as `name: value`
                        // shorthand instead of `:name => value`. Every
                        // other key type uses the explicit hash-rocket
                        // form with spaces around `=>`.
                        if let Value::Sym(sid) = k {
                            format!("{}: {}", interner.resolve(*sid), v.to_inspect(heap, interner))
                        } else {
                            format!("{} => {}", k.to_inspect(heap, interner), v.to_inspect(heap, interner))
                        }
                    })
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::Range(id) => {
                let r = heap.range(*id);
                let sep = if r.exclusive { "..." } else { ".." };
                format!("{}{}{}", r.begin.to_display(heap, interner), sep, r.end.to_display(heap, interner))
            }
            Value::Block(_) => "#<Proc>".into(),
        }
    }
    pub(crate) fn to_inspect(&self, heap: &Heap, interner: &Interner) -> String {
        match self {
            Value::Str(s) => format!("\"{}\"", s),
            Value::Sym(id) => format!(":{}", interner.resolve(*id)),
            Value::Nil => "nil".into(),
            _ => self.to_display(heap, interner),
        }
    }
    pub(crate) fn ruby_eq(&self, other: &Value, heap: &Heap) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            // Numeric coercion: CRuby treats `5 == 5.0` as `true`.
            // NaN never equals anything, including itself —
            // f64::==-on-NaN already gives `false`, so the comparison
            // via `as f64` does the right thing.
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => **a == **b,
            (Value::Sym(a), Value::Sym(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil, Value::Nil) => true,
            (Value::Object(a), Value::Object(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                if a == b { return true; }
                let x = heap.array(*a); let y = heap.array(*b);
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.ruby_eq(q, heap))
            }
            (Value::Hash(a), Value::Hash(b)) => {
                if a == b { return true; }
                let x = heap.hash(*a); let y = heap.hash(*b);
                if x.len() != y.len() { return false; }
                // Order-insensitive: for each (k, v) in `x`, find a
                // matching key in `y` with equal value. O(n*m) but
                // the lookup is unavoidable until we hash keys
                // properly (P3-class follow-up).
                x.iter().all(|(k, v)| {
                    y.iter().any(|(k2, v2)| k.ruby_eq(k2, heap) && v.ruby_eq(v2, heap))
                })
            }
            (Value::Range(a), Value::Range(b)) => {
                if a == b { return true; }
                let x = heap.range(*a); let y = heap.range(*b);
                x.exclusive == y.exclusive
                    && x.begin.ruby_eq(&y.begin, heap)
                    && x.end.ruby_eq(&y.end, heap)
            }
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Format a `Value::Float` for `to_display` / `to_inspect`.
/// Rust's `{:?}` already preserves `.0` on whole numbers
/// (`5.0` → `"5.0"`) so common cases match CRuby for free.
/// Scientific notation for very large / small magnitudes is a
/// known divergence — Rust prints `1e16`, CRuby prints `1.0e+16`.
/// Restrict diff fixtures to the everyday range until P3-class
/// formatter work lands.
pub(crate) fn format_float(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    format!("{:?}", f)
}
