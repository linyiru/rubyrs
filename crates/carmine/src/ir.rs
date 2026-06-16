//! Conditional Action IR — the compiled form of rouge rule BLOCKS.
//!
//! Track C upgrade path: instead of trace-once recording (sound only
//! for allowlisted lexers) or per-match VM callbacks (a Ruby round
//! trip), rule procs whose bodies fit a restricted Ruby subset are
//! AST-compiled (host-side, via prism) into this IR and executed
//! natively. The compiler whitelists AST shapes; anything it can't
//! express stays a `callback` rule — the decline boundary is the
//! COMPILER, not a runtime guess, which is what makes the mechanism
//! general across lexers rather than per-lexer handwork.
//!
//! JSON encoding (tuple-array style, matching the `actions` format):
//!
//! ```json
//! ["token", "Operator", ["g", 1]]
//! ["token", "Name.Constant", ["cat", ["g", 2], ["g", 3]]]
//! ["groups", ["Keyword", "Text"]]
//! ["push", "funcname"]  /  ["push", null]  /  ["pop", 1]  /  ["goto", "x"]
//! ["iset", "sigil", ["g", 1]]
//! ["lpush", "heredoc_queue", [["gin", 1, ["<<-", "<<~"]], ["g", 3]]]
//! ["if", ["not", ["instate", "heredoc_queue"]], [ …then-ops… ], [ …else-ops… ]]
//! ```
//!
//! Expressions: `["lit", s]`, `["g", i]` (capture i; 0 = whole match —
//! a nil group makes `token` emit nothing, like rouge), `["cat", e…]`,
//! `["bool", b]`, `["gin", i, [lits]]` (membership test as a value).
//! Conditions: `["ivar", name]` (truthy), `["instate", s]` (rouge
//! `state?` — the CURRENT top of stack), `["geq", i, lit]`,
//! `["gin", i, [lits]]`, `["not", c]`.

use std::collections::HashMap;

use serde_json::Value as J;

use crate::Error;
use crate::table::TokenId;

#[derive(Debug, Clone)]
pub(crate) enum IrExpr {
    Lit(String),
    Group(usize),
    Concat(Vec<IrExpr>),
    Bool(bool),
    GroupIn(usize, Vec<String>),
}

/// Case fold applied to a captured group before comparison, mirroring
/// rouge's `m[i].downcase` / `m[i].upcase` in classifier conditions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Fold {
    Lower,
    Upper,
}

impl Fold {
    fn apply(self, s: &str) -> String {
        match self {
            Fold::Lower => s.to_lowercase(),
            Fold::Upper => s.to_uppercase(),
        }
    }

