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
    /// Per-call-site cref chains, flushed into `Proto.const_chains`
    /// on emit. See `Op::LoadConstChain` in bytecode.rs.
    pub(crate) const_chains: Vec<Vec<crate::intern::SymId>>,
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

/// Scope-bounded `current_span` override. Restores the previous
/// span on drop, so early returns from `compile_expr` arms don't
/// need to remember to restore manually. Drop order is enough —
/// any `b.emit(...)` calls inside the scope happen *before* the
/// guard's drop runs, so they pick up the overridden span.
pub(crate) struct SpanGuard<'a> {
    pub(crate) b: &'a mut ProtoBuilder,
    prev: Span,
}

impl<'a> SpanGuard<'a> {
    pub(crate) fn enter(b: &'a mut ProtoBuilder, span: Span) -> Self {
        let prev = b.current_span;
        b.current_span = span;
        Self { b, prev }
    }
}

impl Drop for SpanGuard<'_> {
    fn drop(&mut self) {
        self.b.current_span = self.prev;
    }
}

/// Compile-time intercepts for literal-symbol `Call` arms
/// (no block): `attr_reader` / `attr_writer` / `attr_accessor`
/// and `alias_method`. Returns `true` when an intercept fired
/// (and `b` already holds the emitted ops); the caller should
/// then `return;` to skip the generic `Op::Call` path.
///
/// Each intercept short-circuits only when every relevant arg
/// is a `SymbolLit` — dynamic forms (`attr_accessor(*xs)`,
/// `alias_method(a, b)` with non-symbol args) fall through.
fn try_call_compile_time_intercept(
    b: &mut ProtoBuilder,
    receiver: &Option<Box<SExpr>>,
    name: &str,
    args: &[SExpr],
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) -> bool {
    // attr_reader / attr_writer / attr_accessor
    if receiver.is_none()
        && let Some((do_reader, do_writer)) = crate::ast::attr_reader_writer_flags(name)
        && args.iter().all(|a| matches!(a.node, Expr::SymbolLit(_)))
    {
        for a in args {
            let sym_name = if let Expr::SymbolLit(s) = &a.node { s.clone() } else { unreachable!() };
            let ivar_name = format!("@{}", sym_name);
            if do_reader {
                let body = vec![SExpr { span: a.span, node: Expr::IVarRead(ivar_name.clone()) }];
                let pidx = compile_proto(
                    sym_name.clone(), vec![], &body,
                    b.filename.clone(), protos, interner, cc,
                );
                let nid = interner.intern(&sym_name);
                b.emit(Op::DefMethod(nid, pidx as u32));
            }
            if do_writer {
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
        return true;
    }

    // alias_method :new, :old — both args must be Symbol literals.
    // `Op::AliasMethod`'s VM handler pushes Nil itself, so the
    // compiler must NOT emit a trailing `LoadNil`.
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
        b.emit(Op::AliasMethod(nid, oid));
        return true;
    }

    false
}

/// Compile-time intercepts for `CallWithBlock` arms whose
/// block body becomes the method body: `define_method(:foo) { ... }`
/// and `recv.define_singleton_method(:foo) { ... }`. Returns
/// `true` when an intercept fired.
#[allow(clippy::too_many_arguments)]
fn try_call_with_block_compile_time_intercept(
    b: &mut ProtoBuilder,
    receiver: &Option<Box<SExpr>>,
    name: &str,
    args: &[SExpr],
    block_params: &[BlockParam],
    block_body: &[SExpr],
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) -> bool {
    // define_method(:foo) { |args| ... }
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
        return true;
    }

    // recv.define_singleton_method(:foo) { |args| ... }
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
        return true;
    }

    false
}

