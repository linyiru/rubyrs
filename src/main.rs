use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::process;
use std::rc::Rc;

use ruby_prism::Node;

// ---------- IR ----------

#[derive(Debug, Clone)]
enum Expr {
    IntLit(i64),
    StrLit(String),
    BoolLit(bool),
    Nil,
    LVarRead(String),
    LVarWrite(String, Box<Expr>),
    IVarRead(String),
    IVarWrite(String, Box<Expr>),
    SelfExpr,
    ConstRead(String),
    Call {
        receiver: Option<Box<Expr>>,
        name: String,
        args: Vec<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_body: Vec<Expr>,
        else_body: Vec<Expr>,
    },
    While {
        cond: Box<Expr>,
        body: Vec<Expr>,
    },
    Def {
        name: String,
        params: Vec<String>,
        body: Vec<Expr>,
    },
    Class {
        name: String,
        body: Vec<Expr>,
    },
    ArrayLit(Vec<Expr>),
    HashLit(Vec<(Expr, Expr)>),
}

// ---------- Translate prism AST to Expr ----------

fn cid_to_string(id: ruby_prism::ConstantId<'_>) -> String {
    String::from_utf8_lossy(id.as_slice()).into_owned()
}

fn tr(node: &Node<'_>) -> Expr {
    if let Some(n) = node.as_program_node() {
        let stmts: Vec<Expr> = n.statements().body().iter().map(|c| tr(&c)).collect();
        return if stmts.len() == 1 {
            stmts.into_iter().next().unwrap()
        } else {
            seq(stmts)
        };
    }
    if let Some(n) = node.as_statements_node() {
        let stmts: Vec<Expr> = n.body().iter().map(|c| tr(&c)).collect();
        return seq(stmts);
    }
    if let Some(n) = node.as_integer_node() {
        let v: i32 = n.value().try_into().unwrap_or(0);
        return Expr::IntLit(v as i64);
    }
    if let Some(n) = node.as_string_node() {
        return Expr::StrLit(String::from_utf8_lossy(n.unescaped()).into_owned());
    }
    if node.as_true_node().is_some() { return Expr::BoolLit(true); }
    if node.as_false_node().is_some() { return Expr::BoolLit(false); }
    if node.as_nil_node().is_some() { return Expr::Nil; }
    if node.as_self_node().is_some() { return Expr::SelfExpr; }
    if let Some(n) = node.as_constant_read_node() {
        return Expr::ConstRead(cid_to_string(n.name()));
    }
    if let Some(n) = node.as_local_variable_read_node() {
        return Expr::LVarRead(cid_to_string(n.name()));
    }
    if let Some(n) = node.as_local_variable_write_node() {
        return Expr::LVarWrite(cid_to_string(n.name()), Box::new(tr(&n.value())));
    }
    if let Some(n) = node.as_instance_variable_read_node() {
        return Expr::IVarRead(cid_to_string(n.name()));
    }
    if let Some(n) = node.as_instance_variable_write_node() {
        return Expr::IVarWrite(cid_to_string(n.name()), Box::new(tr(&n.value())));
    }
    if let Some(n) = node.as_call_node() {
        let receiver = n.receiver().map(|r| Box::new(tr(&r)));
        let name = cid_to_string(n.name());
        let args: Vec<Expr> = n
            .arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return Expr::Call { receiver, name, args };
    }
    if let Some(n) = node.as_if_node() {
        let cond = Box::new(tr(&n.predicate()));
        let then_body: Vec<Expr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let else_body: Vec<Expr> = match n.subsequent() {
            Some(sub) => {
                if let Some(en) = sub.as_else_node() {
                    en.statements().map(|s| s.body().iter().map(|c| tr(&c)).collect()).unwrap_or_default()
                } else {
                    vec![tr(&sub)]
                }
            }
            None => vec![],
        };
        return Expr::If { cond, then_body, else_body };
    }
    if let Some(n) = node.as_while_node() {
        let cond = Box::new(tr(&n.predicate()));
        let body: Vec<Expr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return Expr::While { cond, body };
    }
    if let Some(n) = node.as_def_node() {
        let name = cid_to_string(n.name());
        let params: Vec<String> = n.parameters().map(|p| {
            p.requireds().iter()
                .filter_map(|r| r.as_required_parameter_node().map(|rp| cid_to_string(rp.name())))
                .collect()
        }).unwrap_or_default();
        let body: Vec<Expr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return Expr::Def { name, params, body };
    }
    if let Some(n) = node.as_array_node() {
        let elems: Vec<Expr> = n.elements().iter().map(|e| tr(&e)).collect();
        return Expr::ArrayLit(elems);
    }
    if let Some(n) = node.as_hash_node() {
        let pairs: Vec<(Expr, Expr)> = n.elements().iter().filter_map(|e| {
            e.as_assoc_node().map(|a| (tr(&a.key()), tr(&a.value())))
        }).collect();
        return Expr::HashLit(pairs);
    }
    if let Some(n) = node.as_class_node() {
        let name = if let Some(cr) = n.constant_path().as_constant_read_node() {
            cid_to_string(cr.name())
        } else { "?".to_string() };
        let body: Vec<Expr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return Expr::Class { name, body };
    }
    panic!("unsupported node: {:?}", node);
}

