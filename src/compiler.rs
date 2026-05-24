use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{Expr, SExpr};
use crate::bytecode::{BinOpKind, Op, Proto};
use crate::error::Span;
use crate::intern::Interner;

// ---------- Compiler ----------

pub(crate) struct ProtoBuilder {
    pub(crate) code: Vec<Op>,
    pub(crate) op_spans: Vec<Span>,
    pub(crate) locals: HashMap<String, u16>,
    pub(crate) n_locals: u16,
    pub(crate) current_span: Span,
    pub(crate) filename: Rc<str>,
}

impl ProtoBuilder {
    pub(crate) fn new(params: &[String], filename: Rc<str>) -> Self {
        let mut b = Self {
            code: vec![],
            op_spans: vec![],
            locals: HashMap::new(),
            n_locals: 0,
            current_span: Span::ZERO,
            filename,
        };
        for p in params { b.local_slot(p); }
        b
    }
    pub(crate) fn local_slot(&mut self, name: &str) -> u16 {
        if let Some(&s) = self.locals.get(name) { return s; }
        let s = self.n_locals;
        self.locals.insert(name.to_string(), s);
        self.n_locals += 1;
        s
    }
    pub(crate) fn emit(&mut self, op: Op) -> usize {
        let i = self.code.len();
        self.code.push(op);
        self.op_spans.push(self.current_span);
        i
    }
    pub(crate) fn pos(&self) -> usize { self.code.len() }
    pub(crate) fn patch_jump(&mut self, at: usize, target: usize) {
        let off = target as i32 - at as i32 - 1;
        match &mut self.code[at] {
            Op::Jump(o) => *o = off,
            Op::JumpIfFalse(o) => *o = off,
            _ => panic!("ICE: patch_jump on non-jump op at {}", at),
        }
    }
    pub(crate) fn build(self, name: String, params: Vec<String>) -> Proto {
        Proto {
            name, params,
            n_locals: self.n_locals,
            code: self.code,
            op_spans: self.op_spans,
            filename: self.filename,
        }
    }
}

pub(crate) fn compile_body(
    b: &mut ProtoBuilder, exprs: &[SExpr],
    protos: &mut Vec<Proto>, interner: &mut Interner,
) {
    if exprs.is_empty() {
        b.emit(Op::LoadNil);
        return;
    }
    for (i, e) in exprs.iter().enumerate() {
        compile_expr(b, e, protos, interner);
        if i < exprs.len() - 1 {
            b.emit(Op::Pop);
        }
    }
}

