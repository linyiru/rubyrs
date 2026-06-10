//! Template parsing into constant text + output slots + control flow,
//! with `{% include %}` expanded at compile time. Anything outside the
//! subset declines — the whole template, before any output exists.

use crate::{Error, VarNeed};

#[derive(Debug)]
pub(crate) enum Node {
    Text(String),
    /// `{{ expr | filter | filter: args }}`
    Output {
        expr: Expr,
        filters: Vec<FilterCall>,
    },
    If {
        cond: Cond,
        body: Vec<Node>,
    },
    For {
        var: String,
        collection: String,
        limit: Option<usize>,
        body: Vec<Node>,
    },
}

#[derive(Debug)]
pub(crate) enum Expr {
    StrLit(String),
    IntLit(i64),
    /// A variable reference. `root` resolution happens at parse time:
    /// either a supplied values-map key or a loop variable + field.
    Var(VarRef),
}

#[derive(Debug)]
pub(crate) enum VarRef {
    /// Resolved from the [`crate::Values`] map by full dotted path.
    Supplied { path: String, size: bool },
    /// Loop variable, optionally one field deep (`post.url`).
    Loop {
        var: String,
        field: Option<String>,
        size: bool,
    },
}

#[derive(Debug)]
pub(crate) struct FilterCall {
    pub name: String,
    pub args: Vec<Expr>,
}

