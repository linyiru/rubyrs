use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{BlockParam, Expr, SExpr};
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
    /// True iff this builder is compiling a real method body
    /// (the proto bound to an `Op::DefMethod` /
    /// `Op::DefSingletonMethod`). Distinct from `method_name`
    /// which is inherited into blocks for `super`'s forwarding
    /// semantics. `Expr::Return` uses this flag to decide
    /// between `Op::Return` (local; method body) and
    /// `Op::ReturnMethod` (non-local; block, walks frames out
    /// to the enclosing method). Class bodies and the toplevel
    /// `<main>` proto stay false.
    pub(crate) is_method_body: bool,
    /// Stack of in-progress `while` loops within this proto. Each
    /// entry is the list of code offsets where a `break` inside the
    /// loop emitted `Op::BreakLoop(0)` — patched to the loop's join
    /// label when the `while` finishes compiling. Empty when no
    /// loop is active; `Expr::Break` then falls back to the existing
    /// `Op::Break + Op::Return` block/iterator-driver semantics.
    /// The stack lives on the proto builder (not the compile ctx)
    /// because a block introduces a fresh proto with its own
    /// builder — so `break` inside `do … end` inside a `while`
    /// naturally sees an empty stack and breaks the BLOCK, matching
    /// Ruby's lexical break-target rule.
    pub(crate) loop_break_jumps: Vec<Vec<usize>>,
    /// Parallel stack for `next` placeholders. Patched to the loop's
    /// per-iteration check label (the cond expression's position),
    /// so `next` re-evaluates the loop guard and either iterates
    /// again or falls through to the natural exit. Same lexical
    /// scoping as `loop_break_jumps` — a block proto starts with
    /// an empty stack so `next` in a block reaches the iteration
    /// driver, not an enclosing `while` in the parent proto.
    pub(crate) loop_next_jumps: Vec<Vec<usize>>,
    /// Per-proto pool for binary string literals (`"\xNN..."`
    /// inputs where the unescaped bytes aren't valid UTF-8).
    /// Flushed into `Proto.byte_literals` on emit. See
    /// `Op::LoadConstStrBytes` for the runtime side.
    pub(crate) byte_literals: Vec<std::rc::Rc<[u8]>>,
    /// Lexical class/module nesting at the point this proto is
    /// being compiled. Empty at the toplevel, `["Foo"]` inside
    /// `module Foo; ... end`, `["Foo", "Bar"]` inside
    /// `module Foo; class Bar; ... end; end`, etc. Read by
    /// `Expr::Class` and `Expr::ConstWrite` arms to emit a
    /// second alias `StoreConst("Foo::Bar")` next to the bare
    /// `StoreConst("Bar")`, so external code can later resolve
    /// `Foo::Bar` via the existing flat-keyed constants table.
    /// Reads from inside the scope still find the bare name —
    /// dual-write keeps both lookup directions working without
    /// modelling CRuby's full cref-walk constant lookup. See the
    /// class_path emit sites and the docs/SUBSET.md note.
    pub(crate) class_path: Vec<String>,
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
            is_method_body: false,
            loop_break_jumps: vec![],
            loop_next_jumps: vec![],
            class_path: vec![],
            byte_literals: vec![],
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
            Op::JumpIfArgGiven(_, o) => *o = off,
            Op::BreakLoop(o) => *o = off,
            Op::NextLoop(o) => *o = off,
            _ => panic!("ICE: patch_jump on non-jump op at {}", at),
        }
    }
    pub(crate) fn build(self, name: String, params: Vec<String>, n_required_positional: u16) -> Proto {
        Proto {
            name, params, n_required_positional,
            rest_param: None,
            kw_param_defaults: vec![],
            kw_rest_param: None,
            block_param: None,
            n_locals: self.n_locals,
            code: self.code,
            op_spans: self.op_spans,
            filename: self.filename,
            byte_literals: self.byte_literals,
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
        Expr::ConstWrite(name, absolute, val) => {
            // Statement-position const write: skip the Dup the
            // expression form needs, matching the LVarWrite /
            // IVarWrite arms above. Saves `Dup + Pop` per top-level
            // `FOO = ...` line.
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            // Class-path alias: inside `module Foo; X = 1; end`,
            // also store under `Foo::X` so `Foo::X` reads work
            // from outside. Skipped for top-level (empty path),
            // already-pathed names, AND absolute writes (`::X = 1`
            // inside `module Foo` must NOT alias to `Foo::X` —
            // leading `::` explicitly targets top-level only).
            let prefixed_id = (!b.class_path.is_empty() && !*absolute && !name.contains("::"))
                .then(|| interner.intern(&format!("{}::{}", b.class_path.join("::"), name)));
            if prefixed_id.is_some() {
                b.emit(Op::Dup);
            }
            b.emit(Op::StoreConst(id));
            if let Some(pid) = prefixed_id {
                b.emit(Op::StoreConst(pid));
            }
        }
        Expr::GVarWrite(name, val) => {
            // Statement-position global write: same `no Dup` fast
            // path as the other Write arms. The expression form
            // (in compile_expr) Dups for the assignment-as-
            // expression value.
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            b.emit(Op::StoreGlobal(id));
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
        Expr::StrLitBytes(bytes) => {
            // Bytes path — interner can't hold non-UTF-8, so the
            // pool lives per-Proto on the current builder. No
            // dedup attempt (binary literals are rare and usually
            // small; the simple pool keeps Op::LoadConstStrBytes
            // a single index without an extra hash probe per
            // emit).
            let idx = b.byte_literals.len() as u32;
            b.byte_literals.push(std::rc::Rc::from(bytes.as_slice()));
            b.emit(Op::LoadConstStrBytes(idx));
        }
        #[cfg(feature = "regex")]
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
        Expr::ConstWrite(name, absolute, val) => {
            // CRuby: a constant assignment leaves the assigned value
            // on the stack as the expression's result. Same pattern
            // as IVarWrite above (Dup so the value survives the
            // store). Absolute writes (`::X = 1`) skip the
            // class_path alias — see the stmt-form arm for the
            // rationale.
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            let prefixed_id = (!b.class_path.is_empty() && !*absolute && !name.contains("::"))
                .then(|| interner.intern(&format!("{}::{}", b.class_path.join("::"), name)));
            // Stack going in: [val]. We need to leave [val] on
            // stack as the expression result. Each StoreConst pops
            // one; with N stores we need N Dups.
            //   no alias: Dup, StoreConst(bare)            → [val]
            //   alias:    Dup, Dup, StoreConst(bare),
            //                       StoreConst(prefixed)   → [val]
            b.emit(Op::Dup);
            if prefixed_id.is_some() {
                b.emit(Op::Dup);
            }
            b.emit(Op::StoreConst(id));
            if let Some(pid) = prefixed_id {
                b.emit(Op::StoreConst(pid));
            }
        }
        Expr::GVarRead(name) => {
            let id = interner.intern(name);
            b.emit(Op::LoadGlobal(id));
        }
        Expr::GVarWrite(name, val) => {
            // Expression-form: Dup so `x = ($foo = 42)` binds both.
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreGlobal(id));
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
        Expr::While { cond, body, post } => {
            // EnterLoop / ExitLoop bracket the loop so `break` and
            // `next` inside the body can pop dynamic rescue/ensure
            // handlers down to the depth at loop entry. Two parallel
            // placeholder stacks on the builder: break jumps land at
            // the join (loop end), next jumps land at the iter-check
            // (cond expression's position) so the loop re-evaluates
            // the guard and continues or falls through.
            b.emit(Op::EnterLoop);
            b.loop_break_jumps.push(vec![]);
            b.loop_next_jumps.push(vec![]);
            // iter_check is captured per arm — it points at the cond
            // evaluation that decides whether to loop again. For the
            // pre-form this coincides with the loop's start label.
            // For the post-form it sits AFTER the body's terminal Pop,
            // so `next` skips the partial body but still re-checks.
            let iter_check;
            if *post {
                // `begin … end while cond` — body runs first, cond
                // is checked after. JumpIfFalse-to-end, jump-back-
                // to-start, but the start label is BEFORE the cond
                // so the body re-runs only when cond stayed truthy.
                let body_start = b.pos();
                compile_body(b, body, protos, interner, cc);
                b.emit(Op::Pop);
                iter_check = b.pos();
                compile_expr(b, cond, protos, interner, cc);
                let jf = b.emit(Op::JumpIfFalse(0));
                let j = b.emit(Op::Jump(0));
                b.patch_jump(j, body_start);
                let exit_normal = b.pos();
                b.patch_jump(jf, exit_normal);
                b.emit(Op::LoadNil);
            } else {
                // Pre-condition `while cond; …; end` — cond first,
                // body only runs when truthy.
                let start = b.pos();
                iter_check = start;
                compile_expr(b, cond, protos, interner, cc);
                let jf = b.emit(Op::JumpIfFalse(0));
                compile_body(b, body, protos, interner, cc);
                b.emit(Op::Pop);
                let j = b.emit(Op::Jump(0));
                b.patch_jump(j, start);
                let exit_normal = b.pos();
                b.patch_jump(jf, exit_normal);
                b.emit(Op::LoadNil);
            }
            // Patch `next` placeholders to the iter-check label
            // BEFORE the join, because next must re-evaluate cond
            // (the loop's natural-exit LoadNil branch handles the
            // false case).
            for j in b.loop_next_jumps.pop().expect("ICE: while popped loop_next_jumps without push") {
                b.patch_jump(j, iter_check);
            }
            // Join label: both normal exit (after LoadNil) and every
            // `break` converge here. `BreakLoop` jumps with the
            // break value already on the stack, so we don't push
            // again. `ExitLoop` is the last shared step.
            let join = b.pos();
            for j in b.loop_break_jumps.pop().expect("ICE: while popped loop_break_jumps without push") {
                b.patch_jump(j, join);
            }
            b.emit(Op::ExitLoop);
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
            // attr_* compile-time intercept inside a normal class
            // body. Paired with the `class << X` AST-level expansion
            // in ast.rs (see `attr_reader_writer_flags`): if the
            // semantics of either intercept change, update both.
            //
            // Zero-arg form is intentionally accepted as a silent
            // no-op — CRuby 3.4 does the same (verified: no
            // ArgumentError, no methods defined). The for-loop
            // below handles empty args naturally (vacuous iter()).
            if receiver.is_none()
                && let Some((do_reader, do_writer)) = crate::ast::attr_reader_writer_flags(name)
                && args.iter().all(|a| matches!(a.node, Expr::SymbolLit(_)))
            {
                let prev = b.current_span;
                for a in args {
                    let sym_name = if let Expr::SymbolLit(s) = &a.node { s.clone() } else { unreachable!() };
                    let ivar_name = format!("@{}", sym_name);
                    if do_reader {
                        // def <sym>; @<sym>; end
                        let body = vec![SExpr { span: a.span, node: Expr::IVarRead(ivar_name.clone()) }];
                        let pidx = compile_proto(
                            sym_name.clone(), vec![], &body,
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
                            setter_name.clone(), vec!["val".into()], &body,
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
        Expr::Def { name, params, defaults, rest, kw_params, kw_rest, block_param, receiver, body } => {
            // `defaults` is parallel to `params`: leading `None`s are
            // required positionals, trailing `Some(expr)`s are
            // optionals. The compile_proto_kind helper emits a
            // per-optional `Op::JumpIfArgGiven(slot, skip) + <expr>
            // + StoreLocal(slot)` prologue at the top of the body
            // so non-literal defaults (`level = Logger::INFO`,
            // `b = a + 1`) work — slot is bound before the prologue
            // runs, so `a` is already readable.
            let n_required_positional = defaults.iter().take_while(|d| d.is_none()).count() as u16;
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
            // `**kwrest` slot goes at the very end of the param
            // list so the existing kw_params block above stays
            // contiguous (invoke_method indexes by offset).
            // Anonymous `**` (no name) still reserves the slot —
            // it absorbs leftover kwargs but the body has no way
            // to read them. Implemented with a synthesised
            // `__kw_rest_anon` name so the invoke_method binding
            // path uniformly treats both shapes.
            if let Some(krname) = kw_rest {
                let slot_name = if krname.is_empty() { "__kw_rest_anon".to_string() } else { krname.clone() };
                effective_params.push(slot_name);
            }
            // `&blk` named block param goes at the very end, after
            // kw_rest if any. Frame setup binds either Value::Block
            // (if caller passed a block) or Value::Nil into this slot.
            if let Some(bname) = block_param {
                effective_params.push(bname.clone());
            }
            let kw_lit_defaults: Vec<Option<Value>> = kw_params.iter().map(|(_, d)| {
                d.as_ref().map(|sx| literal_to_value(&sx.node, interner))
            }).collect();
            let proto_idx = compile_proto_kind(
                name.clone(), effective_params, n_required_positional, defaults.clone(), body,
                b.filename.clone(), protos, interner, cc, /*is_method=*/true,
                // Methods inherit the lexical class_path so any
                // nested `class Inner` defined inside the method
                // body still aliases under the surrounding nesting.
                b.class_path.clone(),
            );
            if let Some(rname) = rest {
                protos[proto_idx].rest_param = Some(rname.clone());
            }
            protos[proto_idx].kw_param_defaults = kw_lit_defaults;
            if let Some(krname) = kw_rest {
                let slot_name = if krname.is_empty() { "__kw_rest_anon".to_string() } else { krname.clone() };
                protos[proto_idx].kw_rest_param = Some(slot_name);
            }
            if let Some(bname) = block_param {
                protos[proto_idx].block_param = Some(bname.clone());
            }
            let name_id = interner.intern(name);
            match receiver {
                None => {
                    b.emit(Op::DefMethod(name_id, proto_idx as u32));
                }
                Some(recv_expr) if matches!(recv_expr.node, Expr::SelfExpr) => {
                    // `def self.foo` in a class body — install
                    // on the surrounding class's
                    // `singleton_methods` table. Master ships
                    // this via `Op::DefSingletonMethod` (no
                    // operand-stack receiver; target via
                    // `class_stack.last()`).
                    b.emit(Op::DefSingletonMethod(name_id, proto_idx as u32));
                }
                Some(recv_expr) => {
                    // `def obj.foo` — instance-level singleton.
                    // Compile the receiver and emit the
                    // pop-and-install op.
                    compile_expr(b, recv_expr, protos, interner, cc);
                    b.emit(Op::DefObjectSingletonMethod(name_id, proto_idx as u32));
                }
            }
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
            // Child path = parent's class_path + [this name]. Threaded
            // into the body proto so a further-nested `class Inner`
            // sees the full chain and aliases under
            // `Foo::Bar::Inner`.
            let mut child_path = b.class_path.clone();
            child_path.push(name.clone());
            let proto_idx = compile_proto_at(
                format!("<class:{}>", name), vec![], body,
                b.filename.clone(), protos, interner, cc, child_path,
            );
            // Push the superclass (or Nil for "default to Object") for DefClass to pop.
            if let Some(parent) = superclass {
                let parent_id = interner.intern(parent);
                b.emit(Op::LoadConst(parent_id));
            } else {
                b.emit(Op::LoadNil);
            }
            let name_id = interner.intern(name);
            b.emit(Op::DefClass(name_id, proto_idx as u32));
            // Alias under the prefixed path so `Foo::Bar.new` from
            // outside resolves. Skipped when class_path is empty
            // (top-level — no prefix needed) or when name already
            // looks like a path (defensive — Expr::Class names are
            // bare in our AST today, but stay safe). Idempotent on
            // re-open: same Class value stored.
            if !b.class_path.is_empty() && !name.contains("::") {
                b.emit(Op::LoadConst(name_id));
                let prefixed = format!("{}::{}", b.class_path.join("::"), name);
                let pid = interner.intern(&prefixed);
                b.emit(Op::StoreConst(pid));
            }
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
                let (block_proto_idx, param_start, n_params, rest_slot) =
                    compile_block(b, block_params, block_body, protos, interner, cc);
                b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot));
                let nid = interner.intern(&sym_name);
                b.emit(Op::DefMethodBlock(nid));
                b.current_span = prev_span;
                return;
            }
            // `recv.define_singleton_method(:foo) { |args| ... }` —
            // compile-time intercept, analogous to `define_method`
            // above but with an explicit receiver. The block is
            // pushed *after* the receiver so the runtime op pops
            // (block, recv) in that order. Receiver-side TypeError
            // (non-Object) is raised at
            // Op::DefObjectSingletonMethodBlock dispatch time.
            if let Some(r) = receiver
                && name == "define_singleton_method"
                && args.len() == 1
                && matches!(args[0].node, Expr::SymbolLit(_))
            {
                let sym_name = if let Expr::SymbolLit(s) = &args[0].node { s.clone() } else { unreachable!() };
                compile_expr(b, r, protos, interner, cc);
                let (block_proto_idx, param_start, n_params, rest_slot) =
                    compile_block(b, block_params, block_body, protos, interner, cc);
                b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot));
                let nid = interner.intern(&sym_name);
                b.emit(Op::DefObjectSingletonMethodBlock(nid));
                b.current_span = prev_span;
                return;
            }
            let (block_proto_idx, param_start, n_params, rest_slot) =
                compile_block(b, block_params, block_body, protos, interner, cc);
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot));
            for a in args { compile_expr(b, a, protos, interner, cc); }
            let argc = args.len() as u8;
            let cid = *cc as u16; *cc += 1;
            if has_recv {
                b.emit(Op::CallBlock(name_id, argc, cid));
            } else {
                b.emit(Op::CallNoRecvBlock(name_id, argc, cid));
            }
        }
        Expr::CallWithBlockArg { receiver, name, args, block_arg } => {
            // `foo(&proc_value)`. Same stack shape as CallWithBlock
            // (recv, block, args...), but the block slot comes from
            // evaluating `block_arg` instead of constructing a
            // fresh proto via CreateBlock. The runtime arm in
            // do_call_block already pops a Value::Block from below
            // the args; if `block_arg` evaluates to anything else
            // (Nil / Int / etc.), the do_call_block ICE-panic
            // fires — should ideally become a Trap, tracked in
            // SUBSET.md.
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            compile_expr(b, block_arg, protos, interner, cc);
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
            // CRuby `return` has two scoping rules depending on the
            // enclosing context:
            //   1. Inside a `def` body (method_name is Some): local
            //      return — just pop the current frame. `Op::Return`.
            //   2. Inside a block body (method_name is None on the
            //      block's ProtoBuilder, even if a method body lies
            //      outside): non-local return — unwind through every
            //      block frame to the enclosing method, pop that, use
            //      the value as the method's return. `Op::ReturnMethod`.
            // Class bodies / toplevel hit case 2 today; CRuby would
            // raise LocalJumpError. Documented gap.
            match val {
                Some(e) => compile_expr(b, e, protos, interner, cc),
                None => { b.emit(Op::LoadNil); }
            }
            if b.is_method_body {
                b.emit(Op::Return);
            } else {
                b.emit(Op::ReturnMethod);
            }
            // Sentinel for stack-balance — unreachable once the
            // return signal fires.
            b.emit(Op::LoadNil);
            b.current_span = prev_span;
            return;
        }
        Expr::Next(val) => {
            // Two-target codegen mirroring `Expr::Break`:
            //   - Inside a `while` (innermost lexical enclosing
            //     structured loop in this proto): emit `NextLoop`
            //     so the VM pops handlers + jumps to the loop's
            //     iter-check label. The value attached to `next val`
            //     is discarded — `while` doesn't have an iteration
            //     value to update, matching CRuby.
            //   - Otherwise: keep the block / iteration-driver
            //     semantics (Op::Return from the block frame; the
            //     driver reads the value off the stack).
            if !b.loop_next_jumps.is_empty() {
                // `next` in a while loop ignores the optional value
                // expression — but Ruby still evaluates it for side
                // effects. Emit and Pop to preserve evaluation order
                // without polluting the stack at the jump target.
                if let Some(e) = val {
                    compile_expr(b, e, protos, interner, cc);
                    b.emit(Op::Pop);
                }
                let placeholder = b.emit(Op::NextLoop(0));
                b.loop_next_jumps.last_mut().expect("ICE: just checked").push(placeholder);
            } else {
                match val {
                    Some(e) => compile_expr(b, e, protos, interner, cc),
                    None => { b.emit(Op::LoadNil); }
                }
                b.emit(Op::Return);
            }
            // Sentinel value for stack-balance (unreachable in well-formed code).
            b.emit(Op::LoadNil);
            b.current_span = prev_span;
            return;
        }
        Expr::Break(val) => {
            // Compile the break value (or nil) — same for both forms;
            // it stays on the operand stack as the loop expression's
            // value (for structured `while` break) or as the
            // iteration-driver's return (for block break).
            match val {
                Some(e) => compile_expr(b, e, protos, interner, cc),
                None => { b.emit(Op::LoadNil); }
            }
            if !b.loop_break_jumps.is_empty() {
                // Inside a `while` (the innermost lexical enclosing
                // structured loop in this proto). Emit BreakLoop to
                // unwind handlers + jump; record the placeholder so
                // the `while` codegen patches it to the join label.
                let placeholder = b.emit(Op::BreakLoop(0));
                b.loop_break_jumps.last_mut().expect("ICE: just checked").push(placeholder);
            } else {
                // No enclosing `while` in this proto — fall back to
                // block / iteration-driver break (`Op::Break` flags
                // the surrounding host loop, `Op::Return` pops the
                // block frame so `collection_call_block` reads the
                // value off the stack).
                b.emit(Op::Break);
                b.emit(Op::Return);
            }
            // Sentinel for stack-balance of any unreachable code
            // following this statement (matches Op::Return arm).
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
            // Lambda params are now `Vec<BlockParam>` (post K7), so
            // they go straight into compile_block without rewrapping.
            let (block_proto_idx, param_start, n_params, rest_slot) =
                compile_block(b, params, body, protos, interner, cc);
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot));
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
            // path, once for the exception / loop-transfer path which
            // ends in Op::EndEnsure — that terminator routes to either
            // re-raise the in-flight exception or resume a pending
            // break/next walk depending on `vm.pending_loop_transfer`).
            if let (Some(eb), Some(pe)) = (ensure.as_ref(), pe) {
                b.emit(Op::PopEnsure);
                // Normal path: run ensure body, then jump past handler.
                for stmt in eb { compile_stmt(b, stmt, protos, interner, cc); }
                let je = b.emit(Op::Jump(0));
                // Exception path: PushEnsure target. Exception value is on
                // top of stack (only when entered via exception unwind);
                // for `break`/`next` walking through ensures the stack
                // is NOT pushed-to on entry and `vm.pending_loop_transfer`
                // is set instead. `Op::EndEnsure` at the tail handles
                // both cases — exception → pop + re-raise; transfer →
                // resume walk to the loop target.
                let handler_start = b.pos();
                let off = handler_start as i32 - pe as i32 - 1;
                if let Op::PushEnsure(o) = &mut b.code[pe] { *o = off; }
                for stmt in eb { compile_stmt(b, stmt, protos, interner, cc); }
                b.emit(Op::EndEnsure);
                let end = b.pos();
                b.patch_jump(je, end);
            }
        }
    }
    b.current_span = prev_span;
}