/// Compile the body of `Expr::While` — both the pre-condition
/// (`while cond; body; end`) and post-condition (`begin body
/// end while cond`) forms.
///
/// The `loop_break_jumps` / `loop_next_jumps` push/pop pairing
/// is invariant — both stacks must be popped in this function
/// (NOT split across helpers). See #195's R5 risk register.
fn compile_while_arm(
    b: &mut ProtoBuilder,
    cond: &SExpr,
    body: &[SExpr],
    post: bool,
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    b.emit(Op::EnterLoop);
    b.loop_break_jumps.push(vec![]);
    b.loop_next_jumps.push(vec![]);
    let iter_check;
    if post {
        // `begin … end while cond` — body runs first, cond
        // is checked after.
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
        // Pre-condition `while cond; …; end`.
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
    // Patch `next` placeholders to iter_check (re-eval cond);
    // patch `break` placeholders to the join.
    for j in b.loop_next_jumps.pop().expect("ICE: while popped loop_next_jumps without push") {
        b.patch_jump(j, iter_check);
    }
    let join = b.pos();
    for j in b.loop_break_jumps.pop().expect("ICE: while popped loop_break_jumps without push") {
        b.patch_jump(j, join);
    }
    b.emit(Op::ExitLoop);
}

/// Compile the body of `Expr::Def` — `def name(params) ... end`
/// and its receiver-prefixed singleton variants. `defaults` is
/// parallel to `params`: leading `None`s are required, trailing
/// `Some(expr)`s are optionals (the body proto's prologue emits
/// `JumpIfArgGiven + <default> + StoreLocal` per optional).
#[allow(clippy::too_many_arguments)]
fn compile_def_arm(
    b: &mut ProtoBuilder,
    name: &str,
    params: &[String],
    defaults: &[Option<SExpr>],
    rest: &Option<String>,
    kw_params: &[(String, Option<SExpr>)],
    kw_rest: &Option<String>,
    block_param: &Option<String>,
    receiver: &Option<Box<SExpr>>,
    body: &[SExpr],
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    let n_required_positional = defaults.iter().take_while(|d| d.is_none()).count() as u16;
    let mut effective_params: Vec<String> = params.to_vec();
    if let Some(rname) = rest {
        effective_params.push(rname.clone());
    }
    for (kname, _) in kw_params {
        effective_params.push(kname.clone());
    }
    if let Some(krname) = kw_rest {
        let slot_name = if krname.is_empty() { "__kw_rest_anon".to_string() } else { krname.clone() };
        effective_params.push(slot_name);
    }
    if let Some(bname) = block_param {
        effective_params.push(bname.clone());
    }
    let kw_lit_defaults: Vec<Option<Value>> = kw_params.iter().map(|(_, d)| {
        d.as_ref().map(|sx| literal_to_value(&sx.node, interner))
    }).collect();
    let proto_idx = compile_proto_kind(
        name.to_string(), effective_params, n_required_positional, defaults.to_vec(), body,
        b.filename.clone(), protos, interner, cc, /*is_method=*/true,
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
            // `def self.foo` in a class body — install on the
            // surrounding class's `singleton_methods` table.
            b.emit(Op::DefSingletonMethod(name_id, proto_idx as u32));
        }
        Some(recv_expr) => {
            // `def obj.foo` — instance-level singleton; pop the
            // evaluated receiver and install on its eigenclass.
            compile_expr(b, recv_expr, protos, interner, cc);
            b.emit(Op::DefObjectSingletonMethod(name_id, proto_idx as u32));
        }
    }
    b.emit(Op::LoadNil);
}