    fn parse(s: &str) -> Option<Fold> {
        match s {
            "down" => Some(Fold::Lower),
            "up" => Some(Fold::Upper),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum IrCond {
    IvarTruthy(String),
    InState(u32),
    GroupEq(usize, String),
    GroupIn(usize, Vec<String>),
    /// `SET.include?(m[i].downcase)` — fold group i, then exact-compare.
    GroupEqFold(usize, Fold, String),
    GroupInFold(usize, Fold, Vec<String>),
    Not(Box<IrCond>),
}

#[derive(Debug)]
pub(crate) enum IrOp {
    /// `token Tok[, value]` — `value: None` emits the whole match.
    Token {
        token: TokenId,
        value: Option<IrExpr>,
    },
    Groups(Vec<TokenId>),
    Push(Option<u32>),
    Pop(usize),
    Goto(u32),
    IvarSet(String, IrExpr),
    /// `@ivar << [a, b, …]` — append a tuple to a list ivar. rouge
    /// lexers initialize these in `start` blocks (`@q = []`); an
    /// append to an untouched ivar starts a fresh list, which is
    /// equivalent for every observed initializer.
    ListPush(String, Vec<IrExpr>),
    If {
        cond: IrCond,
        then_ops: Vec<IrOp>,
        else_ops: Vec<IrOp>,
    },
}

/// A runtime ivar value on the native lexer (mirrors the tiny state
/// vocabulary rouge rule procs actually use).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IvarVal {
    Nil,
    Bool(bool),
    Str(String),
    /// list of tuples (`@heredoc_queue` shape).
    List(Vec<Vec<IvarVal>>),
}

pub(crate) type Ivars = HashMap<String, IvarVal>;

// ---- JSON loading --------------------------------------------------------

/// Name-interning surface the loader provides (token + state ids).
pub(crate) trait IrInterner {
    fn ir_tok(&mut self, name: &str) -> TokenId;
    fn ir_state(&mut self, name: &str) -> u32;
}

pub(crate) fn parse_ops(arr: &[J], it: &mut dyn IrInterner) -> Result<Vec<IrOp>, Error> {
    arr.iter().map(|op| parse_op(op, it)).collect()
}

fn bad(msg: &str) -> Error {
    Error::Table(format!("ir: {msg}"))
}

fn parse_op(v: &J, it: &mut dyn IrInterner) -> Result<IrOp, Error> {
    let t = v.as_array().ok_or_else(|| bad("op not an array"))?;
    let head = t
        .first()
        .and_then(J::as_str)
        .ok_or_else(|| bad("op head"))?;
    Ok(match head {
        "token" => {
            let tok = t
                .get(1)
                .and_then(J::as_str)
                .ok_or_else(|| bad("token name"))?;
            let value = match t.get(2) {
                None | Some(J::Null) => None,
                Some(e) => Some(parse_expr(e)?),
            };
            IrOp::Token {
                token: it.ir_tok(tok),
                value,
            }
        }
        "groups" => {
            let toks = t
                .get(1)
                .and_then(J::as_array)
                .ok_or_else(|| bad("groups list"))?
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(|n| it.ir_tok(n))
                        .ok_or_else(|| bad("groups tok"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            IrOp::Groups(toks)
        }
        "push" => match t.get(1) {
            None | Some(J::Null) => IrOp::Push(None),
            Some(J::String(s)) => IrOp::Push(Some(it.ir_state(s))),
            _ => return Err(bad("push arg")),
        },
        "pop" => IrOp::Pop(t.get(1).and_then(J::as_u64).unwrap_or(1) as usize),
        "goto" => {
            let s = t
                .get(1)
                .and_then(J::as_str)
                .ok_or_else(|| bad("goto state"))?;
            IrOp::Goto(it.ir_state(s))
        }
        "iset" => {
            let name = t
                .get(1)
                .and_then(J::as_str)
                .ok_or_else(|| bad("iset name"))?;
            IrOp::IvarSet(
                name.to_string(),
                parse_expr(t.get(2).ok_or_else(|| bad("iset expr"))?)?,
            )
        }
        "lpush" => {
            let name = t
                .get(1)
                .and_then(J::as_str)
                .ok_or_else(|| bad("lpush name"))?;
            let exprs = t
                .get(2)
                .and_then(J::as_array)
                .ok_or_else(|| bad("lpush tuple"))?
                .iter()
                .map(parse_expr)
                .collect::<Result<Vec<_>, _>>()?;
            IrOp::ListPush(name.to_string(), exprs)
        }
        "if" => {
            let cond = parse_cond(t.get(1).ok_or_else(|| bad("if cond"))?, it)?;
            let then_ops = parse_ops(
                t.get(2)
                    .and_then(J::as_array)
                    .ok_or_else(|| bad("if then"))?,
                it,
            )?;
            let else_ops = match t.get(3) {
                None | Some(J::Null) => Vec::new(),
                Some(e) => parse_ops(e.as_array().ok_or_else(|| bad("if else"))?, it)?,
            };
            IrOp::If {
                cond,
                then_ops,
                else_ops,
            }
        }
        _ => return Err(bad("unknown op")),
    })
}

fn parse_expr(v: &J) -> Result<IrExpr, Error> {
    let t = v.as_array().ok_or_else(|| bad("expr not an array"))?;
    let head = t
        .first()
        .and_then(J::as_str)
        .ok_or_else(|| bad("expr head"))?;
    Ok(match head {
        "lit" => IrExpr::Lit(
            t.get(1)
                .and_then(J::as_str)
                .ok_or_else(|| bad("lit"))?
                .to_string(),
        ),
        "g" => IrExpr::Group(t.get(1).and_then(J::as_u64).ok_or_else(|| bad("g idx"))? as usize),
        "cat" => IrExpr::Concat(
            t[1..]
                .iter()
                .map(parse_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        "bool" => IrExpr::Bool(t.get(1).and_then(J::as_bool).ok_or_else(|| bad("bool"))?),
        "gin" => IrExpr::GroupIn(
            t.get(1).and_then(J::as_u64).ok_or_else(|| bad("gin idx"))? as usize,
            parse_str_list(t.get(2))?,
        ),
        _ => return Err(bad("unknown expr")),
    })
}

fn parse_cond(v: &J, it: &mut dyn IrInterner) -> Result<IrCond, Error> {
    let t = v.as_array().ok_or_else(|| bad("cond not an array"))?;
    let head = t
        .first()
        .and_then(J::as_str)
        .ok_or_else(|| bad("cond head"))?;
    Ok(match head {
        "ivar" => IrCond::IvarTruthy(
            t.get(1)
                .and_then(J::as_str)
                .ok_or_else(|| bad("ivar name"))?
                .to_string(),
        ),
        "instate" => {
            let s = t.get(1).and_then(J::as_str).ok_or_else(|| bad("instate"))?;
            IrCond::InState(it.ir_state(s))
        }
        "geq" => IrCond::GroupEq(
            t.get(1).and_then(J::as_u64).ok_or_else(|| bad("geq idx"))? as usize,
            t.get(2)
                .and_then(J::as_str)
                .ok_or_else(|| bad("geq lit"))?
                .to_string(),
        ),
        "gin" => IrCond::GroupIn(
            t.get(1).and_then(J::as_u64).ok_or_else(|| bad("gin idx"))? as usize,
            parse_str_list(t.get(2))?,
        ),
        "geqf" => IrCond::GroupEqFold(
            t.get(1).and_then(J::as_u64).ok_or_else(|| bad("geqf idx"))? as usize,
            t.get(2)
                .and_then(J::as_str)
                .and_then(Fold::parse)
                .ok_or_else(|| bad("geqf fold"))?,
            t.get(3)
                .and_then(J::as_str)
                .ok_or_else(|| bad("geqf lit"))?
                .to_string(),
        ),
        "ginf" => IrCond::GroupInFold(
            t.get(1).and_then(J::as_u64).ok_or_else(|| bad("ginf idx"))? as usize,
            t.get(2)
                .and_then(J::as_str)
                .and_then(Fold::parse)
                .ok_or_else(|| bad("ginf fold"))?,
            parse_str_list(t.get(3))?,
        ),
        "not" => IrCond::Not(Box::new(parse_cond(
            t.get(1).ok_or_else(|| bad("not arg"))?,
            it,
        )?)),
        _ => return Err(bad("unknown cond")),
    })
}

fn parse_str_list(v: Option<&J>) -> Result<Vec<String>, Error> {
    v.and_then(J::as_array)
        .ok_or_else(|| bad("string list"))?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or_else(|| bad("string list item"))
        })
        .collect()
}

// ---- evaluation ----------------------------------------------------------

/// Evaluate an expression against the match groups. `groups[0]` is the
/// whole match. A nil capture group propagates as `None` (a `token`
/// with a nil value emits nothing, exactly like rouge's yield guard);
/// `cat` treats nil parts as "" (Ruby string interpolation).
pub(crate) fn eval_expr(expr: &IrExpr, groups: &[Option<String>]) -> Option<EvalVal> {
    Some(match expr {
        IrExpr::Lit(s) => EvalVal::Str(s.clone()),
        IrExpr::Group(i) => match groups.get(*i) {
            Some(Some(s)) => EvalVal::Str(s.clone()),
            _ => EvalVal::Nil,
        },
        IrExpr::Concat(parts) => {
            let mut out = String::new();
            for p in parts {
                match eval_expr(p, groups)? {
                    EvalVal::Str(s) => out.push_str(&s),
                    EvalVal::Nil => {}
                    EvalVal::Bool(b) => out.push_str(if b { "true" } else { "false" }),
                }
            }
            EvalVal::Str(out)
        }
        IrExpr::Bool(b) => EvalVal::Bool(*b),
        IrExpr::GroupIn(i, lits) => {
            let hit = matches!(groups.get(*i), Some(Some(s)) if lits.iter().any(|l| l == s));
            EvalVal::Bool(hit)
        }
    })
}

/// Expression result — the value vocabulary rule procs produce.
#[derive(Debug, Clone)]
pub(crate) enum EvalVal {
    Nil,
    Bool(bool),
    Str(String),
}

impl EvalVal {
    pub(crate) fn into_ivar(self) -> IvarVal {
        match self {
            EvalVal::Nil => IvarVal::Nil,
            EvalVal::Bool(b) => IvarVal::Bool(b),
            EvalVal::Str(s) => IvarVal::Str(s),
        }
    }
}

pub(crate) fn eval_cond(
    cond: &IrCond,
    groups: &[Option<String>],
    ivars: &Ivars,
    current_state: u32,
) -> bool {
    match cond {
        IrCond::IvarTruthy(name) => !matches!(
            ivars.get(name),
            None | Some(IvarVal::Nil) | Some(IvarVal::Bool(false))
        ),
        IrCond::InState(s) => current_state == *s,
        IrCond::GroupEq(i, lit) => {
            matches!(groups.get(*i), Some(Some(g)) if g == lit)
        }
        IrCond::GroupIn(i, lits) => {
            matches!(groups.get(*i), Some(Some(g)) if lits.iter().any(|l| l == g))
        }
        IrCond::GroupEqFold(i, fold, lit) => {
            matches!(groups.get(*i), Some(Some(g)) if &fold.apply(g) == lit)
        }
        IrCond::GroupInFold(i, fold, lits) => {
            matches!(groups.get(*i), Some(Some(g)) if { let f = fold.apply(g); lits.iter().any(|l| *l == f) })
        }
        IrCond::Not(inner) => !eval_cond(inner, groups, ivars, current_state),
    }
}
