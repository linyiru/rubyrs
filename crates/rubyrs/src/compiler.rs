use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{Expr, SExpr};
use crate::bytecode::{BinOpKind, Op, Proto};
use crate::error::Span;
use crate::intern::Interner;
use crate::value::Value;

// ---------- Compiler ----------

pub(crate) struct ProtoBuilder {
    pub(crate) code: Vec<Op>,
    pub(crate) op_spans: Vec<Span>,
    pub(crate) locals: HashMap<String, u16>,
    pub(crate) n_locals: u16,
    pub(crate) current_span: Span,
    pub(crate) filename: Rc<str>,
    /// When compiling a method body, this is the method's name.
    /// `Expr::Super` reads it to know which slot to look up in
    /// the parent class. `None` for class bodies, the toplevel
    /// `<main>` proto, and blocks — using `super` in those
    /// contexts surfaces as a SyntaxError via `AST_ERRORS`.
    /// Blocks could in principle inherit the method context
    /// from the enclosing proto, but that's a follow-up.
    pub(crate) method_name: Option<String>,
    /// Param count snapshot taken when the method body starts
    /// compiling — used to emit `LoadLocal(0..n)` for the
    /// forwarding `super` (bare) form.
    pub(crate) method_param_count: u16,
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
            method_name: None,
            method_param_count: 0,
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