pub(crate) fn compile_expr(
    b: &mut ProtoBuilder, e: &SExpr,
    protos: &mut Vec<Proto>, interner: &mut Interner,
) {
    let prev_span = b.current_span;
    b.current_span = e.span;
    match &e.node {
        Expr::IntLit(i) => { b.emit(Op::LoadConstInt(*i)); }
        Expr::StrLit(s) => { let id = interner.intern(s); b.emit(Op::LoadConstStr(id)); }
        Expr::SymbolLit(s) => { let id = interner.intern(s); b.emit(Op::LoadSymbol(id)); }
        Expr::InterpolatedStr(parts) => {
            if parts.is_empty() {
                let id = interner.intern("");
                b.emit(Op::LoadConstStr(id));
            } else {
                let to_s = interner.intern("to_s");
                for (idx, p) in parts.iter().enumerate() {
                    match &p.node {
                        Expr::StrLit(_) => compile_expr(b, p, protos, interner),
                        _ => {
                            compile_expr(b, p, protos, interner);
                            b.emit(Op::Call(to_s, 0));
                        }
                    }
                    if idx > 0 {
                        b.emit(Op::BinOp(BinOpKind::Add));
                    }
                }
            }
        }
        Expr::BoolLit(true) => { b.emit(Op::LoadTrue); }
        Expr::BoolLit(false) => { b.emit(Op::LoadFalse); }
        Expr::Nil => { b.emit(Op::LoadNil); }
        Expr::SelfExpr => { b.emit(Op::LoadSelf); }
        Expr::LVarRead(name) => {
            let slot = b.local_slot(name);
            b.emit(Op::LoadLocal(slot));
        }
        Expr::LVarWrite(name, val) => {
            // Fast path: `name = name + 1` — extremely common in `while i < N`
            // counters and `each` accumulators. Compile to a single `IncLocal`
            // that does the read-modify-write in place.
            if let Expr::Call { receiver: Some(r), name: op, args } = &val.node {
                if op == "+" && args.len() == 1 {
                    if let (Expr::LVarRead(rn), Expr::IntLit(1)) = (&r.node, &args[0].node) {
                        if rn == name {
                            let slot = b.local_slot(name);
                            b.emit(Op::IncLocal(slot));
                            b.current_span = prev_span;
                            return;
                        }
                    }
                }
            }
            compile_expr(b, val, protos, interner);
            let slot = b.local_slot(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreLocal(slot));
        }
        Expr::IVarRead(name) => {
            let id = interner.intern(name);
            b.emit(Op::LoadIvar(id));
        }
        Expr::IVarWrite(name, val) => {
            // Fast path: @name = @name + 1
            if let Expr::Call { receiver: Some(r), name: op, args } = &val.node {
                if op == "+" && args.len() == 1 {
                    if let (Expr::IVarRead(rn), Expr::IntLit(1)) = (&r.node, &args[0].node) {
                        if rn == name {
                            let id = interner.intern(name);
                            b.emit(Op::IncIvar(id));
                            b.current_span = prev_span;
                            return;
                        }
                    }
                }
            }
            compile_expr(b, val, protos, interner);
            let id = interner.intern(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreIvar(id));
        }
        Expr::ConstRead(name) => {
            let id = interner.intern(name);
            b.emit(Op::LoadConst(id));
        }
        Expr::If { cond, then_body, else_body } => {
            compile_expr(b, cond, protos, interner);
            let jf = b.emit(Op::JumpIfFalse(0));
            compile_body(b, then_body, protos, interner);
            let je = b.emit(Op::Jump(0));
            let else_start = b.pos();
            b.patch_jump(jf, else_start);
            compile_body(b, else_body, protos, interner);
            let end = b.pos();
            b.patch_jump(je, end);
        }
        Expr::While { cond, body } => {
            let start = b.pos();
            compile_expr(b, cond, protos, interner);
            let jf = b.emit(Op::JumpIfFalse(0));
            compile_body(b, body, protos, interner);
            b.emit(Op::Pop);
            let j = b.emit(Op::Jump(0));
            b.patch_jump(j, start);
            let end = b.pos();
            b.patch_jump(jf, end);
            b.emit(Op::LoadNil);
        }
        Expr::Call { receiver, name, args } => {
            if receiver.is_none() && name == "__seq__" {
                compile_body(b, args, protos, interner);
                b.current_span = prev_span;
                return;
            }
            if receiver.is_none() && name == "raise" {
                if args.is_empty() {
                    b.emit(Op::LoadNil);
                } else {
                    compile_expr(b, &args[0], protos, interner);
                }
                b.emit(Op::Raise);
                b.emit(Op::LoadNil);
                b.current_span = prev_span;
                return;
            }
            if let (Some(r), 1, Some(kind)) = (receiver.as_ref(), args.len(), BinOpKind::from_op_name(name)) {
                compile_expr(b, r, protos, interner);
                // Fuse `<expr> <op> <int_literal>` into a single op so the
                // LoadConstInt + BinOp pair becomes one BinOpInt.
                if let Expr::IntLit(rhs) = &args[0].node {
                    b.emit(Op::BinOpInt(kind, *rhs));
                } else {
                    compile_expr(b, &args[0], protos, interner);
                    b.emit(Op::BinOp(kind));
                }
                b.current_span = prev_span;
                return;
            }
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner); }
            for a in args { compile_expr(b, a, protos, interner); }
            let argc = args.len() as u8;
            if has_recv {
                b.emit(Op::Call(name_id, argc));
            } else {
                b.emit(Op::CallNoRecv(name_id, argc));
            }
        }
        Expr::Def { name, params, body } => {
            let proto_idx = compile_proto(name.clone(), params.clone(), body, b.filename.clone(), protos, interner);
            let name_id = interner.intern(name);
            b.emit(Op::DefMethod(name_id, proto_idx as u32));
            b.emit(Op::LoadNil);
        }
        Expr::Class { name, body } => {
            let proto_idx = compile_proto(format!("<class:{}>", name), vec![], body, b.filename.clone(), protos, interner);
            let name_id = interner.intern(name);
            b.emit(Op::DefClass(name_id, proto_idx as u32));
        }
        Expr::ArrayLit(elems) => {
            for e in elems { compile_expr(b, e, protos, interner); }
            b.emit(Op::NewArray(elems.len() as u16));
        }
        Expr::HashLit(pairs) => {
            for (k, v) in pairs {
                compile_expr(b, k, protos, interner);
                compile_expr(b, v, protos, interner);
            }
            b.emit(Op::NewHash(pairs.len() as u16));
        }
        Expr::CallWithBlock { receiver, name, args, block_params, block_body } => {
            let (block_proto_idx, param_start, n_params) =
                compile_block(b, block_params, block_body, protos, interner);
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner); }
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params));
            for a in args { compile_expr(b, a, protos, interner); }
            let argc = args.len() as u8;
            if has_recv {
                b.emit(Op::CallBlock(name_id, argc));
            } else {
                b.emit(Op::CallNoRecvBlock(name_id, argc));
            }
        }
        Expr::Yield(args) => {
            for a in args { compile_expr(b, a, protos, interner); }
            b.emit(Op::Yield(args.len() as u8));
        }
        Expr::Begin { body, rescue } => {
            match rescue {
                None => compile_body(b, body, protos, interner),
                Some(rc) => {
                    let (slot, bind) = match &rc.var {
                        Some(name) => (b.local_slot(name), 1u8),
                        None => (0u16, 0u8),
                    };
                    let pr = b.emit(Op::PushRescue(0, slot, bind));
                    compile_body(b, body, protos, interner);
                    b.emit(Op::PopRescue);
                    let je = b.emit(Op::Jump(0));
                    let handler_start = b.pos();
                    let off = handler_start as i32 - pr as i32 - 1;
                    if let Op::PushRescue(o, _, _) = &mut b.code[pr] { *o = off; }
                    compile_body(b, &rc.body, protos, interner);
                    let end = b.pos();
                    b.patch_jump(je, end);
                }
            }
        }
    }
    b.current_span = prev_span;
}

pub(crate) fn compile_proto(
    name: String, params: Vec<String>, body: &[SExpr],
    filename: Rc<str>, protos: &mut Vec<Proto>, interner: &mut Interner,
) -> usize {
    let mut b = ProtoBuilder::new(&params, filename);
    compile_body(&mut b, body, protos, interner);
    b.emit(Op::Return);
    let idx = protos.len();
    protos.push(b.build(name, params));
    idx
}

pub(crate) fn compile_block(
    parent: &ProtoBuilder, block_params: &[String], body: &[SExpr],
    protos: &mut Vec<Proto>, interner: &mut Interner,
) -> (usize, u16, u16) {
    let mut b = ProtoBuilder {
        code: vec![],
        op_spans: vec![],
        locals: parent.locals.clone(),
        n_locals: parent.n_locals,
        current_span: parent.current_span,
        filename: parent.filename.clone(),
    };
    let param_start = b.n_locals;
    for p in block_params { b.local_slot(p); }
    let n_params = block_params.len() as u16;
    compile_body(&mut b, body, protos, interner);
    b.emit(Op::Return);
    let idx = protos.len();
    protos.push(b.build("<block>".into(), block_params.to_vec()));
    (idx, param_start, n_params)
}
