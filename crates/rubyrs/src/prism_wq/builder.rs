//! Port of `Parser::Builders::Default` (parser 3.3.7.0) — the subset
//! reachable from the prism translation compiler — plus the
//! `Prism::Translation::Parser::Builder` override (`block`/`itarg`).
//!
//! Specialized to the flag configuration RuboCop's `BuilderPrism` runs with
//! (verified empirically + guarded by the Ruby hook before the native path is
//! taken): `emit_forward_arg = true`, `emit_match_pattern = true`, every other
//! `emit_*` class flag falsy, `emit_file_line_as_literals = true`. The
//! version-dependent branches are fixed for `@parser.version >= 33`
//! (Parser33/34/40/41 — the only classes the hook routes here).
//!
//! Parser state the real builder consults but that is inert on the prism
//! translation path (`context.in_def`, `max_numparam_stack`, `static_env`
//! reads) is omitted; `pattern_variables` / `pattern_hash_keys` are modeled
//! because their duplicate checks produce diagnostics. `static_env.declare`
//! calls are write-only on this path and dropped.
//!
//! Node/`updated` identity: the Ruby builder often builds a node and
//! immediately replaces it via `updated` (e.g. `assign` = append child + new
//! map). Only the FINAL tree is materialized, so those rewrites are performed
//! in place here — observable state (type/children/map, parent links set by
//! the last constructor) is identical.

use crate::intern::SymId;
use crate::value::Value;

use super::{decline, ArgVal, CRes, Ctx, Decline, DiagRow, R};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A whitequark AST node under construction.
pub(crate) struct WqNode {
    pub(crate) ty: &'static str,
    pub(crate) children: Vec<Ch>,
    pub(crate) map: Option<Map>,
}

/// One child slot: a sub-node or a scalar Ruby value (nil / Symbol / String /
/// Integer / Float / Rational / Complex instance).
pub(crate) enum Ch {
    N(Box<WqNode>),
    V(Value),
}

impl WqNode {
    pub(crate) fn expr(&self) -> CRes<R> {
        match &self.map {
            Some(Map { expr: Some(e), .. }) => Ok(*e),
            _ => decline("node without map/expression queried for expression"),
        }
    }

    /// First child as a symbol name string.
    fn name_str(&self, vm: &crate::vm::Vm) -> Option<String> {
        match self.children.first() {
            Some(Ch::V(Value::Sym(s))) => Some(vm.interner.resolve(*s).to_string()),
            _ => None,
        }
    }
}

/// `Parser::Source::Map` and subclasses, as pure data. `expr` is the
/// `@expression` range; `k` selects the Ruby class + extra ivars.
#[derive(Clone)]
pub(crate) struct Map {
    /// `@expression` — None models the gem's nil-expression maps (an empty
    /// arg list with no delimiters: `collection_map(nil, [], nil)`).
    pub(crate) expr: Option<R>,
    pub(crate) k: MK,
}

#[derive(Clone)]
pub(crate) enum MK {
    /// `Parser::Source::Map` (base class).
    Bare,
    Collection { b: Option<R>, e: Option<R> },
    /// `op: Some` ⇢ the `@operator` ivar exists (set via `with_operator`).
    Constant { dc: Option<R>, name: R, op: Option<R> },
    Variable { name: Option<R>, op: Option<R> },
    /// `@operator` always present (may be nil): `Map::Operator`.
    Operator { op: Option<R> },
    Send { dot: Option<R>, sel: Option<R>, b: Option<R>, e: Option<R>, op: Option<R> },
    Condition { kw: Option<R>, b: Option<R>, els: Option<R>, e: Option<R> },
    Keyword { kw: Option<R>, b: Option<R>, e: Option<R> },
    Ternary { q: R, c: R },
    For { kw: R, inn: R, b: Option<R>, e: R },
    Definition { kw: R, op: Option<R>, name: Option<R>, e: Option<R> },
    MethodDefinition { kw: R, op: Option<R>, name: R, e: Option<R>, assign: Option<R> },
    RescueBody { kw: R, assoc: Option<R>, b: Option<R> },
    Heredoc { body: R, hd_end: R },
}

impl Map {
    pub(crate) fn with_expression(mut self, expr: R) -> Map {
        self.expr = Some(expr);
        self
    }

    /// `Map#with_operator` — only defined on Variable/Constant/Send (and
    /// Index, unreachable with emit_index=false). Anything else is a port
    /// bug ⇒ decline.
    pub(crate) fn with_operator(mut self, operator: R) -> CRes<Map> {
        match &mut self.k {
            MK::Variable { op, .. } | MK::Constant { op, .. } | MK::Send { op, .. } => {
                *op = Some(operator);
                Ok(self)
            }
            _ => decline("with_operator on unsupported map"),
        }
    }

    /// `map.keyword` for Keyword/Condition maps (`block()` needs it).
    pub(crate) fn keyword(&self) -> Option<R> {
        match &self.k {
            MK::Keyword { kw, .. } | MK::Condition { kw, .. } => *kw,
            _ => None,
        }
    }
}

/// A parser-gem "token" `[value, range]` as handed between the compiler and
/// builder. `TV::B` is a String value (bytes, buffer encoding), `TV::S` a
/// Symbol (constant-pool names).
pub(crate) struct Tok {
    pub(crate) v: TV,
    pub(crate) r: R,
}

pub(crate) enum TV {
    B(Vec<u8>),
    S(SymId),
}

impl Tok {
    pub(crate) fn b(bytes: impl Into<Vec<u8>>, r: R) -> Tok {
        Tok { v: TV::B(bytes.into()), r }
    }
    pub(crate) fn s(sym: SymId, r: R) -> Tok {
        Tok { v: TV::S(sym), r }
    }
    pub(crate) fn bytes(&self) -> CRes<&[u8]> {
        match &self.v {
            TV::B(b) => Ok(b),
            TV::S(_) => decline("token bytes on symbol token"),
        }
    }
}

/// `call_operator` token: `[:dot|:anddot|"::", range]`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DotKind {
    Dot,
    AndDot,
    ColonColon,
}

pub(crate) type ODot = Option<(DotKind, R)>;

pub(crate) fn loc(t: &Option<Tok>) -> Option<R> {
    t.as_ref().map(|t| t.r)
}

// n / n0
pub(crate) fn n(ty: &'static str, children: Vec<Ch>, map: Map) -> Box<WqNode> {
    Box::new(WqNode { ty, children, map: Some(map) })
}

pub(crate) fn n_nomap(ty: &'static str, children: Vec<Ch>) -> Box<WqNode> {
    Box::new(WqNode { ty, children, map: None })
}

pub(crate) fn join_exprs(l: &WqNode, r: &WqNode) -> CRes<R> {
    Ok(l.expr()?.join(r.expr()?))
}

// ---------------------------------------------------------------------------
// Source maps (free fns — no ctx needed)
// ---------------------------------------------------------------------------

pub(crate) fn token_map(t: &Tok) -> Map {
    Map { expr: Some(t.r), k: MK::Bare }
}

pub(crate) fn expr_map(r: R) -> Map {
    Map { expr: Some(r), k: MK::Bare }
}

pub(crate) fn prefix_string_map(r: R) -> Map {
    Map { expr: Some(r), k: MK::Collection { b: Some(R { b: r.b, e: r.b + 1 }), e: None } }
}

pub(crate) fn unquoted_map(r: R) -> Map {
    Map { expr: Some(r), k: MK::Collection { b: None, e: None } }
}

/// `parts.any? ? join_exprs(parts.first, parts.last) : nil` — Ruby's `any?`
/// is truthiness-based, so an all-nil parts list yields None; a mixed list
/// with a nil first/last would raise in Ruby (unreachable on valid input) —
/// decline.
fn first_last_expr(parts: &[Ch]) -> CRes<Option<R>> {
    let any = parts.iter().any(|c| !matches!(c, Ch::V(Value::Nil)));
    if !any {
        return Ok(None);
    }
    let first = match parts.first() {
        Some(Ch::N(n)) => n.expr()?,
        _ => return decline("nil first part in collection expr"),
    };
    let last = match parts.last() {
        Some(Ch::N(n)) => n.expr()?,
        _ => return decline("nil last part in collection expr"),
    };
    Ok(Some(first.join(last)))
}

pub(crate) fn collection_map(begin_t: &Option<Tok>, parts: &[Ch], end_t: &Option<Tok>) -> CRes<Map> {
    let b = loc(begin_t);
    let e = loc(end_t);
    let expr = if let (Some(b), Some(e)) = (b, e) {
        Some(b.join(e))
    } else if let Some(r) = first_last_expr(parts)? {
        Some(r)
    } else {
        // Falls back to whichever delimiter exists; `collection_map(nil,
        // [], nil)` — a Collection map with a nil expression (empty
        // paren-less arg lists) — yields None.
        b.or(e)
    };
    Ok(Map { expr, k: MK::Collection { b, e } })
}

pub(crate) fn string_map(begin_t: &Option<Tok>, parts: &[Ch], end_t: &Option<Tok>) -> CRes<Map> {
    if let Some(bt) = begin_t
        && bt.bytes()?.starts_with(b"<<")
    {
        let end_l = loc(end_t).ok_or(Decline("heredoc map without end"))?;
        let expr = match first_last_expr(parts)? {
            Some(r) => r,
            // `loc(end_t).begin` — a zero-length range at the heredoc end.
            None => R { b: end_l.b, e: end_l.b },
        };
        Ok(Map { expr: Some(bt.r), k: MK::Heredoc { body: expr, hd_end: end_l } })
    } else {
        collection_map(begin_t, parts, end_t)
    }
}

pub(crate) fn regexp_map(begin_t: &Tok, end_t: &Tok, options_e: &WqNode) -> CRes<Map> {
    Ok(Map {
        expr: Some(begin_t.r.join(options_e.expr()?)),
        k: MK::Collection { b: Some(begin_t.r), e: Some(end_t.r) },
    })
}

pub(crate) fn constant_map(scope: Option<&WqNode>, colon2_t: Option<R>, name_r: R) -> CRes<Map> {
    let expr = match scope {
        Some(s) => s.expr()?.join(name_r),
        None => name_r,
    };
    Ok(Map { expr: Some(expr), k: MK::Constant { dc: colon2_t, name: name_r, op: None } })
}

pub(crate) fn variable_map(name_r: R) -> Map {
    Map { expr: Some(name_r), k: MK::Variable { name: Some(name_r), op: None } }
}

pub(crate) fn binary_op_map(left_e: &WqNode, op_r: R, right_e: &WqNode) -> CRes<Map> {
    Ok(Map { expr: Some(join_exprs(left_e, right_e)?), k: MK::Operator { op: Some(op_r) } })
}

pub(crate) fn unary_op_map(op_t: &Tok, arg_e: Option<&WqNode>) -> CRes<Map> {
    let expr = match arg_e {
        Some(a) => op_t.r.join(a.expr()?),
        None => op_t.r,
    };
    Ok(Map { expr: Some(expr), k: MK::Operator { op: Some(op_t.r) } })
}

pub(crate) fn range_map(start_e: Option<&WqNode>, op_t: &Tok, end_e: Option<&WqNode>) -> CRes<Map> {
    let expr = match (start_e, end_e) {
        (Some(s), Some(e)) => join_exprs(s, e)?,
        (Some(s), None) => s.expr()?.join(op_t.r),
        (None, Some(e)) => op_t.r.join(e.expr()?),
        (None, None) => return decline("beginless+endless range"),
    };
    Ok(Map { expr: Some(expr), k: MK::Operator { op: Some(op_t.r) } })
}

pub(crate) fn arg_prefix_map(op_t: &Tok, name_t: &Option<Tok>) -> Map {
    let expr = match loc(name_t) {
        Some(n) => op_t.r.join(n),
        None => op_t.r,
    };
    Map { expr: Some(expr), k: MK::Variable { name: loc(name_t), op: None } }
}