// Many positional args is past clippy's default cap, but every one
// is load-bearing: the proto name + the shape inputs (params,
// required-count, default exprs, body, filename) + the three target
// sinks the compiler mutates (protos vec, interner, call-cache
// counter). Bundling into a builder struct doesn't reduce the
// surface; it just renames it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_proto(
    name: String, params: Vec<String>, body: &[SExpr],
    filename: Rc<str>, protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) -> usize {
    compile_proto_at(name, params, body, filename, protos, interner, cc, vec![])
}

/// Same as `compile_proto` but seeds the new proto's `class_path`
/// with the parent's lexical nesting. Used by the `Expr::Class`
/// arm to thread `module Foo; class Bar; ...; end; end`'s path
/// into the inner body so further nested writes alias correctly.
/// Top-level callers (Runtime::eval, require) keep using the
/// no-arg `compile_proto` shim above.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_proto_at(
    name: String, params: Vec<String>, body: &[SExpr],
    filename: Rc<str>, protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
    class_path: Vec<String>,
) -> usize {
    let n_req = params.len() as u16;
    compile_proto_kind(name, params, n_req, vec![], body, filename, protos, interner, cc, /*is_method=*/false, class_path)
}

/// Same as `compile_proto` but tags the resulting builder as a
/// method body — sets `method_name` / `method_param_count` so
/// `super` knows what to forward. Called by `Expr::Def`'s
/// compile path. Class bodies and the toplevel `<main>` proto
/// stay non-method.
///
/// `default_exprs` is parallel to the *positional* part of `params`:
/// `None` for required slots, `Some(expr)` for optionals. When
/// non-empty, a per-optional prologue (`JumpIfArgGiven(slot, skip)`
/// then the default expression then `StoreLocal(slot)`) is emitted
/// before the body so non-literal defaults work.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_proto_kind(
    name: String, params: Vec<String>, n_required_positional: u16,
    default_exprs: Vec<Option<SExpr>>, body: &[SExpr],
    filename: Rc<str>, protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
    is_method: bool, class_path: Vec<String>,
) -> usize {
    let mut b = ProtoBuilder::new(&params, filename);
    b.class_path = class_path;
    if is_method {
        b.method_name = Some(name.clone());
        b.method_param_count = params.len() as u16;
        b.is_method_body = true;
    }
    // Default-arg prologue. For each optional positional slot:
    // skip to `skip:` if the caller supplied it; otherwise eval
    // the default expression and store into the slot. Earlier
    // positional slots are already bound by frame setup, so a
    // later default may reference an earlier param (`def f(a, b=a+1)`).
    for (i, d) in default_exprs.iter().enumerate() {
        if let Some(def_expr) = d {
            let slot = i as u16;
            let jmp = b.emit(Op::JumpIfArgGiven(slot, 0));
            compile_expr(&mut b, def_expr, protos, interner, cc);
            b.emit(Op::StoreLocal(slot));
            let skip = b.pos();
            b.patch_jump(jmp, skip);
        }
    }
    compile_body(&mut b, body, protos, interner, cc);
    b.emit(Op::Return);
    let idx = protos.len();
    protos.push(b.build(name, params, n_required_positional));
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
        Expr::StrLitBytes(bytes) => Value::new_str_bytes(bytes.clone()),
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

/// Compile a block body into a fresh proto + return its
/// (proto_idx, param_start, n_params, rest_slot) for the
/// caller to encode into the `CreateBlock` op. `rest_slot`
/// is `u16::MAX` when the block has no `*rest` parameter.
pub(crate) fn compile_block(
    parent: &mut ProtoBuilder, block_params: &[BlockParam], body: &[SExpr],
    protos: &mut Vec<Proto>, interner: &mut Interner, cc: &mut u32,
) -> (usize, u16, u16, u16) {
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
        // Blocks are NOT method bodies — `return` inside one
        // unwinds non-locally to the enclosing method
        // (Op::ReturnMethod), not just the block frame.
        is_method_body: false,
        // Fresh `break` target stack for the block. A `break`
        // inside `[1,2,3].each { break }` should signal the
        // iteration driver via the existing `Op::Break` path,
        // NOT jump to an enclosing `while` in the parent proto.
        loop_break_jumps: vec![],
        // Symmetric: `next` inside a block hits the iteration
        // driver (`Op::Return` from the block frame), not an
        // enclosing `while` in the parent.
        loop_next_jumps: vec![],
        // Blocks inherit the enclosing proto's class_path so a
        // bare `class Bar` inside a block running in a class body
        // still aliases under the right `Foo::Bar` key. Real
        // codebases rarely do this; blocks are inherited for
        // consistency, not because we expect it to fire often.
        class_path: parent.class_path.clone(),
        // Blocks get a fresh per-Proto binary-literal pool. The
        // emitted bytecode embeds indices that the runtime
        // resolves through the block's own Proto.
        byte_literals: vec![],
    };
    let param_start = b.n_locals;
    // Slot layout in two phases:
    //   1. Reserve ONE call-interface slot per top-level
    //      BlockParam — contiguous from param_start, so the
    //      invoke_block arg-binding loop fills them in order.
    //   2. After every call-interface slot is reserved, allocate
    //      the destructure inner slots (recursively for nested
    //      destructures). They sit AFTER the call interface so
    //      a Single param following a Destructure doesn't land at
    //      an inner-name index.
    // A "destructure job" pairs the source slot (the anon
    // receiving slot for the top-level destructure, or a parent
    // anon slot for a nested destructure) with the child slots
    // to populate. Order matters: parent jobs run before nested
    // children that read from them, which the post-order
    // allocation below preserves naturally.
    enum Job { Job(u16, Vec<u16>) }
    let mut jobs: Vec<Job> = Vec::new();

    // Recursive helper: allocates inner slots for a Destructure
    // and pushes a Job describing the unpack. For nested
    // destructures, allocates an anon slot to receive the child
    // Array and recurses into it.
    fn alloc_inner(
        b: &mut ProtoBuilder,
        parent_slot: u16,
        inners: &[BlockParam],
        jobs: &mut Vec<Job>,
        depth_tag: &str,
    ) {
        let mut child_slots: Vec<u16> = Vec::with_capacity(inners.len());
        let mut nested: Vec<(u16, &[BlockParam], String)> = Vec::new();
        for (j, inner) in inners.iter().enumerate() {
            match inner {
                BlockParam::Single(name) => {
                    child_slots.push(b.define_local_slot(name));
                }
                BlockParam::Destructure(deeper) => {
                    let anon = format!("{depth_tag}_{j}");
                    let anon_slot = b.define_local_slot(&anon);
                    child_slots.push(anon_slot);
                    nested.push((anon_slot, deeper, anon));
                }
                BlockParam::Rest(_) => {
                    // Rest-in-destructure (`|(a, *b)|`) isn't part
                    // of our subset — Prism would emit it as a
                    // SplatNode inside MultiTargetNode.lefts(),
                    // which the AST translator's `parse_one` does
                    // NOT recognise. If we ever extend support,
                    // the rest slot would go into child_slots
                    // alongside the leading required ones.
                }
            }
        }
        jobs.push(Job::Job(parent_slot, child_slots));
        // Recurse for each nested destructure now that its parent
        // job is queued (parent runs first, so the anon slot is
        // populated before the nested unpack reads it).
        for (anon_slot, deeper, anon_name) in nested {
            alloc_inner(b, anon_slot, deeper, jobs, &anon_name);
        }
    }

    // Phase 1: top-level call-interface slots. Rest params get
    // their own slot but are NOT counted in `n_params` — the
    // call-interface arg loop in invoke_block tops out at
    // n_params, then a separate rest-collector loop fills the
    // rest slot from any overflow args.
    let mut top_destructures: Vec<(u16, &[BlockParam], String)> = Vec::new();
    let mut rest_slot: u16 = u16::MAX;
    let mut n_required: u16 = 0;
    for (i, p) in block_params.iter().enumerate() {
        match p {
            BlockParam::Single(name) => {
                b.define_local_slot(name);
                n_required += 1;
            }
            BlockParam::Destructure(inners) => {
                let anon = format!("__destruct_{i}");
                let anon_slot = b.define_local_slot(&anon);
                top_destructures.push((anon_slot, inners, anon));
                n_required += 1;
            }
            BlockParam::Rest(name) => {
                // Anonymous `|*|` reserves a slot under a synth
                // name so the prologue still has somewhere to put
                // the Array (just unreachable from the body).
                let slot_name = if name.is_empty() { format!("__rest_{i}") } else { name.clone() };
                let s = b.define_local_slot(&slot_name);
                // Only one rest slot per param list — Prism
                // enforces this at parse time; defensive overwrite
                // just keeps the last one if we ever extend.
                rest_slot = s;
            }
        }
    }
    let n_params = n_required;
    // Phase 2: walk every top-level destructure's children.
    for (anon_slot, inners, anon_name) in top_destructures {
        alloc_inner(&mut b, anon_slot, inners, &mut jobs, &anon_name);
    }
    // Compatibility shim: collapse Jobs into the existing
    // (anon_slot, inner_slots) shape the prologue loop walks.
    let destructure_jobs: Vec<(u16, Vec<u16>)> = jobs.into_iter()
        .map(|Job::Job(p, c)| (p, c))
        .collect();

    // Prologue: for each destructure param, read element i from
    // its anonymous slot's Array and store into the named slot.
    // Coerce the source value to an Array via Kernel#Array — that
    // mirrors CRuby's `*` expansion (`Array(nil) == []`,
    // `Array([1,2]) == [1,2]`, `Array(5) == [5]`). Without
    // coercion a non-Array arg would NoMethodError on `[]`.
    if !destructure_jobs.is_empty() {
        let bracket_id = interner.intern("[]");
        for (anon_slot, inner_slots) in &destructure_jobs {
            // Coerce: locals[anon] = Array(locals[anon])
            b.emit(Op::LoadLocal(*anon_slot));
            let cid = *cc as u16; *cc += 1;
            b.emit(Op::CallNoRecv(interner.intern("Array"), 1, cid));
            b.emit(Op::StoreLocal(*anon_slot));
            // Unpack: locals[inner_i] = locals[anon][i]
            for (i, slot) in inner_slots.iter().enumerate() {
                b.emit(Op::LoadLocal(*anon_slot));
                b.emit(Op::LoadConstInt(i as i64));
                let cid = *cc as u16; *cc += 1;
                b.emit(Op::Call(bracket_id, 1, cid));
                b.emit(Op::StoreLocal(*slot));
            }
        }
    }

    compile_body(&mut b, body, protos, interner, cc);
    b.emit(Op::Return);
    // Proto's `params` vec carries the source-visible names. For
    // destructure block params we use the synthesised anonymous
    // name in the call-interface slot; the named inner locals
    // are not part of params (they aren't fed by the caller).
    let proto_params: Vec<String> = block_params.iter().enumerate().filter_map(|(i, p)| match p {
        BlockParam::Single(n) => Some(n.clone()),
        BlockParam::Destructure(_) => Some(format!("__destruct_{i}")),
        // Rest param isn't part of the call-interface params
        // (invoke_block populates it via the rest-collector
        // loop, not the per-arg fill).
        BlockParam::Rest(_) => None,
    }).collect();
    let proto_param_count = proto_params.len();
    let idx = protos.len();
    // Blocks don't use the default-arg prologue (no defaults
    // syntax in our block params), so every slot is required.
    // Propagate the block's slot reservations back to the parent
    // so subsequent outer-scope local allocations (`x = 99` after
    // `f = ->(a,b) { ... }`) don't reuse the block's param /
    // body slots and clobber them on each invocation. The
    // captured Rc is shared, so writes to the block's slots
    // are visible (and DESTRUCTIVE) to anything the parent
    // happens to bind into the same index later.
    let block_n_locals = b.n_locals;
    protos.push(b.build("<block>".into(), proto_params, proto_param_count as u16));
    if parent.n_locals < block_n_locals {
        parent.n_locals = block_n_locals;
    }
    (idx, param_start, n_params, rest_slot)
}