fn seq(stmts: Vec<Expr>) -> Expr {
    Expr::Call { receiver: None, name: "__seq__".to_string(), args: stmts }
}

// ---------- Values ----------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ObjId(u32);

#[derive(Clone)]
enum Value {
    Int(i64),
    Str(Rc<String>),
    Bool(bool),
    Nil,
    Class(Rc<Class>),
    Object(ObjId),
    Array(ObjId),
    Hash(ObjId),
}

struct Class {
    name: String,
    methods: RefCell<HashMap<String, Rc<Method>>>,
}

struct Instance {
    class: Rc<Class>,
    ivars: HashMap<String, Value>,
}

struct Method {
    params: Vec<String>,
    proto_idx: usize,
}

// ---------- GC Heap ----------

enum HeapObj {
    Instance(Instance),
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>), // insertion-ordered, linear lookup (PoC)
}

enum Slot {
    Live(HeapObj),
    Dead,
}

struct Heap {
    slots: Vec<Slot>,
    marks: Vec<bool>,
    free: Vec<u32>,
    live_count: usize,
    next_gc: usize,
}

impl Heap {
    fn new() -> Self {
        Heap { slots: vec![], marks: vec![], free: vec![], live_count: 0, next_gc: 1024 }
    }
    fn alloc(&mut self, obj: HeapObj) -> ObjId {
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
    fn get(&self, id: ObjId) -> &HeapObj {
        match &self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("use-after-free ObjId({})", id.0),
        }
    }
    fn get_mut(&mut self, id: ObjId) -> &mut HeapObj {
        match &mut self.slots[id.0 as usize] {
            Slot::Live(o) => o,
            Slot::Dead => panic!("use-after-free ObjId({})", id.0),
        }
    }
    fn instance(&self, id: ObjId) -> &Instance {
        if let HeapObj::Instance(i) = self.get(id) { i } else { panic!("not instance") }
    }
    fn instance_mut(&mut self, id: ObjId) -> &mut Instance {
        if let HeapObj::Instance(i) = self.get_mut(id) { i } else { panic!("not instance") }
    }
    fn array(&self, id: ObjId) -> &Vec<Value> {
        if let HeapObj::Array(a) = self.get(id) { a } else { panic!("not array") }
    }
    fn array_mut(&mut self, id: ObjId) -> &mut Vec<Value> {
        if let HeapObj::Array(a) = self.get_mut(id) { a } else { panic!("not array") }
    }
    fn hash(&self, id: ObjId) -> &Vec<(Value, Value)> {
        if let HeapObj::Hash(h) = self.get(id) { h } else { panic!("not hash") }
    }
    fn hash_mut(&mut self, id: ObjId) -> &mut Vec<(Value, Value)> {
        if let HeapObj::Hash(h) = self.get_mut(id) { h } else { panic!("not hash") }
    }
    fn should_gc(&self) -> bool { self.live_count >= self.next_gc }