pub(crate) fn kwarg_map(name_t: &Tok, value_e: Option<&WqNode>) -> CRes<Map> {
    let label = name_t.r;
    let name_range = R { b: label.b, e: label.e - 1 };
    let expr = match value_e {
        Some(v) => label.join(v.expr()?),
        None => label,
    };
    Ok(Map { expr: Some(expr), k: MK::Variable { name: Some(name_range), op: None } })
}

pub(crate) fn module_definition_map(
    keyword_t: &Tok,
    name_e: Option<&WqNode>,
    operator_t: Option<R>,
    end_t: &Tok,
) -> CRes<Map> {
    let name_l = match name_e {
        Some(n) => Some(n.expr()?),
        None => None,
    };
    // Definition#initialize computes @expression = @keyword.join(@end).
    Ok(Map {
        expr: Some(keyword_t.r.join(end_t.r)),
        k: MK::Definition { kw: keyword_t.r, op: operator_t, name: name_l, e: Some(end_t.r) },
    })
}

pub(crate) fn definition_map(keyword_t: &Tok, operator_t: Option<R>, name_t: &Tok, end_t: &Tok) -> Map {
    Map {
        expr: Some(keyword_t.r.join(end_t.r)),
        k: MK::MethodDefinition {
            kw: keyword_t.r,
            op: operator_t,
            name: name_t.r,
            e: Some(end_t.r),
            assign: None,
        },
    }
}

pub(crate) fn endless_definition_map(
    keyword_t: &Tok,
    operator_t: Option<R>,
    name_t: &Tok,
    assignment_t: &Tok,
    body_e: &WqNode,
) -> CRes<Map> {
    Ok(Map {
        expr: Some(keyword_t.r.join(body_e.expr()?)),
        k: MK::MethodDefinition {
            kw: keyword_t.r,
            op: operator_t,
            name: name_t.r,
            e: None,
            assign: Some(assignment_t.r),
        },
    })
}

pub(crate) fn send_map(
    receiver_e: Option<&WqNode>,
    dot_t: ODot,
    selector_t: Option<R>,
    begin_t: &Option<Tok>,
    args: &[Ch],
    end_t: &Option<Tok>,
) -> CRes<Map> {
    let begin_l = if let Some(r) = receiver_e { Some(r.expr()?) } else { selector_t };
    let end_l = if let Some(e) = loc(end_t) {
        Some(e)
    } else if let Some(last) = args.last() {
        match last {
            Ch::N(n) => Some(n.expr()?),
            Ch::V(_) => return decline("scalar last arg in send_map"),
        }
    } else {
        selector_t
    };
    let (Some(bl), Some(el)) = (begin_l, end_l) else {
        return decline("send_map with no begin/end");
    };
    Ok(Map {
        expr: Some(bl.join(el)),
        k: MK::Send { dot: dot_t.map(|d| d.1), sel: selector_t, b: loc(begin_t), e: loc(end_t), op: None },
    })
}

pub(crate) fn send_binary_op_map(lhs_e: &WqNode, selector_r: R, rhs_e: &WqNode) -> CRes<Map> {
    Ok(Map {
        expr: Some(join_exprs(lhs_e, rhs_e)?),
        k: MK::Send { dot: None, sel: Some(selector_r), b: None, e: None, op: None },
    })
}

pub(crate) fn send_unary_op_map(selector_r: R, arg_e: Option<&WqNode>) -> CRes<Map> {
    let expr = match arg_e {
        Some(a) => selector_r.join(a.expr()?),
        None => selector_r,
    };
    Ok(Map { expr: Some(expr), k: MK::Send { dot: None, sel: Some(selector_r), b: None, e: None, op: None } })
}

/// `send_index_map` — emit_index=false: `foo[bar]` is a send whose selector
/// spans the bracket pair.
pub(crate) fn send_index_map(receiver_e: &WqNode, lbrack_t: &Tok, rbrack_t: &Tok) -> CRes<Map> {
    Ok(Map {
        expr: Some(receiver_e.expr()?.join(rbrack_t.r)),
        k: MK::Send { dot: None, sel: Some(lbrack_t.r.join(rbrack_t.r)), b: None, e: None, op: None },
    })
}

pub(crate) fn block_map(receiver_l: R, begin_t: &Tok, end_t: &Tok) -> Map {
    Map { expr: Some(receiver_l.join(end_t.r)), k: MK::Collection { b: Some(begin_t.r), e: Some(end_t.r) } }
}

pub(crate) fn keyword_map(
    keyword_t: &Tok,
    begin_t: &Option<Tok>,
    args: Option<&[Ch]>,
    end_t: &Option<Tok>,
) -> CRes<Map> {
    let args = args.unwrap_or(&[]);
    let any = args.iter().any(|c| !matches!(c, Ch::V(Value::Nil)));
    let node_expr = |c: &Ch| -> CRes<R> {
        match c {
            Ch::N(n) => n.expr(),
            Ch::V(_) => decline("nil arg in keyword_map end"),
        }
    };
    let end_l = if let Some(e) = loc(end_t) {
        e
    } else if any && !matches!(args.last(), Some(Ch::V(Value::Nil))) {
        node_expr(args.last().unwrap())?
    } else if any && args.len() > 1 {
        node_expr(&args[args.len() - 2])?
    } else {
        keyword_t.r
    };
    Ok(Map {
        expr: Some(keyword_t.r.join(end_l)),
        k: MK::Keyword { kw: Some(keyword_t.r), b: loc(begin_t), e: loc(end_t) },
    })
}

pub(crate) fn keyword_mod_map(pre_e: &WqNode, keyword_t: &Tok, post_e: &WqNode) -> CRes<Map> {
    Ok(Map {
        expr: Some(join_exprs(pre_e, post_e)?),
        k: MK::Keyword { kw: Some(keyword_t.r), b: None, e: None },
    })
}

pub(crate) fn condition_map(
    keyword_r: R,
    cond_e: Option<&WqNode>,
    begin_t: Option<R>,
    body_e: Option<&WqNode>,
    else_t: Option<R>,
    else_e: Option<&WqNode>,
    end_t: Option<R>,
) -> CRes<Map> {
    let end_l = if let Some(e) = end_t {
        e
    } else if let Some(else_e) = else_e
        && else_e.map.as_ref().is_some_and(|m| m.expr.is_some())
    {
        else_e.expr()?
    } else if let Some(e) = else_t {
        e
    } else if let Some(body_e) = body_e
        && body_e.map.as_ref().is_some_and(|m| m.expr.is_some())
    {
        body_e.expr()?
    } else if let Some(b) = begin_t {
        b
    } else {
        match cond_e {
            Some(c) => c.expr()?,
            None => return decline("condition_map without cond"),
        }
    };
    Ok(Map {
        expr: Some(keyword_r.join(end_l)),
        k: MK::Condition { kw: Some(keyword_r), b: begin_t, els: else_t, e: end_t },
    })
}

pub(crate) fn ternary_map(begin_e: &WqNode, question_r: R, colon_r: R, end_e: &WqNode) -> CRes<Map> {
    Ok(Map { expr: Some(join_exprs(begin_e, end_e)?), k: MK::Ternary { q: question_r, c: colon_r } })
}

pub(crate) fn for_map(keyword_t: &Tok, in_t: &Tok, begin_t: &Option<Tok>, end_t: &Tok) -> Map {
    Map {
        expr: Some(keyword_t.r.join(end_t.r)),
        k: MK::For { kw: keyword_t.r, inn: in_t.r, b: loc(begin_t), e: end_t.r },
    }
}

pub(crate) fn rescue_body_map(
    keyword_t: &Tok,
    exc_list_e: Option<&WqNode>,
    assoc_t: &Option<Tok>,
    exc_var_e: Option<&WqNode>,
    then_t: &Option<Tok>,
    compstmt_e: Option<&WqNode>,
) -> CRes<Map> {
    let mut end_l: Option<R> = match compstmt_e {
        Some(c) => Some(c.expr()?),
        None => None,
    };
    if end_l.is_none() {
        end_l = loc(then_t);
    }
    if end_l.is_none()
        && let Some(v) = exc_var_e
    {
        end_l = Some(v.expr()?);
    }
    if end_l.is_none()
        && let Some(l) = exc_list_e
    {
        end_l = Some(l.expr()?);
    }
    let end_l = end_l.unwrap_or(keyword_t.r);
    Ok(Map {
        expr: Some(keyword_t.r.join(end_l)),
        k: MK::RescueBody { kw: keyword_t.r, assoc: loc(assoc_t), b: loc(then_t) },
    })
}

pub(crate) fn eh_keyword_map(
    compstmt_e: Option<&WqNode>,
    keyword_t: &Option<Tok>,
    body_es: &[&WqNode],
    else_t: &Option<Tok>,
    else_e: Option<&WqNode>,
) -> CRes<Map> {
    let begin_l = if let Some(c) = compstmt_e {
        c.expr()?
    } else if let Some(k) = loc(keyword_t) {
        k
    } else {
        match body_es.first() {
            Some(b) => b.expr()?,
            None => return decline("eh_keyword_map without begin"),
        }
    };
    let end_l = if let Some(et) = loc(else_t) {
        match else_e {
            Some(e) => e.expr()?,
            None => et,
        }
    } else if let Some(last) = body_es.last() {
        last.expr()?
    } else {
        match loc(keyword_t) {
            Some(k) => k,
            None => return decline("eh_keyword_map without end"),
        }
    };
    Ok(Map {
        expr: Some(begin_l.join(end_l)),
        k: MK::Condition { kw: loc(keyword_t), b: None, els: loc(else_t), e: None },
    })
}

pub(crate) fn guard_map(keyword_t: &Tok, guard_body_e: &WqNode) -> CRes<Map> {
    Ok(Map {
        expr: Some(keyword_t.r.join(guard_body_e.expr()?)),
        k: MK::Keyword { kw: Some(keyword_t.r), b: None, e: None },
    })
}

/// `parts.one? && [:str, :dstr].include?(parts.first.type)`.
pub(crate) fn collapse_string_parts(parts: &[Ch]) -> bool {
    if parts.len() != 1 {
        return false;
    }
    matches!(&parts[0], Ch::N(n) if n.ty == "str" || n.ty == "dstr")
}

// ---------------------------------------------------------------------------
// Builder methods (on Ctx: diagnostics + interner + heap access)
// ---------------------------------------------------------------------------

impl<'a> Ctx<'a> {
    /// `value(token).to_sym`.
    pub(crate) fn tok_sym(&mut self, t: &Tok) -> SymId {
        match &t.v {
            TV::B(b) => {
                let b = b.clone();
                self.intern_bytes(&b)
            }
            TV::S(s) => *s,
        }
    }

    /// Token value as an (unfrozen) Ruby String — what `location.slice` /
    /// `value(token)` yields.
    pub(crate) fn tok_str_val(&mut self, t: &Tok) -> CRes<Value> {
        match &t.v {
            TV::B(b) => Ok(self.str_val(b.clone(), false)),
            TV::S(_) => decline("token string on symbol token"),
        }
    }

    /// A Ruby String value with the buffer encoding.
    pub(crate) fn str_val(&self, bytes: Vec<u8>, frozen: bool) -> Value {
        let rs = crate::value::RStr::from_bytes(bytes);
        rs.encoding.set(self.enc);
        rs.frozen.set(frozen);
        Value::Str(std::rc::Rc::new(rs))
    }