    /// Force-allocate a fresh slot for `name`, overwriting any prior
    /// binding. Used for block parameters: in Ruby, a block param shadows
    /// any outer-scope variable of the same name (modern Ruby; "block
    /// local variable" semantics).
    pub(crate) fn define_local_slot(&mut self, name: &str) -> u16 {
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
    pub(crate) fn build(self, name: String, params: Vec<String>, defaults: Vec<Option<Value>>) -> Proto {
        Proto {
            name, params, defaults,
            rest_param: None,
            kw_param_defaults: vec![],
            n_locals: self.n_locals,
            code: self.code,
            op_spans: self.op_spans,
            filename: self.filename,
        }
    }
}

pub(crate) fn compile_body(
    b: &mut ProtoBuilder, exprs: &[SExpr],
    protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) {
    if exprs.is_empty() {
        b.emit(Op::LoadNil);
        return;
    }
    let last = exprs.len() - 1;
    for (i, e) in exprs.iter().enumerate() {
        if i == last {
            // The final expression's value becomes the body's value.
            compile_expr(b, e, protos, interner, cc);
        } else {
            // Intermediate: value is discarded. Specialised stmt emit
            // skips the Dup-for-result + trailing Pop pair where possible.
            compile_stmt(b, e, protos, interner, cc);
        }
    }
}

/// Compile `e` in *statement* position — its result is discarded. For
/// assignment-shaped expressions we emit the store directly (no `Dup`),
/// and for the `Inc*` ops we use the `NoPush` variants. Anything else
/// falls back to `compile_expr` + `Pop`.
fn compile_stmt(
    b: &mut ProtoBuilder, e: &SExpr,
    protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) {
    let prev_span = b.current_span;
    b.current_span = e.span;
    match &e.node {
        Expr::LVarWrite(name, val) => {
            if let Expr::Call { receiver: Some(r), name: op, args } = &val.node
                && op == "+" && args.len() == 1
                    && let (Expr::LVarRead(rn), Expr::IntLit(1)) = (&r.node, &args[0].node)
                        && rn == name {
                            let slot = b.local_slot(name);
                            b.emit(Op::IncLocalNoPush(slot));
                            b.current_span = prev_span;
                            return;
                        }
            // Allocate the LHS slot BEFORE compiling the RHS so a
            // lambda inside the RHS (whose param_start snapshots
            // parent.n_locals) doesn't accidentally overlap this
            // local. Existing `+= 1` fused path already did this
            // — uniformity also helps when the RHS is `proc {}` or
            // any other closure-creating expression.
            let slot = b.local_slot(name);
            compile_expr(b, val, protos, interner, cc);
            b.emit(Op::StoreLocal(slot));
        }
        Expr::IVarWrite(name, val) => {
            if let Expr::Call { receiver: Some(r), name: op, args } = &val.node
                && op == "+" && args.len() == 1
                    && let (Expr::IVarRead(rn), Expr::IntLit(1)) = (&r.node, &args[0].node)
                        && rn == name {
                            let id = interner.intern(name);
                            b.emit(Op::IncIvarNoPush(id));
                            b.current_span = prev_span;
                            return;
                        }
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            b.emit(Op::StoreIvar(id));
        }
        _ => {
            compile_expr(b, e, protos, interner, cc);
            b.emit(Op::Pop);
        }
    }
    b.current_span = prev_span;
}

pub(crate) fn compile_expr(
    b: &mut ProtoBuilder, e: &SExpr,
    protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) {
    let prev_span = b.current_span;
    b.current_span = e.span;
    match &e.node {
        Expr::IntLit(i) => { b.emit(Op::LoadConstInt(*i)); }
        Expr::FloatLit(f) => { b.emit(Op::LoadConstFloat(*f)); }
        Expr::StrLit(s) => { let id = interner.intern(s); b.emit(Op::LoadConstStr(id)); }
        Expr::RegexLit(src) => { let id = interner.intern(src); b.emit(Op::LoadRegex(id)); }
        Expr::SymbolLit(s) => { let id = interner.intern(s); b.emit(Op::LoadSymbol(id)); }
        Expr::InterpolatedStr(parts) => {
            if parts.is_empty() {
                let id = interner.intern("");
                b.emit(Op::LoadConstStr(id));
            } else {
                let to_s = interner.intern("to_s");
                for (idx, p) in parts.iter().enumerate() {
                    match &p.node {
                        Expr::StrLit(_) => compile_expr(b, p, protos, interner, cc),
                        _ => {
                            compile_expr(b, p, protos, interner, cc);
                            let cid = *cc as u16; *cc += 1;
                            b.emit(Op::Call(to_s, 0, cid));
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
            if let Expr::Call { receiver: Some(r), name: op, args } = &val.node
                && op == "+" && args.len() == 1
                    && let (Expr::LVarRead(rn), Expr::IntLit(1)) = (&r.node, &args[0].node)
                        && rn == name {
                            let slot = b.local_slot(name);
                            b.emit(Op::IncLocal(slot));
                            b.current_span = prev_span;
                            return;
                        }
            // See note in compile_stmt's LVarWrite arm: pre-allocate
            // the LHS slot so a closure-creating RHS (lambda, proc)
            // doesn't take this slot as its param_start.
            let slot = b.local_slot(name);
            compile_expr(b, val, protos, interner, cc);
            b.emit(Op::Dup);
            b.emit(Op::StoreLocal(slot));
        }
        Expr::IVarRead(name) => {
            let id = interner.intern(name);
            b.emit(Op::LoadIvar(id));
        }
        Expr::IVarWrite(name, val) => {
            // Fast path: @name = @name + 1
            if let Expr::Call { receiver: Some(r), name: op, args } = &val.node
                && op == "+" && args.len() == 1
                    && let (Expr::IVarRead(rn), Expr::IntLit(1)) = (&r.node, &args[0].node)
                        && rn == name {
                            let id = interner.intern(name);
                            b.emit(Op::IncIvar(id));
                            b.current_span = prev_span;
                            return;
                        }
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreIvar(id));
        }
        Expr::ConstRead(name) => {
            let id = interner.intern(name);
            b.emit(Op::LoadConst(id));
        }
        Expr::MultiWrite { targets, value } => {
            // Compile the RHS once, leave it on the stack. Without a
            // splat: dup-and-index each target by positive index;
            // missing indices fall through Array#[] -> nil, matching
            // CRuby's "extra targets get nil" rule.
            //
            // With a splat `a, *r, b = arr`: pre-targets index from
            // the front, post-targets index from the back (negative
            // indices already supported by Array#[]), and the splat
            // slice is computed via the internal `Array#__mw_splat`
            // primitive which always returns a fresh Array (never
            // nil), correctly handling underflow when there are
            // fewer source elements than pre+post.
            //
            // The source Array remains on stack as the expression's
            // result, matching CRuby.
            use crate::ast::MultiWriteTarget as MWT;
            compile_expr(b, value, protos, interner, cc);
            let bracket_id = interner.intern("[]");
            let splat_id = interner.intern("__mw_splat");

            let splat_pos = targets.iter().position(|t| matches!(
                t, MWT::SplatLocal(_) | MWT::SplatIvar(_)
            ));

            let emit_store = |b: &mut ProtoBuilder, interner: &mut Interner, t: &MWT| {
                match t {
                    MWT::Local(name) => {
                        let slot = b.local_slot(name);
                        b.emit(Op::StoreLocal(slot));
                    }
                    MWT::Ivar(name) => {
                        let id = interner.intern(name);
                        b.emit(Op::StoreIvar(id));
                    }
                    MWT::SplatLocal(Some(name)) => {
                        let slot = b.local_slot(name);
                        b.emit(Op::StoreLocal(slot));
                    }
                    MWT::SplatLocal(None) => {
                        b.emit(Op::Pop);
                    }
                    MWT::SplatIvar(name) => {
                        let id = interner.intern(name);
                        b.emit(Op::StoreIvar(id));
                    }
                }
            };

            match splat_pos {
                None => {
                    for (i, target) in targets.iter().enumerate() {
                        b.emit(Op::Dup);
                        b.emit(Op::LoadConstInt(i as i64));
                        let cid = *cc as u16; *cc += 1;
                        b.emit(Op::Call(bracket_id, 1, cid));
                        emit_store(b, interner, target);
                    }
                }
                Some(s) => {
                    let post = targets.len() - s - 1;
                    let post_id = interner.intern("__mw_post");
                    // Pre-splat: plain `arr[i]`. Pre claims from
                    // the front first; out-of-bounds returns nil
                    // via Array#[]'s existing semantics, which is
                    // exactly what CRuby does for pre-targets
                    // (only the post group can be "starved" by a
                    // greedy pre — never the other way around).
                    for (i, target) in targets.iter().enumerate().take(s) {
                        b.emit(Op::Dup);
                        b.emit(Op::LoadConstInt(i as i64));
                        let cid = *cc as u16; *cc += 1;
                        b.emit(Op::Call(bracket_id, 1, cid));
                        emit_store(b, interner, target);
                    }
                    // splat slice: `arr.__mw_splat(pre, post)`
                    b.emit(Op::Dup);
                    b.emit(Op::LoadConstInt(s as i64));
                    b.emit(Op::LoadConstInt(post as i64));
                    let cid = *cc as u16; *cc += 1;
                    b.emit(Op::Call(splat_id, 2, cid));
                    emit_store(b, interner, &targets[s]);
                    // Post-splat: need pre+post counts at runtime
                    // so __mw_post can implement CRuby's "pre
                    // wins" rule (post slots beyond what's left
                    // become nil, not a wrap-around to the start).
                    for j in 0..post {
                        b.emit(Op::Dup);
                        b.emit(Op::LoadConstInt(j as i64));
                        b.emit(Op::LoadConstInt(s as i64));
                        b.emit(Op::LoadConstInt(post as i64));
                        let cid = *cc as u16; *cc += 1;
                        b.emit(Op::Call(post_id, 3, cid));
                        emit_store(b, interner, &targets[s + 1 + j]);
                    }
                }
            }
        }
        Expr::If { cond, then_body, else_body } => {
            compile_expr(b, cond, protos, interner, cc);
            let jf = b.emit(Op::JumpIfFalse(0));
            compile_body(b, then_body, protos, interner, cc);
            let je = b.emit(Op::Jump(0));
            let else_start = b.pos();
            b.patch_jump(jf, else_start);
            compile_body(b, else_body, protos, interner, cc);
            let end = b.pos();
            b.patch_jump(je, end);
        }
        Expr::Or(lhs, rhs) => {
            // a || b — keep a if truthy, otherwise eval b.
            // We only have JumpIfFalse (pops top) so the lowering is:
            //   <a>; Dup; JumpIfFalse to_skip
            //   Jump end                  ; a was truthy: leave the kept copy
            //   to_skip: Pop              ; pop the falsy a we kept on stack
            //            <b>
            //   end:
            compile_expr(b, lhs, protos, interner, cc);
            b.emit(Op::Dup);
            let jf = b.emit(Op::JumpIfFalse(0));
            let je = b.emit(Op::Jump(0));
            let to_skip = b.pos();
            b.patch_jump(jf, to_skip);
            b.emit(Op::Pop);
            compile_expr(b, rhs, protos, interner, cc);
            let end = b.pos();
            b.patch_jump(je, end);
        }
        Expr::And(lhs, rhs) => {
            // a && b — keep a if falsy, otherwise eval b.
            //   <a>; Dup; JumpIfFalse to_skip   ; truthy: fall through
            //   Pop; <b>; Jump end              ; truthy: eval and use b
            //   to_skip:                        ; falsy: leave a as result
            //   end:
            compile_expr(b, lhs, protos, interner, cc);
            b.emit(Op::Dup);
            let jf = b.emit(Op::JumpIfFalse(0));
            b.emit(Op::Pop);
            compile_expr(b, rhs, protos, interner, cc);
            let je = b.emit(Op::Jump(0));
            let to_skip = b.pos();
            b.patch_jump(jf, to_skip);
            let end = b.pos();
            b.patch_jump(je, end);
        }
        Expr::While { cond, body } => {
            let start = b.pos();
            compile_expr(b, cond, protos, interner, cc);
            let jf = b.emit(Op::JumpIfFalse(0));
            compile_body(b, body, protos, interner, cc);
            b.emit(Op::Pop);
            let j = b.emit(Op::Jump(0));
            b.patch_jump(j, start);
            let end = b.pos();
            b.patch_jump(jf, end);
            b.emit(Op::LoadNil);
        }
        Expr::Call { receiver, name, args } => {
            if receiver.is_none() && name == "__seq__" {
                compile_body(b, args, protos, interner, cc);
                b.current_span = prev_span;
                return;
            }
            // attr_accessor / attr_reader / attr_writer — compile-time
            // desugar. Inside a class body these install getter/setter
            // methods on the surrounding class via the normal
            // `Op::DefMethod` machinery; outside a class body they
            // still emit `DefMethod` ops (which target the top-level
            // method registry) — that's a divergence from CRuby
            // (where `attr_*` raises NoMethodError at top level) but
            // harmless and avoids needing a "class-body context" the
            // compiler doesn't currently track. Each arg must be a
            // Symbol literal — dynamic forms (`attr_accessor(*xs)`)
            // pass through as a regular Call and will fail at
            // dispatch.
            if receiver.is_none()
                && (name == "attr_accessor" || name == "attr_reader" || name == "attr_writer")
                && args.iter().all(|a| matches!(a.node, Expr::SymbolLit(_)))
            {
                let do_reader = name != "attr_writer";
                let do_writer = name != "attr_reader";
                let prev = b.current_span;
                for a in args {
                    let sym_name = if let Expr::SymbolLit(s) = &a.node { s.clone() } else { unreachable!() };
                    let ivar_name = format!("@{}", sym_name);
                    if do_reader {
                        // def <sym>; @<sym>; end
                        let body = vec![SExpr { span: a.span, node: Expr::IVarRead(ivar_name.clone()) }];
                        let pidx = compile_proto(
                            sym_name.clone(), vec![], vec![], &body,
                            b.filename.clone(), protos, interner, cc,
                        );
                        let nid = interner.intern(&sym_name);
                        b.emit(Op::DefMethod(nid, pidx as u32));
                    }
                    if do_writer {
                        // def <sym>=(val); @<sym> = val; end
                        let setter_name = format!("{sym_name}=");
                        let val_read = SExpr { span: a.span, node: Expr::LVarRead("val".into()) };
                        let body = vec![SExpr {
                            span: a.span,
                            node: Expr::IVarWrite(ivar_name.clone(), Box::new(val_read)),
                        }];
                        let pidx = compile_proto(
                            setter_name.clone(), vec!["val".into()], vec![None], &body,
                            b.filename.clone(), protos, interner, cc,
                        );
                        let nid = interner.intern(&setter_name);
                        b.emit(Op::DefMethod(nid, pidx as u32));
                    }
                }
                b.emit(Op::LoadNil);
                b.current_span = prev;
                return;
            }
            // `alias_method :new, :old` — compile-time intercept when
            // both args are Symbol literals. Emits a single
            // `Op::AliasMethod`, which copies the Rc<Method> entry
            // inside the surrounding class (or toplevel) at runtime.
            // Dynamic-symbol forms fall through to a normal Call and
            // currently fail at dispatch.
            if receiver.is_none()
                && name == "alias_method"
                && args.len() == 2
                && matches!(args[0].node, Expr::SymbolLit(_))
                && matches!(args[1].node, Expr::SymbolLit(_))
            {
                let new_name = if let Expr::SymbolLit(s) = &args[0].node { s.clone() } else { unreachable!() };
                let old_name = if let Expr::SymbolLit(s) = &args[1].node { s.clone() } else { unreachable!() };
                let nid = interner.intern(&new_name);
                let oid = interner.intern(&old_name);
                // `Op::AliasMethod`'s VM handler pushes Nil itself
                // (matching `Op::DefMethod`'s shape), so the
                // compiler must NOT emit a trailing `LoadNil` — that
                // would leave a stray Nil on the operand stack each
                // alias, which the class-body Return only happens
                // to swallow because it truncates to `base_sp`.
                // Inside a loop or with multiple aliases per body
                // the imbalance accumulates.
                b.emit(Op::AliasMethod(nid, oid));
                b.current_span = prev_span;
                return;
            }
            if receiver.is_none() && name == "raise" {
                match args.len() {
                    0 => { b.emit(Op::LoadNil); }
                    1 => {
                        // Single arg: a String literal (wrap as
                        // RuntimeError), an Exception instance
                        // (pass-through), or an Exception class
                        // (instantiate via `normalize_exception`).
                        compile_expr(b, &args[0], protos, interner, cc);
                    }
                    _ => {
                        // `raise SomeClass, "msg", *more` — synthesise
                        // `SomeClass.new("msg", *more)` so the regular
                        // `new` path runs `initialize` with the
                        // remaining args. The Instance returned is the
                        // value `Raise` consumes; `normalize_exception`
                        // sees an Object and leaves it alone.
                        let new_call = SExpr {
                            span: args[0].span,
                            node: Expr::Call {
                                receiver: Some(Box::new(args[0].clone())),
                                name: "new".to_string(),
                                args: args[1..].to_vec(),
                            },
                        };
                        compile_expr(b, &new_call, protos, interner, cc);
                    }
                }
                b.emit(Op::Raise);
                b.emit(Op::LoadNil);
                b.current_span = prev_span;
                return;
            }
            if let (Some(r), 1, Some(kind)) = (receiver.as_ref(), args.len(), BinOpKind::from_op_name(name)) {
                compile_expr(b, r, protos, interner, cc);
                // Fuse `<expr> <op> <int_literal>` into a single op so the
                // LoadConstInt + BinOp pair becomes one BinOpInt.
                if let Expr::IntLit(rhs) = &args[0].node {
                    b.emit(Op::BinOpInt(kind, *rhs));
                } else {
                    compile_expr(b, &args[0], protos, interner, cc);
                    b.emit(Op::BinOp(kind));
                }
                b.current_span = prev_span;
                return;
            }
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            for a in args { compile_expr(b, a, protos, interner, cc); }
            let argc = args.len() as u8;
            let cid = *cc as u16; *cc += 1;
            if has_recv {
                b.emit(Op::Call(name_id, argc, cid));
            } else {
                b.emit(Op::CallNoRecv(name_id, argc, cid));
            }
        }
        Expr::Def { name, params, defaults, rest, kw_params, body } => {
            let lit_defaults: Vec<Option<Value>> = defaults.iter().map(|d| {
                d.as_ref().map(|sx| literal_to_value(&sx.node, interner))
            }).collect();
            // Param layout in slot order: positional, then rest
            // (if any), then keyword params (in source order).
            // ProtoBuilder allocates slots in that sequence; the
            // Proto's `rest_param` + `kw_param_defaults` tell
            // invoke_method how to bind.
            let mut effective_params = params.clone();
            if let Some(rname) = rest {
                effective_params.push(rname.clone());
            }
            for (kname, _) in kw_params {
                effective_params.push(kname.clone());
            }
            let kw_lit_defaults: Vec<Option<Value>> = kw_params.iter().map(|(_, d)| {
                d.as_ref().map(|sx| literal_to_value(&sx.node, interner))
            }).collect();
            let proto_idx = compile_proto_kind(
                name.clone(), effective_params, lit_defaults, body,
                b.filename.clone(), protos, interner, cc, /*is_method=*/true,
            );
            if let Some(rname) = rest {
                protos[proto_idx].rest_param = Some(rname.clone());
            }
            protos[proto_idx].kw_param_defaults = kw_lit_defaults;
            let name_id = interner.intern(name);
            b.emit(Op::DefMethod(name_id, proto_idx as u32));
            b.emit(Op::LoadNil);
        }
        Expr::Super(args_opt) => {
            // `super` only makes sense inside a method body. The
            // current ProtoBuilder records that via `method_name`.
            // Outside (class body, toplevel, block) we synthesise
            // a SyntaxError-via-AST_ERRORS — actually no, we
            // can't reach AST_ERRORS from compile. Just emit
            // LoadNil and let runtime NoMethodError surface;
            // documented as a gap. Realistic scripts only put
            // `super` in methods anyway.
            let mname = b.method_name.clone();
            let argc: u8 = match args_opt {
                Some(args) => {
                    for a in args { compile_expr(b, a, protos, interner, cc); }
                    args.len() as u8
                }
                None => {
                    // Forwarding form — push each enclosing-method
                    // param from its local slot. Params are
                    // always slots `0..method_param_count`
                    // (`ProtoBuilder::new` assigns them in order).
                    for i in 0..b.method_param_count {
                        b.emit(Op::LoadLocal(i));
                    }
                    b.method_param_count as u8
                }
            };
            let mname = mname.unwrap_or_else(|| "<super-outside-method>".to_string());
            let name_id = interner.intern(&mname);
            b.emit(Op::Super(name_id, argc));
        }
        Expr::Class { name, superclass, body } => {
            let proto_idx = compile_proto(format!("<class:{}>", name), vec![], vec![], body, b.filename.clone(), protos, interner, cc);
            // Push the superclass (or Nil for "default to Object") for DefClass to pop.
            if let Some(parent) = superclass {
                let parent_id = interner.intern(parent);
                b.emit(Op::LoadConst(parent_id));
            } else {
                b.emit(Op::LoadNil);
            }
            let name_id = interner.intern(name);
            b.emit(Op::DefClass(name_id, proto_idx as u32));
        }
        Expr::ArrayLit(elems) => {
            for e in elems { compile_expr(b, e, protos, interner, cc); }
            b.emit(Op::NewArray(elems.len() as u16));
        }
        Expr::RangeLit { begin, end, exclusive } => {
            compile_expr(b, begin, protos, interner, cc);
            compile_expr(b, end, protos, interner, cc);
            b.emit(Op::NewRange(if *exclusive { 1 } else { 0 }));
        }
        Expr::HashLit(pairs) => {
            for (k, v) in pairs {
                compile_expr(b, k, protos, interner, cc);
                compile_expr(b, v, protos, interner, cc);
            }
            b.emit(Op::NewHash(pairs.len() as u16));
        }
        Expr::CallWithBlock { receiver, name, args, block_params, block_body } => {
            // `define_method(:foo) { |args| ... }` — compile-time
            // intercept. The block becomes the method body; its
            // captured locals Rc stays shared with the surrounding
            // frame so closures work. Only the literal-Symbol form
            // is intercepted; dynamic `define_method(sym, &p)`
            // falls through.
            if receiver.is_none()
                && name == "define_method"
                && args.len() == 1
                && matches!(args[0].node, Expr::SymbolLit(_))
            {
                let sym_name = if let Expr::SymbolLit(s) = &args[0].node { s.clone() } else { unreachable!() };
                let (block_proto_idx, param_start, n_params) =
                    compile_block(b, block_params, block_body, protos, interner, cc);
                b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params));
                let nid = interner.intern(&sym_name);
                b.emit(Op::DefMethodBlock(nid));
                b.current_span = prev_span;
                return;
            }
            let (block_proto_idx, param_start, n_params) =
                compile_block(b, block_params, block_body, protos, interner, cc);
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params));
            for a in args { compile_expr(b, a, protos, interner, cc); }
            let argc = args.len() as u8;
            let cid = *cc as u16; *cc += 1;
            if has_recv {
                b.emit(Op::CallBlock(name_id, argc, cid));
            } else {
                b.emit(Op::CallNoRecvBlock(name_id, argc, cid));
            }
        }
        Expr::Return(val) => {
            // Non-local return: unwind through any block frames to
            // the enclosing method, popping it and using the value
            // as the method's return. The VM handles the unwind in
            // its dispatch loop via the `method_return` signal.
            match val {
                Some(e) => compile_expr(b, e, protos, interner, cc),
                None => { b.emit(Op::LoadNil); }
            }
            b.emit(Op::ReturnMethod);
            // Sentinel for stack-balance — unreachable once the
            // method_return signal fires.
            b.emit(Op::LoadNil);
            b.current_span = prev_span;
            return;
        }
        Expr::Next(val) => {
            // `next [val]` — exits the current block iteration only.
            // Op::Return pops a single frame, which from inside a
            // block is exactly the block's frame.
            match val {
                Some(e) => compile_expr(b, e, protos, interner, cc),
                None => { b.emit(Op::LoadNil); }
            }
            b.emit(Op::Return);
            // Sentinel value for stack-balance (unreachable in well-formed code).
            b.emit(Op::LoadNil);
            b.current_span = prev_span;
            return;
        }
        Expr::Break(val) => {
            match val {
                Some(e) => compile_expr(b, e, protos, interner, cc),
                None => { b.emit(Op::LoadNil); }
            }
            b.emit(Op::Break);
            b.emit(Op::Return);
            b.emit(Op::LoadNil);
            b.current_span = prev_span;
            return;
        }
        Expr::Yield(args) => {
            for a in args { compile_expr(b, a, protos, interner, cc); }
            b.emit(Op::Yield(args.len() as u8));
        }
        Expr::Apply { receiver, name, splat } => {
            // `foo(*arr)` — compile receiver (if any) then the
            // splat expression. The VM op `ApplyCall(NoRecv)`
            // pops the Array and uses its elements as args.
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            compile_expr(b, splat, protos, interner, cc);
            let name_id = interner.intern(name);
            let cid = *cc as u16; *cc += 1;
            if has_recv {
                b.emit(Op::ApplyCall(name_id, cid));
            } else {
                b.emit(Op::ApplyCallNoRecv(name_id, cid));
            }
        }
        Expr::Lambda { params, body } => {
            // `->(p) { body }` — compile the body as a block proto
            // and emit CreateBlock. Result stays on the stack as a
            // Value::Block (which supports `.call(args)` already).
            let (block_proto_idx, param_start, n_params) =
                compile_block(b, params, body, protos, interner, cc);
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params));
        }
        Expr::Begin { body, rescue, ensure } => {
            // Layered: optional outer ensure, zero-or-more inner
            // rescue clauses, then the body. With multiple `rescue`
            // clauses we want the first source-listed clause to be
            // tried first; the VM's unwinder pops handlers LIFO, so
            // we push them in REVERSE source order. Each clause
            // declaring an explicit class (or none = bare = filter
            // StandardError) gets its own PushRescue. A single
            // clause with multiple classes (`rescue A, B`) currently
            // honours only the first class — multi-class rescue
            // expansion is a follow-up. ConstantPath
            // (`Foo::Bar`) uses the trailing segment.
            let pe = ensure.as_ref().map(|_| b.emit(Op::PushEnsure(0)));

            if rescue.is_empty() {
                compile_body(b, body, protos, interner, cc);
            } else {
                // Ruby semantics: the first source-listed `rescue`
                // clause is tried first. The VM's unwinder pops the
                // rescues stack LIFO, so we PUSH in REVERSE source
                // order — the first source clause ends up on top.
                let stderr_sym = interner.intern("StandardError");
                let mut placeholders: Vec<usize> = Vec::with_capacity(rescue.len());
                for rc in rescue.iter().rev() {
                    let (slot, bind) = match &rc.var {
                        Some(name) => (b.local_slot(name), 1u8),
                        None => (0u16, 0u8),
                    };
                    // Single-class rescue today: we honour the first
                    // class in the clause's list. Multi-class
                    // (`rescue A, B => e`) is a follow-up — for now
                    // the second-and-later classes are silently
                    // ignored. Documented in ADR-pending.
                    let filter_sym = match rc.classes.first() {
                        Some(name) => interner.intern(name),
                        None => stderr_sym,
                    };
                    placeholders.push(b.emit(Op::PushRescue(0, slot, bind, filter_sym)));
                }
                compile_body(b, body, protos, interner, cc);
                for _ in &placeholders { b.emit(Op::PopRescue); }
                // Normal-path exit from body jumps past every
                // handler body to the merge point. Each handler
                // body also jumps to the same merge after running.
                let mut jump_to_end: Vec<usize> = Vec::with_capacity(rescue.len() + 1);
                jump_to_end.push(b.emit(Op::Jump(0)));
                // Handler bodies emitted in the same order as the
                // PushRescues we collected (reverse source order).
                // Handler-body order in code doesn't affect runtime
                // semantics — each PushRescue carries its own
                // handler_ip — so we keep the iteration consistent.
                for (i, rc) in rescue.iter().rev().enumerate() {
                    let placeholder = placeholders[i];
                    let handler_start = b.pos();
                    let off = handler_start as i32 - placeholder as i32 - 1;
                    if let Op::PushRescue(o, _, _, _) = &mut b.code[placeholder] {
                        *o = off;
                    }
                    compile_body(b, &rc.body, protos, interner, cc);
                    jump_to_end.push(b.emit(Op::Jump(0)));
                }
                let end = b.pos();
                for j in jump_to_end { b.patch_jump(j, end); }
            }

            // Ensure layer (compile body twice — once inline for the normal
            // path, once for the exception path which ends in Raise).
            if let (Some(eb), Some(pe)) = (ensure.as_ref(), pe) {
                b.emit(Op::PopEnsure);
                // Normal path: run ensure body, then jump past handler.
                for stmt in eb { compile_stmt(b, stmt, protos, interner, cc); }
                let je = b.emit(Op::Jump(0));
                // Exception path: PushEnsure target. Exception value is on
                // top of stack; ensure body must not touch the stack (we
                // call compile_stmt which preserves it); then Raise re-throws.
                let handler_start = b.pos();
                let off = handler_start as i32 - pe as i32 - 1;
                if let Op::PushEnsure(o) = &mut b.code[pe] { *o = off; }
                for stmt in eb { compile_stmt(b, stmt, protos, interner, cc); }
                b.emit(Op::Raise);
                let end = b.pos();
                b.patch_jump(je, end);
            }
        }
    }
    b.current_span = prev_span;
}