    fn collect(&mut self, roots: &[Value]) {
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

    fn visit_value(v: &Value, marks: &mut [bool], worklist: &mut Vec<ObjId>) {
        let id = match v {
            Value::Object(id) | Value::Array(id) | Value::Hash(id) => *id,
            _ => return,
        };
        let i = id.0 as usize;
        if !marks[i] {
            marks[i] = true;
            worklist.push(id);
        }
    }
}

impl Value {
    fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }
    fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Integer",
            Value::Str(_) => "String",
            Value::Bool(_) => "Boolean",
            Value::Nil => "NilClass",
            Value::Class(_) => "Class",
            Value::Object(_) => "Object",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
        }
    }
    fn to_display(&self, heap: &Heap) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Str(s) => (**s).clone(),
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
        }
    }
    fn to_inspect(&self, heap: &Heap) -> String {
        match self {
            Value::Str(s) => format!("\"{}\"", s),
            Value::Nil => "nil".into(),
            _ => self.to_display(heap),
        }
    }
    fn ruby_eq(&self, other: &Value, heap: &Heap) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => **a == **b,
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

// ---------- Bytecode ----------

#[derive(Debug, Clone)]
enum Op {
    LoadConstInt(i64),
    LoadConstStr(u32),   // proto.strings idx
    LoadNil,
    LoadTrue,
    LoadFalse,
    LoadSelf,
    LoadLocal(u16),
    StoreLocal(u16),     // pops
    Dup,
    Pop,
    LoadIvar(u32),       // proto.strings idx
    StoreIvar(u32),      // pops
    LoadConst(u32),      // class name idx
    Jump(i32),
    JumpIfFalse(i32),    // pops cond
    Call(u32, u8),       // name idx, argc; receiver on stack BELOW args
    CallNoRecv(u32, u8), // implicit self / builtin / toplevel
    DefMethod(u32, u32), // name idx, proto idx
    DefClass(u32, u32),  // name idx, body proto idx
    NewArray(u16),       // pop N values, build array
    NewHash(u16),        // pop 2N values (key, val, key, val, ...), build hash
    Return,
}

#[derive(Debug)]
struct Proto {
    name: String,
    params: Vec<String>,
    n_locals: u16,
    code: Vec<Op>,
    strings: Vec<String>,
}

// ---------- Compiler ----------

struct ProtoBuilder {
    code: Vec<Op>,
    strings: Vec<String>,
    locals: HashMap<String, u16>,
    n_locals: u16,
}

impl ProtoBuilder {
    fn new(params: &[String]) -> Self {
        let mut b = Self {
            code: vec![],
            strings: vec![],
            locals: HashMap::new(),
            n_locals: 0,
        };
        for p in params { b.local_slot(p); }
        b
    }
    fn local_slot(&mut self, name: &str) -> u16 {
        if let Some(&s) = self.locals.get(name) { return s; }
        let s = self.n_locals;
        self.locals.insert(name.to_string(), s);
        self.n_locals += 1;
        s
    }
    fn intern(&mut self, s: &str) -> u32 {
        for (i, x) in self.strings.iter().enumerate() {
            if x == s { return i as u32; }
        }
        self.strings.push(s.to_string());
        (self.strings.len() - 1) as u32
    }
    fn emit(&mut self, op: Op) -> usize {
        let i = self.code.len();
        self.code.push(op);
        i
    }
    fn pos(&self) -> usize { self.code.len() }
    fn patch_jump(&mut self, at: usize, target: usize) {
        let off = target as i32 - at as i32 - 1;
        match &mut self.code[at] {
            Op::Jump(o) => *o = off,
            Op::JumpIfFalse(o) => *o = off,
            _ => panic!("not a jump at {}", at),
        }
    }
    fn build(self, name: String, params: Vec<String>) -> Proto {
        Proto {
            name, params,
            n_locals: self.n_locals,
            code: self.code,
            strings: self.strings,
        }
    }
}