/// Compile the body of `Expr::Class` — `class Name < Parent ; ... ; end`
/// and `module Name ; ... ; end`. Threads the lexical
/// `class_path` so nested definitions alias under the qualified
/// name (`Foo::Bar::Inner`).
///
/// `qual_id` is `SymId(u32::MAX)` when no prefix applies (top
/// level or already-qualified name) — load-bearing sentinel
/// read by both `DefClass` and the final `StoreConst` alias.
/// See #195's R4 risk register.
#[allow(clippy::too_many_arguments)]
fn compile_class_arm(
    b: &mut ProtoBuilder,
    name: &str,
    superclass: &Option<String>,
    body: &[SExpr],
    is_module: bool,
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    let mut child_path = b.class_path.clone();
    child_path.push(name.to_string());
    let proto_idx = compile_proto_at(
        format!("<class:{}>", name), vec![], body,
        b.filename.clone(), protos, interner, cc, child_path,
    );
    // Push the superclass (or Nil for "default to Object") for
    // DefClass to pop. The parent reference is a const read at
    // the SURROUNDING lexical scope (not the child class's
    // scope).
    if let Some(parent) = superclass {
        if let Some(chain) = build_const_chain(&b.class_path, parent, interner) {
            let idx = b.const_chains.len() as u32;
            b.const_chains.push(chain);
            b.emit(Op::LoadConstChain(idx));
        } else {
            let parent_id = interner.intern(parent);
            b.emit(Op::LoadConst(parent_id));
        }
    } else {
        b.emit(Op::LoadNil);
    }
    let name_id = interner.intern(name);
    // `SymId(u32::MAX)` sentinel = "no prefix" (top level or
    // already-qualified). Drives both DefClass's qual-name slot
    // AND the StoreConst alias below. Do NOT replace with
    // Option<SymId> — the bytecode op fields are SymId-typed
    // and the runtime reader compares `qual_id.0 != u32::MAX`.
    let qual_id = if !b.class_path.is_empty() && !name.contains("::") {
        let prefixed = format!("{}::{}", b.class_path.join("::"), name);
        interner.intern(&prefixed)
    } else {
        crate::intern::SymId(u32::MAX)
    };
    if is_module {
        b.emit(Op::DefModule(name_id, proto_idx as u32, qual_id));
    } else {
        b.emit(Op::DefClass(name_id, proto_idx as u32, qual_id));
    }
    // Alias under the prefixed path so `Foo::Bar.new` from
    // outside resolves. Skipped at top level. Idempotent on
    // re-open. DefClass's Return arm pushes the freshly-created
    // / re-opened class; Dup leaves it for the expression value
    // while StoreConst consumes the dup'd copy.
    if qual_id.0 != u32::MAX {
        b.emit(Op::Dup);
        b.emit(Op::StoreConst(qual_id));
    }
}

/// Compile the body of `Expr::MultiWrite` — Ruby's
/// `a, b, *r, c = expr` parallel assignment. Compiles the
/// RHS once, then `Dup`s the value across per-target
/// `[]` / `__mw_splat` / `__mw_post` calls, storing the
/// result into each target via the inner `emit_store`
/// closure.
///
/// CRuby semantics:
///   - Without a splat: extra targets get `nil`, extra source
///     elements are silently dropped (Array#[] returns nil
///     past the end).
///   - With a splat: pre-targets claim from the front; the
///     splat slice is computed via `__mw_splat(pre, post)`
///     (always returns a fresh Array, never nil); post-targets
///     use `__mw_post(j, pre, post)` to implement the
///     "pre wins" rule.
///
/// The source Array remains on the stack as the expression's
/// result (matches CRuby).
fn compile_multiwrite_arm(
    b: &mut ProtoBuilder,
    targets: &[crate::ast::MultiWriteTarget],
    value: &SExpr,
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
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
                emit_method_call(b, bracket_id, 1, true, false, cc);
                emit_store(b, interner, target);
            }
        }
        Some(s) => {
            let post = targets.len() - s - 1;
            let post_id = interner.intern("__mw_post");
            for (i, target) in targets.iter().enumerate().take(s) {
                b.emit(Op::Dup);
                b.emit(Op::LoadConstInt(i as i64));
                emit_method_call(b, bracket_id, 1, true, false, cc);
                emit_store(b, interner, target);
            }
            b.emit(Op::Dup);
            b.emit(Op::LoadConstInt(s as i64));
            b.emit(Op::LoadConstInt(post as i64));
            emit_method_call(b, splat_id, 2, true, false, cc);
            emit_store(b, interner, &targets[s]);
            for j in 0..post {
                b.emit(Op::Dup);
                b.emit(Op::LoadConstInt(j as i64));
                b.emit(Op::LoadConstInt(s as i64));
                b.emit(Op::LoadConstInt(post as i64));
                emit_method_call(b, post_id, 3, true, false, cc);
                emit_store(b, interner, &targets[s + 1 + j]);
            }
        }
    }
}

