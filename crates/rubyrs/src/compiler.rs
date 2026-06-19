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
    /// Slot index of the rest (splat) parameter, when the
    /// surrounding method declares one (`def m(*)`,
    /// `def m(*args)`). `None` otherwise. Used by bare `super`
    /// (no parens) emission: when the method has ONLY a rest
    /// param (no pre/post / kw / block), CRuby's "forward all
    /// args unchanged" idiom requires splatting the rest array
    /// back out, not passing it as a single Array argument. We
    /// emit `LoadLocal(slot); ApplySuper(name)` for that case.
    /// Hit by `def initialize(*); super; end` in vendored
    /// rack-protection middlewares (HostAuthorization,
    /// EscapedParams) layering setup on Base#initialize(app,
    /// options = {}).
    pub(crate) method_rest_slot: Option<u16>,
    /// Slot of the `&block` parameter, when the method declares one.
    /// Bare `super` must forward it AS A BLOCK (not as a positional
    /// arg) — the old "LoadLocal each slot 0..count" forwarding passed
    /// it positionally, so `def m(a, &b); super; end` over-counted args.
    pub(crate) method_block_slot: Option<u16>,
    /// Count of POSITIONAL parameter slots (required + optional +
    /// post-rest), EXCLUDING the rest / kw / kw-rest / block slots.
    /// Bare `super` forwards exactly these as positionals (splatting
    /// the rest in the middle when present).
    pub(crate) method_n_positional: u16,
    /// Count of post-rest required positionals (`def m(*a, b, c)` → 2).
    /// Needed so bare `super` forwards `[pre…, *rest, post…]` in order.
    pub(crate) method_n_post_rest: u16,
    /// True when the method declares keyword params or a `**kwrest`.
    /// Bare `super` forwards named kwargs by reconstructing a trailing
    /// Hash from `method_kw_params` (see below). A `**kwrest` slot
    /// (`method_kw_rest_slot`) still falls back to the legacy slot-dump
    /// — merging the rest Hash isn't modelled yet.
    pub(crate) method_has_kw: bool,
    /// `(name, slot)` for each declared keyword parameter, in order.
    /// Bare `super` rebuilds `{ name: <slot value>, … }` so the callee
    /// binds them as KEYWORDS rather than positionals — public_suffix's
    /// `Wildcard#initialize(value:, length:, private:); super; end`.
    pub(crate) method_kw_params: Vec<(String, u16)>,
    /// Slot of the `**kwrest` parameter when the method declares one.
    /// Bare-`super` kwarg forwarding bails to the legacy path when this
    /// is `Some` (rest-Hash merge is a follow-up).
    pub(crate) method_kw_rest_slot: Option<u16>,
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
    /// Parallel `redo` placeholder stack for `while`/`until` loops.
    /// `redo` re-runs the loop BODY without re-checking the condition,
    /// so these are patched to the body-start label (vs `next`'s
    /// iter-check). Reuses `Op::NextLoop` (a loop-transfer that just
    /// jumps to a target with the same handler-unwind) patched to a
    /// different target. Empty outside a `while`; a `redo` then targets
    /// `block_redo_target` (block body) or raises.
    pub(crate) loop_redo_jumps: Vec<Vec<usize>>,
    /// Body-start offset for a BLOCK proto, so a bare `redo` inside a
    /// block (`loop do … redo … end`, `each { … redo … }`) re-runs the
    /// block body in the same frame (an intra-proto `Op::Jump`). `None`
    /// in non-block protos and until the block's body begins compiling.
    pub(crate) block_redo_target: Option<usize>,
    /// Stack of in-progress `begin / rescue` blocks' "begin top"
    /// positions, pushed while compiling a rescue clause's body
    /// so that `Expr::Retry` inside the body jumps backwards to
    /// re-run the begin block (re-registering rescue handlers
    /// via the PushRescue ops on each retry). Each entry is the
    /// bytecode offset AFTER PushEnsure but BEFORE the first
    /// PushRescue — re-entering at that point re-pushes the
    /// rescue handlers without double-pushing the ensure layer.
    /// Empty outside any rescue clause body; `Expr::Retry` with
    /// an empty stack emits a runtime raise instead. (TRY_RUNS
    /// pass-10 layer #9.)
    pub(crate) retry_targets: Vec<usize>,
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
    // Legacy `attr :name, true` (1.8 accessor form): single
    // Symbol arg followed by a literal `true` / `false`. Treated
    // as reader + writer (when `true`) or reader only (when
    // `false`). CRuby 3.4 still accepts this with a warning; the
    // sinatra-4 load chain doesn't hit this branch but rack-4
    // gems in the wild do. Intercept it BEFORE the all-symbols
    // gate below so the BoolLit arg doesn't push it through to
    // the runtime dispatch path (which would NoMethodError).
    // (TRY_RUNS pass-10 layer #10.)
    if receiver.is_none()
        && name == "attr"
        && args.len() == 2
        && let Expr::SymbolLit(sym_name) = &args[0].node
        && let Expr::BoolLit(accessor) = &args[1].node
    {
        let do_reader = true;
        let do_writer = *accessor;
        let sym_name = sym_name.clone();
        let ivar_name = format!("@{}", sym_name);
        if do_reader {
            let body = vec![SExpr { span: args[0].span, node: Expr::IVarRead(ivar_name.clone()) }];
            let pidx = compile_proto(
                sym_name.clone(), vec![], &body,
                b.filename.clone(), protos, interner, cc,
            );
            let nid = interner.intern(&sym_name);
            b.emit(Op::DefMethod(nid, pidx as u32));
        }
        if do_writer {
            let setter_name = format!("{sym_name}=");
            let val_read = SExpr { span: args[0].span, node: Expr::LVarRead("val".into()) };
            let body = vec![SExpr {
                span: args[0].span,
                node: Expr::IVarWrite(ivar_name.clone(), Box::new(val_read)),
            }];
            let pidx = compile_proto(
                setter_name.clone(), vec!["val".into()], &body,
                b.filename.clone(), protos, interner, cc,
            );
            let nid = interner.intern(&setter_name);
            b.emit(Op::DefMethod(nid, pidx as u32));
        }
        b.emit(Op::LoadNil);
        return true;
    }

    // attr_reader / attr_writer / attr_accessor / attr (legacy
    // reader form — `attr :a`, `attr :a, :b`).
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
        let (block_proto_idx, param_start, n_params, rest_slot, kw_rest_slot) =
            compile_block(b, block_params, block_body, protos, interner, cc);
        b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot, kw_rest_slot));
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
        let (block_proto_idx, param_start, n_params, rest_slot, kw_rest_slot) =
            compile_block(b, block_params, block_body, protos, interner, cc);
        b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot, kw_rest_slot));
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
    b.loop_redo_jumps.push(vec![]);
    let iter_check;
    // Body-start label: `redo` re-runs the body without re-checking the
    // condition, so it targets here (vs `next`'s iter_check).
    let redo_target;
    if post {
        // `begin … end while cond` — body runs first, cond
        // is checked after.
        let body_start = b.pos();
        redo_target = body_start;
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
        redo_target = b.pos();
        compile_body(b, body, protos, interner, cc);
        b.emit(Op::Pop);
        let j = b.emit(Op::Jump(0));
        b.patch_jump(j, start);
        let exit_normal = b.pos();
        b.patch_jump(jf, exit_normal);
        b.emit(Op::LoadNil);
    }
    // Patch `redo` placeholders to the body-start (re-run body without
    // re-checking the condition); `next` to iter_check; `break` to join.
    for j in b.loop_redo_jumps.pop().expect("ICE: while popped loop_redo_jumps without push") {
        b.patch_jump(j, redo_target);
    }
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
    n_required_post: u16,
    kw_params: &[(String, Option<SExpr>)],
    kw_rest: &Option<String>,
    block_param: &Option<String>,
    receiver: &Option<Box<SExpr>>,
    body: &[SExpr],
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    // `defaults` is laid out as `[pre_rest_required..., optionals...,
    // post_rest_required...]` — both required runs carry `None`, only
    // the middle optionals carry `Some(default_expr)`. So
    // `n_pre_rest = total - n_optional - n_required_post`. Counting the
    // leading-None run wouldn't distinguish the post-required tail when
    // there are no optionals, so we derive both required counts from
    // `defaults.len()` and `n_required_post` instead.
    let n_optional = defaults.iter().filter(|d| d.is_some()).count() as u16;
    let n_required_positional = (defaults.len() as u16)
        .saturating_sub(n_optional)
        .saturating_sub(n_required_post);
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
    // Split kwarg defaults into literal (binder fast path) and
    // computed (prologue-emitted) buckets. For literals, store
    // the Value in `kw_param_defaults[i]`; for non-literals
    // (`ConstRead`, method-chain, prior-param ref, ...), set
    // `kw_has_computed_default[i] = true`, leave the literal
    // entry as `None`, and remember the SExpr so the prologue
    // can emit the eval at the right kw slot index.
    let kw_lit_defaults: Vec<Option<Value>> = kw_params.iter().map(|(_, d)| {
        d.as_ref().and_then(|sx| {
            if expr_is_compile_time_literal(&sx.node) {
                Some(literal_to_value(&sx.node, interner))
            } else {
                None
            }
        })
    }).collect();
    let kw_has_computed: Vec<bool> = kw_params.iter().map(|(_, d)| {
        d.as_ref()
            .map(|sx| !expr_is_compile_time_literal(&sx.node))
            .unwrap_or(false)
    }).collect();
    // Position of the rest slot inside effective_params: after
    // pre-rest required positionals + optional defaults.
    // Only set when there IS a rest param (named or anonymous).
    let rest_slot_for_super: Option<u16> = rest.as_ref().map(|_| {
        (n_required_positional as usize + n_optional as usize) as u16
    });
    // Build the kw prologue triples (kw_idx, slot, expr) the
    // compile_proto_kind callee uses to emit the per-kwarg
    // computed-default prologue. `kw_start` mirrors the dispatch
    // binder's layout: after positionals, optionals, post-required,
    // and the rest slot (if any).
    let kw_start: u16 = n_required_positional
        + n_optional
        + n_required_post
        + if rest.is_some() { 1 } else { 0 };
    let kw_computed_prologue: Vec<(u16, u16, SExpr)> = kw_params
        .iter()
        .enumerate()
        .filter_map(|(i, (_, d))| {
            let sx = d.as_ref()?;
            if expr_is_compile_time_literal(&sx.node) {
                None
            } else {
                Some((i as u16, kw_start + i as u16, sx.clone()))
            }
        })
        .collect();
    // Hard cap: `Frame::kw_given_mask` is a `u64`, so kwarg
    // indices ≥64 with computed defaults can't be marked as
    // "caller-supplied" by the binder (`1u64 << 64`
    // overflows). The matching `Op::JumpIfKwArgGiven`
    // handler in vm/step.rs also guards `kw_idx < 64`, so a
    // method with a computed-default kwarg at index ≥64
    // would SILENTLY re-run the default eval every call and
    // overwrite the caller-supplied value in the slot.
    // Surface a SyntaxError at compile time instead — no
    // real-world signature comes anywhere near 64 kwargs, so
    // tripping this is a programmer error worth refusing.
    // The cap mirrors the documented 64-cap on the Op /
    // Frame fields.
    if let Some(last) = kw_computed_prologue.last()
        && last.0 >= 64
    {
        // Compile contexts use `ctx.errors`; the SExpr
        // builder accumulates AST errors via a parallel
        // mechanism on `b`'s parent. Use a plain panic-ish
        // String error pushed through the existing build
        // diagnostic surface (`b.filename` carries the
        // source filename). Routing through compile-time
        // diagnostics keeps the cap visible at the def site
        // rather than failing mysteriously at call time.
        // (Limitation: we don't have a TranslationCtx here;
        // emit via the `errors` global the build harness
        // surfaces.)
        panic!(
            "rubyrs: method `{}` has {} kwargs with computed defaults; \
             the per-Frame kw_given_mask u64 caps at 64 (see vm.rs \
             Frame::kw_given_mask). Reduce computed-default kwargs \
             or widen the mask first.",
            name,
            last.0 + 1,
        );
    }
    // Bare-`super` kwarg forwarding layout: each keyword param's slot
    // is `kw_start + i`; the `**kwrest` slot (if any) follows them.
    let method_kw_params: Vec<(String, u16)> = kw_params
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.clone(), kw_start + i as u16))
        .collect();
    let method_kw_rest_slot: Option<u16> =
        kw_rest.as_ref().map(|_| kw_start + kw_params.len() as u16);
    let proto_idx = compile_proto_kind(
        name.to_string(), effective_params, n_required_positional, defaults.to_vec(), body,
        b.filename.clone(), protos, interner, cc, /*is_method=*/true,
        b.class_path.clone(),
        rest_slot_for_super,
        n_required_post,
        /*has_kw=*/ !kw_params.is_empty() || kw_rest.is_some(),
        /*has_block=*/ block_param.is_some(),
        kw_computed_prologue,
        method_kw_params,
        method_kw_rest_slot,
    );
    if let Some(rname) = rest {
        protos[proto_idx].rest_param = Some(rname.clone());
    }
    protos[proto_idx].n_required_post = n_required_post;
    protos[proto_idx].kw_param_defaults = kw_lit_defaults;
    protos[proto_idx].kw_has_computed_default = kw_has_computed;
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
    // The Def* op already leaves the method-name Symbol on the stack as
    // the expression value (`def foo` → `:foo`), so no trailing LoadNil.
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
    superclass: &Option<Box<SExpr>>,
    body: &[SExpr],
    is_module: bool,
    absolute: bool,
    protos: &mut Vec<Proto>,
    interner: &mut Interner,
    cc: &mut u32,
) {
    // An ABSOLUTE path (`class ::Foo` / `module ::Bar`) defines at top
    // level, ignoring the enclosing lexical scope — so the body's
    // class_path starts fresh at the name, NOT under `b.class_path`,
    // and the qualified-name slot below is forced to the no-prefix
    // sentinel.
    let mut child_path = if absolute { Vec::new() } else { b.class_path.clone() };
    child_path.push(name.to_string());
    let proto_idx = compile_proto_at(
        format!("<class:{}>", name), vec![], body,
        b.filename.clone(), protos, interner, cc, child_path,
    );
    // Push the superclass (or Nil for "default to Object") for
    // DefClass to pop. The parent expression is evaluated at the
    // SURROUNDING lexical scope (not the child class's scope) —
    // hence we compile it BEFORE pushing the new class-body
    // proto. Const-shaped parents (the most common case:
    // `class Sub < Const` or `class Sub < ::Foo::Bar`) get
    // single-op LoadConst / LoadConstChain via the fast path
    // below; arbitrary expressions (`class Sub < local_var` or
    // `class Sub < DelegateClass(Hash)`) route through the
    // generic compiler walker.
    if let Some(parent_expr) = superclass {
        compile_expr(b, parent_expr, protos, interner, cc);
    } else {
        b.emit(Op::LoadNil);
    }
    let name_id = interner.intern(name);
    // `SymId(u32::MAX)` sentinel = "no prefix" (top level or
    // already-qualified). Drives both DefClass's qual-name slot
    // AND the StoreConst alias below. Do NOT replace with
    // Option<SymId> — the bytecode op fields are SymId-typed
    // and the runtime reader compares `qual_id.0 != u32::MAX`.
    let qual_id = if !absolute && !b.class_path.is_empty() && !name.contains("::") {
        let prefixed = format!("{}::{}", b.class_path.join("::"), name);
        interner.intern(&prefixed)
    } else {
        // Top level, already-qualified, or an absolute `::Foo` path —
        // no lexical-scope prefix.
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
    // Store one target, consuming the value on TOP of the stack. A
    // nested target re-enters `destructure` on that value (mutual
    // recursion; both are nested fns so deeper nesting just recurses).
    fn store_one(
        b: &mut ProtoBuilder,
        t: &MWT,
        protos: &mut Vec<Proto>,
        interner: &mut Interner,
        cc: &mut u32,
    ) {
        match t {
            MWT::Local(name) => {
                let slot = b.local_slot(name);
                b.emit(Op::StoreLocal(slot));
            }
            MWT::Ivar(name) => {
                let id = interner.intern(name);
                b.emit(Op::StoreIvar(id));
            }
            MWT::Global(name) => {
                let id = interner.intern(name);
                b.emit(Op::StoreGlobal(id));
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
            MWT::SplatGlobal(name) => {
                let id = interner.intern(name);
                b.emit(Op::StoreGlobal(id));
            }
            MWT::Const(name) | MWT::SplatConst(name) => {
                // Mirror `Expr::ConstWrite`: store the bare name AND,
                // inside a class/module body, a class-path-prefixed
                // alias (`Rake::Version::MAJOR`) so external resolution
                // finds it. The Dup+two-stores net-consumes exactly one
                // stack value, same as the single-store targets.
                let id = interner.intern(name);
                let prefixed_id = (!b.class_path.is_empty() && !name.contains("::"))
                    .then(|| interner.intern(&format!("{}::{}", b.class_path.join("::"), name)));
                if prefixed_id.is_some() {
                    b.emit(Op::Dup);
                }
                b.emit(Op::StoreConst(id));
                if let Some(pid) = prefixed_id {
                    b.emit(Op::StoreConst(pid));
                }
            }
            MWT::SplatCall { receiver, name } => {
                // Stack: [..., rest_array]. Same dispatch shape
                // as MWT::Call: compile receiver, swap to land
                // `[..., recv, rest_array]`, call `name=` setter
                // with arity 1, pop the return value. The rest
                // slice was already gathered into a fresh Array
                // by the multi-write splat-collection path
                // before this emit_store runs, so the writer
                // receives the same Array CRuby would pass.
                compile_expr(b, receiver, protos, interner, cc);
                b.emit(Op::Swap);
                let setter_name = if name.ends_with('=') {
                    name.clone()
                } else {
                    format!("{name}=")
                };
                let id = interner.intern(&setter_name);
                emit_method_call(b, id, 1, /*has_recv=*/true, false, false, cc);
                b.emit(Op::Pop);
            }
            MWT::Call { receiver, name } => {
                // Stack: [..., val]. Evaluate receiver, swap so
                // dispatch sees [..., recv, val], call setter,
                // pop return value. Note: receiver is
                // evaluated AFTER the RHS in source order — a
                // documented Tier-1 divergence from CRuby's
                // "evaluate receivers first" rule. Acceptable
                // for the rare cases where receiver evaluation
                // has visible side effects across the
                // assignment boundary; the common shape
                // (`obj.x, obj.y = a, b` with `obj` a plain
                // local) sees no observable difference.
                compile_expr(b, receiver, protos, interner, cc);
                b.emit(Op::Swap);
                // Prism's CallTargetNode.name() returns the
                // FULL setter symbol (with trailing `=`).
                // Belt-and-suspenders: tolerate either form so
                // a future Prism upgrade that flips the
                // convention doesn't silently produce `x==`.
                let setter_name = if name.ends_with('=') {
                    name.clone()
                } else {
                    format!("{name}=")
                };
                let id = interner.intern(&setter_name);
                emit_method_call(b, id, 1, /*has_recv=*/true, false, false, cc);
                b.emit(Op::Pop);
            }
            MWT::Index { receiver, args } => {
                // Stack: [..., val]. Stash val into a synthetic
                // local so we can rebuild
                // `[recv, idx1, ..., idxN, val]` on top for the
                // `[]=` dispatch. The shared `__mw_idx_val`
                // slot is overwritten by each Index target in
                // the same multi-write — fine because their
                // emit_store calls are strictly sequential and
                // each one consumes its own stash before the
                // next runs. Same "receiver evaluated after
                // RHS" divergence as MWT::Call above.
                let val_slot = b.local_slot("__mw_idx_val");
                b.emit(Op::StoreLocal(val_slot));
                compile_expr(b, receiver, protos, interner, cc);
                for a in args {
                    compile_expr(b, a, protos, interner, cc);
                }
                b.emit(Op::LoadLocal(val_slot));
                let setter_id = interner.intern("[]=");
                let argc = (args.len() + 1) as u8;
                emit_method_call(b, setter_id, argc, /*has_recv=*/true, false, false, cc);
                b.emit(Op::Pop);
            }
            MWT::Nested(subs) => {
                // The value on top is itself an aggregate — destructure
                // it into the inner target list (recursively).
                destructure(b, subs, protos, interner, cc);
            }
        }
    }

    // Destructure the value on TOP of the stack into `targets`,
    // consuming it. Coerces to an Array (massign semantics), then feeds
    // each target its slice (`[]` / `__mw_splat` / `__mw_post`).
    fn destructure(
        b: &mut ProtoBuilder,
        targets: &[MWT],
        protos: &mut Vec<Proto>,
        interner: &mut Interner,
        cc: &mut u32,
    ) {
        b.emit(Op::MassignSplat);
        let bracket_id = interner.intern("[]");
        let splat_id = interner.intern("__mw_splat");
        let splat_pos = targets.iter().position(|t| matches!(
            t,
            MWT::SplatLocal(_)
                | MWT::SplatIvar(_)
                | MWT::SplatGlobal(_)
                | MWT::SplatConst(_)
                | MWT::SplatCall { .. }
        ));
        match splat_pos {
            None => {
                for (i, target) in targets.iter().enumerate() {
                    b.emit(Op::Dup);
                    b.emit(Op::LoadConstInt(i as i64));
                    emit_method_call(b, bracket_id, 1, true, false, false, cc);
                    store_one(b, target, protos, interner, cc);
                }
            }
            Some(s) => {
                let post = targets.len() - s - 1;
                let post_id = interner.intern("__mw_post");
                for (i, target) in targets.iter().enumerate().take(s) {
                    b.emit(Op::Dup);
                    b.emit(Op::LoadConstInt(i as i64));
                    emit_method_call(b, bracket_id, 1, true, false, false, cc);
                    store_one(b, target, protos, interner, cc);
                }
                b.emit(Op::Dup);
                b.emit(Op::LoadConstInt(s as i64));
                b.emit(Op::LoadConstInt(post as i64));
                emit_method_call(b, splat_id, 2, true, false, false, cc);
                store_one(b, &targets[s], protos, interner, cc);
                for j in 0..post {
                    b.emit(Op::Dup);
                    b.emit(Op::LoadConstInt(j as i64));
                    b.emit(Op::LoadConstInt(s as i64));
                    b.emit(Op::LoadConstInt(post as i64));
                    emit_method_call(b, post_id, 3, true, false, false, cc);
                    store_one(b, &targets[s + 1 + j], protos, interner, cc);
                }
            }
        }
        // Drop the coerced Array — the destructure target is consumed.
        b.emit(Op::Pop);
    }

    compile_expr(b, value, protos, interner, cc);
    // The massign expression's VALUE is the ORIGINAL RHS (not the
    // coerced destructuring Array): `(a, b = nil)` is `nil`, `(a, b =
    // [1,2])` is `[1,2]`. Keep a copy beneath; `destructure` pops the
    // coerced Array, leaving the original RHS as the result. Without
    // this `while (x, y = queue.shift)` looped forever (nil coerced to
    // `[]`, truthy) — zeitwerk's eager-load directory queue.
    b.emit(Op::Dup);
    destructure(b, targets, protos, interner, cc);
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
        // Establish the rescue-stack baseline before any
        // PushRescue ops fire. On retry,
        // `TruncateRescuesToBeginBaseline` shrinks
        // `frame.rescues` back to this depth so stale
        // multi-class siblings from a previous iteration's
        // partial unwind don't survive into the new
        // iteration. EnterBegin is emitted ONCE per begin
        // block; retry's backward jump targets `begin_top`,
        // which is AFTER EnterBegin so the baseline isn't
        // double-pushed. (Code-review #306 round 1.)
        b.emit(Op::EnterBegin);
        // Capture the begin-top — AFTER EnterBegin / PushEnsure
        // (so retry doesn't double-push them) and BEFORE the
        // first PushRescue (so retry re-registers rescue
        // handlers). Pushed onto `retry_targets` while
        // compiling each rescue clause body below.
        // (TRY_RUNS pass-10 layer #9.)
        let begin_top = b.pos();
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
            let mut group = Vec::with_capacity(rc.classes.len().max(1));
            if rc.classes.is_empty() {
                group.push(b.emit(Op::PushRescue(0, slot, bind, stderr_sym)));
            } else {
                for n in rc.classes.iter().rev() {
                    // Splatted-local filter (`rescue *exp`) carries
                    // the LOCAL's name — resolve to a slot here and
                    // emit the local-reading op variant; everything
                    // else (plain class names, const-splat markers)
                    // stays on the SymId channel for the runtime to
                    // resolve.
                    if let Some(local_name) = crate::const_marker::strip_splat_local(n) {
                        let src_slot = b.local_slot(local_name);
                        group.push(b.emit(Op::PushRescueSplatLocal(0, slot, bind, src_slot)));
                    } else {
                        group.push(b.emit(Op::PushRescue(0, slot, bind, interner.intern(n))));
                    }
                }
            }
            groups.push(group);
        }
        compile_body(b, body, protos, interner, cc);
        let total: usize = groups.iter().map(|g| g.len()).sum();
        for _ in 0..total { b.emit(Op::PopRescue); }
        // End of the normal (no-exception) path — drop the
        // begin-baseline before falling through. (Code-review
        // #306 round 1.)
        b.emit(Op::ExitBegin);
        let mut jump_to_end: Vec<usize> = Vec::with_capacity(rescue.len() + 1);
        jump_to_end.push(b.emit(Op::Jump(0)));
        for (i, rc) in rescue.iter().rev().enumerate() {
            let group = &groups[i];
            let handler_start = b.pos();
            for &placeholder in group {
                let off = handler_start as i32 - placeholder as i32 - 1;
                match &mut b.code[placeholder] {
                    Op::PushRescue(o, _, _, _)
                    | Op::PushRescueSplatLocal(o, _, _, _) => *o = off,
                    _ => {}
                }
            }
            // Make begin_top reachable from inside this rescue
            // body so any `Expr::Retry` can jump back to re-run
            // the begin block. Popped right after the body
            // compiles so a later sibling rescue clause sees a
            // clean stack (still has its OWN frame on retry —
            // pushed again next iteration). (TRY_RUNS pass-10
            // layer #9.)
            // BEFORE running this rescue body, drop any
            // sibling-class entries from a multi-class clause
            // (`rescue A, B`) that the unwinder left below the
            // matched handler. Without this, a raise of the
            // sibling class FROM INSIDE this rescue body
            // (e.g. `rescue A, B; raise B; end` after A matched)
            // would re-enter this clause's body instead of
            // propagating outside the begin block. (Code-review
            // #306 round 4.)
            b.emit(Op::TruncateRescuesToBeginBaseline);
            b.retry_targets.push(begin_top);
            compile_body(b, &rc.body, protos, interner, cc);
            b.retry_targets.pop();
            // Rescue body completed without `retry`. The
            // unwinder may have left sibling-class entries
            // from a multi-class clause on the rescue stack
            // below the matched handler — truncate to the
            // begin baseline so they don't survive past this
            // begin block and catch unrelated later
            // exceptions. (Code-review #306 round 3.)
            b.emit(Op::TruncateRescuesToBeginBaseline);
            // Drop the begin-baseline before jumping past
            // the rescue chain. (Code-review #306 round 1.)
            b.emit(Op::ExitBegin);
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
///
/// `clippy::too_many_arguments` — the function plumbs the
/// compile-context (`ProtoBuilder`, `protos`, `interner`, `cc`)
/// alongside the call's own four bits of AST. Grouping them into
/// a struct just to please the lint would hide the call shape
/// and isn't worth the indirection.
#[allow(clippy::too_many_arguments)]
fn compile_call_arm(
    b: &mut ProtoBuilder,
    receiver: &Option<Box<SExpr>>,
    name: &str,
    args: &[SExpr],
    kwargs_trailing: bool,
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
                // `raise SomeClass, msg[, backtrace]` →
                // `SomeClass.exception(msg)`. CRuby builds the
                // exception with the MESSAGE ONLY (via `#exception`,
                // not `#new` — works for both a class and an existing
                // instance, which has no `.new`). Passing the
                // backtrace into the constructor (the old `.new(msg,
                // backtrace)`) raised ArgumentError on any exception
                // whose initialize takes 0..1 — rack's QueryParser
                // does `raise InvalidParameterError, e.message,
                // e.backtrace`.
                let sp = args[0].span;
                let exc_call = SExpr {
                    span: sp,
                    node: Expr::Call {
                        receiver: Some(Box::new(args[0].clone())),
                        name: "exception".to_string(),
                        args: vec![args[1].clone()],
                        kwargs_trailing: false,
                    },
                };
                if args.len() >= 3 {
                    // Explicit 3rd backtrace arg: CRuby stamps it onto
                    // the exception (so `e.backtrace` returns it, not
                    // the raise-site frames). Desugar to
                    // `__re = Cls.exception(msg); __re.set_backtrace(
                    // bt); __re` — the sequence's value is the
                    // exception, which `Op::Raise` then raises. The
                    // stored `@backtrace` is non-nil even for `[]`, so
                    // `unwind_with_exception`'s "already set" guard
                    // leaves it intact (rack's ShowExceptions renders
                    // "unknown location" for the empty-backtrace case).
                    let tmp = "__raise_exc_bt";
                    let set_bt = SExpr {
                        span: sp,
                        node: Expr::Call {
                            receiver: Some(Box::new(SExpr {
                                span: sp,
                                node: Expr::LVarRead(tmp.to_string()),
                            })),
                            name: "set_backtrace".to_string(),
                            args: vec![args[2].clone()],
                            kwargs_trailing: false,
                        },
                    };
                    let seq = SExpr {
                        span: sp,
                        node: Expr::Call {
                            receiver: None,
                            name: "__seq__".to_string(),
                            args: vec![
                                SExpr {
                                    span: sp,
                                    node: Expr::LVarWrite(
                                        tmp.to_string(),
                                        Box::new(exc_call),
                                    ),
                                },
                                set_bt,
                                SExpr { span: sp, node: Expr::LVarRead(tmp.to_string()) },
                            ],
                            kwargs_trailing: false,
                        },
                    };
                    compile_expr(b, &seq, protos, interner, cc);
                } else {
                    compile_expr(b, &exc_call, protos, interner, cc);
                }
            }
        }
        b.emit(Op::Raise);
        b.emit(Op::LoadNil);
        return;
    }
    // `<expr> <op> <rhs>` fusion → single BinOp* superinstruction.
    if let (Some(r), 1, Some(kind)) = (receiver.as_ref(), args.len(), BinOpKind::from_op_name(name)) {
        // `<local> <op> <local>` → BinOpLocalLocal. Both operands are
        // confirmed local-variable reads (prism resolves local-vs-method
        // before we see the AST), so reading them straight from the frame
        // is semantically identical to LoadLocal+LoadLocal+BinOp — and
        // `local_slot` returns the same slot the normal `LVarRead` path
        // would. Checked before the `LVarRead` compile of the receiver so
        // we never emit a stray LoadLocal first.
        if let (Expr::LVarRead(lname), Expr::LVarRead(rname)) = (&r.node, &args[0].node) {
            let a_slot = b.local_slot(lname);
            let b_slot = b.local_slot(rname);
            b.emit(Op::BinOpLocalLocal(kind, a_slot, b_slot));
            return;
        }
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
    emit_method_call(b, name_id, argc, has_recv, false, kwargs_trailing, cc);
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
    has_kwargs: bool,
    cc: &mut u32,
) {
    let cid = *cc as u16;
    *cc += 1;
    // The block+kwargs combo (`foo(**kw, &blk)`) emits a dedicated
    // `CallKwBlock*` op so the trailing keyword-splat Hash is treated
    // as kwargs (an empty/nil one dropped), not smuggled in as a
    // positional — see the op doc in bytecode.rs.
    let op = match (has_recv, has_block, has_kwargs) {
        (true, false, false)  => Op::Call(name, argc, cid),
        (false, false, false) => Op::CallNoRecv(name, argc, cid),
        (true, false, true)   => Op::CallKw(name, argc, cid),
        (false, false, true)  => Op::CallKwNoRecv(name, argc, cid),
        (true, true, true)    => Op::CallKwBlock(name, argc, cid),
        (false, true, true)   => Op::CallKwNoRecvBlock(name, argc, cid),
        (true, true, false)   => Op::CallBlock(name, argc, cid),
        (false, true, false)  => Op::CallNoRecvBlock(name, argc, cid),
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
            method_rest_slot: None,
            method_block_slot: None,
            method_n_positional: 0,
            method_n_post_rest: 0,
            method_has_kw: false,
            method_kw_params: vec![],
            method_kw_rest_slot: None,
            is_method_body: false,
            loop_break_jumps: vec![],
            loop_next_jumps: vec![],
            loop_redo_jumps: vec![],
            block_redo_target: None,
            retry_targets: vec![],
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
            Op::JumpIfKwArgGiven(_, o) => *o = off,
            Op::BreakLoop(o) => *o = off,
            Op::NextLoop(o) => *o = off,
            _ => panic!("ICE: patch_jump on non-jump op at {}", at),
        }
    }
    pub(crate) fn build(self, name: String, params: Vec<String>, n_required_positional: u16, lexical_scope: Vec<crate::intern::SymId>) -> Proto {
        // Escape analysis for `Locals::Stack` eligibility: one linear
        // scan of the finished body. Any `Op::CreateBlock` can clone
        // the frame's locals cell into a BlockHandle — that's the only
        // bytecode-level escape (eval compiles a fresh toplevel proto;
        // rubyrs has no Binding / local_variables reflection).
        let creates_block = self
            .code
            .iter()
            .any(|op| matches!(op, Op::CreateBlock(..) | Op::CreateLambda(..)));
        // Slot → name table (inverts the compile-time name→slot map) so
        // Kernel#binding can snapshot the frame's named locals.
        let mut local_names = vec![String::new(); self.n_locals as usize];
        for (nm, &slot) in &self.locals {
            if let Some(entry) = local_names.get_mut(slot as usize) {
                *entry = nm.clone();
            }
        }
        Proto {
            name, params, n_required_positional, local_names,
            // Default false; the parse entries (file load / require /
            // eval) stamp `true` across the whole proto range when the
            // source carried a `# frozen_string_literal: true` comment.
            frozen_string_literal: false,
            // 1 = no line adjustment; eval-with-line stamps the whole
            // proto range via `mark_line_base`.
            line_base: 1,
            // None = UTF-8 literals; eval-of-non-UTF-8-source stamps the
            // range via `mark_source_encoding`.
            source_encoding: None,
            n_required_post: 0,
            rest_param: None,
            kw_param_defaults: vec![],
            kw_has_computed_default: vec![],
            kw_rest_param: None,
            block_param: None,
            block_kw_params: vec![],
            block_param_slot: None,
            n_locals: self.n_locals,
            creates_block,
            code: self.code,
            op_spans: self.op_spans,
            filename: self.filename,
            // Non-block protos (methods, class bodies, toplevel)
            // never need slot resetting; only `compile_block` flips
            // this via the dedicated setter on the resulting Proto.
            block_body_local_start: u16::MAX,
            n_optional_params: 0,
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
    // Absolute paths (`::Foo::Bar`) must be filtered by the caller
    // before reaching this function and emitted as a flat
    // `Op::LoadConst` so they skip cref entirely (CRuby semantics).
    // All three call sites do that — keep this debug_assert as the
    // contract anchor in case a future caller forgets.
    debug_assert!(
        crate::const_marker::strip_absolute(bare).is_none(),
        "build_const_chain: caller must strip the absolute-path marker and emit a flat const load directly (LoadConst / LoadConstOrNil)",
    );
    if class_path.is_empty() {
        return None;
    }
    // Multi-segment `bare` (e.g. `QueryParser::Inner` from inside
    // `Foo::Utils`): cref-walk only the FIRST segment, then append
    // the rest verbatim to every chain entry. CRuby semantics:
    //   `QueryParser::Inner` inside `Foo::Utils` resolves
    //   `QueryParser` via cref (finds `Foo::QueryParser`), then
    //   looks up `Inner` inside it. Since `vm.classes` is keyed by
    //   joined-name, we approximate by trying each cref-prefixed
    //   joined name (`Foo::Utils::QueryParser::Inner`,
    //   `Foo::QueryParser::Inner`, `QueryParser::Inner`) in order.
    // Pre-fix the `bare.contains("::")` guard returned None and the
    // caller emitted a flat LoadConst that never matched the
    // registered joined name — `Foo::QueryParser::Inner` was missed.
    let (head, tail) = match bare.split_once("::") {
        Some((h, t)) => (h, Some(t)),
        None => (bare, None),
    };
    // Build each chain entry into a single `String` buffer to avoid
    // the intermediate `head_qualified` alloc per cref level.
    let join = |prefix: &str| -> String {
        let cap = prefix.len()
            + (if prefix.is_empty() { 0 } else { 2 })
            + head.len()
            + tail.map_or(0, |t| 2 + t.len());
        let mut s = String::with_capacity(cap);
        if !prefix.is_empty() {
            s.push_str(prefix);
            s.push_str("::");
        }
        s.push_str(head);
        if let Some(t) = tail {
            s.push_str("::");
            s.push_str(t);
        }
        s
    };
    let mut chain: Vec<crate::intern::SymId> =
        Vec::with_capacity(class_path.len() + 1);
    for i in (0..class_path.len()).rev() {
        let prefix = class_path[..=i].join("::");
        chain.push(interner.intern(&join(&prefix)));
    }
    chain.push(interner.intern(&join("")));
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
            if let Expr::Call { receiver: Some(r), name: op, args , .. } = &val.node
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
            if let Expr::Call { receiver: Some(r), name: op, args , .. } = &val.node
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
        Expr::RegexLit(src, flags) => { let id = interner.intern(src); b.emit(Op::LoadRegex(id, *flags)); }
        #[cfg(feature = "bignum")]
        Expr::BigIntLit(decimal) => { let id = interner.intern(decimal); b.emit(Op::LoadBigInt(id)); }
        Expr::RationalLit { num, den } => {
            let num_id = interner.intern(num);
            let den_id = interner.intern(den);
            b.emit(Op::LoadRational(num_id, den_id));
        }
        Expr::SymbolLit(s) => { let id = interner.intern(s); b.emit(Op::LoadSymbol(id)); }
        Expr::InterpolatedStr(parts) => {
            if parts.is_empty() {
                let id = interner.intern("");
                b.emit(Op::LoadConstStr(id));
            } else {
                for (idx, p) in parts.iter().enumerate() {
                    match &p.node {
                        Expr::StrLit(_) => compile_expr(b, p, protos, interner, cc),
                        _ => {
                            compile_expr(b, p, protos, interner, cc);
                            // CRuby rb_obj_as_string semantics — a
                            // String part skips to_s entirely; see
                            // Op::InterpToS. Consumes a cache id for
                            // the non-String dispatch path.
                            let cid = *cc as u16;
                            *cc += 1;
                            b.emit(Op::InterpToS(cid));
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
        Expr::InterpolatedRegex(parts, flags) => {
            if parts.is_empty() {
                let id = interner.intern("");
                b.emit(Op::LoadConstStr(id));
            } else {
                for (idx, p) in parts.iter().enumerate() {
                    match &p.node {
                        Expr::StrLit(_) => compile_expr(b, p, protos, interner, cc),
                        _ => {
                            compile_expr(b, p, protos, interner, cc);
                            // Same InterpToS contract as
                            // InterpolatedStr above.
                            let cid = *cc as u16;
                            *cc += 1;
                            b.emit(Op::InterpToS(cid));
                        }
                    }
                    if idx > 0 {
                        b.emit(Op::BinOp(BinOpKind::Add));
                    }
                }
            }
            b.emit(Op::CompileRegex(*flags));
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
            if let Expr::Call { receiver: Some(r), name: op, args , .. } = &val.node
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
            if let Expr::Call { receiver: Some(r), name: op, args , .. } = &val.node
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
            // Absolute paths (`::Foo::Bar`, marked with a leading
            // `::`) skip cref entirely — emit a flat LoadConst with
            // the stripped name so we avoid the const_chains entry
            // and the runtime Vec clone that LoadConstChain pays.
            if let Some(absolute) = crate::const_marker::strip_absolute(name) {
                let id = interner.intern(absolute);
                b.emit(Op::LoadConst(id));
            // Inside a non-empty class/module scope, emit a cref-
            // walking lookup so `Bar` inside `module Foo; ... end`
            // resolves to `Foo::Bar` first and falls back through
            // outer scopes to the bare top-level name. Top-level
            // reads stay on the plain `LoadConst` path.
            } else if let Some(chain) = build_const_chain(&b.class_path, name, interner) {
                let idx = b.const_chains.len() as u32;
                b.const_chains.push(chain);
                b.emit(Op::LoadConstChain(idx));
            } else {
                let id = interner.intern(name);
                b.emit(Op::LoadConst(id));
            }
        }
        Expr::ConstReadOrNil(name) => {
            // Same absolute-path fast path as ConstRead above.
            if let Some(absolute) = crate::const_marker::strip_absolute(name) {
                let id = interner.intern(absolute);
                b.emit(Op::LoadConstOrNil(id));
            } else if let Some(chain) = build_const_chain(&b.class_path, name, interner) {
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
        Expr::Call { receiver, name, args, kwargs_trailing } => {
            compile_call_arm(b, receiver, name, args, *kwargs_trailing, protos, interner, cc);
        }
        Expr::AssignCall { receiver, name, args } => {
            // Assignment-syntax dispatch — same stack shape as a
            // plain explicit-recv call, but routed through
            // Op::CallAset so the expression value is the RHS (the
            // last compiled arg). No block / splat / kwargs forms
            // reach here (ast.rs routes those to the plain Call
            // path).
            let name_id = interner.intern(name);
            compile_expr(b, receiver, protos, interner, cc);
            for a in args { compile_expr(b, a, protos, interner, cc); }
            let cid = *cc as u16;
            *cc += 1;
            b.emit(Op::CallAset(name_id, args.len() as u8, cid));
        }
        Expr::Def { name, params, defaults, rest, n_required_post, kw_params, kw_rest, block_param, receiver, body } => {
            compile_def_arm(
                b, name, params, defaults, rest, *n_required_post, kw_params, kw_rest, block_param, receiver, body,
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
            let mname_resolved = mname.clone().unwrap_or_else(|| "<super-outside-method>".to_string());
            let name_id = interner.intern(&mname_resolved);
            match args_opt {
                Some(args) => {
                    for a in args { compile_expr(b, a, protos, interner, cc); }
                    let argc = args.len() as u8;
                    b.emit(Op::Super(name_id, argc));
                }
                None => {
                    // Forwarding form (bare `super`) — re-pass the
                    // enclosing method's args AS RECEIVED: positionals
                    // (splatting the rest), and the `&block` as a block
                    // (NOT a positional). `simple` excludes the cases
                    // not yet modelled (kwargs, post-rest `*a, b`),
                    // which fall back to the legacy slot-dump.
                    let simple = !b.method_has_kw && b.method_n_post_rest == 0;
                    if simple && let Some(rs) = b.method_rest_slot {
                        // Rest present → assemble `[pre…, *rest]` and
                        // splat via ApplySuper. A single `*rest`
                        // (`def m(*); super; end`) reduces to exactly
                        // the old fast path. With a `&block`, push it
                        // first so ApplySuperBlock sees `[block, array]`.
                        if let Some(bs) = b.method_block_slot {
                            b.emit(Op::LoadLocal(bs));
                        }
                        emit_super_forward_array(b, interner, rs);
                        if b.method_block_slot.is_some() {
                            b.emit(Op::ApplySuperBlock(name_id));
                        } else {
                            b.emit(Op::ApplySuper(name_id));
                        }
                    } else if simple && let Some(bs) = b.method_block_slot {
                        // No rest, but a `&block`: forward positionals
                        // + the block (the old path passed the block
                        // slot positionally → arg over-count).
                        b.emit(Op::LoadLocal(bs));
                        for i in 0..b.method_n_positional {
                            b.emit(Op::LoadLocal(i));
                        }
                        b.emit(Op::NewArray(b.method_n_positional as u32));
                        b.emit(Op::ApplySuperBlock(name_id));
                    } else if b.method_has_kw
                        && b.method_rest_slot.is_some()
                        && b.method_n_post_rest == 0
                    {
                        // Bare `super` from a method with BOTH a `*rest`
                        // and keyword params / `**kwrest`
                        // (`def m(*a, **kw); super; end` — mustermann's
                        // `Concat#initialize(*, **)`). The legacy
                        // slot-dump loads the `*rest` slot as a SINGLE
                        // positional (so `[*a]` rides nested one level
                        // too deep — `[[…]]`); splat it into the
                        // positional array instead, then append the
                        // reconstructed trailing kwargs Hash so the
                        // callee binds keywords. Combines the rest-only
                        // and kw-only branches.
                        let rs = b.method_rest_slot.expect("rest_slot is_some checked");
                        let block_present = b.method_block_slot.is_some();
                        if let Some(bs) = b.method_block_slot {
                            b.emit(Op::LoadLocal(bs));
                        }
                        // positional array `[pre…] + rest`
                        emit_super_forward_array(b, interner, rs);
                        // reconstructed kwargs Hash (`**kwrest` merged
                        // under named kw, explicit keywords winning).
                        let kw = b.method_kw_params.clone();
                        let kw_count = kw.len() as u16;
                        let kwrest = b.method_kw_rest_slot;
                        if let Some(krs) = kwrest {
                            b.emit(Op::LoadLocal(krs));
                        }
                        for (kname, slot) in &kw {
                            let ksym = interner.intern(kname);
                            b.emit(Op::LoadSymbol(ksym));
                            b.emit(Op::LoadLocal(*slot));
                        }
                        b.emit(Op::NewHash(kw_count as u32));
                        if kwrest.is_some() {
                            let merge_id = interner.intern("merge");
                            b.emit(Op::Call(merge_id, 1, u16::MAX));
                        }
                        // Append the kwargs Hash to the positional array
                        // (`arr + [hash]`) ONLY when it is non-empty —
                        // CRuby's keyword separation forwards nothing for
                        // an empty `**kwrest`, and a trailing empty Hash
                        // would otherwise count as an extra positional to
                        // a parent without kw params (`def m(*a, **kw);
                        // super; end` calling `super` to `def base(a, b)`).
                        // Stack here: `[posarr, hash]`.
                        b.emit(Op::Dup);
                        let empty_id = interner.intern("empty?");
                        b.emit(Op::Call(empty_id, 0, u16::MAX));
                        // JumpIfFalse → non-empty path (condition popped).
                        let jf = b.emit(Op::JumpIfFalse(0));
                        // empty: drop the Hash, leave the positional array.
                        b.emit(Op::Pop);
                        let j_done = b.emit(Op::Jump(0));
                        // non-empty: `posarr + [hash]`. ApplySuper peels the
                        // trailing Hash as kwargs (super leaves
                        // `trailing_hash_positional == false`).
                        b.patch_jump(jf, b.pos());
                        b.emit(Op::NewArray(1));
                        let plus_id = interner.intern("+");
                        b.emit(Op::Call(plus_id, 1, u16::MAX));
                        b.patch_jump(j_done, b.pos());
                        if block_present {
                            b.emit(Op::ApplySuperBlock(name_id));
                        } else {
                            b.emit(Op::ApplySuper(name_id));
                        }
                    } else if b.method_has_kw
                        && b.method_rest_slot.is_none()
                    {
                        // Bare `super` from a method with named keyword
                        // params. The legacy slot-dump (below) forwards
                        // the kw slots as POSITIONAL args, so a kwarg-only
                        // parent reports "wrong number of arguments (given
                        // N, expected 0)". Instead forward positionals
                        // 0..n_positional, then a reconstructed trailing
                        // kwargs Hash `{ name => <slot value>, … }` from the
                        // current kw-param slot values, so the callee binds
                        // them as KEYWORDS. The Hash rides as the args
                        // array's trailing element; a `super` call leaves
                        // `trailing_hash_positional == false`, so the
                        // method binder peels it as kwargs. Surfaced by
                        // public_suffix's `Wildcard#initialize(value:,
                        // length:, private:); super; end`. (`**kwrest` and
                        // a mid-signature `*rest` still bail to the legacy
                        // path — merging the rest Hash isn't modelled yet.)
                        let block_present = b.method_block_slot.is_some();
                        if let Some(bs) = b.method_block_slot {
                            b.emit(Op::LoadLocal(bs));
                        }
                        let n_pos = b.method_n_positional;
                        for i in 0..n_pos {
                            b.emit(Op::LoadLocal(i));
                        }
                        // When the method also declares `**kwrest`, the
                        // forwarded kwargs are the named params MERGED
                        // OVER the kwrest hash (`def m(a, x: 1, **rest);
                        // super; end` forwards `rest.merge({x: x})` —
                        // mustermann's `Composite.supported?(option,
                        // type: nil, **options)`). Build the kwrest hash
                        // (dup'd via merge, never mutated) as the base,
                        // then merge the reconstructed named-kw hash so
                        // explicit keywords win.
                        let kw = b.method_kw_params.clone();
                        let kw_count = kw.len() as u16;
                        let kwrest = b.method_kw_rest_slot;
                        if let Some(krs) = kwrest {
                            b.emit(Op::LoadLocal(krs));
                        }
                        for (kname, slot) in &kw {
                            let ksym = interner.intern(kname);
                            b.emit(Op::LoadSymbol(ksym));
                            b.emit(Op::LoadLocal(*slot));
                        }
                        b.emit(Op::NewHash(kw_count as u32));
                        if kwrest.is_some() {
                            // `kwrest.merge(named)` → combined kwargs Hash.
                            let merge_id = interner.intern("merge");
                            b.emit(Op::Call(merge_id, 1, u16::MAX));
                        }
                        b.emit(Op::NewArray((n_pos + 1) as u32));
                        if block_present {
                            b.emit(Op::ApplySuperBlock(name_id));
                        } else {
                            b.emit(Op::ApplySuper(name_id));
                        }
                    } else {
                        // Positional slot-dump. Pure positional uses the
                        // positional count; the kw / post-rest fallback
                        // dumps every slot (approximate — those shapes
                        // are rare with bare super).
                        let n = if simple { b.method_n_positional } else { b.method_param_count };
                        for i in 0..n {
                            b.emit(Op::LoadLocal(i));
                        }
                        b.emit(Op::Super(name_id, n as u8));
                    }
                }
            }
        }
        Expr::SuperApply { args: args_expr, block_arg } => {
            // `super(*args)` — assemble the args Array and let
            // `Op::ApplySuper` pop + drain it. Mirror of the
            // `Expr::Apply` shape used by regular splat-call
            // dispatch. Method-name resolution is the same as
            // direct-form `Expr::Super`. When `block_arg` is
            // present, push block first so the VM sees
            // `[block, array]` and routes through
            // `Op::ApplySuperBlock` (block-aware super dispatch).
            let mname = b.method_name.clone()
                .unwrap_or_else(|| "<super-outside-method>".to_string());
            let name_id = interner.intern(&mname);
            if let Some(ba) = block_arg { compile_expr(b, ba, protos, interner, cc); }
            compile_expr(b, args_expr, protos, interner, cc);
            if block_arg.is_some() {
                b.emit(Op::ApplySuperBlock(name_id));
            } else {
                b.emit(Op::ApplySuper(name_id));
            }
        }
        Expr::SuperWithBlock { args, block_params, block_body } => {
            // `super do … end` — compile the block LITERAL, assemble
            // the args into an Array, and route through
            // `Op::ApplySuperBlock` (same `[block, array]` stack shape
            // as `super(*args, &proc)`). `args == None` forwards the
            // enclosing method's params, mirroring the bare-`super`
            // forwarding in `Expr::Super`.
            let mname = b.method_name.clone()
                .unwrap_or_else(|| "<super-outside-method>".to_string());
            let name_id = interner.intern(&mname);
            // Block literal first → CreateBlock pushes the Block.
            let (block_proto_idx, param_start, n_params, rest_slot, kw_rest_slot) =
                compile_block(b, block_params, block_body, protos, interner, cc);
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot, kw_rest_slot));
            // Args Array on top.
            match args {
                Some(arg_exprs) => {
                    for a in arg_exprs { compile_expr(b, a, protos, interner, cc); }
                    b.emit(Op::NewArray(arg_exprs.len() as u32));
                }
                None => {
                    // Forwarding form: assemble the POSITIONAL args
                    // Array (splatting the rest), EXCLUDING the
                    // method's `&block` slot — the literal block above
                    // replaces it (CRuby: `super do…end` passes the
                    // literal block). A lone `*rest` reduces to the
                    // rest Array itself. kw / post-rest fall back to
                    // the legacy slot-dump (rare).
                    let simple = !b.method_has_kw && b.method_n_post_rest == 0;
                    if simple && let Some(rs) = b.method_rest_slot {
                        emit_super_forward_array(b, interner, rs);
                    } else {
                        let n = if simple { b.method_n_positional } else { b.method_param_count };
                        for i in 0..n {
                            b.emit(Op::LoadLocal(i));
                        }
                        b.emit(Op::NewArray(n as u32));
                    }
                }
            }
            b.emit(Op::ApplySuperBlock(name_id));
        }
        Expr::Class { name, superclass, body, is_module, absolute } => {
            compile_class_arm(b, name, superclass, body, *is_module, *absolute, protos, interner, cc);
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
        Expr::SingletonClassBody { recv, body } => {
            // Real eigenclass body (self = metaclass). Compile the
            // body into its own proto, evaluate the receiver in the
            // SURROUNDING scope (so `class << self` reads the outer
            // self, `class << Const` resolves the constant here),
            // then emit the op that materializes the eigenclass and
            // opens the class-body frame. The body proto inherits
            // the surrounding lexical class_path so nested
            // `module`/`class` and constant reads resolve against
            // the enclosing namespace (CRuby scopes them under the
            // metaclass, but the flat const model keeps them under
            // the surrounding module — observably equivalent for the
            // bare-name reads inside the body).
            let proto_idx = compile_proto_at(
                "<singleton class>".to_string(), vec![], body,
                b.filename.clone(), protos, interner, cc, b.class_path.clone(),
            );
            compile_expr(b, recv, protos, interner, cc);
            b.emit(Op::OpenSingletonClass(proto_idx as u32));
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
            b.emit(Op::NewArray(elems.len() as u32));
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
            b.emit(Op::NewHash(pairs.len() as u32));
        }
        Expr::CallWithBlock { receiver, name, args, block_params, block_body, kwargs_trailing } => {
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
            let (block_proto_idx, param_start, n_params, rest_slot, kw_rest_slot) =
                compile_block(b, block_params, block_body, protos, interner, cc);
            let name_id = interner.intern(name);
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot, kw_rest_slot));
            for a in args { compile_expr(b, a, protos, interner, cc); }
            let argc = args.len() as u8;
            emit_method_call(b, name_id, argc, has_recv, true, *kwargs_trailing, cc);
        }
        Expr::CallWithBlockArg { receiver, name, args, block_arg, kwargs_trailing } => {
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
            emit_method_call(b, name_id, argc, has_recv, true, *kwargs_trailing, cc);
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
        Expr::YieldSplat(arr) => {
            // Push the combined args Array; `Op::ApplyYield` expands it
            // and drives the block with the dynamic argc.
            compile_expr(b, arr, protos, interner, cc);
            b.emit(Op::ApplyYield);
        }
        Expr::Retry => {
            // `retry` re-executes the surrounding begin block. The
            // target is the inner-most `retry_targets` entry, set
            // by `compile_begin_arm` while compiling a rescue
            // clause body. CRuby raises SyntaxError at parse time
            // when `retry` appears outside a rescue; rubyrs catches
            // the out-of-context case here and emits a RuntimeError
            // raise instead — a Tier-1 divergence on the error
            // class for an error-only path. (TRY_RUNS pass-10
            // layer #9.)
            match b.retry_targets.last().copied() {
                Some(target) => {
                    // Truncate stale rescue handlers from the
                    // failed iteration before jumping back to
                    // begin_top. Without this, a multi-class
                    // clause whose unwinder consumed only the
                    // matched filter leaves its siblings on the
                    // rescue stack, where they accumulate across
                    // retries and can catch unrelated exceptions
                    // raised AFTER the begin block completes.
                    // (Code-review #306 round 1.)
                    b.emit(Op::TruncateRescuesToBeginBaseline);
                    let here = b.pos();
                    let off = target as i32 - here as i32 - 1;
                    b.emit(Op::Jump(off));
                    // Sentinel for stack-balance of any unreachable
                    // code following — mirrors Op::Return / Break.
                    b.emit(Op::LoadNil);
                }
                None => {
                    let msg_sym = interner.intern("Invalid retry");
                    b.emit(Op::LoadConstStr(msg_sym));
                    b.emit(Op::Raise);
                    b.emit(Op::LoadNil);
                }
            }
        }
        Expr::Redo => {
            // `redo` re-runs the current iteration. Inside a `while` /
            // `until` it jumps to the body-start (reusing NextLoop's
            // handler-aware transfer, patched to the body label by the
            // while codegen). Inside a block it re-runs the block body
            // in the same frame via an intra-proto Jump. The `while`
            // target takes precedence over the block when both apply
            // (CRuby: redo binds to the innermost loop). Out of any
            // loop/block, CRuby raises LocalJumpError; rubyrs emits a
            // runtime raise (Tier-1 error-class divergence).
            if !b.loop_redo_jumps.is_empty() {
                let placeholder = b.emit(Op::NextLoop(0));
                b.loop_redo_jumps.last_mut().expect("ICE: just checked").push(placeholder);
            } else if let Some(target) = b.block_redo_target {
                let here = b.pos();
                let off = target as i32 - here as i32 - 1;
                b.emit(Op::Jump(off));
            } else {
                let msg_sym = interner.intern("redo called outside of loop");
                b.emit(Op::LoadConstStr(msg_sym));
                b.emit(Op::Raise);
            }
            // Sentinel for stack-balance of any unreachable trailing code.
            b.emit(Op::LoadNil);
        }
        Expr::Apply { receiver, name, splat, block_arg } => {
            // `foo(*arr)` — compile receiver (if any) then the
            // splat expression. The VM op `ApplyCall(NoRecv)`
            // pops the Array and uses its elements as args.
            // When `block_arg` is present (`foo(*arr, &blk)`),
            // emit the block-aware variant: stack becomes
            // `[recv?, block, array]` and the VM expands+dispatches
            // through the block path.
            let has_recv = receiver.is_some();
            if let Some(r) = receiver { compile_expr(b, r, protos, interner, cc); }
            if let Some(ba) = block_arg { compile_expr(b, ba, protos, interner, cc); }
            compile_expr(b, splat, protos, interner, cc);
            let name_id = interner.intern(name);
            let cid = *cc as u16; *cc += 1;
            match (has_recv, block_arg.is_some()) {
                (true,  false) => b.emit(Op::ApplyCall(name_id, cid)),
                (false, false) => b.emit(Op::ApplyCallNoRecv(name_id, cid)),
                (true,  true)  => b.emit(Op::ApplyCallBlock(name_id, cid)),
                (false, true)  => b.emit(Op::ApplyCallNoRecvBlock(name_id, cid)),
            };
        }
        Expr::Lambda { params, body, is_lambda } => {
            // `->(p) { body }` — compile the body as a block proto
            // and emit CreateBlock. Result stays on the stack as a
            // Value::Block (which supports `.call(args)` already).
            // Lambda params are now `Vec<BlockParam>` (post K7), so
            // they go straight into compile_block without rewrapping.
            // A real `->` literal emits CreateLambda (sets Proc#lambda?
            // true); the splat-call-block-forwarding reuse of this
            // variant (is_lambda=false) stays an ordinary block.
            let (block_proto_idx, param_start, n_params, rest_slot, kw_rest_slot) =
                compile_block(b, params, body, protos, interner, cc);
            if *is_lambda {
                b.emit(Op::CreateLambda(block_proto_idx as u32, param_start, n_params, rest_slot, kw_rest_slot));
            } else {
                b.emit(Op::CreateBlock(block_proto_idx as u32, param_start, n_params, rest_slot, kw_rest_slot));
            }
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

/// Detect a `# frozen_string_literal: true` magic comment. CRuby
/// recognises it only on the first line — or the second line when the
/// first is a shebang — so we scan just the leading comment line(s)
/// and stop at the first code line, avoiding false positives from a
/// license header that merely mentions the directive. Returns `true`
/// only for an explicit `true` value (`false` / absent → mutable
/// literals). The parse entries pass this to
/// `mark_frozen_string_literal` over the proto range they compiled.
pub(crate) fn detect_frozen_string_literal(src: &str) -> bool {
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if i == 0 && t.starts_with("#!") {
            continue; // shebang — magic comment may follow on line 2
        }
        let Some(comment) = t.strip_prefix('#') else {
            return false; // first non-comment line → magic-comment region ended
        };
        // CRuby's magic-comment parser treats `-` and `_` as
        // interchangeable in the directive name, so both
        // `frozen_string_literal:` and `frozen-string-literal:` are
        // honoured (Tilt emits the HYPHEN form into its compiled
        // template source). Normalise hyphens to underscores in the
        // directive region before matching. (Only the leading comment
        // lines are scanned, so this can't corrupt a value.)
        let normalized = comment.replace('-', "_");
        if let Some(pos) = normalized.find("frozen_string_literal:") {
            let after = normalized[pos + "frozen_string_literal:".len()..].trim_start();
            let val: String = after.chars().take_while(|c| c.is_alphanumeric()).collect();
            return val == "true";
        }
        // A comment without the directive (e.g. `# encoding: utf-8`)
        // — keep scanning the next leading comment line, but only a
        // couple deep before bailing (magic comments live at the top).
        if i >= 1 {
            return false;
        }
    }
    false
}

/// Stamp `frozen_string_literal = true` onto every proto in
/// `protos[start..]` — the range a parse entry just compiled for one
/// source. Called when `detect_frozen_string_literal` is true so the
/// file's methods / blocks all inherit the file-level setting without
/// threading a flag through every `compile_*` signature.
pub(crate) fn mark_frozen_string_literal(protos: &mut [Proto], start: usize) {
    for p in &mut protos[start..] {
        p.frozen_string_literal = true;
    }
}

/// Stamp `line_base` onto every proto in `protos[start..]` — the range
/// an eval-with-line entry just compiled. Mirrors
/// `mark_frozen_string_literal`: lets `class_eval(src, file, line)` /
/// `eval(src, b, file, line)` map reported line numbers onto the
/// caller's coordinate system without threading the offset through
/// every `compile_*` signature.
pub(crate) fn mark_line_base(protos: &mut [Proto], start: usize, base: i32) {
    for p in &mut protos[start..] {
        p.line_base = base;
    }
}

/// Stamp `source_encoding` onto every proto in `protos[start..]` — the
/// range an eval-of-non-UTF-8-source entry just compiled. Mirrors
/// `mark_line_base` / `mark_frozen_string_literal`: a template engine
/// eval'ing a US-ASCII / Shift_JIS template gets string literals tagged
/// (and, for non-ASCII-compatible encodings, transcoded) to the
/// template's own encoding.
pub(crate) fn mark_source_encoding(protos: &mut [Proto], start: usize, enc: crate::value::EncodingTag) {
    for p in &mut protos[start..] {
        p.source_encoding = Some(enc);
    }
}

/// Emit ops leaving the bare-`super` POSITIONAL forwarding args as a
/// single Array on the stack: `[pre-rest…] + rest`, splatting the
/// enclosing method's rest param. `rs` is the rest slot. There's no
/// dedicated splat-concat op, so we build the pre-rest piece as an
/// Array and join the rest with `Array#+` (the same shape the
/// `super(a, *rest)` AST desugar uses). The caller then feeds the
/// Array to `Op::ApplySuper` / `Op::ApplySuperBlock`, which splats it.
/// Only called when the method has NO post-rest positionals (`*a, b`)
/// — those fall back to the legacy path at the call sites.
fn emit_super_forward_array(b: &mut ProtoBuilder, interner: &mut Interner, rs: u16) {
    let plus_id = interner.intern("+");
    // pre-rest positionals → `[pre…]`
    for i in 0..rs {
        b.emit(Op::LoadLocal(i));
    }
    b.emit(Op::NewArray(rs as u32));
    // `+ rest` (already an Array)
    b.emit(Op::LoadLocal(rs));
    b.emit(Op::Call(plus_id, 1, u16::MAX));
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
    compile_proto_kind(name, params, n_req, vec![], body, filename, protos, interner, cc, /*is_method=*/false, class_path, None, /*n_required_post=*/0, /*has_kw=*/false, /*has_block=*/false, vec![], vec![], None)
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
    rest_slot_for_super: Option<u16>,
    // Bare-`super` forwarding layout: count of post-rest required
    // positionals, and whether the method declares keyword params /
    // `**kwrest` / a `&block`. (Pre-rest positional count is derived
    // from `n_required_positional` + the optional count.)
    n_required_post: u16,
    has_kw: bool,
    has_block: bool,
    // `(kw_idx, kw_slot, computed_default_expr)` triples — one per
    // kwarg with a computed (non-literal) default. The kw prologue
    // emits `Op::JumpIfKwArgGiven(kw_idx, _)` plus the default-eval
    // body for each. Empty when the method has no kwargs or all
    // kwarg defaults are literals.
    kw_computed_prologue: Vec<(u16, u16, SExpr)>,
    // Bare-`super` kwarg forwarding: `(name, slot)` per keyword param
    // and the `**kwrest` slot (if any). See `ProtoBuilder::method_kw_params`.
    method_kw_params: Vec<(String, u16)>,
    method_kw_rest_slot: Option<u16>,
) -> usize {
    let mut b = ProtoBuilder::new(&params, filename);
    b.class_path = class_path;
    if is_method {
        b.method_name = Some(name.clone());
        b.method_param_count = params.len() as u16;
        b.is_method_body = true;
        b.method_rest_slot = rest_slot_for_super;
        // Bare-super forwarding layout (see ProtoBuilder fields).
        let n_optional = default_exprs.iter().filter(|d| d.is_some()).count() as u16;
        b.method_n_positional = n_required_positional + n_optional + n_required_post;
        b.method_n_post_rest = n_required_post;
        b.method_has_kw = has_kw;
        b.method_kw_params = method_kw_params;
        b.method_kw_rest_slot = method_kw_rest_slot;
        // `&block` is the LAST entry in `effective_params`.
        b.method_block_slot = if has_block {
            Some((params.len() as u16).saturating_sub(1))
        } else {
            None
        };
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
    // Kwarg computed-default prologue. Runs after positional
    // defaults so a kwarg default expression can reference any
    // earlier positional param (including ones filled by their
    // own default). Same shape as the positional prologue:
    // `JumpIfKwArgGiven(kw_idx, skip) + <expr> + StoreLocal(slot)`.
    // The binder leaves the slot Nil for computed-default kwargs
    // when caller missing AND sets `kw_given_mask` bit `kw_idx`
    // when present; the jump consults the mask.
    for (kw_idx, slot, def_expr) in &kw_computed_prologue {
        let jmp = b.emit(Op::JumpIfKwArgGiven(*kw_idx, 0));
        compile_expr(&mut b, def_expr, protos, interner, cc);
        b.emit(Op::StoreLocal(*slot));
        let skip = b.pos();
        b.patch_jump(jmp, skip);
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
/// True iff `e` is a compile-time literal that `literal_to_value`
/// can encode directly into a `Value`. Used to route kwarg
/// defaults: literals go into `Proto::kw_param_defaults` for the
/// binder's fast path; non-literals (constants, method calls,
/// prior-param refs, ...) get a `JumpIfKwArgGiven` prologue
/// emitted in the method body, mirroring positional defaults.
fn expr_is_compile_time_literal(e: &Expr) -> bool {
    matches!(
        e,
        Expr::IntLit(_)
            | Expr::FloatLit(_)
            | Expr::StrLit(_)
            | Expr::StrLitBytes(_)
            | Expr::SymbolLit(_)
            | Expr::BoolLit(_)
            | Expr::Nil
    )
}

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
) -> (usize, u16, u16, u16, u16) {
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
        method_rest_slot: parent.method_rest_slot,
        method_block_slot: parent.method_block_slot,
        method_n_positional: parent.method_n_positional,
        method_n_post_rest: parent.method_n_post_rest,
        method_has_kw: parent.method_has_kw,
        method_kw_params: parent.method_kw_params.clone(),
        method_kw_rest_slot: parent.method_kw_rest_slot,
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
        // Fresh redo state: a `redo` in this block re-runs its own
        // body (target set just before the body compiles below), not
        // a `while`/block in the parent.
        loop_redo_jumps: vec![],
        block_redo_target: None,
        // Fresh `retry` target stack — blocks don't inherit
        // begin/rescue context from the parent proto. (CRuby's
        // `retry` always rescues within the textually-enclosing
        // begin block, but blocks introduce a fresh frame for
        // their own begin/rescue layers.)
        retry_targets: vec![],
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
                BlockParam::Optional(_) => {
                    // `|(a, b = 1)|` — an optional inside a destructure
                    // isn't part of the subset (Prism nests it
                    // differently); defensive skip, like the others.
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
                BlockParam::BlockArg(_) => {
                    // `&blk` inside a destructure (`|(a, &b)|`)
                    // isn't legal Ruby; defensive skip.
                }
                BlockParam::KwRest(_) => {
                    // `**opts` inside a destructure (`|(a, **b)|`)
                    // isn't legal Ruby; defensive skip.
                }
                BlockParam::Keyword(..) => {
                    // `k:` inside a destructure (`|(a, k: 1)|`)
                    // isn't legal Ruby; defensive skip.
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
    // M27 A1: when a `|*rest|` or `|&blk|` param is present we also
    // remember the slot name so the proto's `rest_param` /
    // `block_param` fields can be stamped after the slot dance
    // finishes. Without these, a block installed AS A METHOD via
    // `Module#define_method` has its rest / block-arg slots reserved
    // in `locals` but `invoke_method_with_block`'s binder skips them
    // (`has_rest` / `has_block_param` both false), so `*args` arrived
    // empty and `&blk` arrived Nil.
    let mut rest_param_name: Option<String> = None;
    let mut block_arg_name: Option<String> = None;
    // `|.., &b|` absolute slot — stamped onto the proto so
    // invoke_block can bind the caller's block (or Nil) into it.
    let mut block_arg_slot: Option<u16> = None;
    // `|**opts|` keyword-rest: a slot (not counted in n_params,
    // like rest) that invoke_block fills with the trailing kwargs
    // Hash (default `{}`). `u16::MAX` sentinel = no kw-rest param.
    let mut kw_rest_slot: u16 = u16::MAX;
    let mut kw_rest_param_name: Option<String> = None;
    // `|k1:, k2: d|` keyword params: (name, absolute slot,
    // required). Stamped onto the proto below; invoke_block binds
    // by these slots. ast.rs pushes Keyword entries last, so the
    // slots land after every positional/rest/blockarg/kwrest slot.
    let mut block_kw_params: Vec<(String, u16, bool)> = Vec::new();
    // Count of OPTIONAL positional params — stamped onto the proto so
    // Proc#arity reports `-(required + 1)`. Optionals take real slots
    // (bound positionally, default applied via the AST body prologue),
    // so they ALSO count toward n_required/n_params for binding.
    let mut n_optional: u16 = 0;
    for (i, p) in block_params.iter().enumerate() {
        match p {
            BlockParam::Single(name) => {
                b.define_local_slot(name);
                n_required += 1;
            }
            BlockParam::Optional(name) => {
                b.define_local_slot(name);
                n_required += 1;
                n_optional += 1;
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
                rest_param_name = Some(slot_name);
            }
            BlockParam::BlockArg(name) => {
                let slot_name = if name == "&" { format!("__blkarg_{i}") } else { name.clone() };
                let s = b.define_local_slot(&slot_name);
                block_arg_slot = Some(s);
                block_arg_name = Some(slot_name);
            }
            BlockParam::KwRest(name) => {
                // Anonymous `|**|` reserves a synth-named slot
                // (data dropped, slot still bound to `{}`).
                let slot_name = if name.is_empty() { format!("__kwrest_{i}") } else { name.clone() };
                let s = b.define_local_slot(&slot_name);
                kw_rest_slot = s;
                kw_rest_param_name = Some(slot_name);
            }
            BlockParam::Keyword(name, required) => {
                // Not counted in n_required (kw params aren't
                // positional); the binder writes the slot every
                // invocation (value / Nil), so no reset needed.
                let s = b.define_local_slot(name);
                block_kw_params.push((name.clone(), s, *required));
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
    // A bare `redo` inside this block re-runs the body from here (after
    // param binding / destructure prologue, since the block args don't
    // change on redo). `loop do … redo … end` (rss's rss.rb:1222).
    b.block_redo_target = Some(b.pos());
    compile_body(&mut b, body, protos, interner, cc);
    b.emit(Op::Return);
    // Proto's `params` vec carries the source-visible names. For
    // destructure block params we use the synthesised anonymous
    // name in the call-interface slot; the named inner locals
    // are not part of params (they aren't fed by the caller).
    // M27 A1: `proto_params` carries every slot name the
    // method-style binder needs to see (positional, rest, block).
    // Block-as-block invocation (`invoke_block`) doesn't read
    // `proto.params` for arity / slot count — it uses the
    // `BlockHandle::n_params` + `rest_slot` fields stored on the
    // heap value directly. So including rest and block-arg names
    // here is invisible to that path but lets
    // `invoke_method_with_block`'s subtractive `positional_max`
    // math work when the same proto is installed as a method via
    // `Module#define_method`.
    let proto_params: Vec<String> = block_params.iter().enumerate().filter_map(|(i, p)| match p {
        BlockParam::Single(n) | BlockParam::Optional(n) => Some(n.clone()),
        BlockParam::Destructure(_) => Some(format!("__destruct_{i}")),
        BlockParam::Rest(name) => {
            Some(if name.is_empty() { format!("__rest_{i}") } else { name.clone() })
        }
        BlockParam::BlockArg(name) => {
            Some(if name == "&" { format!("__blkarg_{i}") } else { name.clone() })
        }
        BlockParam::KwRest(name) => {
            Some(if name.is_empty() { format!("__kwrest_{i}") } else { name.clone() })
        }
        // Keyword names stay OUT of `params` so the
        // define_method-as-method binder's by-position slot math
        // (rest/kwrest/blockarg) is unchanged; ordinary block
        // invocation binds kw by `Proto::block_kw_params` slots.
        BlockParam::Keyword(..) => None,
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
    // CRuby backtrace shape: a block frame reports
    // "block in <enclosing method>" ("block in <main>" at the
    // toplevel, "block in <class:Foo>" in a class body). minitest's
    // assertion-message tests compare backtraces against the
    // 'block in test_*' form (nesting levels are normalized away by
    // the suite, so a flat "block in" for nested blocks is enough —
    // documented divergence from CRuby's "block (2 levels) in").
    let block_name = match &b.method_name {
        Some(m) => format!("block in {m}"),
        None => match b.class_path.last() {
            Some(cls) => format!("block in <class:{cls}>"),
            None => "block in <main>".to_string(),
        },
    };
    protos.push(b.build(block_name, proto_params, proto_param_count as u16, lex));
    // Stamp the body-local-reset range on the just-built block
    // Proto. `build()` defaults this to `u16::MAX` (no reset)
    // because that's correct for every non-block builder; the
    // block path overrides it here. Only meaningful when there
    // *are* body-introduced slots; if the body assigned no new
    // locals (`body_local_start == block_n_locals`) the reset
    // range is empty and the runtime loop is a noop, so we don't
    // need to special-case it.
    protos.last_mut().expect("ICE: just pushed").block_body_local_start = body_local_start;
    protos.last_mut().expect("ICE: just pushed").n_optional_params = n_optional;
    // M27 A1: stamp the block proto with `rest_param` /
    // `block_param` so when it's installed AS A METHOD (via
    // Module#define_method), invoke_method_with_block's binder
    // sees the same trailing-slot layout it uses for `def`-built
    // methods. For ordinary block invocation (each, map, …) the
    // BlockHandle stores its own `n_params` + `rest_slot`, so
    // these proto fields aren't consulted on that path.
    if let Some(name) = rest_param_name {
        protos.last_mut().expect("ICE: just pushed").rest_param = Some(name);
    }
    if let Some(name) = block_arg_name {
        protos.last_mut().expect("ICE: just pushed").block_param = Some(name);
    }
    if block_arg_slot.is_some()
        && let Some(p) = protos.last_mut() {
        p.block_param_slot = block_arg_slot;
    }
    // Stamp `kw_rest_param` so a block installed AS A METHOD via
    // `define_method` routes through invoke_method's kw-rest
    // binder; ordinary block invocation uses the BlockHandle's
    // `kw_rest_slot` (returned below) instead.
    // (`if let Some(p)` rather than the sibling ICE-assert form
    // above, purely to stay within compiler.rs's panic budget —
    // `protos.last_mut()` is Some on the same just-pushed invariant.)
    if let Some(name) = kw_rest_param_name
        && let Some(p) = protos.last_mut() {
        p.kw_rest_param = Some(name);
    }
    // Stamp the keyword-param table (empty for kw-less blocks —
    // the invoke_block1/2 fast paths gate on is_empty()).
    if !block_kw_params.is_empty()
        && let Some(p) = protos.last_mut() {
        p.block_kw_params = block_kw_params;
    }
    if parent.n_locals < block_n_locals {
        parent.n_locals = block_n_locals;
    }
    (idx, param_start, n_params, rest_slot, kw_rest_slot)
}