fn compile_body(b: &mut ProtoBuilder, exprs: &[Expr], protos: &mut Vec<Proto>) {
    if exprs.is_empty() {
        b.emit(Op::LoadNil);
        return;
    }
    for (i, e) in exprs.iter().enumerate() {
        compile_expr(b, e, protos);
        if i < exprs.len() - 1 {
            b.emit(Op::Pop);
        }
    }
}

fn compile_expr(b: &mut ProtoBuilder, e: &Expr, protos: &mut Vec<Proto>) {
    match e {
        Expr::IntLit(i) => { b.emit(Op::LoadConstInt(*i)); }
        Expr::StrLit(s) => { let i = b.intern(s); b.emit(Op::LoadConstStr(i)); }
        Expr::BoolLit(true) => { b.emit(Op::LoadTrue); }
        Expr::BoolLit(false) => { b.emit(Op::LoadFalse); }
        Expr::Nil => { b.emit(Op::LoadNil); }
        Expr::SelfExpr => { b.emit(Op::LoadSelf); }
        Expr::LVarRead(name) => {
            let slot = b.local_slot(name);
            b.emit(Op::LoadLocal(slot));
        }
        Expr::LVarWrite(name, val) => {
            compile_expr(b, val, protos);
            let slot = b.local_slot(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreLocal(slot));
        }
        Expr::IVarRead(name) => {
            let i = b.intern(name);
            b.emit(Op::LoadIvar(i));
        }
        Expr::IVarWrite(name, val) => {
            compile_expr(b, val, protos);
            let i = b.intern(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreIvar(i));
        }
        Expr::ConstRead(name) => {
            let i = b.intern(name);
            b.emit(Op::LoadConst(i));
        }
        Expr::If { cond, then_body, else_body } => {
            compile_expr(b, cond, protos);
            let jf = b.emit(Op::JumpIfFalse(0));
            compile_body(b, then_body, protos);
            let je = b.emit(Op::Jump(0));
            let else_start = b.pos();
            b.patch_jump(jf, else_start);
            compile_body(b, else_body, protos);
            let end = b.pos();
            b.patch_jump(je, end);
        }
        Expr::While { cond, body } => {
            let start = b.pos();
            compile_expr(b, cond, protos);
            let jf = b.emit(Op::JumpIfFalse(0));
            compile_body(b, body, protos);
            b.emit(Op::Pop);
            let j = b.emit(Op::Jump(0));
            b.patch_jump(j, start);
            let end = b.pos();
            b.patch_jump(jf, end);
            b.emit(Op::LoadNil);
        }
        Expr::Call { receiver, name, args } => {
            // Special: __seq__ as compiler-level sequence
            if receiver.is_none() && name == "__seq__" {
                compile_body(b, args, protos);
                return;
            }
            let name_idx = b.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos); }
            for a in args { compile_expr(b, a, protos); }
            let argc = args.len() as u8;
            if has_recv {
                b.emit(Op::Call(name_idx, argc));
            } else {
                b.emit(Op::CallNoRecv(name_idx, argc));
            }
        }
        Expr::Def { name, params, body } => {
            let proto_idx = compile_proto(name.clone(), params.clone(), body, protos);
            let name_idx = b.intern(name);
            b.emit(Op::DefMethod(name_idx, proto_idx as u32));
            b.emit(Op::LoadNil);
        }
        Expr::Class { name, body } => {
            let proto_idx = compile_proto(format!("<class:{}>", name), vec![], body, protos);
            let name_idx = b.intern(name);
            b.emit(Op::DefClass(name_idx, proto_idx as u32));
        }
        Expr::ArrayLit(elems) => {
            for e in elems { compile_expr(b, e, protos); }
            b.emit(Op::NewArray(elems.len() as u16));
        }
        Expr::HashLit(pairs) => {
            for (k, v) in pairs {
                compile_expr(b, k, protos);
                compile_expr(b, v, protos);
            }
            b.emit(Op::NewHash(pairs.len() as u16));
        }
    }
}