    pub(crate) fn diagnostic(
        &mut self,
        level: &'static str,
        reason: &'static str,
        args: Vec<(&'static str, ArgVal)>,
        loc: R,
        highlights: Vec<R>,
    ) {
        self.diags.push(DiagRow {
            prism: false,
            level,
            reason: reason.to_string(),
            message: None,
            args,
            loc,
            highlights,
        });
        // `if type == :error → @parser.send(:yyerror)` — a no-op in
        // Translation::Parser. The Ruby hook replays rows through
        // `diagnostics.process`, which (on rubyrs, where errors are fatal)
        // raises at exactly the point the interpreted build would have.
    }

    // ----- literals -----

    pub(crate) fn b_nil(&mut self, t: &Tok) -> Box<WqNode> {
        n("nil", vec![], token_map(t))
    }
    pub(crate) fn b_true(&mut self, t: &Tok) -> Box<WqNode> {
        n("true", vec![], token_map(t))
    }
    pub(crate) fn b_false(&mut self, t: &Tok) -> Box<WqNode> {
        n("false", vec![], token_map(t))
    }
    pub(crate) fn b_self(&mut self, t: &Tok) -> Box<WqNode> {
        n("self", vec![], token_map(t))
    }

    /// `numeric(kind, token)` — the token value is the Ruby numeric.
    pub(crate) fn b_numeric(&mut self, kind: &'static str, value: Value, r: R) -> Box<WqNode> {
        n(kind, vec![Ch::V(value)], Map { expr: Some(r), k: MK::Operator { op: None } })
    }

    /// `unary_num(unary_t, numeric)`. NOTE (spec quirk): the prism compiler
    /// passes `[slice[0].to_sym, range]` — a SYMBOL — while `unary_num`
    /// compares `value(unary_t)` against the STRINGS '+'/'-', so the numeric
    /// value is never rewritten on this path; only the map changes (prism's
    /// node.value already carries the sign).
    pub(crate) fn b_unary_num(&mut self, sign_r: R, mut numeric: Box<WqNode>) -> CRes<Box<WqNode>> {
        let old_expr = numeric.expr()?;
        numeric.map = Some(Map {
            expr: Some(sign_r.join(old_expr)),
            k: MK::Operator { op: Some(sign_r) },
        });
        Ok(numeric)
    }

    pub(crate) fn check_alloc(&mut self) -> CRes<()> {
        self.vm.check_alloc().map_err(|_| Decline("alloc cap"))
    }

    /// Canonical Rational value (mirrors `Kernel#Rational` normalization).
    pub(crate) fn rational_val(&mut self, num: i64, den: i64) -> CRes<Value> {
        fn gcd(a: i64, b: i64) -> i64 {
            let (mut a, mut b) = (a.unsigned_abs(), b.unsigned_abs());
            while b != 0 {
                let t = a % b;
                a = b;
                b = t;
            }
            (a.max(1)) as i64
        }
        if den == 0 {
            return decline("rational with zero denominator");
        }
        let g = gcd(num, den);
        let (mut num, mut den) = (num / g, den / g);
        if den < 0 {
            num = -num;
            den = -den;
        }
        self.check_alloc()?;
        #[cfg(feature = "bignum")]
        let repr = crate::heap::RationalRepr { num: num.into(), den: den.into() };
        #[cfg(not(feature = "bignum"))]
        let repr = crate::heap::RationalRepr { num, den };
        let id = self.vm.heap.alloc(crate::heap::HeapObj::Rational(repr));
        Ok(Value::Rational(id))
    }

    /// `Complex(real, imag)` — instance of the preamble Complex class.
    pub(crate) fn complex_val(&mut self, real: Value, imag: Value) -> CRes<Value> {
        let id = self.vm.interner.intern("Complex");
        let Some(class) = self.vm.classes.get(&id).cloned() else {
            return decline("Complex class missing");
        };
        let real_sym = self.vm.interner.intern("@real");
        let imag_sym = self.vm.interner.intern("@imaginary");
        let mut ivars = crate::value::IvarTable::default();
        ivars.insert(&class, real_sym, real);
        ivars.insert(&class, imag_sym, imag);
        self.check_alloc()?;
        let oid = self.vm.heap.alloc(crate::heap::HeapObj::Instance(crate::value::Instance {
            class,
            ivars,
            singleton_class: None,
            frozen: std::cell::Cell::new(false),
        }));
        Ok(Value::Object(oid))
    }

    // ----- strings / symbols -----

    pub(crate) fn b_string_internal(&mut self, value: Value, r: R) -> Box<WqNode> {
        n("str", vec![Ch::V(value)], unquoted_map(r))
    }

    pub(crate) fn b_string_compose(
        &mut self,
        begin_t: Option<Tok>,
        parts: Vec<Ch>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        if collapse_string_parts(&parts) {
            if begin_t.is_none() && end_t.is_none() {
                let Some(Ch::N(first)) = parts.into_iter().next() else {
                    return decline("collapse without node");
                };
                return Ok(first);
            }
            let map = string_map(&begin_t, &parts, &end_t)?;
            let Some(Ch::N(first)) = parts.into_iter().next() else {
                return decline("collapse without node");
            };
            return Ok(n("str", first.children, map));
        }
        let map = string_map(&begin_t, &parts, &end_t)?;
        Ok(n("dstr", parts, map))
    }

    pub(crate) fn b_character(&mut self, value: Value, r: R) -> Box<WqNode> {
        n("str", vec![Ch::V(value)], prefix_string_map(r))
    }

    pub(crate) fn b_xstring_compose(
        &mut self,
        begin_t: Option<Tok>,
        parts: Vec<Ch>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let map = string_map(&begin_t, &parts, &end_t)?;
        Ok(n("xstr", parts, map))
    }

    pub(crate) fn b_symbol(&mut self, value_bytes: &[u8], r: R) -> Box<WqNode> {
        let sym = self.intern_bytes(value_bytes);
        n("sym", vec![Ch::V(Value::Sym(sym))], prefix_string_map(r))
    }

    pub(crate) fn b_symbol_internal(&mut self, value_bytes: &[u8], r: R) -> Box<WqNode> {
        let sym = self.intern_bytes(value_bytes);
        n("sym", vec![Ch::V(Value::Sym(sym))], unquoted_map(r))
    }

    pub(crate) fn b_symbol_compose(
        &mut self,
        begin_t: Option<Tok>,
        parts: Vec<Ch>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        if collapse_string_parts(&parts) {
            let Some(Ch::N(str_node)) = parts.first() else {
                return decline("collapse without node");
            };
            // n(:sym, [str.children.first.to_sym],
            //   collection_map(begin_t, str.loc.expression, end_t))
            // — with both delimiters present (always true on this path) the
            // "parts" argument is never consulted.
            let (Some(bt), Some(et)) = (&begin_t, &end_t) else {
                return decline("symbol_compose collapse without delimiters");
            };
            let sym = match str_node.children.first() {
                Some(Ch::V(Value::Str(s))) => {
                    let bytes = s.content.borrow().clone();
                    self.intern_bytes(&bytes)
                }
                _ => return decline("symbol_compose non-string first child"),
            };
            let map = Map {
                expr: Some(bt.r.join(et.r)),
                k: MK::Collection { b: Some(bt.r), e: Some(et.r) },
            };
            return Ok(n("sym", vec![Ch::V(Value::Sym(sym))], map));
        }
        let map = collection_map(&begin_t, &parts, &end_t)?;
        Ok(n("dsym", parts, map))
    }

    // ----- regexps -----

    pub(crate) fn b_regexp_options(&mut self, opt_bytes: &[u8], r: R) -> Box<WqNode> {
        let mut chars: Vec<u8> = opt_bytes.to_vec();
        chars.sort_unstable();
        chars.dedup();
        let children = chars
            .iter()
            .map(|c| {
                let s = (*c as char).to_string();
                Ch::V(Value::Sym(self.vm.interner.intern(&s)))
            })
            .collect();
        n("regopt", children, token_map(&Tok::b(opt_bytes.to_vec(), r)))
    }

    pub(crate) fn b_regexp_compose(
        &mut self,
        begin_t: Tok,
        mut parts: Vec<Ch>,
        end_t: Tok,
        options: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        // static_regexp validation — Regexp.new may "raise" → invalid_regexp.
        if let Some(err) = self.static_regexp_error(&parts, &options)? {
            let loc_r = begin_t.r.join(end_t.r);
            self.diagnostic("error", "invalid_regexp", vec![("message", ArgVal::Str(err))], loc_r, vec![]);
        }
        let map = regexp_map(&begin_t, &end_t, &options)?;
        parts.push(Ch::N(options));
        Ok(n("regexp", parts, map))
    }

    /// `static_string(nodes)` — concatenated bytes, or None when dynamic.
    pub(crate) fn static_string(&self, nodes: &[Ch]) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for node in nodes {
            let Ch::N(node) = node else { return None };
            match node.ty {
                "str" => match node.children.first() {
                    Some(Ch::V(Value::Str(s))) => out.extend_from_slice(&s.content.borrow()),
                    _ => return None,
                },
                "begin" => out.extend_from_slice(&self.static_string(&node.children)?),
                _ => return None,
            }
        }
        Some(out)
    }

    /// The `static_regexp` compile check, rubyrs-flavored: run the same
    /// preprocess+compile chain `Regexp.new` runs on this VM, so the produced
    /// diagnostic (or its absence — e.g. lazily-compiled fancy patterns)
    /// matches the interpreted path exactly. Returns the error message if
    /// compilation failed.
    fn static_regexp_error(&mut self, parts: &[Ch], options: &WqNode) -> CRes<Option<String>> {
        let Some(source) = self.static_string(parts) else { return Ok(None) };
        let mut has = |name: &str| -> bool {
            let id = self.vm.interner.intern(name);
            options.children.iter().any(|c| matches!(c, Ch::V(Value::Sym(s)) if *s == id))
        };
        let (e, s, nn, x) = (has("e"), has("s"), has("n"), has("x"));
        if (e || s || nn) && !source.is_ascii() {
            // The interpreted path re-encodes via String#encode — decline
            // rather than approximate.
            return decline("static_regexp with e/s/n encoding option");
        }
        #[cfg(feature = "regex")]
        {
            let pat = String::from_utf8_lossy(&source).into_owned();
            let flags: u8 = if x { crate::regex_engine::RB_EXTENDED } else { 0 };
            let translated = crate::vm::step::preprocess_regex_pattern(&pat);
            let prefixed = crate::vm::step::apply_ruby_flags(&translated, flags);
            match crate::regex_engine::compile_with_flags(&prefixed, flags, &translated) {
                Ok(_) => Ok(None),
                Err(err) => Ok(Some(format!("invalid regex /{}/: {}", pat, err))),
            }
        }
        #[cfg(not(feature = "regex"))]
        {
            let _ = x;
            decline("regex feature disabled")
        }
    }

    /// `static_regexp_node` truthiness for `match_op` (version >= 33 rules).
    fn static_regexp_node_ok(&mut self, receiver: &WqNode) -> CRes<bool> {
        if receiver.ty != "regexp" || receiver.children.is_empty() {
            return Ok(false);
        }
        let parts = &receiver.children[..receiver.children.len() - 1];
        if parts.iter().any(|c| !matches!(c, Ch::N(node) if node.ty == "str")) {
            return Ok(false);
        }
        let Some(Ch::N(options)) = receiver.children.last() else {
            return Ok(false);
        };
        if self.static_string(parts).is_none() {
            return Ok(false);
        }
        // An invalid pattern already produced a (fatal-on-rubyrs) diagnostic
        // in regexp_compose, so this outcome is unobservable then.
        Ok(self.static_regexp_error(parts, options)?.is_none())
    }

    // ----- collections -----

    pub(crate) fn b_array(
        &mut self,
        begin_t: Option<Tok>,
        elements: Vec<Ch>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let map = collection_map(&begin_t, &elements, &end_t)?;
        Ok(n("array", elements, map))
    }

    pub(crate) fn b_splat(&mut self, star_t: Tok, arg: Option<Box<WqNode>>) -> CRes<Box<WqNode>> {
        match arg {
            None => Ok(n("splat", vec![], unary_op_map(&star_t, None)?)),
            Some(arg) => {
                let map = unary_op_map(&star_t, Some(&arg))?;
                Ok(n("splat", vec![Ch::N(arg)], map))
            }
        }
    }