/// Compile the body of `Expr::Begin` — the `begin / rescue /
/// ensure / end` arm. Pure mechanical extraction from
/// `compile_expr`'s match arm: rescue clauses push in REVERSE
/// source order (so the unwinder, which is LIFO, tries the
/// first source clause first), multi-class clauses share a
/// single handler body, and the optional ensure layer compiles
/// the ensure body twice (normal-path inline + exception-path
/// terminated by `Op::EndEnsure`).
///
/// The `groups: Vec<Vec<usize>>` + LIFO push order has subtle
/// correctness invariants — see #195's R3 risk register. Do
/// NOT refactor the iteration order; only the surrounding
/// scope changed.
fn compile_begin_arm(
    b: &mut ProtoBuilder,
    body: &[SExpr],
    rescue: &[crate::ast::RescueClause],
    ensure: &Option<Vec<SExpr>>,
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    let pe = ensure.as_ref().map(|_| b.emit(Op::PushEnsure(0)));

    if rescue.is_empty() {
        compile_body(b, body, protos, interner, cc);
    } else {
        let stderr_sym = interner.intern("StandardError");
        // Per-clause groups of PushRescue placeholders. Same
        // outer iteration order as `rescue.iter().rev()` (i.e.
        // last clause first). All placeholders in a single
        // inner Vec patch to the SAME handler body.
        let mut groups: Vec<Vec<usize>> = Vec::with_capacity(rescue.len());
        for rc in rescue.iter().rev() {
            let (slot, bind) = match &rc.var {
                Some(name) => (b.local_slot(name), 1u8),
                None => (0u16, 0u8),
            };
            let filter_syms: Vec<crate::intern::SymId> = if rc.classes.is_empty() {
                vec![stderr_sym]
            } else {
                rc.classes.iter().rev().map(|n| interner.intern(n)).collect()
            };
            let mut group = Vec::with_capacity(filter_syms.len());
            for sym in filter_syms {
                group.push(b.emit(Op::PushRescue(0, slot, bind, sym)));
            }
            groups.push(group);
        }
        compile_body(b, body, protos, interner, cc);
        let total: usize = groups.iter().map(|g| g.len()).sum();
        for _ in 0..total { b.emit(Op::PopRescue); }
        let mut jump_to_end: Vec<usize> = Vec::with_capacity(rescue.len() + 1);
        jump_to_end.push(b.emit(Op::Jump(0)));
        for (i, rc) in rescue.iter().rev().enumerate() {
            let group = &groups[i];
            let handler_start = b.pos();
            for &placeholder in group {
                let off = handler_start as i32 - placeholder as i32 - 1;
                if let Op::PushRescue(o, _, _, _) = &mut b.code[placeholder] {
                    *o = off;
                }
            }
            compile_body(b, &rc.body, protos, interner, cc);
            jump_to_end.push(b.emit(Op::Jump(0)));
        }
        let end = b.pos();
        for j in jump_to_end { b.patch_jump(j, end); }
    }

    // Ensure layer (compile body twice — once inline for the
    // normal path, once for the exception / loop-transfer path
    // which ends in `Op::EndEnsure`). The terminator routes to
    // either re-raise the in-flight exception or resume a
    // pending break/next walk depending on
    // `vm.pending_loop_transfer`.
    if let (Some(eb), Some(pe)) = (ensure.as_ref(), pe) {
        b.emit(Op::PopEnsure);
        for stmt in eb { compile_stmt(b, stmt, protos, interner, cc); }
        let je = b.emit(Op::Jump(0));
        let handler_start = b.pos();
        let off = handler_start as i32 - pe as i32 - 1;
        if let Op::PushEnsure(o) = &mut b.code[pe] { *o = off; }
        for stmt in eb { compile_stmt(b, stmt, protos, interner, cc); }
        b.emit(Op::EndEnsure);
        let end = b.pos();
        b.patch_jump(je, end);
    }
}