fn compile_proto(name: String, params: Vec<String>, body: &[Expr], protos: &mut Vec<Proto>) -> usize {
    let mut b = ProtoBuilder::new(&params);
    compile_body(&mut b, body, protos);
    b.emit(Op::Return);
    let idx = protos.len();
    protos.push(b.build(name, params));
    idx
}

// ---------- VM ----------

struct Frame {
    proto_idx: usize,
    ip: usize,
    locals: Vec<Value>,
    self_val: Value,
    base_sp: usize,
    is_class_body: bool,
    swap_return: Option<Value>, // when Some, discard the frame's return and push this instead
}

struct Vm {
    protos: Vec<Proto>,
    classes: HashMap<String, Rc<Class>>,
    toplevel_methods: HashMap<String, Rc<Method>>,
    class_stack: Vec<Rc<Class>>,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    heap: Heap,
}

impl Vm {
    fn new(protos: Vec<Proto>) -> Self {
        Vm {
            protos,
            classes: HashMap::new(),
            toplevel_methods: HashMap::new(),
            class_stack: vec![],
            stack: Vec::with_capacity(1024),
            frames: vec![],
            heap: Heap::new(),
        }
    }

    fn run(&mut self, entry: usize) -> Value {
        let proto = &self.protos[entry];
        let n_locals = proto.n_locals as usize;
        self.frames.push(Frame {
            proto_idx: entry,
            ip: 0,
            locals: vec_nil(n_locals),
            self_val: Value::Nil,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None,
        });
        self.dispatch();
        self.stack.pop().unwrap_or(Value::Nil)
    }