    pub(crate) fn b_pair(&mut self, key: Box<WqNode>, assoc_r: R, value: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&key, assoc_r, &value)?;
        Ok(n("pair", vec![Ch::N(key), Ch::N(value)], map))
    }

    pub(crate) fn b_pair_keyword(&mut self, key_bytes: &[u8], key_r: R, value: Box<WqNode>) -> CRes<Box<WqNode>> {
        let key_l = R { b: key_r.b, e: key_r.e - 1 };
        let colon_l = R { b: key_r.e - 1, e: key_r.e };
        let pair_expr = key_r.join(value.expr()?);
        let sym = self.intern_bytes(key_bytes);
        let key = n("sym", vec![Ch::V(Value::Sym(sym))], Map {
            expr: Some(key_l),
            k: MK::Collection { b: None, e: None },
        });
        Ok(n("pair", vec![Ch::N(key), Ch::N(value)], Map {
            expr: Some(pair_expr),
            k: MK::Operator { op: Some(colon_l) },
        }))
    }

    pub(crate) fn b_pair_quoted(
        &mut self,
        begin_t: Tok,
        parts: Vec<Ch>,
        end_t: Tok,
        value: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        // pair_quoted_map: quote_l/colon_l carved off the end token.
        let end_l = end_t.r;
        let quote_l = R { b: end_l.e - 2, e: end_l.e - 1 };
        let colon_l = R { b: end_l.e - 1, e: end_l.e };
        let pair_expr = begin_t.r.join(value.expr()?);
        let new_end = Tok::b(end_t.bytes()?.to_vec(), quote_l);
        let key = self.b_symbol_compose(Some(begin_t), parts, Some(new_end))?;
        Ok(n("pair", vec![Ch::N(key), Ch::N(value)], Map {
            expr: Some(pair_expr),
            k: MK::Operator { op: Some(colon_l) },
        }))
    }

    pub(crate) fn b_kwsplat(&mut self, dstar_t: Tok, arg: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = unary_op_map(&dstar_t, Some(&arg))?;
        Ok(n("kwsplat", vec![Ch::N(arg)], map))
    }

    pub(crate) fn b_associate(
        &mut self,
        begin_t: Option<Tok>,
        pairs: Vec<Ch>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        // Duplicate-key warning; key equality is AST::Node eql? over
        // sym/str/int/float (+ rational/complex/regexp at version >= 31).
        let mut seen: Vec<usize> = Vec::new();
        let mut dup_locs: Vec<R> = Vec::new();
        for (i, pair) in pairs.iter().enumerate() {
            let Ch::N(pair) = pair else { continue };
            if pair.ty != "pair" {
                continue;
            }
            let Some(Ch::N(key)) = pair.children.first() else { continue };
            match key.ty {
                "sym" | "str" | "int" | "float" => {}
                "rational" | "complex" | "regexp" => {
                    if self.version < 31 {
                        continue;
                    }
                }
                _ => continue,
            }
            let dup = seen.iter().any(|j| {
                if let (Ch::N(prev_pair), true) = (&pairs[*j], true)
                    && let Some(Ch::N(prev_key)) = prev_pair.children.first()
                {
                    node_eql(prev_key, key, self.vm)
                } else {
                    false
                }
            });
            if dup {
                dup_locs.push(key.expr()?);
            } else {
                seen.push(i);
            }
        }
        for loc_r in dup_locs {
            self.diagnostic("warning", "duplicate_hash_key", vec![], loc_r, vec![]);
        }
        let map = collection_map(&begin_t, &pairs, &end_t)?;
        Ok(n("hash", pairs, map))
    }

    pub(crate) fn b_range(
        &mut self,
        exclusive: bool,
        lhs: Option<Box<WqNode>>,
        op_t: Tok,
        rhs: Option<Box<WqNode>>,
    ) -> CRes<Box<WqNode>> {
        let map = range_map(lhs.as_deref(), &op_t, rhs.as_deref())?;
        let ty = if exclusive { "erange" } else { "irange" };
        let l = lhs.map(Ch::N).unwrap_or(Ch::V(Value::Nil));
        let r = rhs.map(Ch::N).unwrap_or(Ch::V(Value::Nil));
        Ok(n(ty, vec![l, r], map))
    }

    // ----- access -----

    pub(crate) fn b_ident(&mut self, t: &Tok) -> Box<WqNode> {
        let sym = self.tok_sym(t);
        n("ident", vec![Ch::V(Value::Sym(sym))], variable_map(t.r))
    }

    pub(crate) fn b_ivar(&mut self, t: &Tok) -> Box<WqNode> {
        let sym = self.tok_sym(t);
        n("ivar", vec![Ch::V(Value::Sym(sym))], variable_map(t.r))
    }

    pub(crate) fn b_gvar(&mut self, t: &Tok) -> Box<WqNode> {
        let name_bytes: Vec<u8> = match &t.v {
            TV::B(b) => b.clone(),
            TV::S(s) => self.vm.interner.resolve(*s).as_bytes().to_vec(),
        };
        if name_bytes.starts_with(b"$0") && name_bytes.len() > 2 {
            let name = String::from_utf8_lossy(&name_bytes).into_owned();
            self.diagnostic("error", "gvar_name", vec![("name", ArgVal::Str(name))], t.r, vec![]);
        }
        let sym = self.tok_sym(t);
        n("gvar", vec![Ch::V(Value::Sym(sym))], variable_map(t.r))
    }

    pub(crate) fn b_cvar(&mut self, t: &Tok) -> Box<WqNode> {
        let sym = self.tok_sym(t);
        n("cvar", vec![Ch::V(Value::Sym(sym))], variable_map(t.r))
    }

    pub(crate) fn b_back_ref(&mut self, t: &Tok) -> Box<WqNode> {
        let sym = self.tok_sym(t);
        n("back_ref", vec![Ch::V(Value::Sym(sym))], token_map(t))
    }

    pub(crate) fn b_nth_ref(&mut self, number: i64, r: R) -> Box<WqNode> {
        n("nth_ref", vec![Ch::V(Value::Int(number))], expr_map(r))
    }

    pub(crate) fn b_const(&mut self, name_sym: SymId, name_r: R) -> CRes<Box<WqNode>> {
        let map = constant_map(None, None, name_r)?;
        Ok(n("const", vec![Ch::V(Value::Nil), Ch::V(Value::Sym(name_sym))], map))
    }

    pub(crate) fn b_const_global(&mut self, colon3_t: Tok, name_sym: SymId, name_r: R) -> CRes<Box<WqNode>> {
        let cbase = n("cbase", vec![], token_map(&colon3_t));
        let map = constant_map(Some(&cbase), Some(colon3_t.r), name_r)?;
        Ok(n("const", vec![Ch::N(cbase), Ch::V(Value::Sym(name_sym))], map))
    }

    pub(crate) fn b_const_fetch(
        &mut self,
        scope: Box<WqNode>,
        colon2_r: R,
        name_sym: SymId,
        name_r: R,
    ) -> CRes<Box<WqNode>> {
        let map = constant_map(Some(&scope), Some(colon2_r), name_r)?;
        Ok(n("const", vec![Ch::N(scope), Ch::V(Value::Sym(name_sym))], map))
    }

    /// `accessible(__FILE__(t))` with emit_file_line_as_literals=true.
    pub(crate) fn b_accessible_file(&mut self, t: &Tok) -> Box<WqNode> {
        let name = self.str_val(self.buffer_name.clone(), false);
        n("str", vec![Ch::V(name)], token_map(t))
    }

    /// `accessible(__LINE__(t))` — the caller computes the line from the
    /// node's byte offset.
    pub(crate) fn b_accessible_line(&mut self, t: &Tok, line: i64) -> Box<WqNode> {
        n("int", vec![Ch::V(Value::Int(line))], token_map(t))
    }

    /// `accessible(__ENCODING__(t))` with emit_encoding=false:
    /// `s(:const, s(:const, nil, :Encoding), :UTF_8)`; the inner const has a
    /// NIL location.
    pub(crate) fn b_accessible_encoding(&mut self, t: &Tok) -> Box<WqNode> {
        let encoding_sym = self.vm.interner.intern("Encoding");
        let utf8_sym = self.vm.interner.intern("UTF_8");
        let inner = n_nomap("const", vec![Ch::V(Value::Nil), Ch::V(Value::Sym(encoding_sym))]);
        n("const", vec![Ch::N(inner), Ch::V(Value::Sym(utf8_sym))], token_map(t))
    }

    // ----- assignment -----

    pub(crate) fn b_assignable(&mut self, mut node: Box<WqNode>) -> CRes<Box<WqNode>> {
        match node.ty {
            "cvar" => node.ty = "cvasgn",
            "ivar" => node.ty = "ivasgn",
            "gvar" => node.ty = "gvasgn",
            // context.in_def is always false on the translation path (prism
            // reports dynamic-const writes itself as write_target_in_method).
            "const" => node.ty = "casgn",
            "ident" => {
                let name = node.name_str(self.vm).ok_or(Decline("assignable ident"))?;
                let name_loc = node.expr()?;
                self.check_reserved_for_numparam(&name, name_loc);
                node.ty = "lvasgn";
            }
            "match_var" => {
                let name = node.name_str(self.vm).ok_or(Decline("assignable match_var"))?;
                let name_loc = node.expr()?;
                self.check_reserved_for_numparam(&name, name_loc);
            }
            "nil" | "self" | "true" | "false" | "__FILE__" | "__LINE__" | "__ENCODING__" => {
                let loc_r = node.expr()?;
                self.diagnostic("error", "invalid_assignment", vec![], loc_r, vec![]);
            }
            "back_ref" | "nth_ref" => {
                let loc_r = node.expr()?;
                self.diagnostic("error", "backref_assignment", vec![], loc_r, vec![]);
            }
            _ => return decline("assignable: unexpected node type"),
        }
        Ok(node)
    }

    pub(crate) fn check_reserved_for_numparam(&mut self, name: &str, loc_r: R) {
        // @parser.version >= 30 always holds here.
        let b = name.as_bytes();
        if b.len() == 2 && b[0] == b'_' && b[1].is_ascii_digit() && b[1] != b'0' {
            self.diagnostic(
                "error",
                "reserved_for_numparam",
                vec![("name", ArgVal::Str(name.to_string()))],
                loc_r,
                vec![],
            );
        }
    }

    /// `assign(lhs, eql_t, rhs)` — appends rhs and rewrites the map.
    pub(crate) fn b_assign(&mut self, mut lhs: Box<WqNode>, eql_r: R, rhs: Box<WqNode>) -> CRes<Box<WqNode>> {
        let expr = join_exprs(&lhs, &rhs)?;
        let map = lhs
            .map
            .take()
            .ok_or(Decline("assign lhs without map"))?
            .with_operator(eql_r)?
            .with_expression(expr);
        lhs.map = Some(map);
        lhs.children.push(Ch::N(rhs));
        Ok(lhs)
    }

    /// `op_assign(lhs, [op, op_r], rhs)` — op is already `=`-chomped.
    pub(crate) fn b_op_assign(
        &mut self,
        lhs: Box<WqNode>,
        op_bytes: &[u8],
        op_r: R,
        rhs: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        match lhs.ty {
            "gvasgn" | "ivasgn" | "lvasgn" | "cvasgn" | "casgn" | "send" | "csend" => {
                let expr = join_exprs(&lhs, &rhs)?;
                // with_operator DUPs — the lhs child keeps its own map.
                let source_map = lhs
                    .map
                    .clone()
                    .ok_or(Decline("op_assign lhs without map"))?
                    .with_operator(op_r)?
                    .with_expression(expr);
                match op_bytes {
                    b"&&" => Ok(n("and_asgn", vec![Ch::N(lhs), Ch::N(rhs)], source_map)),
                    b"||" => Ok(n("or_asgn", vec![Ch::N(lhs), Ch::N(rhs)], source_map)),
                    _ => {
                        let op_sym = self.intern_bytes(op_bytes);
                        Ok(n(
                            "op_asgn",
                            vec![Ch::N(lhs), Ch::V(Value::Sym(op_sym)), Ch::N(rhs)],
                            source_map,
                        ))
                    }
                }
            }
            "back_ref" | "nth_ref" => {
                let loc_r = lhs.expr()?;
                self.diagnostic("error", "backref_assignment", vec![], loc_r, vec![]);
                // Fatal on rubyrs — the returned node is never observed.
                Ok(lhs)
            }
            _ => decline("op_assign: unexpected lhs"),
        }
    }

    pub(crate) fn b_multi_lhs(
        &mut self,
        begin_t: Option<Tok>,
        items: Vec<Ch>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let map = collection_map(&begin_t, &items, &end_t)?;
        Ok(n("mlhs", items, map))
    }

    pub(crate) fn b_multi_assign(&mut self, lhs: Box<WqNode>, eql_r: R, rhs: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&lhs, eql_r, &rhs)?;
        Ok(n("masgn", vec![Ch::N(lhs), Ch::N(rhs)], map))
    }

    // ----- class/module/method definition -----

    pub(crate) fn b_def_class(
        &mut self,
        class_t: Tok,
        name: Box<WqNode>,
        lt_t: Option<Tok>,
        superclass: Option<Box<WqNode>>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = module_definition_map(&class_t, Some(&name), loc(&lt_t), &end_t)?;
        Ok(n(
            "class",
            vec![Ch::N(name), opt_ch(superclass), opt_ch(body)],
            map,
        ))
    }

    pub(crate) fn b_def_sclass(
        &mut self,
        class_t: Tok,
        lshft_t: Tok,
        expr: Box<WqNode>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = module_definition_map(&class_t, None, Some(lshft_t.r), &end_t)?;
        Ok(n("sclass", vec![Ch::N(expr), opt_ch(body)], map))
    }

    pub(crate) fn b_def_module(
        &mut self,
        module_t: Tok,
        name: Box<WqNode>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = module_definition_map(&module_t, Some(&name), None, &end_t)?;
        Ok(n("module", vec![Ch::N(name), opt_ch(body)], map))
    }

    pub(crate) fn b_def_method(
        &mut self,
        def_t: Tok,
        name_t: Tok,
        args: Box<WqNode>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        let map = definition_map(&def_t, None, &name_t, &end_t);
        Ok(n("def", vec![Ch::V(Value::Sym(sym)), Ch::N(args), opt_ch(body)], map))
    }

    pub(crate) fn b_def_endless_method(
        &mut self,
        def_t: Tok,
        name_t: Tok,
        args: Box<WqNode>,
        assignment_t: Tok,
        body: Option<Box<WqNode>>,
    ) -> CRes<Box<WqNode>> {
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        let body_ref = body.as_deref().ok_or(Decline("endless def without body"))?;
        let map = endless_definition_map(&def_t, None, &name_t, &assignment_t, body_ref)?;
        Ok(n("def", vec![Ch::V(Value::Sym(sym)), Ch::N(args), opt_ch(body)], map))
    }

    #[allow(clippy::too_many_arguments)] // parser-gem builder signature — one Tok per source token
    pub(crate) fn b_def_singleton(
        &mut self,
        def_t: Tok,
        definee: Box<WqNode>,
        dot_t: Tok,
        name_t: Tok,
        args: Box<WqNode>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        self.validate_definee(&definee)?;
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        let map = definition_map(&def_t, Some(dot_t.r), &name_t, &end_t);
        Ok(n(
            "defs",
            vec![Ch::N(definee), Ch::V(Value::Sym(sym)), Ch::N(args), opt_ch(body)],
            map,
        ))
    }

    #[allow(clippy::too_many_arguments)] // parser-gem builder signature — one Tok per source token
    pub(crate) fn b_def_endless_singleton(
        &mut self,
        def_t: Tok,
        definee: Box<WqNode>,
        dot_t: Tok,
        name_t: Tok,
        args: Box<WqNode>,
        assignment_t: Tok,
        body: Option<Box<WqNode>>,
    ) -> CRes<Box<WqNode>> {
        self.validate_definee(&definee)?;
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        let body_ref = body.as_deref().ok_or(Decline("endless defs without body"))?;
        let map = endless_definition_map(&def_t, Some(dot_t.r), &name_t, &assignment_t, body_ref)?;
        Ok(n(
            "defs",
            vec![Ch::N(definee), Ch::V(Value::Sym(sym)), Ch::N(args), opt_ch(body)],
            map,
        ))
    }

    fn tok_name_string(&mut self, t: &Tok) -> String {
        match &t.v {
            TV::B(b) => String::from_utf8_lossy(b).into_owned(),
            TV::S(s) => self.vm.interner.resolve(*s).to_string(),
        }
    }

    fn validate_definee(&mut self, definee: &WqNode) -> CRes<()> {
        match definee.ty {
            "int" | "str" | "dstr" | "sym" | "dsym" | "regexp" | "array" | "hash" => {
                let loc_r = definee.expr()?;
                self.diagnostic("error", "singleton_literal", vec![], loc_r, vec![]);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn b_undef_method(&mut self, undef_t: Tok, names: Vec<Ch>) -> CRes<Box<WqNode>> {
        let map = keyword_map(&undef_t, &None, Some(&names), &None)?;
        Ok(n("undef", names, map))
    }

    pub(crate) fn b_alias(&mut self, alias_t: Tok, to: Box<WqNode>, from: Box<WqNode>) -> CRes<Box<WqNode>> {
        let pair = [Ch::N(to), Ch::N(from)];
        let map = keyword_map(&alias_t, &None, Some(&pair), &None)?;
        let [to, from] = pair;
        Ok(n("alias", vec![to, from], map))
    }

    // ----- formal arguments -----

    pub(crate) fn b_args(
        &mut self,
        begin_t: Option<Tok>,
        args: Vec<Ch>,
        end_t: Option<Tok>,
        check_args: bool,
    ) -> CRes<Box<WqNode>> {
        if check_args {
            self.check_duplicate_args(&args)?;
        }
        self.validate_no_forward_arg_after_restarg(&args)?;
        let map = collection_map(&begin_t, &args, &end_t)?;
        // emit_forward_arg=true → always n(:args, ...).
        Ok(n("args", args, map))
    }

    /// `builder.args(nil, [], nil, false)` — parameterless blocks/lambdas;
    /// the Collection map carries a nil expression.
    pub(crate) fn b_args_none(&mut self) -> CRes<Box<WqNode>> {
        self.b_args(None, vec![], None, false)
    }

    pub(crate) fn b_forward_arg(&mut self, dots_t: Tok) -> Box<WqNode> {
        n("forward_arg", vec![], token_map(&dots_t))
    }

    pub(crate) fn b_forwarded_args(&mut self, dots_t: Tok) -> Box<WqNode> {
        n("forwarded_args", vec![], token_map(&dots_t))
    }

    pub(crate) fn b_forwarded_restarg(&mut self, star_t: Tok) -> Box<WqNode> {
        n("forwarded_restarg", vec![], token_map(&star_t))
    }

    pub(crate) fn b_forwarded_kwrestarg(&mut self, dstar_t: Tok) -> Box<WqNode> {
        n("forwarded_kwrestarg", vec![], token_map(&dstar_t))
    }

    pub(crate) fn b_arg(&mut self, name_t: Tok) -> CRes<Box<WqNode>> {
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        Ok(n("arg", vec![Ch::V(Value::Sym(sym))], variable_map(name_t.r)))
    }

    pub(crate) fn b_optarg(&mut self, name_t: Tok, eql_t: Tok, value: Box<WqNode>) -> CRes<Box<WqNode>> {
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        let expr = name_t.r.join(value.expr()?);
        let map = variable_map(name_t.r).with_operator(eql_t.r)?.with_expression(expr);
        Ok(n("optarg", vec![Ch::V(Value::Sym(sym)), Ch::N(value)], map))
    }

    pub(crate) fn b_restarg(&mut self, star_t: Tok, name_t: Option<Tok>) -> CRes<Box<WqNode>> {
        if let Some(nt) = &name_t {
            let name = self.tok_name_string(nt);
            self.check_reserved_for_numparam(&name, nt.r);
            let sym = self.tok_sym(nt);
            Ok(n("restarg", vec![Ch::V(Value::Sym(sym))], arg_prefix_map(&star_t, &name_t)))
        } else {
            Ok(n("restarg", vec![], arg_prefix_map(&star_t, &None)))
        }
    }

    pub(crate) fn b_kwarg(&mut self, name_bytes: &[u8], name_r: R) -> CRes<Box<WqNode>> {
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        self.check_reserved_for_numparam(&name, name_r);
        let sym = self.intern_bytes(name_bytes);
        let name_t = Tok::b(name_bytes.to_vec(), name_r);
        let map = kwarg_map(&name_t, None)?;
        Ok(n("kwarg", vec![Ch::V(Value::Sym(sym))], map))
    }

    pub(crate) fn b_kwoptarg(&mut self, name_bytes: &[u8], name_r: R, value: Box<WqNode>) -> CRes<Box<WqNode>> {
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        self.check_reserved_for_numparam(&name, name_r);
        let sym = self.intern_bytes(name_bytes);
        let name_t = Tok::b(name_bytes.to_vec(), name_r);
        let map = kwarg_map(&name_t, Some(&value))?;
        Ok(n("kwoptarg", vec![Ch::V(Value::Sym(sym)), Ch::N(value)], map))
    }

    pub(crate) fn b_kwrestarg(&mut self, dstar_t: Tok, name: Option<(SymId, R)>) -> CRes<Box<WqNode>> {
        if let Some((sym, name_r)) = name {
            let name_str = self.vm.interner.resolve(sym).to_string();
            self.check_reserved_for_numparam(&name_str, name_r);
            let name_t = Tok::s(sym, name_r);
            Ok(n("kwrestarg", vec![Ch::V(Value::Sym(sym))], arg_prefix_map(&dstar_t, &Some(name_t))))
        } else {
            Ok(n("kwrestarg", vec![], arg_prefix_map(&dstar_t, &None)))
        }
    }

    pub(crate) fn b_kwnilarg(&mut self, dstar_t: Tok, nil_t: Tok) -> Box<WqNode> {
        n("kwnilarg", vec![], arg_prefix_map(&dstar_t, &Some(nil_t)))
    }

    pub(crate) fn b_shadowarg(&mut self, name_t: Tok) -> CRes<Box<WqNode>> {
        let name = self.tok_name_string(&name_t);
        self.check_reserved_for_numparam(&name, name_t.r);
        let sym = self.tok_sym(&name_t);
        Ok(n("shadowarg", vec![Ch::V(Value::Sym(sym))], variable_map(name_t.r)))
    }

    pub(crate) fn b_blockarg(&mut self, amper_t: Tok, name_t: Option<Tok>) -> CRes<Box<WqNode>> {
        if let Some(name_t) = &name_t {
            let name = self.tok_name_string(name_t);
            self.check_reserved_for_numparam(&name, name_t.r);
        }
        let children = match &name_t {
            Some(t) => {
                let sym = self.tok_sym(t);
                vec![Ch::V(Value::Sym(sym))]
            }
            None => vec![Ch::V(Value::Nil)],
        };
        Ok(n("blockarg", children, arg_prefix_map(&amper_t, &name_t)))
    }

    /// `procarg0(arg)` with emit_procarg0=false — identity.
    pub(crate) fn b_procarg0(&mut self, arg: Box<WqNode>) -> Box<WqNode> {
        arg
    }

    pub(crate) fn b_numargs(&mut self, max_numparam: i64) -> Box<WqNode> {
        n_nomap("numargs", vec![Ch::V(Value::Int(max_numparam))])
    }

    /// Prism builder's `itarg`.
    pub(crate) fn b_itarg(&mut self) -> Box<WqNode> {
        let it = self.vm.interner.intern("it");
        n_nomap("itarg", vec![Ch::V(Value::Sym(it))])
    }

    /// `check_duplicate_args` — duplicate-argument diagnostics.
    fn check_duplicate_args(&mut self, args: &[Ch]) -> CRes<()> {
        let mut map: Vec<(Option<String>, R)> = Vec::new();
        self.check_duplicate_args_inner(args, &mut map)
    }

    fn check_duplicate_args_inner(
        &mut self,
        args: &[Ch],
        map: &mut Vec<(Option<String>, R)>,
    ) -> CRes<()> {
        for arg in args {
            let Ch::N(arg) = arg else { continue };
            match arg.ty {
                "arg" | "optarg" | "restarg" | "blockarg" | "kwarg" | "kwoptarg" | "kwrestarg"
                | "shadowarg" => {
                    let this_name = arg.name_str(self.vm);
                    let name_loc = match &arg.map {
                        Some(Map { k: MK::Variable { name, .. }, .. }) => *name,
                        _ => None,
                    };
                    let Some(this_loc) = name_loc.or_else(|| arg.expr().ok()) else {
                        continue;
                    };
                    if let Some((_, that_loc)) =
                        map.iter().find(|(that_name, _)| that_name == &this_name)
                    {
                        let collides = match &this_name {
                            Some(name) => !name.starts_with('_'),
                            None => false,
                        };
                        if collides {
                            let that_loc = *that_loc;
                            self.diagnostic(
                                "error",
                                "duplicate_argument",
                                vec![],
                                this_loc,
                                vec![that_loc],
                            );
                        }
                    } else {
                        map.push((this_name, this_loc));
                    }
                }
                "mlhs" => {
                    // Recurse over the mlhs children (procarg0 unreachable
                    // with emit_procarg0=false).
                    self.check_duplicate_args_inner(&arg.children, map)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_no_forward_arg_after_restarg(&mut self, args: &[Ch]) -> CRes<()> {
        let mut restarg: Option<R> = None;
        let mut forward_arg: Option<R> = None;
        for arg in args {
            let Ch::N(arg) = arg else { continue };
            match arg.ty {
                "restarg" => restarg = Some(arg.expr()?),
                "forward_arg" => forward_arg = Some(arg.expr()?),
                _ => {}
            }
        }
        if let (Some(fa), Some(ra)) = (forward_arg, restarg) {
            self.diagnostic("error", "forward_arg_after_restarg", vec![], fa, vec![ra]);
        }
        Ok(())
    }

    // ----- method calls -----

    pub(crate) fn b_call_method(
        &mut self,
        receiver: Option<Box<WqNode>>,
        dot_t: ODot,
        selector: Option<(SymId, R)>,
        lparen_t: Option<Tok>,
        args: Vec<Ch>,
        rparen_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let ty: &'static str = if matches!(dot_t, Some((DotKind::AndDot, _))) { "csend" } else { "send" };
        // emit_kwargs=false → no hash→kwargs rewrite.
        let sel_sym = match selector {
            Some((sym, _)) => Value::Sym(sym),
            None => Value::Sym(self.vm.interner.intern("call")),
        };
        let map = send_map(
            receiver.as_deref(),
            dot_t,
            selector.map(|s| s.1),
            &lparen_t,
            &args,
            &rparen_t,
        )?;
        let mut children = vec![opt_ch(receiver), Ch::V(sel_sym)];
        children.extend(args);
        Ok(n(ty, children, map))
    }

    /// `call_lambda` with emit_lambda=false: `s(:send, nil, :lambda)`.
    pub(crate) fn b_call_lambda(&mut self, lambda_r: R) -> CRes<Box<WqNode>> {
        let sym = self.vm.interner.intern("lambda");
        Ok(n(
            "send",
            vec![Ch::V(Value::Nil), Ch::V(Value::Sym(sym))],
            Map {
                expr: Some(lambda_r),
                k: MK::Send { dot: None, sel: Some(lambda_r), b: None, e: None, op: None },
            },
        ))
    }

    /// The PRISM builder's `block` override (itarg → itblock, numargs →
    /// numblock).
    pub(crate) fn b_block(
        &mut self,
        method_call: Box<WqNode>,
        begin_t: Tok,
        args: Box<WqNode>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        if method_call.ty == "yield" {
            let kw = method_call
                .map
                .as_ref()
                .and_then(|m| m.keyword())
                .ok_or(Decline("yield without keyword loc"))?;
            self.diagnostic("error", "block_given_to_yield", vec![], kw, vec![begin_t.r]);
        }
        // last call arg block_pass / forwarded_args check.
        if method_call.children.len() > 2
            && let Some(Ch::N(last_arg)) = method_call.children.last()
            && (last_arg.ty == "block_pass" || last_arg.ty == "forwarded_args")
        {
            let e = last_arg.expr()?;
            self.diagnostic("error", "block_and_blockarg", vec![], e, vec![begin_t.r]);
        }

        let (block_type, args_ch): (&'static str, Ch) = match args.ty {
            "itarg" => ("itblock", args.children.into_iter().next().ok_or(Decline("itarg"))?),
            "numargs" => ("numblock", args.children.into_iter().next().ok_or(Decline("numargs"))?),
            _ => ("block", Ch::N(args)),
        };

        match method_call.ty {
            "send" | "csend" | "super" | "zsuper" => {
                let map = block_map(method_call.expr()?, &begin_t, &end_t);
                Ok(n(block_type, vec![Ch::N(method_call), args_ch, opt_ch(body)], map))
            }
            "return" | "break" | "next" => {
                // "return foo 1 do end" — method_call is actually (return).
                let mut method_call = method_call;
                if method_call.children.is_empty() {
                    return decline("keyword block without inner send");
                }
                let Ch::N(actual_send) = method_call.children.remove(0) else {
                    return decline("keyword block with scalar inner");
                };
                let map = block_map(actual_send.expr()?, &begin_t, &end_t);
                let block = n(block_type, vec![Ch::N(actual_send), args_ch, opt_ch(body)], map);
                let outer_expr = method_call.expr()?.join(block.expr()?);
                let outer_map = method_call
                    .map
                    .take()
                    .ok_or(Decline("keyword without map"))?
                    .with_expression(outer_expr);
                Ok(n(method_call.ty, vec![Ch::N(block)], outer_map))
            }
            _ => decline("block on unexpected method_call type"),
        }
    }

    pub(crate) fn b_block_pass(&mut self, amper_t: Tok, arg: Option<Box<WqNode>>) -> CRes<Box<WqNode>> {
        match arg {
            Some(arg) => {
                let map = unary_op_map(&amper_t, Some(&arg))?;
                Ok(n("block_pass", vec![Ch::N(arg)], map))
            }
            // anonymous block forwarding `foo(&)`.
            None => Ok(n("block_pass", vec![Ch::V(Value::Nil)], unary_op_map(&amper_t, None)?)),
        }
    }

    pub(crate) fn b_attr_asgn(
        &mut self,
        receiver: Box<WqNode>,
        dot_t: ODot,
        selector_bytes: &[u8],
        selector_r: R,
    ) -> CRes<Box<WqNode>> {
        let mut name = selector_bytes.to_vec();
        name.push(b'=');
        let sym = self.intern_bytes(&name);
        let ty: &'static str = if matches!(dot_t, Some((DotKind::AndDot, _))) { "csend" } else { "send" };
        let map = send_map(Some(&receiver), dot_t, Some(selector_r), &None, &[], &None)?;
        Ok(n(ty, vec![Ch::N(receiver), Ch::V(Value::Sym(sym))], map))
    }

    /// `index` with emit_index=false → send :[].
    pub(crate) fn b_index(
        &mut self,
        receiver: Box<WqNode>,
        lbrack_t: Tok,
        indexes: Vec<Ch>,
        rbrack_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = send_index_map(&receiver, &lbrack_t, &rbrack_t)?;
        let sym = self.vm.interner.intern("[]");
        let mut children = vec![Ch::N(receiver), Ch::V(Value::Sym(sym))];
        children.extend(indexes);
        Ok(n("send", children, map))
    }

    /// `index_asgn` with emit_index=false → send :[]=.
    pub(crate) fn b_index_asgn(
        &mut self,
        receiver: Box<WqNode>,
        lbrack_t: Tok,
        indexes: Vec<Ch>,
        rbrack_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = send_index_map(&receiver, &lbrack_t, &rbrack_t)?;
        let sym = self.vm.interner.intern("[]=");
        let mut children = vec![Ch::N(receiver), Ch::V(Value::Sym(sym))];
        children.extend(indexes);
        Ok(n("send", children, map))
    }

    pub(crate) fn b_match_op(&mut self, receiver: Box<WqNode>, match_r: R, arg: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = send_binary_op_map(&receiver, match_r, &arg)?;
        if self.static_regexp_node_ok(&receiver)? {
            // Regexp names are declared into static_env — write-only here.
            Ok(n("match_with_lvasgn", vec![Ch::N(receiver), Ch::N(arg)], map))
        } else {
            let sym = self.vm.interner.intern("=~");
            Ok(n("send", vec![Ch::N(receiver), Ch::V(Value::Sym(sym)), Ch::N(arg)], map))
        }
    }

    /// `not_op(not_t, begin_t, receiver, end_t)` (version > 18 branch).
    pub(crate) fn b_not_op(
        &mut self,
        not_t: Tok,
        begin_t: Option<Tok>,
        receiver: Option<Box<WqNode>>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let bang = self.vm.interner.intern("!");
        match receiver {
            None => {
                let nil_map = collection_map(&begin_t, &[], &end_t)?;
                let nil_node = n("begin", vec![], nil_map);
                let map = send_unary_op_map(not_t.r, Some(&nil_node))?;
                Ok(n("send", vec![Ch::N(nil_node), Ch::V(Value::Sym(bang))], map))
            }
            Some(receiver) => {
                let checked = self.check_condition(receiver)?;
                let args = [Ch::N(checked)];
                let map = send_map(None, None, Some(not_t.r), &begin_t, &args, &end_t)?;
                let [checked] = args;
                Ok(n("send", vec![checked, Ch::V(Value::Sym(bang))], map))
            }
        }
    }

    // ----- control flow -----

    pub(crate) fn b_logical_op(
        &mut self,
        ty: &'static str,
        lhs: Box<WqNode>,
        op_r: R,
        rhs: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&lhs, op_r, &rhs)?;
        Ok(n(ty, vec![Ch::N(lhs), Ch::N(rhs)], map))
    }

    #[allow(clippy::too_many_arguments)] // parser-gem builder signature — one Tok per source token
    pub(crate) fn b_condition(
        &mut self,
        cond_t: Tok,
        cond: Box<WqNode>,
        then_t: Option<Tok>,
        if_true: Option<Box<WqNode>>,
        else_t: Option<Tok>,
        if_false: Option<Box<WqNode>>,
        end_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let cond = self.check_condition(cond)?;
        let map = condition_map(
            cond_t.r,
            Some(&cond),
            loc(&then_t),
            if_true.as_deref(),
            loc(&else_t),
            if_false.as_deref(),
            loc(&end_t),
        )?;
        Ok(n("if", vec![Ch::N(cond), opt_ch(if_true), opt_ch(if_false)], map))
    }

    pub(crate) fn b_condition_mod(
        &mut self,
        if_true: Option<Box<WqNode>>,
        if_false: Option<Box<WqNode>>,
        cond_t: Tok,
        cond: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        let cond = self.check_condition(cond)?;
        let pre = if_true.as_deref().or(if_false.as_deref()).ok_or(Decline("condition_mod without body"))?;
        let map = keyword_mod_map(pre, &cond_t, &cond)?;
        Ok(n("if", vec![Ch::N(cond), opt_ch(if_true), opt_ch(if_false)], map))
    }

    pub(crate) fn b_ternary(
        &mut self,
        cond: Box<WqNode>,
        question_t: Tok,
        if_true: Box<WqNode>,
        colon_t: Tok,
        if_false: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        let cond = self.check_condition(cond)?;
        let map = ternary_map(&cond, question_t.r, colon_t.r, &if_false)?;
        Ok(n("if", vec![Ch::N(cond), Ch::N(if_true), Ch::N(if_false)], map))
    }

    pub(crate) fn b_when(
        &mut self,
        when_t: Tok,
        mut patterns: Vec<Ch>,
        then_t: Option<Tok>,
        body: Option<Box<WqNode>>,
    ) -> CRes<Box<WqNode>> {
        patterns.push(opt_ch(body));
        let map = keyword_map(&when_t, &then_t, Some(&patterns), &None)?;
        Ok(n("when", patterns, map))
    }

    pub(crate) fn b_case(
        &mut self,
        case_t: Tok,
        expr: Option<Box<WqNode>>,
        when_bodies: Vec<Ch>,
        else_t: Option<Tok>,
        else_body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = condition_map(
            case_t.r,
            expr.as_deref(),
            None,
            None,
            loc(&else_t),
            else_body.as_deref(),
            Some(end_t.r),
        )?;
        let mut children = vec![opt_ch(expr)];
        children.extend(when_bodies);
        children.push(opt_ch(else_body));
        Ok(n("case", children, map))
    }

    pub(crate) fn b_loop(
        &mut self,
        ty: &'static str,
        keyword_t: Tok,
        cond: Box<WqNode>,
        do_t: Option<Tok>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let cond = self.check_condition(cond)?;
        let map = keyword_map(&keyword_t, &do_t, None, &Some(end_t))?;
        Ok(n(ty, vec![Ch::N(cond), opt_ch(body)], map))
    }

    pub(crate) fn b_loop_mod(
        &mut self,
        ty: &'static str,
        body: Box<WqNode>,
        keyword_t: Tok,
        cond: Box<WqNode>,
    ) -> CRes<Box<WqNode>> {
        let cond = self.check_condition(cond)?;
        let ty: &'static str = if body.ty == "kwbegin" {
            match ty {
                "while" => "while_post",
                "until" => "until_post",
                _ => return decline("loop_mod type"),
            }
        } else {
            ty
        };
        let map = keyword_mod_map(&body, &keyword_t, &cond)?;
        Ok(n(ty, vec![Ch::N(cond), Ch::N(body)], map))
    }

    #[allow(clippy::too_many_arguments)] // parser-gem builder signature — one Tok per source token
    pub(crate) fn b_for(
        &mut self,
        for_t: Tok,
        iterator: Box<WqNode>,
        in_t: Tok,
        iteratee: Box<WqNode>,
        do_t: Option<Tok>,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = for_map(&for_t, &in_t, &do_t, &end_t);
        Ok(n("for", vec![Ch::N(iterator), Ch::N(iteratee), opt_ch(body)], map))
    }

    pub(crate) fn b_keyword_cmd(
        &mut self,
        ty: &'static str,
        keyword_t: Tok,
        lparen_t: Option<Tok>,
        args: Vec<Ch>,
        rparen_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        if ty == "yield"
            && let Some(Ch::N(last)) = args.last()
            && last.ty == "block_pass"
        {
            let e = last.expr()?;
            self.diagnostic("error", "block_given_to_yield", vec![], keyword_t.r, vec![e]);
        }
        // emit_kwargs=false → no rewrite.
        let map = keyword_map(&keyword_t, &lparen_t, Some(&args), &rparen_t)?;
        Ok(n(ty, args, map))
    }

    pub(crate) fn b_preexe(
        &mut self,
        preexe_t: Tok,
        lbrace_t: Tok,
        compstmt: Option<Box<WqNode>>,
        rbrace_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = keyword_map(&preexe_t, &Some(lbrace_t), Some(&[]), &Some(rbrace_t))?;
        Ok(n("preexe", vec![opt_ch(compstmt)], map))
    }

    pub(crate) fn b_postexe(
        &mut self,
        postexe_t: Tok,
        lbrace_t: Tok,
        compstmt: Option<Box<WqNode>>,
        rbrace_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = keyword_map(&postexe_t, &Some(lbrace_t), Some(&[]), &Some(rbrace_t))?;
        Ok(n("postexe", vec![opt_ch(compstmt)], map))
    }

    // ----- exception handling -----

    pub(crate) fn b_rescue_body(
        &mut self,
        rescue_t: Tok,
        exc_list: Option<Box<WqNode>>,
        assoc_t: Option<Tok>,
        exc_var: Option<Box<WqNode>>,
        then_t: Option<Tok>,
        compound_stmt: Option<Box<WqNode>>,
    ) -> CRes<Box<WqNode>> {
        let map = rescue_body_map(
            &rescue_t,
            exc_list.as_deref(),
            &assoc_t,
            exc_var.as_deref(),
            &then_t,
            compound_stmt.as_deref(),
        )?;
        Ok(n(
            "resbody",
            vec![opt_ch(exc_list), opt_ch(exc_var), opt_ch(compound_stmt)],
            map,
        ))
    }

    #[allow(clippy::vec_box)] // the whole b_* surface traffics in `Box<WqNode>`; the boxes move into `Ch::N` unchanged
    pub(crate) fn b_begin_body(
        &mut self,
        mut compound_stmt: Option<Box<WqNode>>,
        rescue_bodies: Vec<Box<WqNode>>,
        else_t: Option<Tok>,
        else_: Option<Box<WqNode>>,
        ensure_t: Option<Tok>,
        ensure_: Option<Box<WqNode>>,
    ) -> CRes<Option<Box<WqNode>>> {
        if !rescue_bodies.is_empty() {
            let refs: Vec<&WqNode> = rescue_bodies.iter().map(|b| b.as_ref()).collect();
            if let Some(et) = else_t {
                let map = eh_keyword_map(
                    compound_stmt.as_deref(),
                    &None,
                    &refs,
                    &Some(Tok::b(b"else".to_vec(), et.r)),
                    else_.as_deref(),
                )?;
                let mut children = vec![opt_ch(compound_stmt)];
                children.extend(rescue_bodies.into_iter().map(Ch::N));
                children.push(opt_ch(else_));
                compound_stmt = Some(n("rescue", children, map));
            } else {
                let map = eh_keyword_map(compound_stmt.as_deref(), &None, &refs, &None, None)?;
                let mut children = vec![opt_ch(compound_stmt)];
                children.extend(rescue_bodies.into_iter().map(Ch::N));
                children.push(Ch::V(Value::Nil));
                compound_stmt = Some(n("rescue", children, map));
            }
        } else if let Some(et) = else_t {
            // begin; foo; else; bar; end — the else clause is wrapped in an
            // extra begin carrying the else keyword as its begin delimiter.
            let mut statements: Vec<Ch> = Vec::new();
            if let Some(cs) = compound_stmt.take() {
                if cs.ty == "begin" {
                    statements.extend(cs.children);
                } else {
                    statements.push(Ch::N(cs));
                }
            }
            let else_tok = Some(Tok::b(b"else".to_vec(), et.r));
            let else_children = vec![opt_ch(else_)];
            let inner_map = collection_map(&else_tok, &else_children, &None)?;
            statements.push(Ch::N(n("begin", else_children, inner_map)));
            let outer_map = collection_map(&None, &statements, &None)?;
            compound_stmt = Some(n("begin", statements, outer_map));
        }

        if let Some(et) = ensure_t {
            let kw_tok = Some(Tok::b(b"ensure".to_vec(), et.r));
            let ensure_refs: Vec<&WqNode> = ensure_.as_deref().into_iter().collect();
            let map = eh_keyword_map(compound_stmt.as_deref(), &kw_tok, &ensure_refs, &None, None)?;
            compound_stmt = Some(n(
                "ensure",
                vec![opt_ch(compound_stmt), opt_ch(ensure_)],
                map,
            ));
        }

        Ok(compound_stmt)
    }

    // ----- expression grouping -----

    pub(crate) fn b_compstmt(&mut self, mut statements: Vec<Ch>) -> CRes<Option<Box<WqNode>>> {
        match statements.len() {
            0 => Ok(None),
            1 => match statements.pop() {
                Some(Ch::N(node)) => Ok(Some(node)),
                _ => decline("compstmt scalar statement"),
            },
            _ => {
                let map = collection_map(&None, &statements, &None)?;
                Ok(Some(n("begin", statements, map)))
            }
        }
    }

    pub(crate) fn b_begin(
        &mut self,
        begin_t: Tok,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let (bt, et) = (Some(begin_t), Some(end_t));
        match body {
            None => {
                let map = collection_map(&bt, &[], &et)?;
                Ok(n("begin", vec![], map))
            }
            Some(body) => {
                let synthesized = body.ty == "mlhs"
                    || (body.ty == "begin"
                        && matches!(&body.map, Some(Map { k: MK::Collection { b: None, e: None }, .. })));
                if synthesized {
                    let map = collection_map(&bt, &body.children, &et)?;
                    Ok(n(body.ty, body.children, map))
                } else {
                    let children = vec![Ch::N(body)];
                    let map = collection_map(&bt, &children, &et)?;
                    Ok(n("begin", children, map))
                }
            }
        }
    }

    pub(crate) fn b_begin_keyword(
        &mut self,
        begin_t: Tok,
        body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let (bt, et) = (Some(begin_t), Some(end_t));
        match body {
            None => {
                let map = collection_map(&bt, &[], &et)?;
                Ok(n("kwbegin", vec![], map))
            }
            Some(body) => {
                let synthesized = body.ty == "begin"
                    && matches!(&body.map, Some(Map { k: MK::Collection { b: None, e: None }, .. }));
                if synthesized {
                    let map = collection_map(&bt, &body.children, &et)?;
                    Ok(n("kwbegin", body.children, map))
                } else {
                    let children = vec![Ch::N(body)];
                    let map = collection_map(&bt, &children, &et)?;
                    Ok(n("kwbegin", children, map))
                }
            }
        }
    }

    // ----- pattern matching -----

    pub(crate) fn b_case_match(
        &mut self,
        case_t: Tok,
        expr: Box<WqNode>,
        in_bodies: Vec<Ch>,
        else_t: Option<Tok>,
        else_body: Option<Box<WqNode>>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let else_body = match (&else_t, else_body) {
            (Some(et), None) => Some(n("empty_else", vec![], token_map(et))),
            (_, b) => b,
        };
        let map = condition_map(
            case_t.r,
            Some(&expr),
            None,
            None,
            loc(&else_t),
            else_body.as_deref(),
            Some(end_t.r),
        )?;
        let mut children = vec![Ch::N(expr)];
        children.extend(in_bodies);
        children.push(opt_ch(else_body));
        Ok(n("case_match", children, map))
    }

    pub(crate) fn b_match_pattern(&mut self, lhs: Box<WqNode>, match_r: R, rhs: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&lhs, match_r, &rhs)?;
        Ok(n("match_pattern", vec![Ch::N(lhs), Ch::N(rhs)], map))
    }

    pub(crate) fn b_match_pattern_p(&mut self, lhs: Box<WqNode>, match_r: R, rhs: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&lhs, match_r, &rhs)?;
        Ok(n("match_pattern_p", vec![Ch::N(lhs), Ch::N(rhs)], map))
    }

    pub(crate) fn b_in_pattern(
        &mut self,
        in_t: Tok,
        pattern: Box<WqNode>,
        guard: Option<Box<WqNode>>,
        then_t: Option<Tok>,
        body: Option<Box<WqNode>>,
    ) -> CRes<Box<WqNode>> {
        // `keyword_map(in_t, then_t, children.compact, nil)` — the compact
        // list is non-empty (pattern) with a non-nil last element, so end_l
        // is always the last non-nil child's expression.
        let mut end_l = pattern.expr()?;
        if let Some(g) = &guard {
            end_l = g.expr()?;
        }
        if let Some(b) = &body {
            end_l = b.expr()?;
        }
        let map = Map {
            expr: Some(in_t.r.join(end_l)),
            k: MK::Keyword { kw: Some(in_t.r), b: loc(&then_t), e: None },
        };
        Ok(n("in_pattern", vec![Ch::N(pattern), opt_ch(guard), opt_ch(body)], map))
    }

    pub(crate) fn b_if_guard(&mut self, if_t: Tok, if_body: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = guard_map(&if_t, &if_body)?;
        Ok(n("if_guard", vec![Ch::N(if_body)], map))
    }

    pub(crate) fn b_unless_guard(&mut self, unless_t: Tok, unless_body: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = guard_map(&unless_t, &unless_body)?;
        Ok(n("unless_guard", vec![Ch::N(unless_body)], map))
    }

    pub(crate) fn b_match_var(&mut self, name_sym: SymId, name_r: R) -> CRes<Box<WqNode>> {
        let name = self.vm.interner.resolve(name_sym).to_string();
        self.check_lvar_name(&name, name_r);
        self.check_duplicate_pattern_variable(&name, name_r);
        Ok(n("match_var", vec![Ch::V(Value::Sym(name_sym))], variable_map(name_r)))
    }

    pub(crate) fn b_match_hash_var(&mut self, name_bytes: &[u8], expr_r: R) -> CRes<Box<WqNode>> {
        let name_l = R { b: expr_r.b, e: expr_r.e - 1 };
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        self.check_lvar_name(&name, name_l);
        self.check_duplicate_pattern_variable(&name, name_l);
        let sym = self.intern_bytes(name_bytes);
        Ok(n("match_var", vec![Ch::V(Value::Sym(sym))], Map {
            expr: Some(expr_r),
            k: MK::Variable { name: Some(name_l), op: None },
        }))
    }

    pub(crate) fn b_match_hash_var_from_str(
        &mut self,
        begin_t: Tok,
        strings: Vec<Ch>,
        end_t: Tok,
    ) -> CRes<Box<WqNode>> {
        if strings.len() > 1 {
            self.diagnostic("error", "pm_interp_in_var_name", vec![], begin_t.r.join(end_t.r), vec![]);
            // Fatal on rubyrs; unreachable result.
            return decline("pm_interp_in_var_name continuation");
        }
        let Some(first) = strings.into_iter().next() else {
            return decline("match_hash_var_from_str without strings");
        };
        let Ch::N(string) = first else {
            return decline("match_hash_var_from_str scalar");
        };
        match string.ty {
            "str" => {
                let (name_bytes, frozen_name) = match string.children.first() {
                    Some(Ch::V(Value::Str(s))) => (s.content.borrow().clone(), s.clone()),
                    _ => return decline("match_hash_var_from_str non-string"),
                };
                let _ = frozen_name;
                let mut name_l = string.expr()?;
                let name = String::from_utf8_lossy(&name_bytes).into_owned();
                self.check_lvar_name(&name, name_l);
                self.check_duplicate_pattern_variable(&name, name_l);
                if let Some(Map { k: MK::Collection { b: Some(bl), .. }, .. }) = &string.map {
                    name_l = R { b: name_l.b + (bl.e - bl.b), e: name_l.e };
                }
                if let Some(Map { k: MK::Collection { e: Some(el), .. }, .. }) = &string.map {
                    name_l = R { b: name_l.b, e: name_l.e - (el.e - el.b) };
                }
                let expr_l = begin_t.r.join(string.expr()?).join(end_t.r);
                let sym = self.intern_bytes(&name_bytes);
                Ok(n("match_var", vec![Ch::V(Value::Sym(sym))], Map {
                    expr: Some(expr_l),
                    k: MK::Variable { name: Some(name_l), op: None },
                }))
            }
            "begin" => self.b_match_hash_var_from_str(begin_t, string.children, end_t),
            _ => {
                self.diagnostic("error", "pm_interp_in_var_name", vec![], begin_t.r.join(end_t.r), vec![]);
                decline("pm_interp_in_var_name continuation")
            }
        }
    }

    pub(crate) fn b_match_rest(&mut self, star_t: Tok, name_t: Option<(SymId, R)>) -> CRes<Box<WqNode>> {
        match name_t {
            None => Ok(n("match_rest", vec![], unary_op_map(&star_t, None)?)),
            Some((sym, r)) => {
                let name = self.b_match_var(sym, r)?;
                let map = unary_op_map(&star_t, Some(&name))?;
                Ok(n("match_rest", vec![Ch::N(name)], map))
            }
        }
    }

    pub(crate) fn b_hash_pattern(
        &mut self,
        lbrace_t: Option<Tok>,
        kwargs: Vec<Ch>,
        rbrace_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        self.check_duplicate_args(&kwargs)?;
        let map = collection_map(&lbrace_t, &kwargs, &rbrace_t)?;
        Ok(n("hash_pattern", kwargs, map))
    }

    pub(crate) fn b_array_pattern(
        &mut self,
        lbrack_t: Option<Tok>,
        elements: Option<Vec<Ch>>,
        rbrack_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let Some(elements) = elements else {
            let map = collection_map(&lbrack_t, &[], &rbrack_t)?;
            return Ok(n("array_pattern", vec![], map));
        };
        let mut trailing_comma = false;
        let mut node_elements: Vec<Ch> = Vec::with_capacity(elements.len());
        // The map uses the ORIGINAL elements (incl. the trailing-comma
        // wrapper's expression) — compute it first.
        let map = collection_map(&lbrack_t, &elements, &rbrack_t)?;
        for element in elements {
            match element {
                Ch::N(el) if el.ty == "match_with_trailing_comma" => {
                    trailing_comma = true;
                    let inner = el.children.into_iter().next().ok_or(Decline("mwtc"))?;
                    node_elements.push(inner);
                }
                other => {
                    trailing_comma = false;
                    node_elements.push(other);
                }
            }
        }
        let ty: &'static str = if trailing_comma { "array_pattern_with_tail" } else { "array_pattern" };
        Ok(n(ty, node_elements, map))
    }

    pub(crate) fn b_find_pattern(
        &mut self,
        lbrack_t: Option<Tok>,
        elements: Vec<Ch>,
        rbrack_t: Option<Tok>,
    ) -> CRes<Box<WqNode>> {
        let map = collection_map(&lbrack_t, &elements, &rbrack_t)?;
        Ok(n("find_pattern", elements, map))
    }

    pub(crate) fn b_match_with_trailing_comma(&mut self, match_: Box<WqNode>, comma_t: Tok) -> CRes<Box<WqNode>> {
        let map = expr_map(match_.expr()?.join(comma_t.r));
        Ok(n("match_with_trailing_comma", vec![Ch::N(match_)], map))
    }

    pub(crate) fn b_const_pattern(
        &mut self,
        const_: Box<WqNode>,
        ldelim_t: Tok,
        pattern: Box<WqNode>,
        rdelim_t: Tok,
    ) -> CRes<Box<WqNode>> {
        let map = Map {
            expr: Some(const_.expr()?.join(rdelim_t.r)),
            k: MK::Collection { b: Some(ldelim_t.r), e: Some(rdelim_t.r) },
        };
        Ok(n("const_pattern", vec![Ch::N(const_), Ch::N(pattern)], map))
    }

    pub(crate) fn b_pin(&mut self, pin_t: Tok, var: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = send_unary_op_map(pin_t.r, Some(&var))?;
        Ok(n("pin", vec![Ch::N(var)], map))
    }

    pub(crate) fn b_match_alt(&mut self, left: Box<WqNode>, pipe_r: R, right: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&left, pipe_r, &right)?;
        Ok(n("match_alt", vec![Ch::N(left), Ch::N(right)], map))
    }

    pub(crate) fn b_match_as(&mut self, value: Box<WqNode>, assoc_r: R, as_: Box<WqNode>) -> CRes<Box<WqNode>> {
        let map = binary_op_map(&value, assoc_r, &as_)?;
        Ok(n("match_as", vec![Ch::N(value), Ch::N(as_)], map))
    }

    pub(crate) fn b_match_nil_pattern(&mut self, dstar_t: Tok, nil_t: Tok) -> Box<WqNode> {
        n("match_nil_pattern", vec![], arg_prefix_map(&dstar_t, &Some(nil_t)))
    }

    fn check_lvar_name(&mut self, name: &str, loc_r: R) {
        // /\A[[[:lower:]]_][[[:alnum:]]_]*\z/ — Unicode-aware.
        let mut chars = name.chars();
        let ok = match chars.next() {
            Some(c) => {
                (c.is_lowercase() || c == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
            }
            None => false,
        };
        if !ok {
            self.diagnostic(
                "error",
                "lvar_name",
                vec![("name", ArgVal::Sym(name.to_string()))],
                loc_r,
                vec![],
            );
        }
    }

    fn check_duplicate_pattern_variable(&mut self, name: &str, loc_r: R) {
        if name.starts_with('_') {
            return;
        }
        let declared = self.pattern_vars.last().map(|f| f.iter().any(|n| n == name)).unwrap_or(false);
        if declared {
            self.diagnostic(
                "error",
                "duplicate_variable_name",
                vec![("name", ArgVal::Str(name.to_string()))],
                loc_r,
                vec![],
            );
        }
        if let Some(f) = self.pattern_vars.last_mut() {
            f.push(name.to_string());
        }
    }

    // ----- verification -----

    pub(crate) fn check_condition(&mut self, cond: Box<WqNode>) -> CRes<Box<WqNode>> {
        let mut cond = cond;
        match cond.ty {
            "begin" => {
                if cond.children.len() == 1 {
                    let Some(Ch::N(inner)) = cond.children.pop() else {
                        return decline("begin cond scalar");
                    };
                    let checked = self.check_condition(inner)?;
                    cond.children.push(Ch::N(checked));
                }
                Ok(cond)
            }
            "and" | "or" => {
                let mut it = cond.children.drain(..);
                let (Some(Ch::N(lhs)), Some(Ch::N(rhs))) = (it.next(), it.next()) else {
                    return decline("and/or cond arity");
                };
                drop(it);
                let lhs = self.check_condition(lhs)?;
                let rhs = self.check_condition(rhs)?;
                cond.children = vec![Ch::N(lhs), Ch::N(rhs)];
                Ok(cond)
            }
            "irange" | "erange" => {
                let ty: &'static str = if cond.ty == "irange" { "iflipflop" } else { "eflipflop" };
                let mut it = cond.children.drain(..);
                let (Some(lhs), Some(rhs)) = (it.next(), it.next()) else {
                    return decline("range cond arity");
                };
                drop(it);
                let lhs = match lhs {
                    Ch::N(node) => Ch::N(self.check_condition(node)?),
                    v => v,
                };
                let rhs = match rhs {
                    Ch::N(node) => Ch::N(self.check_condition(node)?),
                    v => v,
                };
                cond.ty = ty;
                cond.children = vec![lhs, rhs];
                Ok(cond)
            }
            "regexp" => {
                let map = expr_map(cond.expr()?);
                Ok(n("match_current_line", vec![Ch::N(cond)], map))
            }
            _ => Ok(cond),
        }
    }
}

pub(crate) fn opt_ch(node: Option<Box<WqNode>>) -> Ch {
    match node {
        Some(n) => Ch::N(n),
        None => Ch::V(Value::Nil),
    }
}

/// `AST::Node#eql?` — type + children, recursively (class is derived from
/// type via NODE_MAP, so type equality subsumes it).
pub(crate) fn node_eql(a: &WqNode, b: &WqNode, vm: &crate::vm::Vm) -> bool {
    if a.ty != b.ty || a.children.len() != b.children.len() {
        return false;
    }
    a.children.iter().zip(b.children.iter()).all(|(x, y)| match (x, y) {
        (Ch::N(x), Ch::N(y)) => node_eql(x, y, vm),
        (Ch::V(x), Ch::V(y)) => value_eql(x, y, vm),
        _ => false,
    })
}

fn value_eql(a: &Value, b: &Value, vm: &crate::vm::Vm) -> bool {
    match (a, b) {
        (Value::Sym(x), Value::Sym(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        (Value::Str(x), Value::Str(y)) => *x.content.borrow() == *y.content.borrow(),
        (Value::Rational(x), Value::Rational(y)) => match (vm.heap.get(*x), vm.heap.get(*y)) {
            (crate::heap::HeapObj::Rational(rx), crate::heap::HeapObj::Rational(ry)) => {
                rx.num == ry.num && rx.den == ry.den
            }
            _ => false,
        },
        _ => false,
    }
}
