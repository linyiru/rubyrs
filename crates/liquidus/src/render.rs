//! Rendering: walk the compiled nodes, resolve values, apply filters,
//! splice. Any value shape the subset can't reproduce byte-exactly
//! declines the whole render (the embedder falls back to pure liquid).

use crate::parse::{CmpOp, Cond, Expr, FilterCall, Node, VarRef};
use crate::{Error, LValue, Template, Values, filters};

fn declined(what: &'static str) -> Error {
    Error::Declined(what)
}

/// Per-render loop-variable bindings (tiny: depth ≤ a few).
struct Env<'a> {
    frames: Vec<(&'a str, &'a LValue)>,
}

pub(crate) fn render(tpl: &Template, values: &Values) -> Result<String, Error> {
    let mut out = String::with_capacity(4096);
    let mut env = Env { frames: Vec::new() };
    render_nodes(tpl, values, &tpl.nodes, &mut env, &mut out)?;
    Ok(out)
}

fn render_nodes<'a>(
    tpl: &Template,
    values: &'a Values,
    nodes: &'a [Node],
    env: &mut Env<'a>,
    out: &mut String,
) -> Result<(), Error> {
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::Output { expr, filters } => {
                let v = eval(values, env, expr)?;
                let v = apply_filters(tpl, v, filters)?;
                write_value(&v, out)?;
            }
            Node::If { cond, body } => {
                if eval_cond(values, env, cond)? {
                    render_nodes(tpl, values, body, env, out)?;
                }
            }
            Node::For {
                var,
                collection,
                limit,
                body,
            } => {
                let coll = supplied(values, collection)?;
                let LValue::Array(items) = coll else {
                    // nil collection iterates zero times in liquid;
                    // other scalars have odd coercions — decline.
                    if matches!(coll, LValue::Nil) {
                        continue;
                    }
                    return Err(declined("for-over-non-array"));
                };
                let n = limit.unwrap_or(items.len()).min(items.len());
                for item in &items[..n] {
                    env.frames.push((var.as_str(), item));
                    let r = render_nodes(tpl, values, body, env, out);
                    env.frames.pop();
                    r?;
                }
            }
        }
    }
    Ok(())
}

fn supplied<'a>(values: &'a Values, path: &str) -> Result<&'a LValue, Error> {
    values
        .0
        .get(path)
        .ok_or(Error::Declined("missing-supplied-value"))
}

fn eval<'a>(values: &'a Values, env: &Env<'a>, expr: &Expr) -> Result<LValue, Error> {
    Ok(match expr {
        Expr::StrLit(s) => LValue::Str(s.clone()),
        Expr::IntLit(n) => LValue::Int(*n),
        Expr::Var(var) => match var {
            VarRef::Supplied { path, size } => {
                let v = supplied(values, path)?;
                if *size {
                    // Prefer an explicit `path#size` companion (the
                    // embedder supplies it when a slice hides the
                    // real length).
                    if let Some(LValue::Int(n)) = values.0.get(&format!("{path}#size")) {
                        LValue::Int(*n)
                    } else {
                        size_of(v)?
                    }
                } else {
                    v.clone()
                }
            }
            VarRef::Loop { var, field, size } => {
                let Some((_, item)) = env.frames.iter().rev().find(|(name, _)| name == var) else {
                    return Err(declined("loop-var-out-of-scope"));
                };
                let base: &LValue = match field {
                    Some(f) => item.field(f).unwrap_or(&LValue::Nil),
                    None => item,
                };
                if *size { size_of(base)? } else { base.clone() }
            }
        },
    })
}

fn size_of(v: &LValue) -> Result<LValue, Error> {
    Ok(match v {
        LValue::Array(items) => LValue::Int(items.len() as i64),
        LValue::Str(s) => LValue::Int(s.chars().count() as i64),
        LValue::Map(pairs) => LValue::Int(pairs.len() as i64),
        // nil.size is 0 in liquid (NilClass responds via to_liquid)…
        // but reproducing every coercion isn't worth it — decline.
        _ => return Err(declined("size-of-non-collection")),
    })
}

fn apply_filters(tpl: &Template, mut v: LValue, calls: &[FilterCall]) -> Result<LValue, Error> {
    for call in calls {
        v = filters::apply(tpl, &call.name, v, &call.args)?;
    }
    Ok(v)
}

fn eval_cond(values: &Values, env: &Env<'_>, cond: &Cond) -> Result<bool, Error> {
    match cond {
        Cond::Truthy(e) => {
            let v = eval(values, env, e)?;
            // Liquid truthiness: only nil and false are falsy.
            Ok(!matches!(v, LValue::Nil | LValue::Bool(false)))
        }
        Cond::Compare(lhs, op, rhs) => {
            let l = eval(values, env, lhs)?;
            let r = eval(values, env, rhs)?;
            match (op, &l, &r) {
                (_, LValue::Int(a), LValue::Int(b)) => Ok(match op {
                    CmpOp::Gt => a > b,
                    CmpOp::Lt => a < b,
                    CmpOp::Ge => a >= b,
                    CmpOp::Le => a <= b,
                    CmpOp::Eq => a == b,
                    CmpOp::Ne => a != b,
                }),
                (CmpOp::Eq, LValue::Str(a), LValue::Str(b)) => Ok(a == b),
                (CmpOp::Ne, LValue::Str(a), LValue::Str(b)) => Ok(a != b),
                (CmpOp::Eq | CmpOp::Ne, LValue::Nil, _)
                | (CmpOp::Eq | CmpOp::Ne, _, LValue::Nil) => {
                    let eq = matches!((&l, &r), (LValue::Nil, LValue::Nil));
                    Ok(if matches!(op, CmpOp::Eq) { eq } else { !eq })
                }
                // Mixed-type ordering raises/coerces in odd ways —
                // decline rather than guess.
                _ => Err(declined("compare-shape")),
            }
        }
    }
}

/// Liquid output stringification for the value shapes we accept.
fn write_value(v: &LValue, out: &mut String) -> Result<(), Error> {
    match v {
        LValue::Nil => {}
        LValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        LValue::Int(n) => {
            let mut buf = itoa_buf();
            out.push_str(write_i64(*n, &mut buf));
        }
        LValue::Str(s) => out.push_str(s),
        // Bare arrays join element-wise; bare maps/times have
        // Ruby-inspect-flavoured output — out of subset.
        LValue::Float(_) | LValue::Array(_) | LValue::Map(_) | LValue::Time { .. } => {
            return Err(declined("output-shape"));
        }
    }
    Ok(())
}

fn itoa_buf() -> [u8; 20] {
    [0; 20]
}

fn write_i64(n: i64, buf: &mut [u8; 20]) -> &str {
    use std::io::Write as _;
    let mut cur = std::io::Cursor::new(&mut buf[..]);
    let _ = write!(cur, "{n}");
    let len = cur.position() as usize;
    std::str::from_utf8(&buf[..len]).unwrap_or("")
}