    fn dispatch(&mut self) {
        'outer: loop {
            // SAFETY: frame index valid while running
            let (proto_idx, ip) = {
                let f = self.frames.last().unwrap();
                (f.proto_idx, f.ip)
            };
            let op = self.protos[proto_idx].code[ip].clone();
            self.frames.last_mut().unwrap().ip += 1;

            match op {
                Op::LoadConstInt(i) => self.stack.push(Value::Int(i)),
                Op::LoadConstStr(idx) => {
                    let s = self.protos[proto_idx].strings[idx as usize].clone();
                    self.stack.push(Value::Str(Rc::new(s)));
                }
                Op::LoadNil => self.stack.push(Value::Nil),
                Op::LoadTrue => self.stack.push(Value::Bool(true)),
                Op::LoadFalse => self.stack.push(Value::Bool(false)),
                Op::LoadSelf => {
                    let v = self.frames.last().unwrap().self_val.clone();
                    self.stack.push(v);
                }
                Op::LoadLocal(s) => {
                    let v = self.frames.last().unwrap().locals[s as usize].clone();
                    self.stack.push(v);
                }
                Op::StoreLocal(s) => {
                    let v = self.stack.pop().unwrap();
                    self.frames.last_mut().unwrap().locals[s as usize] = v;
                }
                Op::Dup => {
                    let v = self.stack.last().unwrap().clone();
                    self.stack.push(v);
                }
                Op::Pop => { self.stack.pop(); }
                Op::LoadIvar(idx) => {
                    let name = self.protos[proto_idx].strings[idx as usize].clone();
                    let id_opt = if let Value::Object(id) = &self.frames.last().unwrap().self_val {
                        Some(*id)
                    } else { None };
                    let v = if let Some(id) = id_opt {
                        self.heap.instance(id).ivars.get(&name).cloned().unwrap_or(Value::Nil)
                    } else { Value::Nil };
                    self.stack.push(v);
                }
                Op::StoreIvar(idx) => {
                    let name = self.protos[proto_idx].strings[idx as usize].clone();
                    let v = self.stack.pop().unwrap();
                    let id_opt = if let Value::Object(id) = &self.frames.last().unwrap().self_val {
                        Some(*id)
                    } else { None };
                    if let Some(id) = id_opt {
                        self.heap.instance_mut(id).ivars.insert(name, v);
                    }
                }
                Op::LoadConst(idx) => {
                    let name = &self.protos[proto_idx].strings[idx as usize];
                    let v = self.classes.get(name).map(|c| Value::Class(c.clone())).unwrap_or(Value::Nil);
                    self.stack.push(v);
                }
                Op::Jump(off) => {
                    let f = self.frames.last_mut().unwrap();
                    f.ip = (f.ip as i32 + off) as usize;
                }
                Op::JumpIfFalse(off) => {
                    let v = self.stack.pop().unwrap();
                    if !v.is_truthy() {
                        let f = self.frames.last_mut().unwrap();
                        f.ip = (f.ip as i32 + off) as usize;
                    }
                }
                Op::Call(name_idx, argc) => {
                    let name = self.protos[proto_idx].strings[name_idx as usize].clone();
                    self.do_call(name, argc as usize, false);
                }
                Op::CallNoRecv(name_idx, argc) => {
                    let name = self.protos[proto_idx].strings[name_idx as usize].clone();
                    self.do_call(name, argc as usize, true);
                }
                Op::DefMethod(name_idx, p_idx) => {
                    let name = self.protos[proto_idx].strings[name_idx as usize].clone();
                    let proto = &self.protos[p_idx as usize];
                    let m = Rc::new(Method { params: proto.params.clone(), proto_idx: p_idx as usize });
                    if let Some(cls) = self.class_stack.last() {
                        cls.methods.borrow_mut().insert(name, m);
                    } else {
                        self.toplevel_methods.insert(name, m);
                    }
                    self.stack.push(Value::Nil);
                }
                Op::DefClass(name_idx, p_idx) => {
                    let name = self.protos[proto_idx].strings[name_idx as usize].clone();
                    let cls = self.classes.entry(name.clone()).or_insert_with(|| Rc::new(Class {
                        name: name.clone(),
                        methods: RefCell::new(HashMap::new()),
                    })).clone();
                    self.class_stack.push(cls.clone());
                    // Invoke class body proto with self = Class
                    let proto = &self.protos[p_idx as usize];
                    let n_locals = proto.n_locals as usize;
                    self.frames.push(Frame {
                        proto_idx: p_idx as usize,
                        ip: 0,
                        locals: vec_nil(n_locals),
                        self_val: Value::Class(cls.clone()),
                        base_sp: self.stack.len(),
                        is_class_body: true, swap_return: None,
                    });
                }
                Op::NewArray(n) => {
                    self.maybe_gc();
                    let n = n as usize;
                    let split = self.stack.len() - n;
                    let elems: Vec<Value> = self.stack.drain(split..).collect();
                    let id = self.heap.alloc(HeapObj::Array(elems));
                    self.stack.push(Value::Array(id));
                }
                Op::NewHash(n) => {
                    self.maybe_gc();
                    let n = n as usize;
                    let split = self.stack.len() - n * 2;
                    let flat: Vec<Value> = self.stack.drain(split..).collect();
                    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(n);
                    let mut iter = flat.into_iter();
                    while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                        pairs.push((k, v));
                    }
                    let id = self.heap.alloc(HeapObj::Hash(pairs));
                    self.stack.push(Value::Hash(id));
                }
                Op::Return => {
                    let f = self.frames.pop().unwrap();
                    let ret = self.stack.pop().unwrap_or(Value::Nil);
                    self.stack.truncate(f.base_sp);
                    if f.is_class_body {
                        let cls = self.class_stack.pop().unwrap();
                        self.stack.push(Value::Class(cls));
                    } else if let Some(replacement) = f.swap_return {
                        self.stack.push(replacement);
                    } else {
                        self.stack.push(ret);
                    }
                    if self.frames.is_empty() { break 'outer; }
                }
            }
        }
    }

    fn do_call(&mut self, name: String, argc: usize, no_recv: bool) {
        // Collect args
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let recv = if no_recv { None } else { Some(self.stack.pop().unwrap()) };

        if no_recv {
            // builtin
            if let Some(v) = self.builtin_call(&name, &args) {
                self.stack.push(v);
                return;
            }
            // implicit self (object method)
            let self_val = self.frames.last().unwrap().self_val.clone();
            if let Value::Object(id) = &self_val {
                let cls = self.heap.instance(*id).class.clone();
                if let Some(m) = cls.methods.borrow().get(&name).cloned() {
                    self.invoke_method(m, self_val.clone(), args);
                    return;
                }
            }
            if let Some(m) = self.toplevel_methods.get(&name).cloned() {
                self.invoke_method(m, self_val, args);
                return;
            }
            panic!("undefined method `{}'", name);
        }

        let recv = recv.unwrap();

        if let Some(v) = primitive_call(&recv, &name, &args) {
            self.stack.push(v);
            return;
        }

        if name == "new" {
            if let Value::Class(cls) = &recv {
                self.maybe_gc();
                let id = self.heap.alloc(HeapObj::Instance(Instance {
                    class: cls.clone(),
                    ivars: HashMap::new(),
                }));
                let obj = Value::Object(id);
                if let Some(m) = cls.methods.borrow().get("initialize").cloned() {
                    self.invoke_method(m, obj.clone(), args);
                    self.frames.last_mut().unwrap().swap_return = Some(obj);
                } else {
                    self.stack.push(obj);
                }
                return;
            }
        }

        if let Value::Object(id) = &recv {
            let cls = self.heap.instance(*id).class.clone();
            if let Some(m) = cls.methods.borrow().get(&name).cloned() {
                self.invoke_method(m, recv.clone(), args);
                return;
            }
        }
        if let Some(v) = self.collection_call(&recv, &name, &args) {
            self.stack.push(v);
            return;
        }
        panic!("undefined method `{}' for {}", name, recv.type_name());
    }

    fn collection_call(&mut self, recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
        match recv {
            Value::Array(id) => {
                let id = *id;
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.array(id).len() as i64)),
                    ("push", [v]) | ("<<", [v]) => {
                        self.heap.array_mut(id).push(v.clone());
                        Some(Value::Array(id))
                    }
                    ("[]", [Value::Int(i)]) => {
                        let a = self.heap.array(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                        Some(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                    }
                    ("[]=", [Value::Int(i), v]) => {
                        let a = self.heap.array_mut(id);
                        let idx = if *i < 0 { a.len() as i64 + *i } else { *i } as usize;
                        while a.len() <= idx { a.push(Value::Nil); }
                        a[idx] = v.clone();
                        Some(v.clone())
                    }
                    ("first", []) => Some(self.heap.array(id).first().cloned().unwrap_or(Value::Nil)),
                    ("last", []) => Some(self.heap.array(id).last().cloned().unwrap_or(Value::Nil)),
                    ("empty?", []) => Some(Value::Bool(self.heap.array(id).is_empty())),
                    _ => None,
                }
            }
            Value::Hash(id) => {
                let id = *id;
                match (name, args) {
                    ("length", []) | ("size", []) => Some(Value::Int(self.heap.hash(id).len() as i64)),
                    ("[]", [k]) => {
                        let h = self.heap.hash(id);
                        for (key, val) in h {
                            if key.ruby_eq(k, &self.heap) { return Some(val.clone()); }
                        }
                        Some(Value::Nil)
                    }
                    ("[]=", [k, v]) => {
                        // Need a way to compare without borrowing heap while mutating.
                        // Snapshot positions first.
                        let pos = self.heap.hash(id).iter()
                            .position(|(key, _)| key.ruby_eq(k, &self.heap));
                        let h = self.heap.hash_mut(id);
                        if let Some(p) = pos {
                            h[p].1 = v.clone();
                        } else {
                            h.push((k.clone(), v.clone()));
                        }
                        Some(v.clone())
                    }
                    ("empty?", []) => Some(Value::Bool(self.heap.hash(id).is_empty())),
                    ("keys", []) => {
                        let keys: Vec<Value> = self.heap.hash(id).iter().map(|(k, _)| k.clone()).collect();
                        let nid = self.heap.alloc(HeapObj::Array(keys));
                        Some(Value::Array(nid))
                    }
                    ("values", []) => {
                        let vals: Vec<Value> = self.heap.hash(id).iter().map(|(_, v)| v.clone()).collect();
                        let nid = self.heap.alloc(HeapObj::Array(vals));
                        Some(Value::Array(nid))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn maybe_gc(&mut self) {
        if !self.heap.should_gc() { return; }
        // Gather roots: stack + every frame's locals + self_val + swap_return + class_stack (no instances)
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + 64);
        for v in &self.stack { roots.push(v.clone()); }
        for f in &self.frames {
            roots.push(f.self_val.clone());
            for v in &f.locals { roots.push(v.clone()); }
            if let Some(v) = &f.swap_return { roots.push(v.clone()); }
        }
        self.heap.collect(&roots);
    }

    fn invoke_method(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>) {
        if m.params.len() != args.len() {
            panic!("wrong number of arguments (given {}, expected {})", args.len(), m.params.len());
        }
        let proto = &self.protos[m.proto_idx];
        let n_locals = proto.n_locals as usize;
        let mut locals = vec_nil(n_locals);
        for (i, a) in args.into_iter().enumerate() {
            locals[i] = a;
        }
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals,
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None,
        });
    }
}