// Eight positional args is past clippy's default cap, but every one
// is load-bearing: the proto name + the four shape inputs (params,
// defaults, body, filename) + the three target sinks the compiler
// mutates (protos vec, interner, call-cache counter). Bundling into
// a builder struct doesn't reduce the surface; it just renames it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_proto(
    name: String, params: Vec<String>, defaults: Vec<Option<Value>>, body: &[SExpr],
    filename: Rc<str>, protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) -> usize {
    compile_proto_kind(name, params, defaults, body, filename, protos, interner, cc, /*is_method=*/false)
}

/// Same as `compile_proto` but tags the resulting builder as a
/// method body — sets `method_name` / `method_param_count` so
/// `super` knows what to forward. Called by `Expr::Def`'s
/// compile path. Class bodies and the toplevel `<main>` proto
/// stay non-method.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_proto_kind(
    name: String, params: Vec<String>, defaults: Vec<Option<Value>>, body: &[SExpr],
    filename: Rc<str>, protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
    is_method: bool,
) -> usize {
    let mut b = ProtoBuilder::new(&params, filename);
    if is_method {
        b.method_name = Some(name.clone());
        b.method_param_count = params.len() as u16;
    }
    compile_body(&mut b, body, protos, interner, cc);
    b.emit(Op::Return);
    let idx = protos.len();
    protos.push(b.build(name, params, defaults));
    idx
}