#[derive(Debug)]
pub(crate) enum Cond {
    /// `lhs <op> rhs`
    Compare(Expr, CmpOp, Expr),
    /// bare expression truthiness
    Truthy(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CmpOp {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

fn declined(what: &'static str) -> Error {
    Error::Declined(what)
}

pub(crate) fn parse_template(
    source: &str,
    include: &dyn Fn(&str) -> Option<String>,
    needs: &mut Vec<VarNeed>,
) -> Result<Vec<Node>, Error> {
    let mut ctx = Ctx {
        include,
        needs,
        loop_vars: Vec::new(),
        depth: 0,
    };
    let mut toks = lex(source)?;
    let nodes = parse_nodes(&mut ctx, &mut toks, None)?;
    if toks.pos < toks.items.len() {
        return Err(declined("unbalanced-block-tags"));
    }
    Ok(nodes)
}

struct Ctx<'a> {
    include: &'a dyn Fn(&str) -> Option<String>,
    needs: &'a mut Vec<VarNeed>,
    /// In-scope loop variables with their collection paths, so field
    /// reads on a loop var attach to the collection's VarNeed.
    loop_vars: Vec<(String, String)>,
    depth: usize,
}

// ---- lexing into segments ----------------------------------------------

#[derive(Debug)]
enum Tok {
    Text(String),
    /// `{{ … }}` inner content
    Output(String),
    /// `{% … %}` inner content
    Tag(String),
}

struct Toks {
    items: Vec<Tok>,
    pos: usize,
}

fn lex(source: &str) -> Result<Toks, Error> {
    let mut items = Vec::new();
    let mut rest = source;
    loop {
        let next_out = rest.find("{{");
        let next_tag = rest.find("{%");
        let (at, is_tag) = match (next_out, next_tag) {
            (None, None) => {
                if !rest.is_empty() {
                    items.push(Tok::Text(rest.to_string()));
                }
                break;
            }
            (Some(o), None) => (o, false),
            (None, Some(t)) => (t, true),
            (Some(o), Some(t)) => {
                if o < t {
                    (o, false)
                } else {
                    (t, true)
                }
            }
        };
        if at > 0 {
            items.push(Tok::Text(rest[..at].to_string()));
        }
        let open_len = 2;
        let body_start = at + open_len;
        let closer = if is_tag { "%}" } else { "}}" };
        let Some(close_rel) = rest[body_start..].find(closer) else {
            return Err(declined("unterminated-tag"));
        };
        let inner = &rest[body_start..body_start + close_rel];
        // `{{-` / `-}}` whitespace control is out of subset.
        if inner.starts_with('-') || inner.ends_with('-') {
            return Err(declined("whitespace-control"));
        }
        if is_tag {
            items.push(Tok::Tag(inner.trim().to_string()));
        } else {
            items.push(Tok::Output(inner.trim().to_string()));
        }
        rest = &rest[body_start + close_rel + 2..];
    }
    Ok(Toks { items, pos: 0 })
}

// ---- recursive node parsing ---------------------------------------------

/// Parse until `stop` tag (e.g. "endif") or end of tokens.
fn parse_nodes(ctx: &mut Ctx<'_>, toks: &mut Toks, stop: Option<&str>) -> Result<Vec<Node>, Error> {
    let mut out = Vec::new();
    while toks.pos < toks.items.len() {
        match &toks.items[toks.pos] {
            Tok::Text(t) => {
                out.push(Node::Text(t.clone()));
                toks.pos += 1;
            }
            Tok::Output(inner) => {
                let inner = inner.clone();
                toks.pos += 1;
                out.push(parse_output(ctx, &inner)?);
            }
            Tok::Tag(inner) => {
                let inner = inner.clone();
                let word = inner.split_whitespace().next().unwrap_or("");
                if Some(word) == stop {
                    toks.pos += 1;
                    return Ok(out);
                }
                toks.pos += 1;
                match word {
                    "if" => {
                        let cond = parse_cond(ctx, inner["if".len()..].trim())?;
                        let body = parse_nodes(ctx, toks, Some("endif"))?;
                        out.push(Node::If { cond, body });
                    }
                    "for" => {
                        let (var, collection, limit) =
                            parse_for_head(ctx, inner["for".len()..].trim())?;
                        ctx.loop_vars.push((var.clone(), collection.clone()));
                        let body = parse_nodes(ctx, toks, Some("endfor"));
                        ctx.loop_vars.pop();
                        out.push(Node::For {
                            var,
                            collection,
                            limit,
                            body: body?,
                        });
                    }
                    "include" => {
                        let arg = inner["include".len()..].trim();
                        if arg.is_empty() || arg.contains('=') || arg.contains('{') {
                            // include parameters / variable includes
                            return Err(declined("include-params"));
                        }
                        if ctx.depth >= 8 {
                            return Err(declined("include-depth"));
                        }
                        let Some(body_src) = (ctx.include)(arg) else {
                            return Err(declined("include-unresolved"));
                        };
                        let mut sub = lex(&body_src)?;
                        ctx.depth += 1;
                        let nodes = parse_nodes(ctx, &mut sub, None);
                        ctx.depth -= 1;
                        let nodes = nodes?;
                        if sub.pos < sub.items.len() {
                            return Err(declined("unbalanced-block-tags"));
                        }
                        out.extend(nodes);
                    }
                    // Everything else — assign/capture/unless/case/raw/
                    // comment/cycle/tablerow/elsif/else/break/continue/
                    // highlight/... — is out of the Phase-1 subset.
                    _ => return Err(declined("unsupported-tag")),
                }
            }
        }
    }
    if stop.is_some() {
        return Err(declined("unclosed-block-tag"));
    }
    Ok(out)
}

fn parse_output(ctx: &mut Ctx<'_>, inner: &str) -> Result<Node, Error> {
    let mut parts = split_pipes(inner);
    if parts.is_empty() {
        return Err(declined("empty-output"));
    }
    let expr_src = parts.remove(0);
    let mut expr = parse_expr(ctx, expr_src.trim())?;
    let mut filters = Vec::new();
    for p in parts {
        filters.push(parse_filter(ctx, p.trim())?);
    }
    // `x | size` is `x.size` (liquid routes both through #size) —
    // rewrite onto the VarRef so the slice-aware `path#size`
    // companion logic applies uniformly.
    if filters
        .first()
        .map(|f| f.name == "size" && f.args.is_empty())
        == Some(true)
        && let Expr::Var(var) = &mut expr
    {
        let rewritten = match var {
            VarRef::Supplied {
                path,
                size: size @ false,
            } => {
                *size = true;
                // This use turned out to be size-only: shrink ITS
                // need contribution (the most recent push for this
                // path) to the zero-length prefix.
                if let Some(need) = ctx.needs.iter_mut().rev().find(|n| &n.path == path) {
                    need.need_size = true;
                    need.slice = Some(0);
                }
                true
            }
            VarRef::Loop {
                size: size @ false, ..
            } => {
                *size = true;
                true
            }
            _ => false,
        };
        if rewritten {
            filters.remove(0);
        }
    }
    Ok(Node::Output { expr, filters })
}

/// Split on `|` outside quotes.
fn split_pipes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_dq = false;
    let mut in_sq = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            '|' if !in_dq && !in_sq => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn parse_filter(ctx: &mut Ctx<'_>, src: &str) -> Result<FilterCall, Error> {
    let (name, args_src) = match src.find(':') {
        Some(c) => (&src[..c], Some(&src[c + 1..])),
        None => (src, None),
    };
    let name = name.trim();
    if !matches!(
        name,
        "date"
            | "date_to_xmlschema"
            | "slugify"
            | "upcase"
            | "downcase"
            | "truncate"
            | "relative_url"
            | "escape"
            | "number_of_words"
            | "size"
            | "strip"
            | "append"
            | "prepend"
    ) {
        return Err(declined("unsupported-filter"));
    }
    let mut args = Vec::new();
    if let Some(args_src) = args_src {
        for a in split_commas(args_src) {
            let a = a.trim();
            let e = parse_expr(ctx, a)?;
            // Filter arguments must be literals — a variable argument
            // would need dynamic resolution paths we don't track.
            if matches!(e, Expr::Var(_)) {
                return Err(declined("filter-var-arg"));
            }
            args.push(e);
        }
    }
    Ok(FilterCall {
        name: name.to_string(),
        args,
    })
}

fn split_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_dq = false;
    let mut in_sq = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            ',' if !in_dq && !in_sq => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn parse_expr(ctx: &mut Ctx<'_>, src: &str) -> Result<Expr, Error> {
    if src.is_empty() {
        return Err(declined("empty-expr"));
    }
    let bytes = src.as_bytes();
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let q = bytes[0] as char;
        if src.len() < 2 || !src.ends_with(q) {
            return Err(declined("unterminated-string-literal"));
        }
        let inner = &src[1..src.len() - 1];
        if inner.contains(q) || inner.contains('\\') {
            return Err(declined("string-literal-escape"));
        }
        return Ok(Expr::StrLit(inner.to_string()));
    }
    if bytes[0].is_ascii_digit() || (bytes[0] == b'-' && src.len() > 1) {
        if src.contains('.') {
            return Err(declined("float-literal"));
        }
        return match src.parse::<i64>() {
            Ok(n) => Ok(Expr::IntLit(n)),
            Err(_) => Err(declined("bad-int-literal")),
        };
    }
    match src {
        "true" | "false" | "nil" | "null" | "empty" | "blank" => {
            return Err(declined("keyword-literal"));
        }
        _ => {}
    }
    parse_var(ctx, src).map(Expr::Var)
}

fn parse_var(ctx: &mut Ctx<'_>, src: &str) -> Result<VarRef, Error> {
    if src.contains('[') {
        return Err(declined("index-access"));
    }
    let mut segs: Vec<&str> = src.split('.').collect();
    if segs.iter().any(|s| s.is_empty() || !is_ident(s)) {
        return Err(declined("bad-variable-path"));
    }
    let size = if segs.last() == Some(&"size") {
        segs.pop();
        true
    } else {
        false
    };
    if segs.is_empty() {
        return Err(declined("bad-variable-path"));
    }
    if segs[0] == "forloop" {
        return Err(declined("forloop-variable"));
    }
    if let Some((_, collection)) = ctx.loop_vars.iter().find(|(v, _)| v == segs[0]) {
        let var = segs[0].to_string();
        let field = match segs.len() {
            1 => None,
            2 => Some(segs[1].to_string()),
            _ => return Err(declined("deep-loop-field")),
        };
        // Attach the field to the collection's need so the embedder
        // knows which item fields to materialize.
        if let Some(f) = &field {
            let collection = collection.clone();
            if let Some(need) = ctx.needs.iter_mut().rev().find(|n| n.path == collection)
                && !need.fields.contains(f)
            {
                need.fields.push(f.clone());
            }
        }
        return Ok(VarRef::Loop { var, field, size });
    }
    let path = segs.join(".");
    // A size-only use doesn't consume the value body: record it as
    // slice Some(0) ("zero-length prefix suffices") so the merge in
    // `compile` widens to whatever VALUE uses actually need.
    ctx.needs.push(VarNeed {
        path: path.clone(),
        slice: if size { Some(0) } else { None },
        need_size: size,
        fields: Vec::new(),
    });
    Ok(VarRef::Supplied { path, size })
}

fn is_ident(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn parse_cond(ctx: &mut Ctx<'_>, src: &str) -> Result<Cond, Error> {
    if src.contains(" and ") || src.contains(" or ") || src.contains(" contains ") {
        return Err(declined("compound-condition"));
    }
    for (op_src, op) in [
        (">=", CmpOp::Ge),
        ("<=", CmpOp::Le),
        ("==", CmpOp::Eq),
        ("!=", CmpOp::Ne),
        (">", CmpOp::Gt),
        ("<", CmpOp::Lt),
    ] {
        if let Some(at) = src.find(op_src) {
            let lhs = parse_expr(ctx, src[..at].trim())?;
            let rhs = parse_expr(ctx, src[at + op_src.len()..].trim())?;
            return Ok(Cond::Compare(lhs, op, rhs));
        }
    }
    Ok(Cond::Truthy(parse_expr(ctx, src.trim())?))
}

fn parse_for_head(ctx: &mut Ctx<'_>, src: &str) -> Result<(String, String, Option<usize>), Error> {
    // `<var> in <collection> [limit: N]`
    let Some(in_at) = src.find(" in ") else {
        return Err(declined("for-shape"));
    };
    let var = src[..in_at].trim();
    if !is_ident(var) {
        return Err(declined("for-shape"));
    }
    let rest = src[in_at + 4..].trim();
    let (coll_src, limit) = match rest.find("limit:") {
        Some(at) => {
            let n_src = rest[at + "limit:".len()..].trim();
            let n = n_src
                .parse::<usize>()
                .map_err(|_| declined("for-limit-shape"))?;
            (rest[..at].trim(), Some(n))
        }
        None => (rest, None),
    };
    if coll_src.contains("reversed") || coll_src.contains("offset") || coll_src.contains("(") {
        return Err(declined("for-modifiers"));
    }
    // Collection must be a supplied path (not a loop var, not a
    // literal).
    let var_ref = parse_var(ctx, coll_src)?;
    let VarRef::Supplied { path, size: false } = var_ref else {
        return Err(declined("for-collection-shape"));
    };
    // Patch the need we just recorded with the slice information.
    if let Some(need) = ctx.needs.iter_mut().rev().find(|n| n.path == path) {
        need.slice = limit;
    }
    Ok((var.to_string(), path, limit))
}