fn vec_nil(n: usize) -> Vec<Value> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n { v.push(Value::Nil); }
    v
}

impl Vm {
    fn builtin_call(&self, name: &str, args: &[Value]) -> Option<Value> {
        match name {
            "puts" => {
                if args.is_empty() { println!(); }
                else { for a in args { println!("{}", a.to_display(&self.heap)); } }
                Some(Value::Nil)
            }
            "print" => {
                for a in args { print!("{}", a.to_display(&self.heap)); }
                Some(Value::Nil)
            }
            _ => None,
        }
    }
}

fn primitive_call(recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
    match (recv, name, args) {
        (Value::Int(a), op, [Value::Int(b)]) => match op {
            "+" => Some(Value::Int(a + b)),
            "-" => Some(Value::Int(a - b)),
            "*" => Some(Value::Int(a * b)),
            "/" => Some(Value::Int(a / b)),
            "%" => Some(Value::Int(a % b)),
            "==" => Some(Value::Bool(a == b)),
            "!=" => Some(Value::Bool(a != b)),
            "<"  => Some(Value::Bool(a < b)),
            "<=" => Some(Value::Bool(a <= b)),
            ">"  => Some(Value::Bool(a > b)),
            ">=" => Some(Value::Bool(a >= b)),
            _ => None,
        },
        (Value::Int(a), "to_s", []) => Some(Value::Str(Rc::new(a.to_string()))),
        (Value::Str(a), "+", [Value::Str(b)]) => {
            let mut s = (**a).clone();
            s.push_str(b);
            Some(Value::Str(Rc::new(s)))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(**a == **b)),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        (Value::Str(a), "length", []) => Some(Value::Int(a.chars().count() as i64)),
        _ => None,
    }
}

// ---------- Entry ----------

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: rubyrs <file.rb>");
        process::exit(1);
    }
    let source = fs::read_to_string(&args[1]).expect("cannot read file");
    let result = ruby_prism::parse(source.as_bytes());
    let errs: Vec<_> = result.errors().collect();
    if !errs.is_empty() {
        for e in errs { eprintln!("parse error: {:?}", e); }
        process::exit(2);
    }
    let prog = tr(&result.node());

    if env::var("DEBUG_AST").is_ok() {
        eprintln!("{:#?}", prog);
    }

    let mut protos: Vec<Proto> = vec![];
    let entry = compile_proto("<main>".into(), vec![], &[prog], &mut protos);
    if env::var("DEBUG_BC").is_ok() {
        for (i, p) in protos.iter().enumerate() {
            eprintln!("proto {} {}", i, p.name);
            for (j, op) in p.code.iter().enumerate() {
                eprintln!("  {:04} {:?}", j, op);
            }
        }
    }
    let mut vm = Vm::new(protos);
    vm.run(entry);
    if env::var("GC_STATS").is_ok() {
        eprintln!("gc: live={} slots={} freed_slots={}", vm.heap.live_count, vm.heap.slots.len(), vm.heap.free.len());
    }
}