/// Convert an `Expr` known to be a literal into a runtime `Value`.
/// AST translation has already gated which `Expr` variants reach
/// here, so this only needs the literal cases.
fn literal_to_value(e: &Expr, interner: &mut Interner) -> Value {
    match e {
        Expr::IntLit(n) => Value::Int(*n),
        Expr::FloatLit(f) => Value::Float(*f),
        Expr::StrLit(s) => Value::new_str(s.as_str()),
        Expr::SymbolLit(s) => {
            let id = interner.intern(s);
            Value::Sym(id)
        }
        Expr::BoolLit(b) => Value::Bool(*b),
        Expr::Nil => Value::Nil,
        // AST translator guarantees we only see literals here, so
        // anything else is a compiler bug, not a script bug.
        _ => panic!("ICE: literal_to_value on non-literal Expr: {:?}", e),
    }
}

pub(crate) fn compile_block(
    parent: &ProtoBuilder, block_params: &[String], body: &[SExpr],
    protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) -> (usize, u16, u16) {
    let mut b = ProtoBuilder {
        code: vec![],
        op_spans: vec![],
        locals: parent.locals.clone(),
        n_locals: parent.n_locals,
        current_span: parent.current_span,
        filename: parent.filename.clone(),
        // A block inherits its enclosing method's `super` context
        // so `super` inside a block forwards to the parent
        // class's same-named method — matches CRuby. If the
        // parent isn't a method (class body / toplevel),
        // method_name stays None and `super` will surface as a
        // NoMethodError-shaped Trap.
        method_name: parent.method_name.clone(),
        method_param_count: parent.method_param_count,
    };
    let param_start = b.n_locals;
    // Block params get fresh slots and shadow any outer binding of the
    // same name; matching CRuby's "block local variable" semantics.
    for p in block_params { b.define_local_slot(p); }
    let n_params = block_params.len() as u16;
    compile_body(&mut b, body, protos, interner, cc);
    b.emit(Op::Return);
    let idx = protos.len();
    protos.push(b.build("<block>".into(), block_params.to_vec(), vec![None; block_params.len()]));
    (idx, param_start, n_params)
}