/// Compile the body of `Expr::Call` — the no-block call arm.
/// Tries the literal-symbol intercepts (`attr_*` / `alias_method`)
/// and the special forms (`__seq__`, `raise`, BinOp fusion);
/// falls through to a generic `emit_method_call` when none
/// matched.
fn compile_call_arm(
    b: &mut ProtoBuilder,
    receiver: &Option<Box<SExpr>>,
    name: &str,
    args: &[SExpr],
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    if receiver.is_none() && name == "__seq__" {
        compile_body(b, args, protos, interner, cc);
        return;
    }
    if try_call_compile_time_intercept(b, receiver, name, args, protos, interner, cc) {
        return;
    }
    // `raise [class, msg, *more]` — three call shapes that all
    // funnel through `Op::Raise + LoadNil`. The runtime helper
    // `normalize_exception` covers String / Exception instance /
    // Exception class inputs.
    if receiver.is_none() && name == "raise" {
        match args.len() {
            0 => { b.emit(Op::LoadNil); }
            1 => { compile_expr(b, &args[0], protos, interner, cc); }
            _ => {
                // `raise SomeClass, "msg", *more` →
                // `SomeClass.new("msg", *more)` so initialize fires.
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
        return;
    }
    // `<expr> <op> <int_literal>` fusion → BinOpInt single op.
    if let (Some(r), 1, Some(kind)) = (receiver.as_ref(), args.len(), BinOpKind::from_op_name(name)) {
        compile_expr(b, r, protos, interner, cc);
        if let Expr::IntLit(rhs) = &args[0].node {
            b.emit(Op::BinOpInt(kind, *rhs));
        } else {
            compile_expr(b, &args[0], protos, interner, cc);
            b.emit(Op::BinOp(kind));
        }
        return;
    }
    // Generic dispatch.
    let name_id = interner.intern(name);
    let has_recv = receiver.is_some();
    if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
    for a in args { compile_expr(b, a, protos, interner, cc); }
    let argc = args.len() as u8;
    emit_method_call(b, name_id, argc, has_recv, false, cc);
}

/// Allocate a fresh inline-cache id and emit the appropriate
/// `Op::Call*` variant for a method dispatch. Centralises the
/// 4-way `(has_recv, has_block)` matrix that previously lived
/// as if/else pairs inline in `compile_expr`.
fn emit_method_call(
    b: &mut ProtoBuilder,
    name: crate::intern::SymId,
    argc: u8,
    has_recv: bool,
    has_block: bool,
    cc: &mut u32,
) {
    let cid = *cc as u16;
    *cc += 1;
    let op = match (has_recv, has_block) {
        (true, false) => Op::Call(name, argc, cid),
        (false, false) => Op::CallNoRecv(name, argc, cid),
        (true, true) => Op::CallBlock(name, argc, cid),
        (false, true) => Op::CallNoRecvBlock(name, argc, cid),
    };
    b.emit(op);
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
            const_chains: vec![],
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
    pub(crate) fn build(self, name: String, params: Vec<String>, n_required_positional: u16, lexical_scope: Vec<crate::intern::SymId>) -> Proto {
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
            // Non-block protos (methods, class bodies, toplevel)
            // never need slot resetting; only `compile_block` flips
            // this via the dedicated setter on the resulting Proto.
            block_body_local_start: u16::MAX,
            byte_literals: self.byte_literals,
            const_chains: self.const_chains,
            lexical_scope,
        }
    }
}

/// Build the qualified-SymId chain used by `Module.nesting` from a
/// compile-time `class_path`. Innermost-first, matching CRuby's
/// output: for `class_path = ["A", "B", "C"]` the result is
/// `[sym("A::B::C"), sym("A::B"), sym("A")]`. The top-level proto
/// (empty class_path) returns an empty vec, which `Module.nesting`
/// reports as `[]` — also matching CRuby.
fn build_lexical_scope(
    class_path: &[String],
    interner: &mut crate::intern::Interner,
) -> Vec<crate::intern::SymId> {
    (0..class_path.len())
        .rev()
        .map(|i| interner.intern(&class_path[..=i].join("::")))
        .collect()
}

/// Build the cref chain a bare-name constant read should walk at
/// runtime, given the lexical `class_path` at emit time. Returns
/// `None` when the chain is just `[bare_sym]` (no inner scopes) —
/// the caller should fall back to a plain `LoadConst(bare_sym)`
/// in that case (saves one indirection through `Proto.const_chains`).
///
/// Chain order is innermost-scope first, matching CRuby's "cref
/// walks from the inside out and falls through to the top level":
/// for class_path `["Foo", "Bar"]` and bare `"X"` it produces
/// `[sym("Foo::Bar::X"), sym("Foo::X"), sym("X")]`. First hit in
/// `Vm.classes` or `Vm.constants` wins.
fn build_const_chain(
    class_path: &[String],
    bare: &str,
    interner: &mut crate::intern::Interner,
) -> Option<Vec<crate::intern::SymId>> {
    if class_path.is_empty() || bare.contains("::") {
        return None;
    }
    let mut chain: Vec<crate::intern::SymId> =
        Vec::with_capacity(class_path.len() + 1);
    for i in (0..class_path.len()).rev() {
        let prefix = class_path[..=i].join("::");
        chain.push(interner.intern(&format!("{}::{}", prefix, bare)));
    }
    chain.push(interner.intern(bare));
    Some(chain)
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
    let mut _span_guard = SpanGuard::enter(b, e.span);
    let b = &mut *_span_guard.b;
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
        #[cfg(feature = "bignum")]
        Expr::BigIntLit(decimal) => { let id = interner.intern(decimal); b.emit(Op::LoadBigInt(id)); }
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
                            emit_method_call(b, to_s, 0, true, false, cc);
                        }
                    }
                    if idx > 0 {
                        b.emit(Op::BinOp(BinOpKind::Add));
                    }
                }
            }
        }
        // `/pre #{x} post/` — same build sequence as InterpolatedStr,
        // followed by `CompileRegex` which pops the assembled String
        // and pushes a `Value::Regex`. Empty `/#{}/` (parts.is_empty())
        // builds an empty pattern and then a regex that matches
        // everywhere; CRuby behaves the same.
        #[cfg(feature = "regex")]
        Expr::InterpolatedRegex(parts) => {
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
                            emit_method_call(b, to_s, 0, true, false, cc);
                        }
                    }
                    if idx > 0 {
                        b.emit(Op::BinOp(BinOpKind::Add));
                    }
                }
            }
            b.emit(Op::CompileRegex);
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
                            return;
                        }
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            b.emit(Op::Dup);
            b.emit(Op::StoreIvar(id));
        }
        Expr::SourceFile => {
            // Materialise the current proto's filename as a
            // string literal — CRuby's `__FILE__` reports the
            // file the literal lexically appears in. `b.filename`
            // is `Rc<str>` from the surrounding compile call;
            // intern + LoadConstStr it.
            let fname: String = b.filename.to_string();
            let id = interner.intern(&fname);
            b.emit(Op::LoadConstStr(id));
        }
        Expr::SourceLine(n) => {
            b.emit(Op::LoadConstInt(*n));
        }
        Expr::CvarRead(name) => {
            let id = interner.intern(name);
            b.emit(Op::LoadCvar(id));
        }
        Expr::CvarWrite(name, val) => {
            compile_expr(b, val, protos, interner, cc);
            let id = interner.intern(name);
            // Mirror IVarWrite's "leave value on stack" shape so
            // `(@@foo = 1)` is a usable expression — same as
            // CRuby (assignment expressions return their RHS).
            b.emit(Op::Dup);
            b.emit(Op::StoreCvar(id));
        }
        Expr::ConstRead(name) => {
            // Inside a non-empty class/module scope, emit a cref-
            // walking lookup so `Bar` inside `module Foo; ... end`
            // resolves to `Foo::Bar` first and falls back through
            // outer scopes to the bare top-level name. Top-level
            // reads stay on the plain `LoadConst` path.
            if let Some(chain) = build_const_chain(&b.class_path, name, interner) {
                let idx = b.const_chains.len() as u32;
                b.const_chains.push(chain);
                b.emit(Op::LoadConstChain(idx));
            } else {
                let id = interner.intern(name);
                b.emit(Op::LoadConst(id));
            }
        }
        Expr::ConstReadOrNil(name) => {
            if let Some(chain) = build_const_chain(&b.class_path, name, interner) {
                let idx = b.const_chains.len() as u32;
                b.const_chains.push(chain);
                b.emit(Op::LoadConstChainOrNil(idx));
            } else {
                let id = interner.intern(name);
                b.emit(Op::LoadConstOrNil(id));
            }
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
            compile_multiwrite_arm(b, targets, value, protos, interner, cc);
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
            compile_while_arm(b, cond, body, *post, protos, interner, cc);
        }
        Expr::Call { receiver, name, args } => {
            compile_call_arm(b, receiver, name, args, protos, interner, cc);
        }
        Expr::Def { name, params, defaults, rest, kw_params, kw_rest, block_param, receiver, body } => {
            compile_def_arm(
                b, name, params, defaults, rest, kw_params, kw_rest, block_param, receiver, body,
                protos, interner, cc,
            );
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
        Expr::SuperApply(args_expr) => {
            // `super(*args)` — assemble the args Array and let
            // `Op::ApplySuper` pop + drain it. Mirror of the
            // `Expr::Apply` shape used by regular splat-call
            // dispatch. Method-name resolution is the same as
            // direct-form `Expr::Super`.
            let mname = b.method_name.clone()
                .unwrap_or_else(|| "<super-outside-method>".to_string());
            let name_id = interner.intern(&mname);
            compile_expr(b, args_expr, protos, interner, cc);
            b.emit(Op::ApplySuper(name_id));
        }
        Expr::Class { name, superclass, body, is_module } => {
            compile_class_arm(b, name, superclass, body, *is_module, protos, interner, cc);
        }
        Expr::AliasSingletonMethod(new_name, old_name) => {
            // Counterpart to the existing alias_method compile-
            // time intercept (which emits Op::AliasMethod against
            // class_stack.last().methods). AST emits this variant
            // only inside `class << X` body, where alias must
            // target the singleton-method table instead.
            let nid = interner.intern(new_name);
            let oid = interner.intern(old_name);
            b.emit(Op::AliasSingletonMethod(nid, oid));
            // Like Op::AliasMethod, the handler pushes Nil itself;
            // no trailing LoadNil here.
        }
        Expr::SingletonChainPrepend(src) => {
            // Evaluate the module/class argument (`Module.new { ... }`,
            // a constant lookup, anything that lands a Value::Class
            // on the stack), then emit the op that pushes it onto
            // the surrounding class's `singleton_prepends`. Handler
            // pushes Nil for the expression result.
            compile_expr(b, src, protos, interner, cc);
            b.emit(Op::SingletonChainPrepend);
        }
        Expr::PushClassVisibilityPublic => {
            b.emit(Op::PushClassVisibilityPublic);
        }
        Expr::PopClassVisibility => {
            b.emit(Op::PopClassVisibility);
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
            // Compile-time intercepts for literal-symbol arms whose
            // block body becomes the method body:
            // `define_method(:foo) { ... }` and
            // `recv.define_singleton_method(:foo) { ... }`. Only the
            // literal-Symbol form is intercepted; dynamic forms
            // fall through to the generic CallBlock emit below.
            // See `try_call_with_block_compile_time_intercept` for
            // the per-intercept emit shape.
            if try_call_with_block_compile_time_intercept(
                b, receiver, name, args, block_params, block_body, protos, interner, cc,
            ) {
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
            emit_method_call(b, name_id, argc, has_recv, true, cc);
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
            emit_method_call(b, name_id, argc, has_recv, true, cc);
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
            compile_begin_arm(b, body, rescue, ensure, protos, interner, cc);
        }
    }
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
    let lex = build_lexical_scope(&b.class_path, interner);
    let idx = protos.len();
    protos.push(b.build(name, params, n_required_positional, lex));
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
        // Blocks get a fresh per-Proto const-chain pool too. The
        // ChainOrNil / Chain ops carry indices that resolve
        // through this Proto's table, not the parent's.
        const_chains: vec![],
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

    // Snapshot of `n_locals` JUST before the body compiles. Every
    // local allocated past this point is body-introduced (the
    // block's `x = ...` first-assignment shape) and gets reset to
    // `Nil` per invocation by invoke_block, matching CRuby's
    // "block-locals are fresh each call" semantics. Outer-scope
    // variables (slot index < parent.n_locals at compile time)
    // and the block's own params / destructure-inner slots
    // (allocated in the param-binding phase above) keep their
    // values across invocations.
    let body_local_start = b.n_locals;
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
    let lex = build_lexical_scope(&b.class_path, interner);
    protos.push(b.build("<block>".into(), proto_params, proto_param_count as u16, lex));
    // Stamp the body-local-reset range on the just-built block
    // Proto. `build()` defaults this to `u16::MAX` (no reset)
    // because that's correct for every non-block builder; the
    // block path overrides it here. Only meaningful when there
    // *are* body-introduced slots; if the body assigned no new
    // locals (`body_local_start == block_n_locals`) the reset
    // range is empty and the runtime loop is a noop, so we don't
    // need to special-case it.
    protos.last_mut().expect("ICE: just pushed").block_body_local_start = body_local_start;
    if parent.n_locals < block_n_locals {
        parent.n_locals = block_n_locals;
    }
    (idx, param_start, n_params, rest_slot)
}
