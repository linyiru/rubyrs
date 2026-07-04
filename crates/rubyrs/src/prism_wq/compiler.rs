//! Port of `Prism::Translation::Parser::Compiler` (prism 1.9.0's
//! translation/parser/compiler.rb) — visits the decoded prism tree and drives
//! the builder port. One function per prism node type, in the same shapes as
//! the Ruby visitor; any node/edge the port doesn't cover declines the file to
//! the interpreted translation.

use crate::value::Value;

use super::builder::{Ch, DotKind, ODot, Tok, WqNode};
use super::ids::{self, nt};
use super::{decline, ArgVal, CRes, Ctx, Decline, DiagRow, PDiag, PInt, PNode, R};

/// prism flag bits used by the compiler (bits 0-1 are the shared newline
/// flags; see prism node.rb's *Flags modules).
const RANGE_EXCLUDE_END: u32 = 1 << 2;
const CALL_SAFE_NAVIGATION: u32 = 1 << 2;

/// The compiler's per-subtree options (`copy_compiler` copies).
#[derive(Clone, Copy, Default)]
pub(crate) struct Fl {
    fw_star: bool,
    fw_dstar: bool,
    fw_amp: bool,
    fw_dots: bool,
    in_destructure: bool,
    in_pattern: bool,
}

impl Fl {
    fn pattern(self) -> Fl {
        Fl { in_pattern: true, ..self }
    }
    fn destructure(self) -> Fl {
        Fl { in_destructure: true, ..self }
    }
}

/// `build_ast`: visit the ProgramNode.
pub(crate) fn visit_root(ctx: &mut Ctx<'_>, root: &PNode) -> CRes<Option<Box<WqNode>>> {
    if root.ty != nt::PROGRAM_NODE {
        return decline("root is not ProgramNode");
    }
    let Some(stmts) = root.node(ids::program_node::STATEMENTS) else {
        return decline("program without statements");
    };
    visit_statements_opt(ctx, Fl::default(), Some(stmts))
}

// ---------------------------------------------------------------------------
// Helpers (compiler.rb privates)
// ---------------------------------------------------------------------------

/// `token(location)` — `[location.slice, srange(location)]`.
fn token(ctx: &mut Ctx<'_>, bloc: (u32, u32)) -> Tok {
    Tok::b(ctx.slice(bloc).to_vec(), ctx.r(bloc))
}

fn otoken(ctx: &mut Ctx<'_>, bloc: Option<(u32, u32)>) -> Option<Tok> {
    bloc.map(|l| token(ctx, l))
}

/// `srange_offsets(start, end)` in byte offsets.
fn srange_offsets(ctx: &Ctx<'_>, start: u32, end: u32) -> R {
    ctx.r((start, end))
}

/// `srange_semicolon(start, end)` — find `/\A\s*;/` in the byteslice.
fn srange_semicolon(ctx: &mut Ctx<'_>, start: u32, end: Option<u32>) -> Option<Tok> {
    let end = end.unwrap_or(ctx.src.len() as u32) as usize;
    let start = start as usize;
    let slice = ctx.src.get(start..end)?;
    let mut i = 0;
    while i < slice.len() && matches!(slice[i], b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c) {
        i += 1;
    }
    if i < slice.len() && slice[i] == b';' {
        let final_offset = (start + i + 1) as u32;
        Some(Tok::b(b";".to_vec(), ctx.r((final_offset - 1, final_offset))))
    } else {
        None
    }
}

fn visit_opt(ctx: &mut Ctx<'_>, fl: Fl, node: Option<&PNode>) -> CRes<Option<Box<WqNode>>> {
    match node {
        Some(node) => Ok(Some(visit(ctx, fl, node)?)),
        None => Ok(None),
    }
}

fn visit_all(ctx: &mut Ctx<'_>, fl: Fl, nodes: &[PNode]) -> CRes<Vec<Ch>> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        out.push(Ch::N(visit(ctx, fl, node)?));
    }
    Ok(out)
}

/// `visit(node.statements)` where statements is a StatementsNode — compstmt.
fn visit_statements_opt(ctx: &mut Ctx<'_>, fl: Fl, stmts: Option<&PNode>) -> CRes<Option<Box<WqNode>>> {
    match stmts {
        None => Ok(None),
        Some(stmts) => {
            if stmts.ty != nt::STATEMENTS_NODE {
                return decline("expected StatementsNode");
            }
            let body = visit_all(ctx, fl, stmts.list(ids::statements_node::BODY))?;
            ctx.b_compstmt(body)
        }
    }
}

/// `visit(node.arguments) || []` — ArgumentsNode → visited argument list.
fn visit_arguments_opt(ctx: &mut Ctx<'_>, fl: Fl, args: Option<&PNode>) -> CRes<Vec<Ch>> {
    match args {
        None => Ok(vec![]),
        Some(args) => {
            if args.ty != nt::ARGUMENTS_NODE {
                return decline("expected ArgumentsNode");
            }
            visit_all(ctx, fl, args.list(ids::arguments_node::ARGUMENTS))
        }
    }
}

/// The `{"." => :dot, "&." => :anddot, "::" => "::"}` call-operator mapping.
fn call_operator(ctx: &mut Ctx<'_>, bloc: Option<(u32, u32)>) -> CRes<ODot> {
    let Some(bloc) = bloc else { return Ok(None) };
    let kind = match ctx.slice(bloc) {
        b"." => DotKind::Dot,
        b"&." => DotKind::AndDot,
        b"::" => DotKind::ColonColon,
        _ => return decline("unknown call operator"),
    };
    Ok(Some((kind, ctx.r(bloc))))
}

/// `find_forwarding(node.parameters)` → the forwarding flags for a def body.
fn find_forwarding(ctx: &Ctx<'_>, params: Option<&PNode>) -> Fl {
    let _ = ctx;
    let mut fl = Fl::default();
    let Some(params) = params else { return fl };
    if params.ty != nt::PARAMETERS_NODE {
        return fl;
    }
    if let Some(rest) = params.opt_node(ids::parameters_node::REST)
        && rest.ty == nt::REST_PARAMETER_NODE
        && rest.cid(ids::rest_parameter_node::NAME).is_none()
    {
        fl.fw_star = true;
    }
    if let Some(kr) = params.opt_node(ids::parameters_node::KEYWORD_REST) {
        if kr.ty == nt::KEYWORD_REST_PARAMETER_NODE && kr.cid(ids::keyword_rest_parameter_node::NAME).is_none() {
            fl.fw_dstar = true;
        }
        if kr.ty == nt::FORWARDING_PARAMETER_NODE {
            fl.fw_amp = true;
            fl.fw_dots = true;
        }
    }
    if let Some(block) = params.opt_node(ids::parameters_node::BLOCK)
        && block.ty == nt::BLOCK_PARAMETER_NODE
        && block.cid(ids::block_parameter_node::NAME).is_none()
    {
        fl.fw_amp = true;
    }
    fl
}

/// `multi_target_elements` — lefts + rest (unless ImplicitRest) + rights.
fn multi_target_elements(node: &PNode, lefts: usize, rest: usize, rights: usize) -> Vec<&PNode> {
    let mut elements: Vec<&PNode> = node.list(lefts).iter().collect();
    if let Some(r) = node.opt_node(rest)
        && r.ty != nt::IMPLICIT_REST_NODE
    {
        elements.push(r);
    }
    elements.extend(node.list(rights).iter());
    elements
}

/// `within_pattern { |compiler| ... }`.
fn within_pattern<T>(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    f: impl FnOnce(&mut Ctx<'_>, Fl) -> CRes<T>,
) -> CRes<T> {
    ctx.pattern_vars.push(Vec::new());
    let result = f(ctx, fl.pattern());
    ctx.pattern_vars.pop();
    result
}

/// `procarg0?(parameters)`.
fn procarg0(params: Option<&PNode>) -> bool {
    let Some(p) = params else { return false };
    if p.ty != nt::PARAMETERS_NODE {
        return false;
    }
    p.list(ids::parameters_node::REQUIREDS).len() == 1
        && p.list(ids::parameters_node::OPTIONALS).is_empty()
        && p.opt_node(ids::parameters_node::REST).is_none()
        && p.list(ids::parameters_node::POSTS).is_empty()
        && p.list(ids::parameters_node::KEYWORDS).is_empty()
        && p.opt_node(ids::parameters_node::KEYWORD_REST).is_none()
        && p.opt_node(ids::parameters_node::BLOCK).is_none()
}

fn pint_neg(i: &PInt) -> CRes<PInt> {
    match i {
        PInt::Small(n) => Ok(PInt::Small(n.checked_neg().ok_or(Decline("negate overflow"))?)),
        #[cfg(feature = "bignum")]
        PInt::Big(b) => Ok(PInt::Big(-b.clone())),
    }
}

fn pint_to_i64(i: &PInt) -> CRes<i64> {
    match i {
        PInt::Small(n) => Ok(*n),
        #[cfg(feature = "bignum")]
        PInt::Big(_) => decline("bignum where i64 expected"),
    }
}

/// `Rational(numerator, denominator)` from a RationalNode.
fn rational_node_value(ctx: &mut Ctx<'_>, node: &PNode) -> CRes<Value> {
    let num = pint_to_i64(node.int(ids::rational_node::NUMERATOR).ok_or(Decline("rational numerator"))?)?;
    let den = pint_to_i64(node.int(ids::rational_node::DENOMINATOR).ok_or(Decline("rational denominator"))?)?;
    ctx.rational_val(num, den)
}

/// `node.value` for Integer/Float/Rational nodes (numeric_negate + imaginary).
fn numeric_node_value(ctx: &mut Ctx<'_>, node: &PNode, negate: bool) -> CRes<Value> {
    match node.ty {
        nt::INTEGER_NODE => {
            let v = node.int(ids::integer_node::VALUE).ok_or(Decline("integer value"))?;
            let v = if negate {
                pint_neg(v)?
            } else {
                match v {
                    PInt::Small(n) => PInt::Small(*n),
                    #[cfg(feature = "bignum")]
                    PInt::Big(b) => PInt::Big(b.clone()),
                }
            };
            int_value_ref(ctx, &v)
        }
        nt::FLOAT_NODE => {
            let v = node.double(ids::float_node::VALUE).ok_or(Decline("float value"))?;
            Ok(Value::Float(if negate { -v } else { v }))
        }
        nt::RATIONAL_NODE => {
            let num = pint_to_i64(node.int(ids::rational_node::NUMERATOR).ok_or(Decline("rational numerator"))?)?;
            let den = pint_to_i64(node.int(ids::rational_node::DENOMINATOR).ok_or(Decline("rational denominator"))?)?;
            let num = if negate { num.checked_neg().ok_or(Decline("negate overflow"))? } else { num };
            ctx.rational_val(num, den)
        }
        _ => decline("numeric_node_value: unexpected type"),
    }
}

/// `visit(numeric_negate(message_loc, receiver))` — build the negated literal
/// with location = message_loc.join(receiver.location).
fn visit_numeric_negate(ctx: &mut Ctx<'_>, msg_bloc: (u32, u32), receiver: &PNode) -> CRes<Box<WqNode>> {
    let joined = (msg_bloc.0.min(receiver.loc.0), msg_bloc.1.max(receiver.loc.1));
    match receiver.ty {
        nt::INTEGER_NODE => {
            let v = numeric_node_value(ctx, receiver, true)?;
            visit_numeric_literal(ctx, joined, "int", v)
        }
        nt::FLOAT_NODE => {
            let v = numeric_node_value(ctx, receiver, true)?;
            visit_numeric_literal(ctx, joined, "float", v)
        }
        nt::RATIONAL_NODE => {
            let v = numeric_node_value(ctx, receiver, true)?;
            visit_numeric_literal(ctx, joined, "rational", v)
        }
        nt::IMAGINARY_NODE => {
            // copy(numeric: numeric_negate(...)) → Complex(0, -numeric.value)
            let numeric = receiver.node(ids::imaginary_node::NUMERIC).ok_or(Decline("imaginary numeric"))?;
            let imag = numeric_node_value(ctx, numeric, true)?;
            let v = ctx.complex_val(Value::Int(0), imag)?;
            visit_numeric_literal(ctx, joined, "complex", v)
        }
        _ => decline("numeric_negate: unexpected receiver"),
    }
}

/// `visit_numeric(node, builder.<kind>([value, srange(node.location)]))` for
/// a (possibly negated-copy) literal at `bloc`.
fn visit_numeric_literal(ctx: &mut Ctx<'_>, bloc: (u32, u32), kind: &'static str, value: Value) -> CRes<Box<WqNode>> {
    let numeric = ctx.b_numeric(kind, value, ctx.r(bloc));
    apply_numeric_sign(ctx, bloc, numeric)
}

/// `visit_numeric` — wrap with unary_num when the slice leads with a sign.
/// NOTE (spec quirk): `unary_num`'s value rewrite compares `value(unary_t)` —
/// a SYMBOL on this path — against the STRINGS '+'/'-', so the numeric value
/// is never actually mutated; only the map changes.
fn apply_numeric_sign(ctx: &mut Ctx<'_>, bloc: (u32, u32), numeric: Box<WqNode>) -> CRes<Box<WqNode>> {
    let slice = ctx.slice(bloc);
    if matches!(slice.first(), Some(b'+') | Some(b'-')) {
        let sign_r = srange_offsets(ctx, bloc.0, bloc.0 + 1);
        ctx.b_unary_num(sign_r, numeric)
    } else {
        Ok(numeric)
    }
}

// ---------------------------------------------------------------------------
// String-splitting helpers (line continuations)
// ---------------------------------------------------------------------------

/// Ruby `String#lines` over bytes (split after \n, keep the terminator).
fn byte_lines(s: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in s.iter().enumerate() {
        if *b == b'\n' {
            out.push(&s[start..=i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Trailing backslash count of `line` after chomping the line terminator.
fn trailing_backslashes(chomped: &[u8]) -> usize {
    chomped.iter().rev().take_while(|b| **b == b'\\').count()
}

fn chomp(line: &[u8]) -> &[u8] {
    if line.ends_with(b"\r\n") {
        &line[..line.len() - 2]
    } else if line.ends_with(b"\n") || line.ends_with(b"\r") {
        &line[..line.len() - 1]
    } else {
        line
    }
}

/// `string_nodes_from_line_continuations(unescaped, escaped, start_offset,
/// opening)` — the parser gem emits one :str node per source line.
fn string_nodes_from_line_continuations(
    ctx: &mut Ctx<'_>,
    unescaped: &[u8],
    escaped: &[u8],
    start_offset: u32,
    opening: Option<&[u8]>,
) -> CRes<Vec<Ch>> {
    let unescaped_lines = byte_lines(unescaped);
    let escaped_lines = byte_lines(escaped);
    let op = opening.unwrap_or(b"");
    let percent_array = op.starts_with(b"%w") || op.starts_with(b"%W") || op.starts_with(b"%i") || op.starts_with(b"%I");
    let regex = op == b"/" || op.starts_with(b"%r");

    let non_interpolating =
        op.ends_with(b"'") || op.starts_with(b"%q") || op.starts_with(b"%s") || op.starts_with(b"%w") || op.starts_with(b"%i");

    let mut out: Vec<Ch> = Vec::new();

    if non_interpolating {
        let mut start_offset = start_offset;
        let mut current_length: u32 = 0;
        let mut current_line: Vec<u8> = Vec::new();

        for (index, escaped_line) in escaped_lines.iter().enumerate() {
            let unescaped_line: &[u8] = unescaped_lines.get(index).copied().unwrap_or(b"");
            current_length += escaped_line.len() as u32;
            current_line.extend_from_slice(unescaped_line);

            // `escaped_line[/(\\)*\n$/, 1]&.length&.odd?` — the single-char
            // capture group means "odd" is true whenever ≥1 backslash
            // directly precedes the newline.
            let glue = if percent_array {
                escaped_line.ends_with(b"\n") && chomp(escaped_line).ends_with(b"\\")
            } else {
                false
            };
            if glue && index != escaped_lines.len() - 1 {
                start_offset += escaped_line.len() as u32;
                continue;
            }
            let value = ctx.str_val(std::mem::take(&mut current_line), false);
            let r = srange_offsets(ctx, start_offset, start_offset + current_length);
            out.push(Ch::N(ctx.b_string_internal(value, r)));
            start_offset += escaped_line.len() as u32;
            current_length = 0;
        }
        return Ok(out);
    }

    // Interpolating strings.
    let mut escaped_lengths: Vec<u32> = Vec::new();
    let mut normalized_lengths: Vec<u32> = Vec::new();
    let mut do_next_tokens: Vec<bool> = Vec::new();

    // chunk_while { |before, after| before[/(\\*)\r?\n$/, 1]&.length&.odd? }
    let mut chunk: Vec<&[u8]> = Vec::new();
    let mut chunks: Vec<Vec<&[u8]>> = Vec::new();
    for (i, line) in escaped_lines.iter().enumerate() {
        chunk.push(line);
        let continues = {
            // matches (\\*)\r?\n at end — group counts ALL backslashes here.
            let (body, has_nl) = if line.ends_with(b"\r\n") {
                (&line[..line.len() - 2], true)
            } else if line.ends_with(b"\n") {
                (&line[..line.len() - 1], true)
            } else {
                (&line[..], false)
            };
            has_nl && trailing_backslashes(body) % 2 == 1
        };
        if !continues || i == escaped_lines.len() - 1 {
            chunks.push(std::mem::take(&mut chunk));
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }

    for lines in &chunks {
        escaped_lengths.push(lines.iter().map(|l| l.len() as u32).sum());

        let unescaped_lines_count: usize = if regex {
            0 // Will always be preserved as is.
        } else {
            let mut total = 0usize;
            for line in lines {
                // count 'n' occurrences preceded by an odd number of
                // backslashes; minus 1 when the line doesn't end in \n and
                // count > 0.
                let mut count = 0usize;
                for (i, b) in line.iter().enumerate() {
                    if *b == b'n' {
                        let backslashes = line[..i].iter().rev().take_while(|c| **c == b'\\').count();
                        if backslashes % 2 == 1 {
                            count += 1;
                        }
                    }
                }
                if !line.ends_with(b"\n") && count > 0 {
                    count -= 1;
                }
                total += count;
            }
            total
        };

        let extra = if percent_array { lines.len() } else { 1 };
        for _ in 0..(unescaped_lines_count + extra) {
            normalized_lengths.push(0);
            do_next_tokens.push(false);
        }
        let total_len: u32 = lines.iter().map(|l| l.len() as u32).sum();
        *normalized_lengths.last_mut().unwrap() = total_len;
        *do_next_tokens.last_mut().unwrap() = true;
    }

    let mut current_line: Vec<u8> = Vec::new();
    let mut current_normalized_length: u32 = 0;
    let mut emitted_count = 0usize;
    let mut start_offset = start_offset;

    for (index, unescaped_line) in unescaped_lines.iter().enumerate() {
        current_line.extend_from_slice(unescaped_line);
        current_normalized_length += normalized_lengths.get(index).copied().unwrap_or(0);

        if do_next_tokens.get(index).copied().unwrap_or(false) {
            let value = ctx.str_val(std::mem::take(&mut current_line), false);
            let r = srange_offsets(ctx, start_offset, start_offset + current_normalized_length);
            out.push(Ch::N(ctx.b_string_internal(value, r)));
            start_offset += escaped_lengths.get(emitted_count).copied().unwrap_or(0);
            current_normalized_length = 0;
            emitted_count += 1;
        }
    }

    Ok(out)
}

/// `string_nodes_from_interpolation(node, opening)` — flat-map the parts.
fn string_nodes_from_interpolation(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    parts: &[PNode],
    opening: Option<&[u8]>,
) -> CRes<Vec<Ch>> {
    let opening = opening.map(|b| b.to_vec());
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        if part.ty == nt::STRING_NODE
            && ctx
                .slice(part.bloc(ids::string_node::CONTENT_LOC).ok_or(Decline("content_loc"))?)
                .contains(&b'\n')
            && part.opt_bloc(ids::string_node::OPENING_LOC).is_none()
        {
            let unescaped = part.str_bytes(ids::string_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
            let content_loc = part.bloc(ids::string_node::CONTENT_LOC).ok_or(Decline("content_loc"))?;
            let content = ctx.slice(content_loc).to_vec();
            out.extend(string_nodes_from_line_continuations(
                ctx,
                &unescaped,
                &content,
                content_loc.0,
                opening.as_deref(),
            )?);
        } else {
            out.push(Ch::N(visit(ctx, fl, part)?));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Heredocs
// ---------------------------------------------------------------------------

/// `visit_heredoc` — parts assembly for string/xstring heredocs. `compose`
/// selects string_compose vs xstring_compose.
fn visit_heredoc(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    opening_loc: (u32, u32),
    closing_loc: (u32, u32),
    parts: &[PNode],
    xstring: bool,
) -> CRes<Box<WqNode>> {
    let opening = ctx.slice(opening_loc).to_vec();
    let mut children: Vec<Ch> = Vec::new();
    let mut indented = false;

    if opening.starts_with(b"<<~")
        && !parts.is_empty()
        && parts[0].ty != nt::STRING_NODE
    {
        let first_loc = parts[0].loc;
        let line_start = ctx.line_start(first_loc.0);
        // location.copy(start_offset: start - start_line_slice.bytesize) —
        // the length is preserved, so the end shifts left too.
        let len = first_loc.1 - first_loc.0;
        let t = token(ctx, (line_start, line_start + len));
        let value = ctx.tok_str_val(&t)?;
        children.push(Ch::N(ctx.b_string_internal(value, t.r)));
        indented = true;
    }

    for part in parts {
        let pushing: Vec<Ch> = if part.ty == nt::STRING_NODE
            && ctx
                .slice(part.bloc(ids::string_node::CONTENT_LOC).ok_or(Decline("content_loc"))?)
                .contains(&b'\n')
        {
            let unescaped = part.str_bytes(ids::string_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
            let content_loc = part.bloc(ids::string_node::CONTENT_LOC).ok_or(Decline("content_loc"))?;
            let content = ctx.slice(content_loc).to_vec();
            string_nodes_from_line_continuations(ctx, &unescaped, &content, part.loc.0, Some(&opening))?
        } else {
            vec![Ch::N(visit(ctx, fl, part)?)]
        };

        for child in pushing {
            let Ch::N(child) = child else { return decline("heredoc scalar child") };
            let child_is_empty_str = child.ty == "str"
                && matches!(child.children.last(), Some(Ch::V(Value::Str(s))) if s.content.borrow().is_empty());
            if child_is_empty_str {
                // nothing
                continue;
            }
            let mergeable = child.ty == "str"
                && matches!(children.last(), Some(Ch::N(prev)) if prev.ty == "str"
                    && matches!(prev.children.first(), Some(Ch::V(Value::Str(s))) if !s.content.borrow().ends_with(b"\n".as_slice())));
            if mergeable {
                let Some(Ch::N(appendee)) = children.last_mut() else { unreachable!() };
                // "#{appendee.children.first}#{child.children.first}" — a new
                // unfrozen string.
                let mut merged: Vec<u8> = match appendee.children.first() {
                    Some(Ch::V(Value::Str(s))) => s.content.borrow().clone(),
                    _ => return decline("heredoc merge non-str"),
                };
                match child.children.first() {
                    Some(Ch::V(Value::Str(s))) => merged.extend_from_slice(&s.content.borrow()),
                    _ => return decline("heredoc merge non-str"),
                }
                let joined = appendee.expr()?.join(child.expr()?);
                let value = ctx.str_val(merged, false);
                appendee.children = vec![Ch::V(value)];
                if let Some(m) = &mut appendee.map {
                    m.expr = Some(joined);
                }
            } else {
                children.push(Ch::N(child));
            }
        }
    }

    // closing_t: [closing.chomp, srange_offsets(start, end - trailing_ws_len)]
    let closing = ctx.slice(closing_loc).to_vec();
    let chomped = chomp(&closing).to_vec();
    let trailing_ws = closing
        .iter()
        .rev()
        .take_while(|b| matches!(**b, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
        .count() as u32;
    let closing_t = Tok::b(chomped, srange_offsets(ctx, closing_loc.0, closing_loc.1 - trailing_ws));

    let opening_t = token(ctx, opening_loc);
    let mut composed = if xstring {
        ctx.b_xstring_compose(Some(opening_t), children, Some(closing_t))?
    } else {
        ctx.b_string_compose(Some(opening_t), children, Some(closing_t))?
    };

    if indented {
        // composed.updated(nil, children[1..-1]) — drop the synthetic
        // leading-whitespace child; the map stays.
        if composed.children.is_empty() {
            return decline("indented heredoc without children");
        }
        composed.children.remove(0);
    }
    Ok(composed)
}

// ---------------------------------------------------------------------------
// The visitor
// ---------------------------------------------------------------------------

fn visit(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    match node.ty {
        nt::ALIAS_METHOD_NODE => {
            let kw = token(ctx, node.bloc(ids::alias_method_node::KEYWORD_LOC).ok_or(Decline("alias kw"))?);
            let new_name = visit(ctx, fl, node.node(ids::alias_method_node::NEW_NAME).ok_or(Decline("alias new"))?)?;
            let old_name = visit(ctx, fl, node.node(ids::alias_method_node::OLD_NAME).ok_or(Decline("alias old"))?)?;
            ctx.b_alias(kw, new_name, old_name)
        }
        nt::ALIAS_GLOBAL_VARIABLE_NODE => {
            let kw = token(ctx, node.bloc(ids::alias_global_variable_node::KEYWORD_LOC).ok_or(Decline("alias kw"))?);
            let new_name = visit(ctx, fl, node.node(ids::alias_global_variable_node::NEW_NAME).ok_or(Decline("alias new"))?)?;
            let old_name = visit(ctx, fl, node.node(ids::alias_global_variable_node::OLD_NAME).ok_or(Decline("alias old"))?)?;
            ctx.b_alias(kw, new_name, old_name)
        }
        nt::ALTERNATION_PATTERN_NODE => {
            let left = visit(ctx, fl, node.node(ids::alternation_pattern_node::LEFT).ok_or(Decline("alt left"))?)?;
            let op = ctx.r(node.bloc(ids::alternation_pattern_node::OPERATOR_LOC).ok_or(Decline("alt op"))?);
            let right = visit(ctx, fl, node.node(ids::alternation_pattern_node::RIGHT).ok_or(Decline("alt right"))?)?;
            ctx.b_match_alt(left, op, right)
        }
        nt::AND_NODE => {
            let left = visit(ctx, fl, node.node(ids::and_node::LEFT).ok_or(Decline("and left"))?)?;
            let op = ctx.r(node.bloc(ids::and_node::OPERATOR_LOC).ok_or(Decline("and op"))?);
            let right = visit(ctx, fl, node.node(ids::and_node::RIGHT).ok_or(Decline("and right"))?)?;
            ctx.b_logical_op("and", left, op, right)
        }
        nt::OR_NODE => {
            let left = visit(ctx, fl, node.node(ids::or_node::LEFT).ok_or(Decline("or left"))?)?;
            let op = ctx.r(node.bloc(ids::or_node::OPERATOR_LOC).ok_or(Decline("or op"))?);
            let right = visit(ctx, fl, node.node(ids::or_node::RIGHT).ok_or(Decline("or right"))?)?;
            ctx.b_logical_op("or", left, op, right)
        }
        nt::ARRAY_NODE => visit_array_node(ctx, fl, node),
        nt::ARRAY_PATTERN_NODE => visit_array_pattern_node(ctx, fl, node),
        nt::ASSOC_NODE => visit_assoc_node(ctx, fl, node),
        nt::ASSOC_SPLAT_NODE => {
            let op_loc = node.bloc(ids::assoc_splat_node::OPERATOR_LOC).ok_or(Decline("assoc splat op"))?;
            let value = node.opt_node(ids::assoc_splat_node::VALUE);
            if fl.in_pattern {
                let op_t = token(ctx, op_loc);
                let name = match value {
                    Some(v) => {
                        // token(node.value&.location) — the name token.
                        let t = token(ctx, v.loc);
                        let sym = ctx.tok_sym(&t);
                        Some((sym, t.r))
                    }
                    None => None,
                };
                ctx.b_match_rest(op_t, name)
            } else if value.is_none() && fl.fw_dstar {
                {
                let t = token(ctx, op_loc);
                Ok(ctx.b_forwarded_kwrestarg(t))
            }
            } else {
                let op_t = token(ctx, op_loc);
                let value = visit_opt(ctx, fl, value)?.ok_or(Decline("kwsplat without value"))?;
                ctx.b_kwsplat(op_t, value)
            }
        }
        nt::BACK_REFERENCE_READ_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_back_ref(&t))
        }
        nt::BEGIN_NODE => visit_begin_node(ctx, fl, node),
        nt::BLOCK_ARGUMENT_NODE => {
            let op = token(ctx, node.bloc(ids::block_argument_node::OPERATOR_LOC).ok_or(Decline("blockarg op"))?);
            let expr = visit_opt(ctx, fl, node.opt_node(ids::block_argument_node::EXPRESSION))?;
            ctx.b_block_pass(op, expr)
        }
        nt::BLOCK_LOCAL_VARIABLE_NODE => {
            let t = token(ctx, node.loc);
            ctx.b_shadowarg(t)
        }
        nt::BLOCK_PARAMETER_NODE => {
            let op = token(ctx, node.bloc(ids::block_parameter_node::OPERATOR_LOC).ok_or(Decline("bparam op"))?);
            let name_t = otoken(ctx, node.opt_bloc(ids::block_parameter_node::NAME_LOC));
            ctx.b_blockarg(op, name_t)
        }
        nt::BREAK_NODE => {
            let kw = token(ctx, node.bloc(ids::break_node::KEYWORD_LOC).ok_or(Decline("break kw"))?);
            let args = visit_arguments_opt(ctx, fl, node.opt_node(ids::break_node::ARGUMENTS))?;
            ctx.b_keyword_cmd("break", kw, None, args, None)
        }
        nt::NEXT_NODE => {
            let kw = token(ctx, node.bloc(ids::next_node::KEYWORD_LOC).ok_or(Decline("next kw"))?);
            let args = visit_arguments_opt(ctx, fl, node.opt_node(ids::next_node::ARGUMENTS))?;
            ctx.b_keyword_cmd("next", kw, None, args, None)
        }
        nt::RETURN_NODE => {
            let kw = token(ctx, node.bloc(ids::return_node::KEYWORD_LOC).ok_or(Decline("return kw"))?);
            let args = visit_arguments_opt(ctx, fl, node.opt_node(ids::return_node::ARGUMENTS))?;
            ctx.b_keyword_cmd("return", kw, None, args, None)
        }
        nt::CALL_NODE => visit_call_node(ctx, fl, node),
        nt::CALL_OPERATOR_WRITE_NODE => {
            let recv = visit_opt(ctx, fl, node.opt_node(ids::call_operator_write_node::RECEIVER))?;
            let dot = call_operator(ctx, node.opt_bloc(ids::call_operator_write_node::CALL_OPERATOR_LOC))?;
            let msg_loc = node.opt_bloc(ids::call_operator_write_node::MESSAGE_LOC);
            let read_name = ctx.cname(node, ids::call_operator_write_node::READ_NAME)?;
            let sel = msg_loc.map(|l| (read_name, ctx.r(l)));
            let lhs = ctx.b_call_method(recv, dot, sel, None, vec![], None)?;
            let op_loc = node.bloc(ids::call_operator_write_node::BINARY_OPERATOR_LOC).ok_or(Decline("op loc"))?;
            let op_bytes = chomp_eq(ctx.slice(op_loc));
            let op_bytes = op_bytes.to_vec();
            let op_r = ctx.r(op_loc);
            let value = visit(ctx, fl, node.node(ids::call_operator_write_node::VALUE).ok_or(Decline("value"))?)?;
            ctx.b_op_assign(lhs, &op_bytes, op_r, value)
        }
        nt::CALL_AND_WRITE_NODE => {
            visit_call_logical_write(ctx, fl, node, ids::call_and_write_node::RECEIVER, ids::call_and_write_node::CALL_OPERATOR_LOC, ids::call_and_write_node::MESSAGE_LOC, ids::call_and_write_node::READ_NAME, ids::call_and_write_node::OPERATOR_LOC, ids::call_and_write_node::VALUE)
        }
        nt::CALL_OR_WRITE_NODE => {
            visit_call_logical_write(ctx, fl, node, ids::call_or_write_node::RECEIVER, ids::call_or_write_node::CALL_OPERATOR_LOC, ids::call_or_write_node::MESSAGE_LOC, ids::call_or_write_node::READ_NAME, ids::call_or_write_node::OPERATOR_LOC, ids::call_or_write_node::VALUE)
        }
        nt::CALL_TARGET_NODE => {
            let recv = visit(ctx, fl, node.node(ids::call_target_node::RECEIVER).ok_or(Decline("ct recv"))?)?;
            let dot = call_operator(ctx, node.opt_bloc(ids::call_target_node::CALL_OPERATOR_LOC))?;
            let msg_loc = node.bloc(ids::call_target_node::MESSAGE_LOC).ok_or(Decline("ct msg"))?;
            let msg_bytes = ctx.slice(msg_loc).to_vec();
            let msg_r = ctx.r(msg_loc);
            ctx.b_attr_asgn(recv, dot, &msg_bytes, msg_r)
        }
        nt::CAPTURE_PATTERN_NODE => {
            let value = visit(ctx, fl, node.node(ids::capture_pattern_node::VALUE).ok_or(Decline("cap value"))?)?;
            let op = ctx.r(node.bloc(ids::capture_pattern_node::OPERATOR_LOC).ok_or(Decline("cap op"))?);
            let target = visit(ctx, fl, node.node(ids::capture_pattern_node::TARGET).ok_or(Decline("cap target"))?)?;
            ctx.b_match_as(value, op, target)
        }
        nt::CASE_NODE => visit_case_node(ctx, fl, node),
        nt::CASE_MATCH_NODE => visit_case_match_node(ctx, fl, node),
        nt::CLASS_NODE => {
            let class_t = token(ctx, node.bloc(ids::class_node::CLASS_KEYWORD_LOC).ok_or(Decline("class kw"))?);
            let name = visit(ctx, fl, node.node(ids::class_node::CONSTANT_PATH).ok_or(Decline("class path"))?)?;
            let lt_t = otoken(ctx, node.opt_bloc(ids::class_node::INHERITANCE_OPERATOR_LOC));
            let superclass = visit_opt(ctx, fl, node.opt_node(ids::class_node::SUPERCLASS))?;
            let body = visit_body_reset_fw(ctx, fl, node.opt_node(ids::class_node::BODY))?;
            let end_t = token(ctx, node.bloc(ids::class_node::END_KEYWORD_LOC).ok_or(Decline("class end"))?);
            ctx.b_def_class(class_t, name, lt_t, superclass, body, end_t)
        }
        nt::MODULE_NODE => {
            let module_t = token(ctx, node.bloc(ids::module_node::MODULE_KEYWORD_LOC).ok_or(Decline("module kw"))?);
            let name = visit(ctx, fl, node.node(ids::module_node::CONSTANT_PATH).ok_or(Decline("module path"))?)?;
            let body = visit_body_reset_fw(ctx, fl, node.opt_node(ids::module_node::BODY))?;
            let end_t = token(ctx, node.bloc(ids::module_node::END_KEYWORD_LOC).ok_or(Decline("module end"))?);
            ctx.b_def_module(module_t, name, body, end_t)
        }
        nt::SINGLETON_CLASS_NODE => {
            let class_t = token(ctx, node.bloc(ids::singleton_class_node::CLASS_KEYWORD_LOC).ok_or(Decline("sclass kw"))?);
            let op_t = token(ctx, node.bloc(ids::singleton_class_node::OPERATOR_LOC).ok_or(Decline("sclass op"))?);
            let expr = visit(ctx, fl, node.node(ids::singleton_class_node::EXPRESSION).ok_or(Decline("sclass expr"))?)?;
            let body = visit_body_reset_fw(ctx, fl, node.opt_node(ids::singleton_class_node::BODY))?;
            let end_t = token(ctx, node.bloc(ids::singleton_class_node::END_KEYWORD_LOC).ok_or(Decline("sclass end"))?);
            ctx.b_def_sclass(class_t, op_t, expr, body, end_t)
        }
        nt::CLASS_VARIABLE_READ_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_cvar(&t))
        }
        nt::CLASS_VARIABLE_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::class_variable_write_node::NAME_LOC).ok_or(Decline("cvw name"))?);
            let cvar = ctx.b_cvar(&name_t);
            let lhs = ctx.b_assignable(cvar)?;
            let op_r = ctx.r(node.bloc(ids::class_variable_write_node::OPERATOR_LOC).ok_or(Decline("cvw op"))?);
            let value = visit(ctx, fl, node.node(ids::class_variable_write_node::VALUE).ok_or(Decline("cvw value"))?)?;
            ctx.b_assign(lhs, op_r, value)
        }
        nt::CLASS_VARIABLE_OPERATOR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::class_variable_operator_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let cvar = ctx.b_cvar(&name_t);
            let lhs = ctx.b_assignable(cvar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::class_variable_operator_write_node::BINARY_OPERATOR_LOC, ids::class_variable_operator_write_node::VALUE)
        }
        nt::CLASS_VARIABLE_AND_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::class_variable_and_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let cvar = ctx.b_cvar(&name_t);
            let lhs = ctx.b_assignable(cvar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::class_variable_and_write_node::OPERATOR_LOC, ids::class_variable_and_write_node::VALUE)
        }
        nt::CLASS_VARIABLE_OR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::class_variable_or_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let cvar = ctx.b_cvar(&name_t);
            let lhs = ctx.b_assignable(cvar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::class_variable_or_write_node::OPERATOR_LOC, ids::class_variable_or_write_node::VALUE)
        }
        nt::CLASS_VARIABLE_TARGET_NODE => {
            let t = token(ctx, node.loc);
            let cvar = ctx.b_cvar(&t);
            ctx.b_assignable(cvar)
        }
        nt::CONSTANT_READ_NODE => {
            let name = ctx.cname(node, ids::constant_read_node::NAME)?;
            ctx.b_const(name, ctx.r(node.loc))
        }
        nt::CONSTANT_WRITE_NODE => {
            let name = ctx.cname(node, ids::constant_write_node::NAME)?;
            let name_r = ctx.r(node.bloc(ids::constant_write_node::NAME_LOC).ok_or(Decline("cw name"))?);
            let konst = ctx.b_const(name, name_r)?;
            let lhs = ctx.b_assignable(konst)?;
            let op_r = ctx.r(node.bloc(ids::constant_write_node::OPERATOR_LOC).ok_or(Decline("cw op"))?);
            let value = visit(ctx, fl, node.node(ids::constant_write_node::VALUE).ok_or(Decline("cw value"))?)?;
            ctx.b_assign(lhs, op_r, value)
        }
        nt::CONSTANT_OPERATOR_WRITE_NODE => {
            let name = ctx.cname(node, ids::constant_operator_write_node::NAME)?;
            let name_r = ctx.r(node.bloc(ids::constant_operator_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let konst = ctx.b_const(name, name_r)?;
            let lhs = ctx.b_assignable(konst)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::constant_operator_write_node::BINARY_OPERATOR_LOC, ids::constant_operator_write_node::VALUE)
        }
        nt::CONSTANT_AND_WRITE_NODE => {
            let name = ctx.cname(node, ids::constant_and_write_node::NAME)?;
            let name_r = ctx.r(node.bloc(ids::constant_and_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let konst = ctx.b_const(name, name_r)?;
            let lhs = ctx.b_assignable(konst)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::constant_and_write_node::OPERATOR_LOC, ids::constant_and_write_node::VALUE)
        }
        nt::CONSTANT_OR_WRITE_NODE => {
            let name = ctx.cname(node, ids::constant_or_write_node::NAME)?;
            let name_r = ctx.r(node.bloc(ids::constant_or_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let konst = ctx.b_const(name, name_r)?;
            let lhs = ctx.b_assignable(konst)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::constant_or_write_node::OPERATOR_LOC, ids::constant_or_write_node::VALUE)
        }
        nt::CONSTANT_TARGET_NODE => {
            let name = ctx.cname(node, ids::constant_target_node::NAME)?;
            let konst = ctx.b_const(name, ctx.r(node.loc))?;
            ctx.b_assignable(konst)
        }
        nt::CONSTANT_PATH_NODE => visit_constant_path(ctx, fl, node, ids::constant_path_node::PARENT, ids::constant_path_node::NAME, ids::constant_path_node::DELIMITER_LOC, ids::constant_path_node::NAME_LOC),
        nt::CONSTANT_PATH_WRITE_NODE => {
            let target = visit(ctx, fl, node.node(ids::constant_path_write_node::TARGET).ok_or(Decline("cpw target"))?)?;
            let lhs = ctx.b_assignable(target)?;
            let op_r = ctx.r(node.bloc(ids::constant_path_write_node::OPERATOR_LOC).ok_or(Decline("cpw op"))?);
            let value = visit(ctx, fl, node.node(ids::constant_path_write_node::VALUE).ok_or(Decline("cpw value"))?)?;
            ctx.b_assign(lhs, op_r, value)
        }
        nt::CONSTANT_PATH_OPERATOR_WRITE_NODE => {
            let target = visit(ctx, fl, node.node(ids::constant_path_operator_write_node::TARGET).ok_or(Decline("target"))?)?;
            let lhs = ctx.b_assignable(target)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::constant_path_operator_write_node::BINARY_OPERATOR_LOC, ids::constant_path_operator_write_node::VALUE)
        }
        nt::CONSTANT_PATH_AND_WRITE_NODE => {
            let target = visit(ctx, fl, node.node(ids::constant_path_and_write_node::TARGET).ok_or(Decline("target"))?)?;
            let lhs = ctx.b_assignable(target)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::constant_path_and_write_node::OPERATOR_LOC, ids::constant_path_and_write_node::VALUE)
        }
        nt::CONSTANT_PATH_OR_WRITE_NODE => {
            let target = visit(ctx, fl, node.node(ids::constant_path_or_write_node::TARGET).ok_or(Decline("target"))?)?;
            let lhs = ctx.b_assignable(target)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::constant_path_or_write_node::OPERATOR_LOC, ids::constant_path_or_write_node::VALUE)
        }
        nt::CONSTANT_PATH_TARGET_NODE => {
            let path = visit_constant_path(ctx, fl, node, ids::constant_path_target_node::PARENT, ids::constant_path_target_node::NAME, ids::constant_path_target_node::DELIMITER_LOC, ids::constant_path_target_node::NAME_LOC)?;
            ctx.b_assignable(path)
        }
        nt::DEF_NODE => visit_def_node(ctx, fl, node),
        nt::DEFINED_NODE => visit_defined_node(ctx, fl, node),
        nt::ELSE_NODE => {
            visit_statements_opt(ctx, fl, node.opt_node(ids::else_node::STATEMENTS))?
                .ok_or(Decline("else without statements visited as expression"))
        }
        nt::EMBEDDED_STATEMENTS_NODE => {
            let begin_t = token(ctx, node.bloc(ids::embedded_statements_node::OPENING_LOC).ok_or(Decline("embstmt open"))?);
            let stmts = visit_statements_opt(ctx, fl, node.opt_node(ids::embedded_statements_node::STATEMENTS))?;
            let end_t = token(ctx, node.bloc(ids::embedded_statements_node::CLOSING_LOC).ok_or(Decline("embstmt close"))?);
            ctx.b_begin(begin_t, stmts, end_t)
        }
        nt::EMBEDDED_VARIABLE_NODE => {
            visit(ctx, fl, node.node(ids::embedded_variable_node::VARIABLE).ok_or(Decline("embvar"))?)
        }
        nt::FALSE_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_false(&t))
        }
        nt::TRUE_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_true(&t))
        }
        nt::NIL_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_nil(&t))
        }
        nt::SELF_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_self(&t))
        }
        nt::FIND_PATTERN_NODE => visit_find_pattern_node(ctx, fl, node),
        nt::FLOAT_NODE => {
            let v = node.double(ids::float_node::VALUE).ok_or(Decline("float value"))?;
            visit_numeric_literal(ctx, node.loc, "float", Value::Float(v))
        }
        nt::INTEGER_NODE => {
            let v = node.int(ids::integer_node::VALUE).ok_or(Decline("int value"))?;
            let v = int_value_ref(ctx, v)?;
            visit_numeric_literal(ctx, node.loc, "int", v)
        }
        nt::RATIONAL_NODE => {
            let v = rational_node_value(ctx, node)?;
            visit_numeric_literal(ctx, node.loc, "rational", v)
        }
        nt::IMAGINARY_NODE => {
            let numeric = node.node(ids::imaginary_node::NUMERIC).ok_or(Decline("imaginary numeric"))?;
            let imag = numeric_node_value(ctx, numeric, false)?;
            let v = ctx.complex_val(Value::Int(0), imag)?;
            visit_numeric_literal(ctx, node.loc, "complex", v)
        }
        nt::FOR_NODE => visit_for_node(ctx, fl, node),
        nt::FORWARDING_ARGUMENTS_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_forwarded_args(t))
        }
        nt::FORWARDING_PARAMETER_NODE => {
            let t = token(ctx, node.loc);
            Ok(ctx.b_forward_arg(t))
        }
        nt::FORWARDING_SUPER_NODE => {
            let super_t = Tok::b(b"super".to_vec(), srange_offsets(ctx, node.loc.0, node.loc.0 + 5));
            let call = ctx.b_keyword_cmd("zsuper", super_t, None, vec![], None)?;
            visit_block(ctx, fl, call, node.opt_node(ids::forwarding_super_node::BLOCK))
        }
        nt::GLOBAL_VARIABLE_READ_NODE => Ok({
            let t = token(ctx, node.loc);
            ctx.b_gvar(&t)
        }),
        nt::GLOBAL_VARIABLE_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::global_variable_write_node::NAME_LOC).ok_or(Decline("gvw name"))?);
            let gvar = ctx.b_gvar(&name_t);
            let lhs = ctx.b_assignable(gvar)?;
            let op_r = ctx.r(node.bloc(ids::global_variable_write_node::OPERATOR_LOC).ok_or(Decline("gvw op"))?);
            let value = visit(ctx, fl, node.node(ids::global_variable_write_node::VALUE).ok_or(Decline("gvw value"))?)?;
            ctx.b_assign(lhs, op_r, value)
        }
        nt::GLOBAL_VARIABLE_OPERATOR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::global_variable_operator_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let gvar = ctx.b_gvar(&name_t);
            let lhs = ctx.b_assignable(gvar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::global_variable_operator_write_node::BINARY_OPERATOR_LOC, ids::global_variable_operator_write_node::VALUE)
        }
        nt::GLOBAL_VARIABLE_AND_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::global_variable_and_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let gvar = ctx.b_gvar(&name_t);
            let lhs = ctx.b_assignable(gvar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::global_variable_and_write_node::OPERATOR_LOC, ids::global_variable_and_write_node::VALUE)
        }
        nt::GLOBAL_VARIABLE_OR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::global_variable_or_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let gvar = ctx.b_gvar(&name_t);
            let lhs = ctx.b_assignable(gvar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::global_variable_or_write_node::OPERATOR_LOC, ids::global_variable_or_write_node::VALUE)
        }
        nt::GLOBAL_VARIABLE_TARGET_NODE => {
            let t = token(ctx, node.loc);
            let gvar = ctx.b_gvar(&t);
            ctx.b_assignable(gvar)
        }
        nt::HASH_NODE => {
            let begin_t = token(ctx, node.bloc(ids::hash_node::OPENING_LOC).ok_or(Decline("hash open"))?);
            let elements = visit_all(ctx, fl, node.list(ids::hash_node::ELEMENTS))?;
            let end_t = token(ctx, node.bloc(ids::hash_node::CLOSING_LOC).ok_or(Decline("hash close"))?);
            ctx.b_associate(Some(begin_t), elements, Some(end_t))
        }
        nt::KEYWORD_HASH_NODE => {
            let elements = visit_all(ctx, fl, node.list(ids::keyword_hash_node::ELEMENTS))?;
            ctx.b_associate(None, elements, None)
        }
        nt::HASH_PATTERN_NODE => visit_hash_pattern_node(ctx, fl, node),
        nt::IF_NODE => visit_if_node(ctx, fl, node),
        nt::UNLESS_NODE => visit_unless_node(ctx, fl, node),
        nt::IMPLICIT_NODE | nt::IMPLICIT_REST_NODE | nt::BLOCK_NODE | nt::ENSURE_NODE | nt::RESCUE_NODE => {
            decline("directly-visited structural node")
        }
        nt::IN_NODE => visit_in_node(ctx, fl, node),
        nt::INDEX_OPERATOR_WRITE_NODE => {
            let lhs = visit_index_write_lhs(ctx, fl, node, ids::index_operator_write_node::RECEIVER, ids::index_operator_write_node::OPENING_LOC, ids::index_operator_write_node::ARGUMENTS, ids::index_operator_write_node::BLOCK, ids::index_operator_write_node::CLOSING_LOC)?;
            let op_loc = node.bloc(ids::index_operator_write_node::BINARY_OPERATOR_LOC).ok_or(Decline("iop op"))?;
            let op_bytes = chomp_eq(ctx.slice(op_loc)).to_vec();
            let op_r = ctx.r(op_loc);
            let value = visit(ctx, fl, node.node(ids::index_operator_write_node::VALUE).ok_or(Decline("iop value"))?)?;
            ctx.b_op_assign(lhs, &op_bytes, op_r, value)
        }
        nt::INDEX_AND_WRITE_NODE => {
            let lhs = visit_index_write_lhs(ctx, fl, node, ids::index_and_write_node::RECEIVER, ids::index_and_write_node::OPENING_LOC, ids::index_and_write_node::ARGUMENTS, ids::index_and_write_node::BLOCK, ids::index_and_write_node::CLOSING_LOC)?;
            let op_loc = node.bloc(ids::index_and_write_node::OPERATOR_LOC).ok_or(Decline("iand op"))?;
            let op_bytes = chomp_eq(ctx.slice(op_loc)).to_vec();
            let op_r = ctx.r(op_loc);
            let value = visit(ctx, fl, node.node(ids::index_and_write_node::VALUE).ok_or(Decline("iand value"))?)?;
            ctx.b_op_assign(lhs, &op_bytes, op_r, value)
        }
        nt::INDEX_OR_WRITE_NODE => {
            let lhs = visit_index_write_lhs(ctx, fl, node, ids::index_or_write_node::RECEIVER, ids::index_or_write_node::OPENING_LOC, ids::index_or_write_node::ARGUMENTS, ids::index_or_write_node::BLOCK, ids::index_or_write_node::CLOSING_LOC)?;
            let op_loc = node.bloc(ids::index_or_write_node::OPERATOR_LOC).ok_or(Decline("ior op"))?;
            let op_bytes = chomp_eq(ctx.slice(op_loc)).to_vec();
            let op_r = ctx.r(op_loc);
            let value = visit(ctx, fl, node.node(ids::index_or_write_node::VALUE).ok_or(Decline("ior value"))?)?;
            ctx.b_op_assign(lhs, &op_bytes, op_r, value)
        }
        nt::INDEX_TARGET_NODE => {
            let recv = visit(ctx, fl, node.node(ids::index_target_node::RECEIVER).ok_or(Decline("it recv"))?)?;
            let lbrack = token(ctx, node.bloc(ids::index_target_node::OPENING_LOC).ok_or(Decline("it open"))?);
            let args = visit_arguments_opt(ctx, fl, node.opt_node(ids::index_target_node::ARGUMENTS))?;
            let rbrack = token(ctx, node.bloc(ids::index_target_node::CLOSING_LOC).ok_or(Decline("it close"))?);
            ctx.b_index_asgn(recv, lbrack, args, rbrack)
        }
        nt::INSTANCE_VARIABLE_READ_NODE => Ok({
            let t = token(ctx, node.loc);
            ctx.b_ivar(&t)
        }),
        nt::INSTANCE_VARIABLE_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::instance_variable_write_node::NAME_LOC).ok_or(Decline("ivw name"))?);
            let ivar = ctx.b_ivar(&name_t);
            let lhs = ctx.b_assignable(ivar)?;
            let op_r = ctx.r(node.bloc(ids::instance_variable_write_node::OPERATOR_LOC).ok_or(Decline("ivw op"))?);
            let value = visit(ctx, fl, node.node(ids::instance_variable_write_node::VALUE).ok_or(Decline("ivw value"))?)?;
            ctx.b_assign(lhs, op_r, value)
        }
        nt::INSTANCE_VARIABLE_OPERATOR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::instance_variable_operator_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let ivar = ctx.b_ivar(&name_t);
            let lhs = ctx.b_assignable(ivar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::instance_variable_operator_write_node::BINARY_OPERATOR_LOC, ids::instance_variable_operator_write_node::VALUE)
        }
        nt::INSTANCE_VARIABLE_AND_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::instance_variable_and_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let ivar = ctx.b_ivar(&name_t);
            let lhs = ctx.b_assignable(ivar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::instance_variable_and_write_node::OPERATOR_LOC, ids::instance_variable_and_write_node::VALUE)
        }
        nt::INSTANCE_VARIABLE_OR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::instance_variable_or_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let ivar = ctx.b_ivar(&name_t);
            let lhs = ctx.b_assignable(ivar)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::instance_variable_or_write_node::OPERATOR_LOC, ids::instance_variable_or_write_node::VALUE)
        }
        nt::INSTANCE_VARIABLE_TARGET_NODE => {
            let t = token(ctx, node.loc);
            let ivar = ctx.b_ivar(&t);
            ctx.b_assignable(ivar)
        }
        nt::INTERPOLATED_REGULAR_EXPRESSION_NODE | nt::INTERPOLATED_MATCH_LAST_LINE_NODE => {
            visit_interpolated_regexp(ctx, fl, node)
        }
        nt::INTERPOLATED_STRING_NODE => visit_interpolated_string_node(ctx, fl, node),
        nt::INTERPOLATED_SYMBOL_NODE => {
            let opening = node.opt_bloc(ids::interpolated_symbol_node::OPENING_LOC);
            let parts = node.list(ids::interpolated_symbol_node::PARTS);
            let opening_bytes = opening.map(|l| ctx.slice(l).to_vec());
            let children = string_nodes_from_interpolation(ctx, fl, parts, opening_bytes.as_deref())?;
            let begin_t = otoken(ctx, opening);
            let end_t = otoken(ctx, node.opt_bloc(ids::interpolated_symbol_node::CLOSING_LOC));
            ctx.b_symbol_compose(begin_t, children, end_t)
        }
        nt::INTERPOLATED_XSTRING_NODE => {
            let opening_loc = node.bloc(ids::interpolated_xstring_node::OPENING_LOC).ok_or(Decline("ixstr open"))?;
            let parts = node.list(ids::interpolated_xstring_node::PARTS);
            if ctx.slice(opening_loc).starts_with(b"<<") {
                let closing_loc = node.bloc(ids::interpolated_xstring_node::CLOSING_LOC).ok_or(Decline("ixstr close"))?;
                return visit_heredoc(ctx, fl, opening_loc, closing_loc, parts, true);
            }
            let opening_bytes = ctx.slice(opening_loc).to_vec();
            let children = string_nodes_from_interpolation(ctx, fl, parts, Some(&opening_bytes))?;
            let begin_t = Some(token(ctx, opening_loc));
            let end_t = otoken(ctx, node.opt_bloc(ids::interpolated_xstring_node::CLOSING_LOC));
            ctx.b_xstring_compose(begin_t, children, end_t)
        }
        nt::IT_LOCAL_VARIABLE_READ_NODE => {
            let it = ctx.vm.interner.intern("it");
            let mut node_ = ctx.b_ident(&Tok::s(it, ctx.r(node.loc)));
            node_.ty = "lvar";
            Ok(node_)
        }
        nt::IT_PARAMETERS_NODE => Ok(ctx.b_itarg()),
        nt::KEYWORD_REST_PARAMETER_NODE => {
            let op_t = token(ctx, node.bloc(ids::keyword_rest_parameter_node::OPERATOR_LOC).ok_or(Decline("kwrest op"))?);
            let name = match node.cid(ids::keyword_rest_parameter_node::NAME) {
                Some(cid) => {
                    let bytes = ctx.cpool_bytes(cid).ok_or(Decline("kwrest pool"))?.to_vec();
                    let sym = ctx.intern_bytes(&bytes);
                    let name_r = ctx.r(node.bloc(ids::keyword_rest_parameter_node::NAME_LOC).ok_or(Decline("kwrest name loc"))?);
                    Some((sym, name_r))
                }
                None => None,
            };
            ctx.b_kwrestarg(op_t, name)
        }
        nt::LAMBDA_NODE => visit_lambda_node(ctx, fl, node),
        nt::LOCAL_VARIABLE_READ_NODE => {
            let t = token(ctx, node.loc);
            let mut n = ctx.b_ident(&t);
            n.ty = "lvar";
            Ok(n)
        }
        nt::LOCAL_VARIABLE_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::local_variable_write_node::NAME_LOC).ok_or(Decline("lvw name"))?);
            let ident = ctx.b_ident(&name_t);
            let lhs = ctx.b_assignable(ident)?;
            let op_r = ctx.r(node.bloc(ids::local_variable_write_node::OPERATOR_LOC).ok_or(Decline("lvw op"))?);
            let value = visit(ctx, fl, node.node(ids::local_variable_write_node::VALUE).ok_or(Decline("lvw value"))?)?;
            ctx.b_assign(lhs, op_r, value)
        }
        nt::LOCAL_VARIABLE_OPERATOR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::local_variable_operator_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let ident = ctx.b_ident(&name_t);
            let lhs = ctx.b_assignable(ident)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::local_variable_operator_write_node::BINARY_OPERATOR_LOC, ids::local_variable_operator_write_node::VALUE)
        }
        nt::LOCAL_VARIABLE_AND_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::local_variable_and_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let ident = ctx.b_ident(&name_t);
            let lhs = ctx.b_assignable(ident)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::local_variable_and_write_node::OPERATOR_LOC, ids::local_variable_and_write_node::VALUE)
        }
        nt::LOCAL_VARIABLE_OR_WRITE_NODE => {
            let name_t = token(ctx, node.bloc(ids::local_variable_or_write_node::NAME_LOC).ok_or(Decline("name"))?);
            let ident = ctx.b_ident(&name_t);
            let lhs = ctx.b_assignable(ident)?;
            visit_var_op_write(ctx, fl, node, lhs, ids::local_variable_or_write_node::OPERATOR_LOC, ids::local_variable_or_write_node::VALUE)
        }
        nt::LOCAL_VARIABLE_TARGET_NODE => {
            if fl.in_pattern {
                let name = ctx.cname(node, ids::local_variable_target_node::NAME)?;
                let mv = ctx.b_match_var(name, ctx.r(node.loc))?;
                ctx.b_assignable(mv)
            } else {
                let t = token(ctx, node.loc);
                let ident = ctx.b_ident(&t);
                ctx.b_assignable(ident)
            }
        }
        nt::MATCH_PREDICATE_NODE => {
            let value = visit(ctx, fl, node.node(ids::match_predicate_node::VALUE).ok_or(Decline("mp value"))?)?;
            let op_r = ctx.r(node.bloc(ids::match_predicate_node::OPERATOR_LOC).ok_or(Decline("mp op"))?);
            let pattern_node = node.node(ids::match_predicate_node::PATTERN).ok_or(Decline("mp pattern"))?;
            let pattern = within_pattern(ctx, fl, |ctx, pfl| visit(ctx, pfl, pattern_node))?;
            ctx.b_match_pattern_p(value, op_r, pattern)
        }
        nt::MATCH_REQUIRED_NODE => {
            let value = visit(ctx, fl, node.node(ids::match_required_node::VALUE).ok_or(Decline("mr value"))?)?;
            let op_r = ctx.r(node.bloc(ids::match_required_node::OPERATOR_LOC).ok_or(Decline("mr op"))?);
            let pattern_node = node.node(ids::match_required_node::PATTERN).ok_or(Decline("mr pattern"))?;
            let pattern = within_pattern(ctx, fl, |ctx, pfl| visit(ctx, pfl, pattern_node))?;
            ctx.b_match_pattern(value, op_r, pattern)
        }
        nt::MATCH_WRITE_NODE => {
            let call = node.node(ids::match_write_node::CALL).ok_or(Decline("mw call"))?;
            if call.ty != nt::CALL_NODE {
                return decline("match_write call");
            }
            let recv = visit(ctx, fl, call.opt_node(ids::call_node::RECEIVER).ok_or(Decline("mw recv"))?)?;
            let msg_r = ctx.r(call.bloc(ids::call_node::MESSAGE_LOC).ok_or(Decline("mw msg"))?);
            let args = call.opt_node(ids::call_node::ARGUMENTS).ok_or(Decline("mw args"))?;
            let first = args.list(ids::arguments_node::ARGUMENTS).first().ok_or(Decline("mw arg0"))?;
            let arg = visit(ctx, fl, first)?;
            ctx.b_match_op(recv, msg_r, arg)
        }
        nt::MISSING_NODE => decline("missing node (syntax error)"),
        nt::MULTI_TARGET_NODE => {
            let elements = multi_target_elements(node, ids::multi_target_node::LEFTS, ids::multi_target_node::REST, ids::multi_target_node::RIGHTS);
            let visited = visit_refs(ctx, fl, &elements)?;
            let lparen = otoken(ctx, node.opt_bloc(ids::multi_target_node::LPAREN_LOC));
            let rparen = otoken(ctx, node.opt_bloc(ids::multi_target_node::RPAREN_LOC));
            ctx.b_multi_lhs(lparen, visited, rparen)
        }
        nt::MULTI_WRITE_NODE => {
            let mut elements = multi_target_elements(node, ids::multi_write_node::LEFTS, ids::multi_write_node::REST, ids::multi_write_node::RIGHTS);
            if elements.len() == 1
                && elements[0].ty == nt::MULTI_TARGET_NODE
                && node.opt_node(ids::multi_write_node::REST).is_none()
            {
                let inner = elements[0];
                elements = multi_target_elements(inner, ids::multi_target_node::LEFTS, ids::multi_target_node::REST, ids::multi_target_node::RIGHTS);
            }
            let visited = visit_refs(ctx, fl, &elements)?;
            let lparen = otoken(ctx, node.opt_bloc(ids::multi_write_node::LPAREN_LOC));
            let rparen = otoken(ctx, node.opt_bloc(ids::multi_write_node::RPAREN_LOC));
            let mlhs = ctx.b_multi_lhs(lparen, visited, rparen)?;
            let op_r = ctx.r(node.bloc(ids::multi_write_node::OPERATOR_LOC).ok_or(Decline("mw op"))?);
            let value = visit(ctx, fl, node.node(ids::multi_write_node::VALUE).ok_or(Decline("mw value"))?)?;
            ctx.b_multi_assign(mlhs, op_r, value)
        }
        nt::NO_KEYWORDS_PARAMETER_NODE => {
            let op_t = token(ctx, node.bloc(ids::no_keywords_parameter_node::OPERATOR_LOC).ok_or(Decline("nokw op"))?);
            let kw_t = token(ctx, node.bloc(ids::no_keywords_parameter_node::KEYWORD_LOC).ok_or(Decline("nokw kw"))?);
            if fl.in_pattern {
                Ok(ctx.b_match_nil_pattern(op_t, kw_t))
            } else {
                Ok(ctx.b_kwnilarg(op_t, kw_t))
            }
        }
        nt::NUMBERED_PARAMETERS_NODE => {
            let max = node.uint(ids::numbered_parameters_node::MAXIMUM).ok_or(Decline("numparams max"))?;
            Ok(ctx.b_numargs(max as i64))
        }
        nt::NUMBERED_REFERENCE_READ_NODE => {
            let number = node.uint(ids::numbered_reference_read_node::NUMBER).ok_or(Decline("nref number"))?;
            Ok(ctx.b_nth_ref(number as i64, ctx.r(node.loc)))
        }
        nt::OPTIONAL_KEYWORD_PARAMETER_NODE => {
            let name_loc = node.bloc(ids::optional_keyword_parameter_node::NAME_LOC).ok_or(Decline("okw name"))?;
            let name = ctx.cname_bytes(node, ids::optional_keyword_parameter_node::NAME)?;
            let name_r = ctx.r(name_loc);
            let value = visit(ctx, fl, node.node(ids::optional_keyword_parameter_node::VALUE).ok_or(Decline("okw value"))?)?;
            ctx.b_kwoptarg(&name, name_r, value)
        }
        nt::REQUIRED_KEYWORD_PARAMETER_NODE => {
            let name_loc = node.bloc(ids::required_keyword_parameter_node::NAME_LOC).ok_or(Decline("rkw name"))?;
            let name = ctx.cname_bytes(node, ids::required_keyword_parameter_node::NAME)?;
            ctx.b_kwarg(&name, ctx.r(name_loc))
        }
        nt::OPTIONAL_PARAMETER_NODE => {
            let name_t = token(ctx, node.bloc(ids::optional_parameter_node::NAME_LOC).ok_or(Decline("opt name"))?);
            let eql_t = token(ctx, node.bloc(ids::optional_parameter_node::OPERATOR_LOC).ok_or(Decline("opt eq"))?);
            let value = visit(ctx, fl, node.node(ids::optional_parameter_node::VALUE).ok_or(Decline("opt value"))?)?;
            ctx.b_optarg(name_t, eql_t, value)
        }
        nt::REQUIRED_PARAMETER_NODE => {
            let t = token(ctx, node.loc);
            ctx.b_arg(t)
        }
        nt::PARENTHESES_NODE => {
            let begin_t = token(ctx, node.bloc(ids::parentheses_node::OPENING_LOC).ok_or(Decline("paren open"))?);
            let body = visit_paren_body(ctx, fl, node.opt_node(ids::parentheses_node::BODY))?;
            let end_t = token(ctx, node.bloc(ids::parentheses_node::CLOSING_LOC).ok_or(Decline("paren close"))?);
            ctx.b_begin(begin_t, body, end_t)
        }
        nt::PINNED_EXPRESSION_NODE => {
            let expr_node = node.node(ids::pinned_expression_node::EXPRESSION).ok_or(Decline("pin expr"))?;
            // Don't treat * and similar as match_rest: in_pattern = false.
            let parts = visit(ctx, Fl { in_pattern: false, ..fl }, expr_node)?;
            let lparen = token(ctx, node.bloc(ids::pinned_expression_node::LPAREN_LOC).ok_or(Decline("pin lparen"))?);
            let rparen = token(ctx, node.bloc(ids::pinned_expression_node::RPAREN_LOC).ok_or(Decline("pin rparen"))?);
            let expression = ctx.b_begin(lparen, Some(parts), rparen)?;
            let op_t = token(ctx, node.bloc(ids::pinned_expression_node::OPERATOR_LOC).ok_or(Decline("pin op"))?);
            ctx.b_pin(op_t, expression)
        }
        nt::PINNED_VARIABLE_NODE => {
            let op_t = token(ctx, node.bloc(ids::pinned_variable_node::OPERATOR_LOC).ok_or(Decline("pin op"))?);
            let var = visit(ctx, fl, node.node(ids::pinned_variable_node::VARIABLE).ok_or(Decline("pin var"))?)?;
            ctx.b_pin(op_t, var)
        }
        nt::POST_EXECUTION_NODE => {
            let kw = token(ctx, node.bloc(ids::post_execution_node::KEYWORD_LOC).ok_or(Decline("postexe kw"))?);
            let open = token(ctx, node.bloc(ids::post_execution_node::OPENING_LOC).ok_or(Decline("postexe open"))?);
            let stmts = visit_statements_opt(ctx, fl, node.opt_node(ids::post_execution_node::STATEMENTS))?;
            let close = token(ctx, node.bloc(ids::post_execution_node::CLOSING_LOC).ok_or(Decline("postexe close"))?);
            ctx.b_postexe(kw, open, stmts, close)
        }
        nt::PRE_EXECUTION_NODE => {
            let kw = token(ctx, node.bloc(ids::pre_execution_node::KEYWORD_LOC).ok_or(Decline("preexe kw"))?);
            let open = token(ctx, node.bloc(ids::pre_execution_node::OPENING_LOC).ok_or(Decline("preexe open"))?);
            let stmts = visit_statements_opt(ctx, fl, node.opt_node(ids::pre_execution_node::STATEMENTS))?;
            let close = token(ctx, node.bloc(ids::pre_execution_node::CLOSING_LOC).ok_or(Decline("preexe close"))?);
            ctx.b_preexe(kw, open, stmts, close)
        }
        nt::RANGE_NODE => visit_range_like(ctx, fl, node, ids::range_node::LEFT, ids::range_node::RIGHT, ids::range_node::OPERATOR_LOC),
        nt::FLIP_FLOP_NODE => visit_range_like(ctx, fl, node, ids::flip_flop_node::LEFT, ids::flip_flop_node::RIGHT, ids::flip_flop_node::OPERATOR_LOC),
        nt::REDO_NODE => {
            let t = token(ctx, node.loc);
            ctx.b_keyword_cmd("redo", t, None, vec![], None)
        }
        nt::RETRY_NODE => {
            let t = token(ctx, node.loc);
            ctx.b_keyword_cmd("retry", t, None, vec![], None)
        }
        nt::REGULAR_EXPRESSION_NODE | nt::MATCH_LAST_LINE_NODE => visit_regular_expression(ctx, node),
        nt::RESCUE_MODIFIER_NODE => {
            let expr = visit(ctx, fl, node.node(ids::rescue_modifier_node::EXPRESSION).ok_or(Decline("rm expr"))?)?;
            let kw = token(ctx, node.bloc(ids::rescue_modifier_node::KEYWORD_LOC).ok_or(Decline("rm kw"))?);
            let rescue_expr = visit(ctx, fl, node.node(ids::rescue_modifier_node::RESCUE_EXPRESSION).ok_or(Decline("rm rexpr"))?)?;
            let body = ctx.b_rescue_body(kw, None, None, None, None, Some(rescue_expr))?;
            ctx.b_begin_body(Some(expr), vec![body], None, None, None, None)?
                .ok_or(Decline("rescue_mod produced nil"))
        }
        nt::REST_PARAMETER_NODE => {
            let op_t = token(ctx, node.bloc(ids::rest_parameter_node::OPERATOR_LOC).ok_or(Decline("rest op"))?);
            let name_t = otoken(ctx, node.opt_bloc(ids::rest_parameter_node::NAME_LOC));
            ctx.b_restarg(op_t, name_t)
        }
        nt::SHAREABLE_CONSTANT_NODE => {
            visit(ctx, fl, node.node(ids::shareable_constant_node::WRITE).ok_or(Decline("shareable write"))?)
        }
        nt::SOURCE_ENCODING_NODE => Ok({
            let t = token(ctx, node.loc);
            ctx.b_accessible_encoding(&t)
        }),
        nt::SOURCE_FILE_NODE => Ok({
            let t = token(ctx, node.loc);
            ctx.b_accessible_file(&t)
        }),
        nt::SOURCE_LINE_NODE => Ok({
            let t = token(ctx, node.loc);
            let line = ctx.line_of(node.loc.0);
            ctx.b_accessible_line(&t, line)
        }),
        nt::SPLAT_NODE => {
            let op_loc = node.bloc(ids::splat_node::OPERATOR_LOC).ok_or(Decline("splat op"))?;
            let expression = node.opt_node(ids::splat_node::EXPRESSION);
            if expression.is_none() && fl.fw_star && !fl.in_destructure && !fl.in_pattern {
                {
                let t = token(ctx, op_loc);
                Ok(ctx.b_forwarded_restarg(t))
            }
            } else if fl.in_destructure {
                let op_t = token(ctx, op_loc);
                let name_t = expression.map(|e| token(ctx, e.loc));
                ctx.b_restarg(op_t, name_t)
            } else if fl.in_pattern {
                let op_t = token(ctx, op_loc);
                let name = match expression {
                    Some(e) => {
                        let t = token(ctx, e.loc);
                        let sym = ctx.tok_sym(&t);
                        Some((sym, t.r))
                    }
                    None => None,
                };
                ctx.b_match_rest(op_t, name)
            } else {
                let op_t = token(ctx, op_loc);
                let expr = visit_opt(ctx, fl, expression)?;
                ctx.b_splat(op_t, expr)
            }
        }
        nt::STATEMENTS_NODE => {
            visit_statements_opt(ctx, fl, Some(node))?.ok_or(Decline("empty statements as expression"))
        }
        nt::STRING_NODE => visit_string_node(ctx, fl, node),
        nt::SUPER_NODE => {
            let kw = token(ctx, node.bloc(ids::super_node::KEYWORD_LOC).ok_or(Decline("super kw"))?);
            let mut arguments: Vec<&PNode> = match node.opt_node(ids::super_node::ARGUMENTS) {
                Some(a) => a.list(ids::arguments_node::ARGUMENTS).iter().collect(),
                None => vec![],
            };
            let mut block = node.opt_node(ids::super_node::BLOCK);
            if let Some(b) = block
                && b.ty == nt::BLOCK_ARGUMENT_NODE
            {
                arguments.push(b);
                block = None;
            }
            let lparen = otoken(ctx, node.opt_bloc(ids::super_node::LPAREN_LOC));
            let args = visit_refs(ctx, fl, &arguments)?;
            let rparen = otoken(ctx, node.opt_bloc(ids::super_node::RPAREN_LOC));
            let call = ctx.b_keyword_cmd("super", kw, lparen, args, rparen)?;
            visit_block(ctx, fl, call, block)
        }
        nt::SYMBOL_NODE => visit_symbol_node(ctx, node),
        nt::UNDEF_NODE => {
            let kw = token(ctx, node.bloc(ids::undef_node::KEYWORD_LOC).ok_or(Decline("undef kw"))?);
            let names = visit_all(ctx, fl, node.list(ids::undef_node::NAMES))?;
            ctx.b_undef_method(kw, names)
        }
        nt::UNTIL_NODE => visit_while_like(ctx, fl, node, "until", ids::until_node::KEYWORD_LOC, ids::until_node::DO_KEYWORD_LOC, ids::until_node::CLOSING_LOC, ids::until_node::PREDICATE, ids::until_node::STATEMENTS),
        nt::WHILE_NODE => visit_while_like(ctx, fl, node, "while", ids::while_node::KEYWORD_LOC, ids::while_node::DO_KEYWORD_LOC, ids::while_node::CLOSING_LOC, ids::while_node::PREDICATE, ids::while_node::STATEMENTS),
        nt::WHEN_NODE => visit_when_node(ctx, fl, node),
        nt::XSTRING_NODE => visit_x_string_node(ctx, fl, node),
        nt::YIELD_NODE => {
            let kw = token(ctx, node.bloc(ids::yield_node::KEYWORD_LOC).ok_or(Decline("yield kw"))?);
            let lparen = otoken(ctx, node.opt_bloc(ids::yield_node::LPAREN_LOC));
            let args = visit_arguments_opt(ctx, fl, node.opt_node(ids::yield_node::ARGUMENTS))?;
            let rparen = otoken(ctx, node.opt_bloc(ids::yield_node::RPAREN_LOC));
            ctx.b_keyword_cmd("yield", kw, lparen, args, rparen)
        }
        _ => decline("unhandled node type"),
    }
}

// `ctx` is only touched on the `bignum` arm (`PInt::Big` doesn't
// exist without the feature) — same shape as sprintf.rs's bignum
// formatter.
#[cfg_attr(not(feature = "bignum"), allow(unused_variables))]
fn int_value_ref(ctx: &mut Ctx<'_>, i: &PInt) -> CRes<Value> {
    match i {
        PInt::Small(n) => Ok(Value::Int(*n)),
        #[cfg(feature = "bignum")]
        PInt::Big(b) => {
            ctx.check_alloc()?;
            Ok(Value::BigInt(ctx.vm.heap.alloc(crate::heap::HeapObj::BigInt(b.clone()))))
        }
    }
}

fn chomp_eq(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"=").unwrap_or(bytes)
}

fn visit_refs(ctx: &mut Ctx<'_>, fl: Fl, nodes: &[&PNode]) -> CRes<Vec<Ch>> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        out.push(Ch::N(visit(ctx, fl, node)?));
    }
    Ok(out)
}

/// `node.body&.accept(copy_compiler(forwarding: []))`.
fn visit_body_reset_fw(ctx: &mut Ctx<'_>, fl: Fl, body: Option<&PNode>) -> CRes<Option<Box<WqNode>>> {
    let sub = Fl {
        fw_star: false,
        fw_dstar: false,
        fw_amp: false,
        fw_dots: false,
        ..fl
    };
    match body {
        None => Ok(None),
        Some(b) if b.ty == nt::STATEMENTS_NODE => visit_statements_opt(ctx, sub, Some(b)),
        Some(b) if b.ty == nt::BEGIN_NODE => Ok(Some(visit(ctx, sub, b)?)),
        Some(_) => decline("unexpected class body node"),
    }
}

/// Parenthesized body: StatementsNode → compstmt; BeginNode → visit.
fn visit_paren_body(ctx: &mut Ctx<'_>, fl: Fl, body: Option<&PNode>) -> CRes<Option<Box<WqNode>>> {
    match body {
        None => Ok(None),
        Some(b) if b.ty == nt::STATEMENTS_NODE => visit_statements_opt(ctx, fl, Some(b)),
        Some(b) => Ok(Some(visit(ctx, fl, b)?)),
    }
}

// ---------------------------------------------------------------------------
// Bigger visit fns
// ---------------------------------------------------------------------------

fn visit_array_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.opt_bloc(ids::array_node::OPENING_LOC);
    let opening = opening_loc.map(|l| ctx.slice(l).to_vec());
    let is_percent = matches!(&opening, Some(op) if op.starts_with(b"%w") || op.starts_with(b"%W") || op.starts_with(b"%i") || op.starts_with(b"%I"));

    let elements: Vec<Ch> = if is_percent {
        let opening = opening.as_deref();
        let mut out = Vec::new();
        for element in node.list(ids::array_node::ELEMENTS) {
            if element.ty == nt::STRING_NODE {
                let content_loc = element.bloc(ids::string_node::CONTENT_LOC).ok_or(Decline("content_loc"))?;
                let content = ctx.slice(content_loc).to_vec();
                if content.contains(&b'\n') {
                    let unescaped = element.str_bytes(ids::string_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
                    out.extend(string_nodes_from_line_continuations(ctx, &unescaped, &content, content_loc.0, opening)?);
                } else {
                    let unescaped = element.str_bytes(ids::string_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
                    let value = ctx.str_val(unescaped, true);
                    let r = ctx.r(content_loc);
                    out.push(Ch::N(ctx.b_string_internal(value, r)));
                }
            } else if element.ty == nt::INTERPOLATED_STRING_NODE {
                let parts = element.list(ids::interpolated_string_node::PARTS);
                let children = string_nodes_from_interpolation(ctx, fl, parts, opening)?;
                let begin_t = otoken(ctx, element.opt_bloc(ids::interpolated_string_node::OPENING_LOC));
                let end_t = otoken(ctx, element.opt_bloc(ids::interpolated_string_node::CLOSING_LOC));
                out.push(Ch::N(ctx.b_string_compose(begin_t, children, end_t)?));
            } else {
                out.push(Ch::N(visit(ctx, fl, element)?));
            }
        }
        out
    } else {
        visit_all(ctx, fl, node.list(ids::array_node::ELEMENTS))?
    };

    let begin_t = otoken(ctx, opening_loc);
    let end_t = otoken(ctx, node.opt_bloc(ids::array_node::CLOSING_LOC));
    ctx.b_array(begin_t, elements, end_t)
}

fn visit_array_pattern_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let rest = node.opt_node(ids::array_pattern_node::REST);
    let mut elements: Vec<&PNode> = node.list(ids::array_pattern_node::REQUIREDS).iter().collect();
    if let Some(r) = rest
        && r.ty != nt::IMPLICIT_REST_NODE
    {
        elements.push(r);
    }
    elements.extend(node.list(ids::array_pattern_node::POSTS).iter());
    let mut visited = visit_refs(ctx, fl, &elements)?;

    if let Some(r) = rest
        && r.ty == nt::IMPLICIT_REST_NODE
    {
        let comma_t = token(ctx, r.loc);
        let last = visited.pop().ok_or(Decline("trailing comma without elements"))?;
        let Ch::N(last) = last else { return decline("trailing comma scalar") };
        visited.push(Ch::N(ctx.b_match_with_trailing_comma(last, comma_t)?));
    }

    let opening = otoken(ctx, node.opt_bloc(ids::array_pattern_node::OPENING_LOC));
    let closing = otoken(ctx, node.opt_bloc(ids::array_pattern_node::CLOSING_LOC));

    if let Some(constant) = node.opt_node(ids::array_pattern_node::CONSTANT) {
        let konst = visit(ctx, fl, constant)?;
        if visited.is_empty() {
            let (Some(op), Some(cl)) = (opening, closing) else {
                return decline("const array pattern without delimiters");
            };
            let op2 = Tok::b(op.bytes()?.to_vec(), op.r);
            let cl2 = Tok::b(cl.bytes()?.to_vec(), cl.r);
            let inner = ctx.b_array_pattern(Some(op2), Some(visited), Some(cl2))?;
            ctx.b_const_pattern(konst, op, inner, cl)
        } else {
            let (Some(op), Some(cl)) = (opening, closing) else {
                return decline("const array pattern without delimiters");
            };
            let inner = ctx.b_array_pattern(None, Some(visited), None)?;
            ctx.b_const_pattern(konst, op, inner, cl)
        }
    } else {
        ctx.b_array_pattern(opening, Some(visited), closing)
    }
}

fn visit_find_pattern_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let mut elements: Vec<&PNode> = Vec::new();
    elements.push(node.node(ids::find_pattern_node::LEFT).ok_or(Decline("find left"))?);
    elements.extend(node.list(ids::find_pattern_node::REQUIREDS).iter());
    elements.push(node.node(ids::find_pattern_node::RIGHT).ok_or(Decline("find right"))?);
    let visited = visit_refs(ctx, fl, &elements)?;

    let opening = otoken(ctx, node.opt_bloc(ids::find_pattern_node::OPENING_LOC));
    let closing = otoken(ctx, node.opt_bloc(ids::find_pattern_node::CLOSING_LOC));

    if let Some(constant) = node.opt_node(ids::find_pattern_node::CONSTANT) {
        let konst = visit(ctx, fl, constant)?;
        let (Some(op), Some(cl)) = (opening, closing) else {
            return decline("const find pattern without delimiters");
        };
        let inner = ctx.b_find_pattern(None, visited, None)?;
        ctx.b_const_pattern(konst, op, inner, cl)
    } else {
        ctx.b_find_pattern(opening, visited, closing)
    }
}

fn visit_hash_pattern_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let mut elements: Vec<&PNode> = node.list(ids::hash_pattern_node::ELEMENTS).iter().collect();
    if let Some(rest) = node.opt_node(ids::hash_pattern_node::REST) {
        elements.push(rest);
    }
    let visited = visit_refs(ctx, fl, &elements)?;

    let opening = otoken(ctx, node.opt_bloc(ids::hash_pattern_node::OPENING_LOC));
    let closing = otoken(ctx, node.opt_bloc(ids::hash_pattern_node::CLOSING_LOC));

    if let Some(constant) = node.opt_node(ids::hash_pattern_node::CONSTANT) {
        let konst = visit(ctx, fl, constant)?;
        let (Some(op), Some(cl)) = (opening, closing) else {
            return decline("const hash pattern without delimiters");
        };
        let inner = ctx.b_hash_pattern(None, visited, None)?;
        ctx.b_const_pattern(konst, op, inner, cl)
    } else {
        ctx.b_hash_pattern(opening, visited, closing)
    }
}

fn visit_assoc_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let key = node.node(ids::assoc_node::KEY).ok_or(Decline("assoc key"))?;
    let value = node.node(ids::assoc_node::VALUE).ok_or(Decline("assoc value"))?;
    let operator_loc = node.opt_bloc(ids::assoc_node::OPERATOR_LOC);

    if value.ty == nt::IMPLICIT_NODE {
        let ivalue = value.node(ids::implicit_node::VALUE).ok_or(Decline("implicit value"))?;
        if fl.in_pattern {
            if key.ty == nt::SYMBOL_NODE {
                if key.opt_bloc(ids::symbol_node::OPENING_LOC).is_none() {
                    let unescaped = key.str_bytes(ids::symbol_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
                    return ctx.b_match_hash_var(&unescaped, ctx.r(key.loc));
                }
                let opening = token(ctx, key.opt_bloc(ids::symbol_node::OPENING_LOC).ok_or(Decline("open"))?);
                let closing = token(ctx, key.opt_bloc(ids::symbol_node::CLOSING_LOC).ok_or(Decline("close"))?);
                let unescaped = key.str_bytes(ids::symbol_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
                let value_loc = key.opt_bloc(ids::symbol_node::VALUE_LOC).ok_or(Decline("value_loc"))?;
                let value = ctx.str_val(unescaped, true);
                let vr = ctx.r(value_loc);
                let inner = ctx.b_string_internal(value, vr);
                return ctx.b_match_hash_var_from_str(opening, vec![Ch::N(inner)], closing);
            }
            // Interpolated-symbol key.
            if key.ty != nt::INTERPOLATED_SYMBOL_NODE {
                return decline("pattern hash key type");
            }
            let opening = token(ctx, key.opt_bloc(ids::interpolated_symbol_node::OPENING_LOC).ok_or(Decline("open"))?);
            let closing = token(ctx, key.opt_bloc(ids::interpolated_symbol_node::CLOSING_LOC).ok_or(Decline("close"))?);
            let parts = visit_all(ctx, fl, key.list(ids::interpolated_symbol_node::PARTS))?;
            return ctx.b_match_hash_var_from_str(opening, parts, closing);
        }

        // { a: } shorthand outside patterns.
        let key_sym_unescaped = if key.ty == nt::SYMBOL_NODE {
            key.str_bytes(ids::symbol_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec()
        } else {
            return decline("implicit value with non-symbol key");
        };
        let key_value_loc = key.opt_bloc(ids::symbol_node::VALUE_LOC).ok_or(Decline("value_loc"))?;

        let implicit_value = match ivalue.ty {
            nt::CALL_NODE => {
                let name = ctx.cname(ivalue, ids::call_node::NAME)?;
                let msg_loc = ivalue.bloc(ids::call_node::MESSAGE_LOC).ok_or(Decline("implicit msg"))?;
                let msg_r = ctx.r(msg_loc);
                ctx.b_call_method(None, None, Some((name, msg_r)), None, vec![], None)?
            }
            nt::CONSTANT_READ_NODE => {
                let name = ctx.cname(ivalue, ids::constant_read_node::NAME)?;
                ctx.b_const(name, ctx.r(key_value_loc))?
            }
            nt::LOCAL_VARIABLE_READ_NODE | nt::IT_LOCAL_VARIABLE_READ_NODE => {
                let name = if ivalue.ty == nt::LOCAL_VARIABLE_READ_NODE {
                    ctx.cname(ivalue, ids::local_variable_read_node::NAME)?
                } else {
                    ctx.vm.interner.intern("it")
                };
                let mut n = ctx.b_ident(&Tok::s(name, ctx.r(key_value_loc)));
                n.ty = "lvar";
                n
            }
            _ => return decline("implicit value type"),
        };
        return ctx.b_pair_keyword(&key_sym_unescaped, ctx.r(key.loc), implicit_value);
    }

    if let Some(op_loc) = operator_loc {
        let key_v = visit(ctx, fl, key)?;
        let op_r = ctx.r(op_loc);
        let value_v = visit(ctx, fl, value)?;
        return ctx.b_pair(key_v, op_r, value_v);
    }

    if key.ty == nt::SYMBOL_NODE && key.opt_bloc(ids::symbol_node::OPENING_LOC).is_none() {
        let unescaped = key.str_bytes(ids::symbol_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
        let value_v = visit(ctx, fl, value)?;
        return ctx.b_pair_keyword(&unescaped, ctx.r(key.loc), value_v);
    }

    // pair_quoted: "key": value / :"key" interpolation.
    let (opening_loc, closing_loc, parts) = match key.ty {
        nt::SYMBOL_NODE => {
            let unescaped = key.str_bytes(ids::symbol_node::UNESCAPED).ok_or(Decline("unescaped"))?.to_vec();
            let value_loc = key.opt_bloc(ids::symbol_node::VALUE_LOC).ok_or(Decline("value_loc"))?;
            let sval = ctx.str_val(unescaped, true);
            let vr = ctx.r(value_loc);
            let inner = ctx.b_string_internal(sval, vr);
            (
                key.opt_bloc(ids::symbol_node::OPENING_LOC).ok_or(Decline("open"))?,
                key.opt_bloc(ids::symbol_node::CLOSING_LOC).ok_or(Decline("close"))?,
                vec![Ch::N(inner)],
            )
        }
        nt::INTERPOLATED_SYMBOL_NODE => {
            let parts = visit_all(ctx, fl, key.list(ids::interpolated_symbol_node::PARTS))?;
            (
                key.opt_bloc(ids::interpolated_symbol_node::OPENING_LOC).ok_or(Decline("open"))?,
                key.opt_bloc(ids::interpolated_symbol_node::CLOSING_LOC).ok_or(Decline("close"))?,
                parts,
            )
        }
        _ => return decline("pair_quoted key type"),
    };
    let begin_t = token(ctx, opening_loc);
    let end_t = token(ctx, closing_loc);
    let value_v = visit(ctx, fl, value)?;
    ctx.b_pair_quoted(begin_t, parts, end_t, value_v)
}

fn visit_begin_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let mut rescue_bodies: Vec<Box<WqNode>> = Vec::new();

    let else_clause = node.opt_node(ids::begin_node::ELSE_CLAUSE);
    let ensure_clause = node.opt_node(ids::begin_node::ENSURE_CLAUSE);
    let end_keyword_loc = node.opt_bloc(ids::begin_node::END_KEYWORD_LOC);

    let mut rescue_clause = node.opt_node(ids::begin_node::RESCUE_CLAUSE);
    while let Some(rc) = rescue_clause {
        if rc.ty != nt::RESCUE_NODE {
            return decline("rescue clause type");
        }
        let reference = rc.opt_node(ids::rescue_node::REFERENCE);
        let exceptions = rc.list(ids::rescue_node::EXCEPTIONS);
        let keyword_loc = rc.bloc(ids::rescue_node::KEYWORD_LOC).ok_or(Decline("rescue kw"))?;
        let statements = rc.opt_node(ids::rescue_node::STATEMENTS);
        let subsequent = rc.opt_node(ids::rescue_node::SUBSEQUENT);

        let find_start_offset = reference
            .map(|r| r.loc.1)
            .or_else(|| exceptions.last().map(|e| e.loc.1))
            .unwrap_or(keyword_loc.1);
        let find_end_offset = statements
            .map(|s| s.loc.0)
            .or_else(|| subsequent.map(|s| s.loc.0))
            .or_else(|| else_clause.map(|e| e.loc.0))
            .or_else(|| ensure_clause.map(|e| e.loc.0))
            .or_else(|| end_keyword_loc.map(|l| l.0))
            .unwrap_or(find_start_offset + 1);

        let kw_t = token(ctx, keyword_loc);
        let exc_list = if !exceptions.is_empty() {
            let visited = visit_all(ctx, fl, exceptions)?;
            Some(ctx.b_array(None, visited, None)?)
        } else {
            None
        };
        let assoc_t = otoken(ctx, rc.opt_bloc(ids::rescue_node::OPERATOR_LOC));
        let exc_var = visit_opt(ctx, fl, reference)?;
        let then_t = srange_semicolon(ctx, find_start_offset, Some(find_end_offset));
        let stmts = visit_statements_opt(ctx, fl, statements)?;
        rescue_bodies.push(ctx.b_rescue_body(kw_t, exc_list, assoc_t, exc_var, then_t, stmts)?);

        rescue_clause = subsequent;
    }

    let stmts = visit_statements_opt(ctx, fl, node.opt_node(ids::begin_node::STATEMENTS))?;
    let else_t = match else_clause {
        Some(ec) => otoken(ctx, ec.opt_bloc(ids::else_node::ELSE_KEYWORD_LOC).map(Some).unwrap_or(None).or(ec.bloc(ids::else_node::ELSE_KEYWORD_LOC))),
        None => None,
    };
    let else_v = match else_clause {
        Some(ec) => visit_statements_opt(ctx, fl, ec.opt_node(ids::else_node::STATEMENTS))?,
        None => None,
    };
    let ensure_t = match ensure_clause {
        Some(ec) => otoken(ctx, ec.bloc(ids::ensure_node::ENSURE_KEYWORD_LOC)),
        None => None,
    };
    let ensure_v = match ensure_clause {
        Some(ec) => visit_statements_opt(ctx, fl, ec.opt_node(ids::ensure_node::STATEMENTS))?,
        None => None,
    };

    let begin_body = ctx.b_begin_body(stmts, rescue_bodies, else_t, else_v, ensure_t, ensure_v)?;

    if let Some(begin_kw) = node.opt_bloc(ids::begin_node::BEGIN_KEYWORD_LOC) {
        let begin_t = token(ctx, begin_kw);
        let end_t = token(ctx, end_keyword_loc.ok_or(Decline("begin without end"))?);
        ctx.b_begin_keyword(begin_t, begin_body, end_t)
    } else {
        begin_body.ok_or(Decline("begin_body nil without keyword"))
    }
}

fn visit_call_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let name = ctx.cname_bytes(node, ids::call_node::NAME)?;
    let arguments_node = node.opt_node(ids::call_node::ARGUMENTS);
    let mut arguments: Vec<&PNode> = match arguments_node {
        Some(a) => a.list(ids::arguments_node::ARGUMENTS).iter().collect(),
        None => vec![],
    };
    let mut block = node.opt_node(ids::call_node::BLOCK);

    if let Some(b) = block
        && b.ty == nt::BLOCK_ARGUMENT_NODE
    {
        arguments.push(b);
        block = None;
    }

    let call_operator_loc = node.opt_bloc(ids::call_node::CALL_OPERATOR_LOC);
    let message_loc = node.opt_bloc(ids::call_node::MESSAGE_LOC);
    let opening_loc = node.opt_bloc(ids::call_node::OPENING_LOC);
    let closing_loc = node.opt_bloc(ids::call_node::CLOSING_LOC);
    let receiver = node.opt_node(ids::call_node::RECEIVER);

    if call_operator_loc.is_none() {
        match name.as_slice() {
            b"-@" => {
                if let Some(recv) = receiver
                    && matches!(recv.ty, nt::INTEGER_NODE | nt::FLOAT_NODE | nt::RATIONAL_NODE | nt::IMAGINARY_NODE)
                {
                    let msg_loc = message_loc.ok_or(Decline("-@ without message"))?;
                    return visit_numeric_negate(ctx, msg_loc, recv);
                }
            }
            b"!" => {
                let msg_t = token(ctx, message_loc.ok_or(Decline("! without message"))?);
                let begin_t = otoken(ctx, opening_loc);
                let recv = visit_opt(ctx, fl, receiver)?;
                let end_t = otoken(ctx, closing_loc);
                let call = ctx.b_not_op(msg_t, begin_t, recv, end_t)?;
                return visit_block(ctx, fl, call, block);
            }
            b"=~" => {
                if let Some(recv) = receiver
                    && recv.ty == nt::REGULAR_EXPRESSION_NODE
                    && let Some(args_n) = arguments_node
                    && let Some(first) = args_n.list(ids::arguments_node::ARGUMENTS).first()
                {
                    let recv_v = visit(ctx, fl, recv)?;
                    let msg_r = ctx.r(message_loc.ok_or(Decline("=~ msg"))?);
                    let arg = visit(ctx, fl, first)?;
                    return ctx.b_match_op(recv_v, msg_r, arg);
                }
            }
            b"[]" => {
                let recv = visit_opt(ctx, fl, receiver)?.ok_or(Decline("[] without receiver"))?;
                let lbrack = token(ctx, opening_loc.ok_or(Decline("[] open"))?);
                let args = visit_refs(ctx, fl, &arguments)?;
                let rbrack = token(ctx, closing_loc.ok_or(Decline("[] close"))?);
                let call = ctx.b_index(recv, lbrack, args, rbrack)?;
                return visit_block(ctx, fl, call, block);
            }
            b"[]=" => {
                let message_is_brackets_eq = message_loc.map(|l| ctx.slice(l) == b"[]=").unwrap_or(false);
                if !message_is_brackets_eq
                    && let Some(args_n) = arguments_node
                    && block.is_none()
                    && node.flags & CALL_SAFE_NAVIGATION == 0
                {
                    let all_args = args_n.list(ids::arguments_node::ARGUMENTS);
                    if all_args.is_empty() {
                        return decline("[]= without arguments");
                    }
                    let mut index_args: Vec<&PNode> = all_args[..all_args.len() - 1].iter().collect();
                    if let Some(b) = node.opt_node(ids::call_node::BLOCK) {
                        index_args.push(b);
                    }
                    let recv = visit_opt(ctx, fl, receiver)?.ok_or(Decline("[]= without receiver"))?;
                    let lbrack = token(ctx, opening_loc.ok_or(Decline("[]= open"))?);
                    let visited_args = visit_refs(ctx, fl, &index_args)?;
                    let rbrack = token(ctx, closing_loc.ok_or(Decline("[]= close"))?);
                    let lhs = ctx.b_index_asgn(recv, lbrack, visited_args, rbrack)?;
                    let eq_r = ctx.r(node.opt_bloc(ids::call_node::EQUAL_LOC).ok_or(Decline("[]= equal"))?);
                    let value = visit(ctx, fl, all_args.last().unwrap())?;
                    let assigned = ctx.b_assign(lhs, eq_r, value)?;
                    return visit_block(ctx, fl, assigned, block);
                }
            }
            _ => {}
        }
    }

    let dot = call_operator(ctx, call_operator_loc)?;

    let message_ends_eq = message_loc.map(|l| ctx.slice(l).ends_with(b"=")).unwrap_or(false);
    let call = if name.ends_with(b"=")
        && !message_ends_eq
        && let Some(args_n) = arguments_node
        && block.is_none()
    {
        let recv = visit_opt(ctx, fl, receiver)?;
        let msg_loc = message_loc.ok_or(Decline("attr= without message"))?;
        let msg_bytes = ctx.slice(msg_loc).to_vec();
        let msg_r = ctx.r(msg_loc);
        let recv = recv.ok_or(Decline("attr= without receiver"))?;
        let lhs = ctx.b_attr_asgn(recv, dot, &msg_bytes, msg_r)?;
        let eq_r = ctx.r(node.opt_bloc(ids::call_node::EQUAL_LOC).ok_or(Decline("attr= equal"))?);
        let last = args_n.list(ids::arguments_node::ARGUMENTS).last().ok_or(Decline("attr= arg"))?;
        let value = visit(ctx, fl, last)?;
        ctx.b_assign(lhs, eq_r, value)?
    } else {
        let recv = visit_opt(ctx, fl, receiver)?;
        let selector = match message_loc {
            Some(l) => {
                let sym = ctx.intern_bytes(&name);
                Some((sym, ctx.r(l)))
            }
            None => None,
        };
        let lparen = otoken(ctx, opening_loc);
        let args = visit_refs(ctx, fl, &arguments)?;
        let rparen = otoken(ctx, closing_loc);
        ctx.b_call_method(recv, dot, selector, lparen, args, rparen)?
    };

    visit_block(ctx, fl, call, block)
}

#[allow(clippy::too_many_arguments)] // Prism node-field indexes — flat by design
fn visit_call_logical_write(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    node: &PNode,
    receiver_f: usize,
    call_op_f: usize,
    msg_f: usize,
    read_name_f: usize,
    op_f: usize,
    value_f: usize,
) -> CRes<Box<WqNode>> {
    let recv = visit_opt(ctx, fl, node.opt_node(receiver_f))?;
    let dot = call_operator(ctx, node.opt_bloc(call_op_f))?;
    let read_name = ctx.cname(node, read_name_f)?;
    let sel = node.opt_bloc(msg_f).map(|l| (read_name, ctx.r(l)));
    let lhs = ctx.b_call_method(recv, dot, sel, None, vec![], None)?;
    let op_loc = node.bloc(op_f).ok_or(Decline("logical op loc"))?;
    let op_bytes = chomp_eq(ctx.slice(op_loc)).to_vec();
    let op_r = ctx.r(op_loc);
    let value = visit(ctx, fl, node.node(value_f).ok_or(Decline("logical value"))?)?;
    ctx.b_op_assign(lhs, &op_bytes, op_r, value)
}

fn visit_var_op_write(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    node: &PNode,
    lhs: Box<WqNode>,
    op_f: usize,
    value_f: usize,
) -> CRes<Box<WqNode>> {
    let op_loc = node.bloc(op_f).ok_or(Decline("op loc"))?;
    let op_bytes = chomp_eq(ctx.slice(op_loc)).to_vec();
    let op_r = ctx.r(op_loc);
    let value = visit(ctx, fl, node.node(value_f).ok_or(Decline("op value"))?)?;
    ctx.b_op_assign(lhs, &op_bytes, op_r, value)
}

#[allow(clippy::too_many_arguments)] // Prism node-field indexes — flat by design
fn visit_index_write_lhs(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    node: &PNode,
    receiver_f: usize,
    opening_f: usize,
    arguments_f: usize,
    block_f: usize,
    closing_f: usize,
) -> CRes<Box<WqNode>> {
    let mut arguments: Vec<&PNode> = match node.opt_node(arguments_f) {
        Some(a) => a.list(ids::arguments_node::ARGUMENTS).iter().collect(),
        None => vec![],
    };
    if let Some(b) = node.opt_node(block_f) {
        arguments.push(b);
    }
    let recv = visit_opt(ctx, fl, node.opt_node(receiver_f))?.ok_or(Decline("index write receiver"))?;
    let lbrack = token(ctx, node.bloc(opening_f).ok_or(Decline("index write open"))?);
    let args = visit_refs(ctx, fl, &arguments)?;
    let rbrack = token(ctx, node.bloc(closing_f).ok_or(Decline("index write close"))?);
    ctx.b_index(recv, lbrack, args, rbrack)
}

fn visit_case_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let case_t = token(ctx, node.bloc(ids::case_node::CASE_KEYWORD_LOC).ok_or(Decline("case kw"))?);
    let predicate = visit_opt(ctx, fl, node.opt_node(ids::case_node::PREDICATE))?;
    let conditions = visit_all(ctx, fl, node.list(ids::case_node::CONDITIONS))?;
    let else_clause = node.opt_node(ids::case_node::ELSE_CLAUSE);
    let else_t = match else_clause {
        Some(ec) => otoken(ctx, ec.bloc(ids::else_node::ELSE_KEYWORD_LOC)),
        None => None,
    };
    let else_body = match else_clause {
        Some(ec) => visit_statements_opt(ctx, fl, ec.opt_node(ids::else_node::STATEMENTS))?,
        None => None,
    };
    let end_t = token(ctx, node.bloc(ids::case_node::END_KEYWORD_LOC).ok_or(Decline("case end"))?);
    ctx.b_case(case_t, predicate, conditions, else_t, else_body, end_t)
}

fn visit_case_match_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let case_t = token(ctx, node.bloc(ids::case_match_node::CASE_KEYWORD_LOC).ok_or(Decline("case kw"))?);
    let predicate = visit(ctx, fl, node.node(ids::case_match_node::PREDICATE).ok_or(Decline("case pred"))?)?;
    let conditions = visit_all(ctx, fl, node.list(ids::case_match_node::CONDITIONS))?;
    let else_clause = node.opt_node(ids::case_match_node::ELSE_CLAUSE);
    let else_t = match else_clause {
        Some(ec) => otoken(ctx, ec.bloc(ids::else_node::ELSE_KEYWORD_LOC)),
        None => None,
    };
    let else_body = match else_clause {
        Some(ec) => visit_statements_opt(ctx, fl, ec.opt_node(ids::else_node::STATEMENTS))?,
        None => None,
    };
    let end_t = token(ctx, node.bloc(ids::case_match_node::END_KEYWORD_LOC).ok_or(Decline("case end"))?);
    ctx.b_case_match(case_t, predicate, conditions, else_t, else_body, end_t)
}

fn visit_def_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let def_t = token(ctx, node.bloc(ids::def_node::DEF_KEYWORD_LOC).ok_or(Decline("def kw"))?);
    let name_t = token(ctx, node.bloc(ids::def_node::NAME_LOC).ok_or(Decline("def name"))?);
    let params = node.opt_node(ids::def_node::PARAMETERS);
    let lparen = otoken(ctx, node.opt_bloc(ids::def_node::LPAREN_LOC));
    let visited_params = match params {
        Some(p) => visit_parameters_list(ctx, fl, p)?,
        None => vec![],
    };
    let rparen = otoken(ctx, node.opt_bloc(ids::def_node::RPAREN_LOC));
    let args = ctx.b_args(lparen, visited_params, rparen, false)?;

    let body_fl = {
        let fw = find_forwarding(ctx, params);
        Fl { in_destructure: fl.in_destructure, in_pattern: fl.in_pattern, ..fw }
    };
    let body = match node.opt_node(ids::def_node::BODY) {
        None => None,
        Some(b) if b.ty == nt::STATEMENTS_NODE => visit_statements_opt(ctx, body_fl, Some(b))?,
        Some(b) => Some(visit(ctx, body_fl, b)?),
    };

    let receiver = node.opt_node(ids::def_node::RECEIVER);
    let equal_loc = node.opt_bloc(ids::def_node::EQUAL_LOC);

    if let Some(eq) = equal_loc {
        let assignment_t = token(ctx, eq);
        if let Some(recv) = receiver {
            let definee = if recv.ty == nt::PARENTHESES_NODE {
                let inner = recv.opt_node(ids::parentheses_node::BODY).ok_or(Decline("defs paren body"))?;
                visit(ctx, fl, inner)?
            } else {
                visit(ctx, fl, recv)?
            };
            let dot_t = token(ctx, node.opt_bloc(ids::def_node::OPERATOR_LOC).ok_or(Decline("defs dot"))?);
            ctx.b_def_endless_singleton(def_t, definee, dot_t, name_t, args, assignment_t, body)
        } else {
            ctx.b_def_endless_method(def_t, name_t, args, assignment_t, body)
        }
    } else if let Some(recv) = receiver {
        let definee = if recv.ty == nt::PARENTHESES_NODE {
            let inner = recv.opt_node(ids::parentheses_node::BODY).ok_or(Decline("defs paren body"))?;
            visit(ctx, fl, inner)?
        } else {
            visit(ctx, fl, recv)?
        };
        let dot_t = token(ctx, node.opt_bloc(ids::def_node::OPERATOR_LOC).ok_or(Decline("defs dot"))?);
        let end_t = token(ctx, node.opt_bloc(ids::def_node::END_KEYWORD_LOC).ok_or(Decline("defs end"))?);
        ctx.b_def_singleton(def_t, definee, dot_t, name_t, args, body, end_t)
    } else {
        let end_t = token(ctx, node.opt_bloc(ids::def_node::END_KEYWORD_LOC).ok_or(Decline("def end"))?);
        ctx.b_def_method(def_t, name_t, args, body, end_t)
    }
}

fn visit_defined_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let keyword_loc = node.bloc(ids::defined_node::KEYWORD_LOC).ok_or(Decline("defined kw"))?;
    let lparen_loc = node.opt_bloc(ids::defined_node::LPAREN_LOC);
    let rparen_loc = node.opt_bloc(ids::defined_node::RPAREN_LOC);
    let value = node.node(ids::defined_node::VALUE).ok_or(Decline("defined value"))?;

    if let Some(lp) = lparen_loc {
        let joined = (keyword_loc.0.min(lp.0), keyword_loc.1.max(lp.1));
        if ctx.slice(joined).contains(&b'\n') {
            let kw = token(ctx, keyword_loc);
            let begin_t = token(ctx, lp);
            let inner = visit(ctx, fl, value)?;
            let end_t = token(ctx, rparen_loc.ok_or(Decline("defined rparen"))?);
            let wrapped = ctx.b_begin(begin_t, Some(inner), end_t)?;
            return ctx.b_keyword_cmd("defined?", kw, None, vec![Ch::N(wrapped)], None);
        }
    }
    let kw = token(ctx, keyword_loc);
    let lparen = otoken(ctx, lparen_loc);
    let inner = visit(ctx, fl, value)?;
    let rparen = otoken(ctx, rparen_loc);
    ctx.b_keyword_cmd("defined?", kw, lparen, vec![Ch::N(inner)], rparen)
}

fn visit_for_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let for_t = token(ctx, node.bloc(ids::for_node::FOR_KEYWORD_LOC).ok_or(Decline("for kw"))?);
    let index = visit(ctx, fl, node.node(ids::for_node::INDEX).ok_or(Decline("for index"))?)?;
    let in_t = token(ctx, node.bloc(ids::for_node::IN_KEYWORD_LOC).ok_or(Decline("for in"))?);
    let collection_node = node.node(ids::for_node::COLLECTION).ok_or(Decline("for coll"))?;
    let collection = visit(ctx, fl, collection_node)?;
    let statements = node.opt_node(ids::for_node::STATEMENTS);
    let end_keyword_loc = node.bloc(ids::for_node::END_KEYWORD_LOC).ok_or(Decline("for end"))?;

    let do_t = if let Some(dk) = node.opt_bloc(ids::for_node::DO_KEYWORD_LOC) {
        Some(token(ctx, dk))
    } else {
        // srange_semicolon may find nothing — the For map's @begin is nil.
        let end_offset = statements.map(|s| s.loc.0).unwrap_or(end_keyword_loc.0);
        srange_semicolon(ctx, collection_node.loc.1, Some(end_offset))
    };
    let stmts = visit_statements_opt(ctx, fl, statements)?;
    let end_t = token(ctx, end_keyword_loc);
    ctx.b_for(for_t, index, in_t, collection, do_t, stmts, end_t)
}

fn visit_if_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let if_keyword_loc = node.opt_bloc(ids::if_node::IF_KEYWORD_LOC);
    let predicate = node.node(ids::if_node::PREDICATE).ok_or(Decline("if pred"))?;
    let statements = node.opt_node(ids::if_node::STATEMENTS);
    let subsequent = node.opt_node(ids::if_node::SUBSEQUENT);
    let then_keyword_loc = node.opt_bloc(ids::if_node::THEN_KEYWORD_LOC);
    let end_keyword_loc = node.opt_bloc(ids::if_node::END_KEYWORD_LOC);

    let Some(if_kw_loc) = if_keyword_loc else {
        // Ternary.
        let cond = visit(ctx, fl, predicate)?;
        let question_t = token(ctx, then_keyword_loc.ok_or(Decline("ternary ?"))?);
        let if_true = visit_statements_opt(ctx, fl, statements)?.ok_or(Decline("ternary true"))?;
        let sub = subsequent.ok_or(Decline("ternary else"))?;
        if sub.ty != nt::ELSE_NODE {
            return decline("ternary subsequent");
        }
        let colon_t = token(ctx, sub.bloc(ids::else_node::ELSE_KEYWORD_LOC).ok_or(Decline("ternary :"))?);
        let if_false = visit_statements_opt(ctx, fl, sub.opt_node(ids::else_node::STATEMENTS))?.ok_or(Decline("ternary false"))?;
        return ctx.b_ternary(cond, question_t, if_true, colon_t, if_false);
    };

    if if_kw_loc.0 == node.loc.0 {
        let cond_t = token(ctx, if_kw_loc);
        let cond = visit(ctx, fl, predicate)?;
        let then_t = if let Some(tk) = then_keyword_loc {
            Some(token(ctx, tk))
        } else {
            let end_offset = statements
                .map(|s| s.loc.0)
                .or_else(|| subsequent.map(|s| s.loc.0))
                .or_else(|| end_keyword_loc.map(|l| l.0))
                .ok_or(Decline("if then boundary"))?;
            srange_semicolon(ctx, predicate.loc.1, Some(end_offset))
        };
        let if_true = visit_statements_opt(ctx, fl, statements)?;
        let else_t = match subsequent {
            Some(sub) if sub.ty == nt::IF_NODE => {
                otoken(ctx, sub.opt_bloc(ids::if_node::IF_KEYWORD_LOC))
            }
            Some(sub) if sub.ty == nt::ELSE_NODE => {
                otoken(ctx, sub.bloc(ids::else_node::ELSE_KEYWORD_LOC))
            }
            _ => None,
        };
        let if_false = match subsequent {
            Some(sub) if sub.ty == nt::ELSE_NODE => visit_statements_opt(ctx, fl, sub.opt_node(ids::else_node::STATEMENTS))?,
            Some(sub) => Some(visit(ctx, fl, sub)?),
            None => None,
        };
        let end_t = if ctx.slice(if_kw_loc) != b"elsif" {
            otoken(ctx, end_keyword_loc)
        } else {
            None
        };
        ctx.b_condition(cond_t, cond, then_t, if_true, else_t, if_false, end_t)
    } else {
        // Modifier if.
        let if_true = visit_statements_opt(ctx, fl, statements)?;
        let if_false = match subsequent {
            Some(sub) if sub.ty == nt::ELSE_NODE => visit_statements_opt(ctx, fl, sub.opt_node(ids::else_node::STATEMENTS))?,
            Some(sub) => Some(visit(ctx, fl, sub)?),
            None => None,
        };
        let cond_t = token(ctx, if_kw_loc);
        let cond = visit(ctx, fl, predicate)?;
        ctx.b_condition_mod(if_true, if_false, cond_t, cond)
    }
}

fn visit_unless_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let keyword_loc = node.bloc(ids::unless_node::KEYWORD_LOC).ok_or(Decline("unless kw"))?;
    let predicate = node.node(ids::unless_node::PREDICATE).ok_or(Decline("unless pred"))?;
    let statements = node.opt_node(ids::unless_node::STATEMENTS);
    let else_clause = node.opt_node(ids::unless_node::ELSE_CLAUSE);
    let end_keyword_loc = node.opt_bloc(ids::unless_node::END_KEYWORD_LOC);

    if keyword_loc.0 == node.loc.0 {
        let cond_t = token(ctx, keyword_loc);
        let cond = visit(ctx, fl, predicate)?;
        let then_t = if let Some(tk) = node.opt_bloc(ids::unless_node::THEN_KEYWORD_LOC) {
            Some(token(ctx, tk))
        } else {
            let end_offset = statements
                .map(|s| s.loc.0)
                .or_else(|| else_clause.map(|s| s.loc.0))
                .or_else(|| end_keyword_loc.map(|l| l.0))
                .ok_or(Decline("unless then boundary"))?;
            srange_semicolon(ctx, predicate.loc.1, Some(end_offset))
        };
        let if_true = match else_clause {
            Some(ec) => visit_statements_opt(ctx, fl, ec.opt_node(ids::else_node::STATEMENTS))?,
            None => None,
        };
        let else_t = match else_clause {
            Some(ec) => otoken(ctx, ec.bloc(ids::else_node::ELSE_KEYWORD_LOC)),
            None => None,
        };
        let if_false = visit_statements_opt(ctx, fl, statements)?;
        let end_t = otoken(ctx, end_keyword_loc);
        ctx.b_condition(cond_t, cond, then_t, if_true, else_t, if_false, end_t)
    } else {
        // condition_mod(visit(node.else_clause), visit(node.statements), ...)
        // — for modifier-unless the ELSE clause is the if_true child.
        let if_true = match else_clause {
            Some(ec) => visit_statements_opt(ctx, fl, ec.opt_node(ids::else_node::STATEMENTS))?,
            None => None,
        };
        let if_false = visit_statements_opt(ctx, fl, statements)?;
        let cond_t = token(ctx, keyword_loc);
        let cond = visit(ctx, fl, predicate)?;
        ctx.b_condition_mod(if_true, if_false, cond_t, cond)
    }
}

#[allow(clippy::too_many_arguments)] // Prism node-field indexes — flat by design
fn visit_while_like(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    node: &PNode,
    ty: &'static str,
    kw_f: usize,
    do_f: usize,
    closing_f: usize,
    pred_f: usize,
    stmts_f: usize,
) -> CRes<Box<WqNode>> {
    let keyword_loc = node.bloc(kw_f).ok_or(Decline("loop kw"))?;
    let predicate = node.node(pred_f).ok_or(Decline("loop pred"))?;
    let statements = node.opt_node(stmts_f);
    let closing_loc = node.opt_bloc(closing_f);

    if node.loc.0 == keyword_loc.0 {
        let kw_t = token(ctx, keyword_loc);
        let cond = visit(ctx, fl, predicate)?;
        let do_t = if let Some(dk) = node.opt_bloc(do_f) {
            Some(token(ctx, dk))
        } else {
            let end_offset = statements
                .map(|s| s.loc.0)
                .or_else(|| closing_loc.map(|l| l.0))
                .ok_or(Decline("loop do boundary"))?;
            srange_semicolon(ctx, predicate.loc.1, Some(end_offset))
        };
        let body = visit_statements_opt(ctx, fl, statements)?;
        let end_t = token(ctx, closing_loc.ok_or(Decline("loop end"))?);
        ctx.b_loop(ty, kw_t, cond, do_t, body, end_t)
    } else {
        let body = visit_statements_opt(ctx, fl, statements)?.ok_or(Decline("loop_mod body"))?;
        let kw_t = token(ctx, keyword_loc);
        let cond = visit(ctx, fl, predicate)?;
        ctx.b_loop_mod(ty, body, kw_t, cond)
    }
}

fn visit_when_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let kw_t = token(ctx, node.bloc(ids::when_node::KEYWORD_LOC).ok_or(Decline("when kw"))?);
    let conditions = node.list(ids::when_node::CONDITIONS);
    let visited = visit_all(ctx, fl, conditions)?;
    let statements = node.opt_node(ids::when_node::STATEMENTS);
    let then_t = if let Some(tk) = node.opt_bloc(ids::when_node::THEN_KEYWORD_LOC) {
        Some(token(ctx, tk))
    } else {
        let last_cond = conditions.last().ok_or(Decline("when without conditions"))?;
        let end_offset = statements.map(|s| s.loc.0);
        srange_semicolon(ctx, last_cond.loc.1, end_offset)
    };
    let body = visit_statements_opt(ctx, fl, statements)?;
    ctx.b_when(kw_t, visited, then_t, body)
}

fn visit_in_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let pattern_node = node.node(ids::in_node::PATTERN).ok_or(Decline("in pattern"))?;
    let (pattern, guard) = match pattern_node.ty {
        nt::IF_NODE if pattern_node.opt_bloc(ids::if_node::IF_KEYWORD_LOC).is_some() => {
            let stmts = pattern_node.opt_node(ids::if_node::STATEMENTS).ok_or(Decline("guard stmts"))?;
            let pattern = within_pattern(ctx, fl, |ctx, pfl| visit_statements_opt(ctx, pfl, Some(stmts)))?
                .ok_or(Decline("guard pattern"))?;
            let if_t = token(ctx, pattern_node.opt_bloc(ids::if_node::IF_KEYWORD_LOC).ok_or(Decline("guard if"))?);
            let pred = visit(ctx, fl, pattern_node.node(ids::if_node::PREDICATE).ok_or(Decline("guard pred"))?)?;
            let guard = ctx.b_if_guard(if_t, pred)?;
            (pattern, Some(guard))
        }
        nt::UNLESS_NODE => {
            let stmts = pattern_node.opt_node(ids::unless_node::STATEMENTS).ok_or(Decline("guard stmts"))?;
            let pattern = within_pattern(ctx, fl, |ctx, pfl| visit_statements_opt(ctx, pfl, Some(stmts)))?
                .ok_or(Decline("guard pattern"))?;
            let unless_t = token(ctx, pattern_node.bloc(ids::unless_node::KEYWORD_LOC).ok_or(Decline("guard unless"))?);
            let pred = visit(ctx, fl, pattern_node.node(ids::unless_node::PREDICATE).ok_or(Decline("guard pred"))?)?;
            let guard = ctx.b_unless_guard(unless_t, pred)?;
            (pattern, Some(guard))
        }
        _ => {
            let pattern = within_pattern(ctx, fl, |ctx, pfl| visit(ctx, pfl, pattern_node))?;
            (pattern, None)
        }
    };

    let in_t = token(ctx, node.bloc(ids::in_node::IN_LOC).ok_or(Decline("in loc"))?);
    let statements = node.opt_node(ids::in_node::STATEMENTS);
    let then_t = if let Some(tk) = node.opt_bloc(ids::in_node::THEN_LOC) {
        Some(token(ctx, tk))
    } else {
        srange_semicolon(ctx, pattern_node.loc.1, statements.map(|s| s.loc.0))
    };
    let body = visit_statements_opt(ctx, fl, statements)?;
    ctx.b_in_pattern(in_t, pattern, guard, then_t, body)
}

fn visit_constant_path(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    node: &PNode,
    parent_f: usize,
    name_f: usize,
    delim_f: usize,
    name_loc_f: usize,
) -> CRes<Box<WqNode>> {
    let name = match node.cid(name_f) {
        Some(cid) => {
            let bytes = ctx.cpool_bytes(cid).ok_or(Decline("cpath pool"))?.to_vec();
            ctx.intern_bytes(&bytes)
        }
        None => return decline("constant path without name"),
    };
    let name_r = ctx.r(node.bloc(name_loc_f).ok_or(Decline("cpath name loc"))?);
    let delim = node.bloc(delim_f).ok_or(Decline("cpath delim"))?;
    match node.opt_node(parent_f) {
        None => {
            let t = token(ctx, delim);
            ctx.b_const_global(t, name, name_r)
        }
        Some(parent) => {
            let scope = visit(ctx, fl, parent)?;
            let delim_r = ctx.r(delim);
            ctx.b_const_fetch(scope, delim_r, name, name_r)
        }
    }
}

fn visit_range_like(
    ctx: &mut Ctx<'_>,
    fl: Fl,
    node: &PNode,
    left_f: usize,
    right_f: usize,
    op_f: usize,
) -> CRes<Box<WqNode>> {
    let exclusive = node.flags & RANGE_EXCLUDE_END != 0;
    let left = visit_opt(ctx, fl, node.opt_node(left_f))?;
    let op_t = token(ctx, node.bloc(op_f).ok_or(Decline("range op"))?);
    let right = visit_opt(ctx, fl, node.opt_node(right_f))?;
    ctx.b_range(exclusive, left, op_t, right)
}

fn visit_regular_expression(ctx: &mut Ctx<'_>, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.bloc(ids::regular_expression_node::OPENING_LOC).ok_or(Decline("re open"))?;
    let content_loc = node.bloc(ids::regular_expression_node::CONTENT_LOC).ok_or(Decline("re content"))?;
    let closing_loc = node.bloc(ids::regular_expression_node::CLOSING_LOC).ok_or(Decline("re close"))?;
    let unescaped = node.str_bytes(ids::regular_expression_node::UNESCAPED).ok_or(Decline("re unescaped"))?.to_vec();
    let content = ctx.slice(content_loc).to_vec();
    let opening = ctx.slice(opening_loc).to_vec();

    let parts: Vec<Ch> = if content.is_empty() {
        vec![]
    } else if content.contains(&b'\n') {
        string_nodes_from_line_continuations(ctx, &unescaped, &content, content_loc.0, Some(&opening))?
    } else {
        let value = ctx.str_val(unescaped, true);
        let r = ctx.r(content_loc);
        vec![Ch::N(ctx.b_string_internal(value, r))]
    };

    let closing = ctx.slice(closing_loc).to_vec();
    let begin_t = token(ctx, opening_loc);
    let end_t = Tok::b(
        closing.first().map(|b| vec![*b]).unwrap_or_default(),
        srange_offsets(ctx, closing_loc.0, closing_loc.0 + 1),
    );
    let opts_bytes = closing.get(1..).unwrap_or(&[]).to_vec();
    let opts_r = srange_offsets(ctx, closing_loc.0 + 1, closing_loc.1);
    let options = ctx.b_regexp_options(&opts_bytes, opts_r);
    ctx.b_regexp_compose(begin_t, parts, end_t, options)
}

fn visit_interpolated_regexp(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.bloc(ids::interpolated_regular_expression_node::OPENING_LOC).ok_or(Decline("ire open"))?;
    let closing_loc = node.bloc(ids::interpolated_regular_expression_node::CLOSING_LOC).ok_or(Decline("ire close"))?;
    let parts = node.list(ids::interpolated_regular_expression_node::PARTS);
    let opening = ctx.slice(opening_loc).to_vec();
    let children = string_nodes_from_interpolation(ctx, fl, parts, Some(&opening))?;
    let closing = ctx.slice(closing_loc).to_vec();
    let begin_t = token(ctx, opening_loc);
    let end_t = Tok::b(
        closing.first().map(|b| vec![*b]).unwrap_or_default(),
        srange_offsets(ctx, closing_loc.0, closing_loc.0 + 1),
    );
    let opts_bytes = closing.get(1..).unwrap_or(&[]).to_vec();
    let opts_r = srange_offsets(ctx, closing_loc.0 + 1, closing_loc.1);
    let options = ctx.b_regexp_options(&opts_bytes, opts_r);
    ctx.b_regexp_compose(begin_t, children, end_t, options)
}

fn visit_interpolated_string_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.opt_bloc(ids::interpolated_string_node::OPENING_LOC);
    let parts = node.list(ids::interpolated_string_node::PARTS);
    let heredoc = matches!(opening_loc, Some(l) if ctx.slice(l).starts_with(b"<<"));
    if heredoc {
        let closing_loc = node.opt_bloc(ids::interpolated_string_node::CLOSING_LOC).ok_or(Decline("istr close"))?;
        return visit_heredoc(ctx, fl, opening_loc.unwrap(), closing_loc, parts, false);
    }
    let opening_bytes = opening_loc.map(|l| ctx.slice(l).to_vec());
    let children = string_nodes_from_interpolation(ctx, fl, parts, opening_bytes.as_deref())?;
    let begin_t = otoken(ctx, opening_loc);
    let end_t = otoken(ctx, node.opt_bloc(ids::interpolated_string_node::CLOSING_LOC));
    ctx.b_string_compose(begin_t, children, end_t)
}

fn visit_string_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.opt_bloc(ids::string_node::OPENING_LOC);
    let content_loc = node.bloc(ids::string_node::CONTENT_LOC).ok_or(Decline("str content"))?;
    let closing_loc = node.opt_bloc(ids::string_node::CLOSING_LOC);
    let unescaped = node.str_bytes(ids::string_node::UNESCAPED).ok_or(Decline("str unescaped"))?.to_vec();
    let opening = opening_loc.map(|l| ctx.slice(l).to_vec());

    if matches!(&opening, Some(op) if op.starts_with(b"<<")) {
        // to_interpolated: parts = [copy(location: content_loc, opening_loc:
        // nil, closing_loc: nil)] — visit_heredoc consumes the copied part's
        // LOCATION (= content_loc).
        let closing_loc = closing_loc.ok_or(Decline("heredoc str close"))?;
        return visit_heredoc_single_string(ctx, node, opening_loc.unwrap(), closing_loc, content_loc, &unescaped, false);
    }

    if matches!(&opening, Some(op) if op == b"?") {
        let value = ctx.str_val(unescaped, true);
        return Ok(ctx.b_character(value, ctx.r(node.loc)));
    }

    if matches!(&opening, Some(op) if op.starts_with(b"%")) && unescaped.is_empty() {
        let begin_t = otoken(ctx, opening_loc);
        let end_t = otoken(ctx, closing_loc);
        return ctx.b_string_compose(begin_t, vec![], end_t);
    }

    let content = ctx.slice(content_loc).to_vec();
    let parts = if content.contains(&b'\n') {
        string_nodes_from_line_continuations(ctx, &unescaped, &content, content_loc.0, opening.as_deref())?
    } else {
        let value = ctx.str_val(unescaped, true);
        let r = ctx.r(content_loc);
        vec![Ch::N(ctx.b_string_internal(value, r))]
    };

    let begin_t = otoken(ctx, opening_loc);
    let end_t = otoken(ctx, closing_loc);
    let _ = fl;
    ctx.b_string_compose(begin_t, parts, end_t)
}

/// `visit_heredoc(node.to_interpolated)` for a plain StringNode heredoc — the
/// single part is a copy of the node relocated to content_loc.
fn visit_heredoc_single_string(
    ctx: &mut Ctx<'_>,
    node: &PNode,
    opening_loc: (u32, u32),
    closing_loc: (u32, u32),
    content_loc: (u32, u32),
    unescaped: &[u8],
    xstring: bool,
) -> CRes<Box<WqNode>> {
    let opening = ctx.slice(opening_loc).to_vec();
    let mut children: Vec<Ch> = Vec::new();
    // (indented-marker branch never fires: the part IS a StringNode.)

    let content = ctx.slice(content_loc).to_vec();
    let pushing: Vec<Ch> = if content.contains(&b'\n') {
        // part.location.start_offset == content_loc.0 for the copy.
        string_nodes_from_line_continuations(ctx, unescaped, &content, content_loc.0, Some(&opening))?
    } else {
        let value = ctx.str_val(unescaped.to_vec(), true);
        let r = ctx.r(content_loc);
        vec![Ch::N(ctx.b_string_internal(value, r))]
    };
    let _ = node;

    for child in pushing {
        let Ch::N(child) = child else { return decline("heredoc scalar child") };
        let child_is_empty_str = child.ty == "str"
            && matches!(child.children.last(), Some(Ch::V(Value::Str(s))) if s.content.borrow().is_empty());
        if child_is_empty_str {
            continue;
        }
        let mergeable = child.ty == "str"
            && matches!(children.last(), Some(Ch::N(prev)) if prev.ty == "str"
                && matches!(prev.children.first(), Some(Ch::V(Value::Str(s))) if !s.content.borrow().ends_with(b"\n".as_slice())));
        if mergeable {
            let Some(Ch::N(appendee)) = children.last_mut() else { unreachable!() };
            let mut merged: Vec<u8> = match appendee.children.first() {
                Some(Ch::V(Value::Str(s))) => s.content.borrow().clone(),
                _ => return decline("heredoc merge non-str"),
            };
            match child.children.first() {
                Some(Ch::V(Value::Str(s))) => merged.extend_from_slice(&s.content.borrow()),
                _ => return decline("heredoc merge non-str"),
            }
            let joined = appendee.expr()?.join(child.expr()?);
            let value = ctx.str_val(merged, false);
            appendee.children = vec![Ch::V(value)];
            if let Some(m) = &mut appendee.map {
                m.expr = Some(joined);
            }
        } else {
            children.push(Ch::N(child));
        }
    }

    let closing = ctx.slice(closing_loc).to_vec();
    let chomped = chomp(&closing).to_vec();
    let trailing_ws = closing
        .iter()
        .rev()
        .take_while(|b| matches!(**b, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
        .count() as u32;
    let closing_t = Tok::b(chomped, srange_offsets(ctx, closing_loc.0, closing_loc.1 - trailing_ws));
    let opening_t = token(ctx, opening_loc);
    if xstring {
        ctx.b_xstring_compose(Some(opening_t), children, Some(closing_t))
    } else {
        ctx.b_string_compose(Some(opening_t), children, Some(closing_t))
    }
}

fn visit_x_string_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.bloc(ids::xstring_node::OPENING_LOC).ok_or(Decline("xstr open"))?;
    let content_loc = node.bloc(ids::xstring_node::CONTENT_LOC).ok_or(Decline("xstr content"))?;
    let closing_loc = node.bloc(ids::xstring_node::CLOSING_LOC).ok_or(Decline("xstr close"))?;
    let unescaped = node.str_bytes(ids::xstring_node::UNESCAPED).ok_or(Decline("xstr unescaped"))?.to_vec();
    let opening = ctx.slice(opening_loc).to_vec();
    let _ = fl;

    if opening.starts_with(b"<<") {
        // to_interpolated: single StringNode part located at content_loc,
        // composed via xstring_compose (never collapsed).
        return visit_heredoc_single_string(ctx, node, opening_loc, closing_loc, content_loc, &unescaped, true);
    }

    let content = ctx.slice(content_loc).to_vec();
    let parts: Vec<Ch> = if content.is_empty() {
        vec![]
    } else if content.contains(&b'\n') {
        string_nodes_from_line_continuations(ctx, &unescaped, &content, content_loc.0, Some(&opening))?
    } else {
        let value = ctx.str_val(unescaped, true);
        let r = ctx.r(content_loc);
        vec![Ch::N(ctx.b_string_internal(value, r))]
    };

    let begin_t = Some(token(ctx, opening_loc));
    let end_t = Some(token(ctx, closing_loc));
    ctx.b_xstring_compose(begin_t, parts, end_t)
}

fn visit_symbol_node(ctx: &mut Ctx<'_>, node: &PNode) -> CRes<Box<WqNode>> {
    let opening_loc = node.opt_bloc(ids::symbol_node::OPENING_LOC);
    let value_loc = node.opt_bloc(ids::symbol_node::VALUE_LOC);
    let closing_loc = node.opt_bloc(ids::symbol_node::CLOSING_LOC);
    let unescaped = node.str_bytes(ids::symbol_node::UNESCAPED).ok_or(Decline("sym unescaped"))?.to_vec();

    if closing_loc.is_none() {
        if opening_loc.is_none() {
            return Ok(ctx.b_symbol_internal(&unescaped, ctx.r(node.loc)));
        }
        return Ok(ctx.b_symbol(&unescaped, ctx.r(node.loc)));
    }

    // symbol_compose — :"foo" or %s[...].
    let value = value_loc.map(|l| ctx.slice(l).to_vec()).unwrap_or_default();
    let parts: Vec<Ch> = if value.is_empty() {
        vec![]
    } else if value.contains(&b'\n') {
        let value_loc = value_loc.ok_or(Decline("sym value loc"))?;
        let opening = opening_loc.map(|l| ctx.slice(l).to_vec());
        string_nodes_from_line_continuations(ctx, &unescaped, &value, value_loc.0, opening.as_deref())?
    } else {
        let value_loc = value_loc.ok_or(Decline("sym value loc"))?;
        let sval = ctx.str_val(unescaped, true);
        let r = ctx.r(value_loc);
        vec![Ch::N(ctx.b_string_internal(sval, r))]
    };

    let begin_t = otoken(ctx, opening_loc);
    let end_t = otoken(ctx, closing_loc);
    ctx.b_symbol_compose(begin_t, parts, end_t)
}

fn visit_lambda_node(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Box<WqNode>> {
    let operator_loc = node.bloc(ids::lambda_node::OPERATOR_LOC).ok_or(Decline("lambda op"))?;
    let opening_loc = node.bloc(ids::lambda_node::OPENING_LOC).ok_or(Decline("lambda open"))?;
    let closing_loc = node.bloc(ids::lambda_node::CLOSING_LOC).ok_or(Decline("lambda close"))?;
    let parameters = node.opt_node(ids::lambda_node::PARAMETERS);

    let call = ctx.b_call_lambda(ctx.r(operator_loc))?;
    let begin_t = token(ctx, opening_loc);
    let end_t = token(ctx, closing_loc);

    let args = match parameters {
        None => ctx.b_args_none()?,
        Some(p) if p.ty == nt::NUMBERED_PARAMETERS_NODE || p.ty == nt::IT_PARAMETERS_NODE => {
            visit(ctx, fl, p)?
        }
        Some(p) => {
            if p.ty != nt::BLOCK_PARAMETERS_NODE {
                return decline("lambda parameters type");
            }
            let popen = otoken(ctx, p.opt_bloc(ids::block_parameters_node::OPENING_LOC));
            let items = visit_block_parameters(ctx, fl, p)?;
            let pclose = otoken(ctx, p.opt_bloc(ids::block_parameters_node::CLOSING_LOC));
            ctx.b_args(popen, items, pclose, false)?
        }
    };

    let body = visit_body_generic(ctx, fl, node.opt_node(ids::lambda_node::BODY))?;
    ctx.b_block(call, begin_t, args, body, end_t)
}

fn visit_body_generic(ctx: &mut Ctx<'_>, fl: Fl, body: Option<&PNode>) -> CRes<Option<Box<WqNode>>> {
    match body {
        None => Ok(None),
        Some(b) if b.ty == nt::STATEMENTS_NODE => visit_statements_opt(ctx, fl, Some(b)),
        Some(b) => Ok(Some(visit(ctx, fl, b)?)),
    }
}

/// `visit_block_parameters_node` — parameters + shadowed locals.
fn visit_block_parameters(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Vec<Ch>> {
    let mut out = match node.opt_node(ids::block_parameters_node::PARAMETERS) {
        Some(p) => visit_parameters_list(ctx, fl, p)?,
        None => vec![],
    };
    out.extend(visit_all(ctx, fl, node.list(ids::block_parameters_node::LOCALS))?);
    Ok(out)
}

/// `visit_parameters_node`.
fn visit_parameters_list(ctx: &mut Ctx<'_>, fl: Fl, node: &PNode) -> CRes<Vec<Ch>> {
    if node.ty != nt::PARAMETERS_NODE {
        return decline("expected ParametersNode");
    }
    let mut params: Vec<Ch> = Vec::new();

    for required in node.list(ids::parameters_node::REQUIREDS) {
        if required.ty == nt::REQUIRED_PARAMETER_NODE {
            params.push(Ch::N(visit(ctx, fl, required)?));
        } else {
            params.push(Ch::N(visit(ctx, fl.destructure(), required)?));
        }
    }
    for optional in node.list(ids::parameters_node::OPTIONALS) {
        params.push(Ch::N(visit(ctx, fl, optional)?));
    }
    if let Some(rest) = node.opt_node(ids::parameters_node::REST)
        && rest.ty != nt::IMPLICIT_REST_NODE
    {
        params.push(Ch::N(visit(ctx, fl, rest)?));
    }
    for post in node.list(ids::parameters_node::POSTS) {
        if post.ty == nt::REQUIRED_PARAMETER_NODE {
            params.push(Ch::N(visit(ctx, fl, post)?));
        } else {
            params.push(Ch::N(visit(ctx, fl.destructure(), post)?));
        }
    }
    for kw in node.list(ids::parameters_node::KEYWORDS) {
        params.push(Ch::N(visit(ctx, fl, kw)?));
    }
    if let Some(kr) = node.opt_node(ids::parameters_node::KEYWORD_REST) {
        params.push(Ch::N(visit(ctx, fl, kr)?));
    }
    if let Some(block) = node.opt_node(ids::parameters_node::BLOCK) {
        params.push(Ch::N(visit(ctx, fl, block)?));
    }
    Ok(params)
}

/// `visit_block(call, block)`.
fn visit_block(ctx: &mut Ctx<'_>, fl: Fl, call: Box<WqNode>, block: Option<&PNode>) -> CRes<Box<WqNode>> {
    let Some(block) = block else { return Ok(call) };
    if block.ty != nt::BLOCK_NODE {
        return decline("visit_block non-block");
    }
    let parameters = block.opt_node(ids::block_node::PARAMETERS);
    let begin_t = token(ctx, block.bloc(ids::block_node::OPENING_LOC).ok_or(Decline("block open"))?);
    let end_t = token(ctx, block.bloc(ids::block_node::CLOSING_LOC).ok_or(Decline("block close"))?);

    let args = match parameters {
        None => ctx.b_args_none()?,
        Some(p) if p.ty == nt::NUMBERED_PARAMETERS_NODE || p.ty == nt::IT_PARAMETERS_NODE => {
            visit(ctx, fl, p)?
        }
        Some(p) => {
            if p.ty != nt::BLOCK_PARAMETERS_NODE {
                return decline("block parameters type");
            }
            let popen = otoken(ctx, p.opt_bloc(ids::block_parameters_node::OPENING_LOC));
            let inner_params = p.opt_node(ids::block_parameters_node::PARAMETERS);
            let items = if procarg0(inner_params) {
                let inner = inner_params.unwrap();
                let parameter = &inner.list(ids::parameters_node::REQUIREDS)[0];
                let visited = if parameter.ty == nt::REQUIRED_PARAMETER_NODE {
                    visit(ctx, fl, parameter)?
                } else {
                    visit(ctx, fl.destructure(), parameter)?
                };
                let mut items = vec![Ch::N(ctx.b_procarg0(visited))];
                items.extend(visit_all(ctx, fl, p.list(ids::block_parameters_node::LOCALS))?);
                items
            } else {
                visit_block_parameters(ctx, fl, p)?
            };
            let pclose = otoken(ctx, p.opt_bloc(ids::block_parameters_node::CLOSING_LOC));
            ctx.b_args(popen, items, pclose, false)?
        }
    };

    let body = visit_body_generic(ctx, fl, block.opt_node(ids::block_node::BODY))?;
    ctx.b_block(call, begin_t, args, body, end_t)
}

// ---------------------------------------------------------------------------
// prism error/warning → parser diagnostic rows (translation/parser.rb)
// ---------------------------------------------------------------------------

fn diag_type(d: &PDiag) -> &'static str {
    crate::prism_node_specs::DIAGNOSTIC_TYPES.get(d.ty as usize).copied().unwrap_or("")
}

pub(crate) fn error_diagnostic(ctx: &mut Ctx<'_>, e: &PDiag) -> CRes<DiagRow> {
    let ty = diag_type(e);
    let mut bloc = (e.start, e.end);
    let mut args: Vec<(&'static str, ArgVal)> = vec![];
    let slice_string = |ctx: &Ctx<'_>, l: (u32, u32)| String::from_utf8_lossy(ctx.slice(l)).into_owned();

    let reason: &'static str = match ty {
        "argument_block_multi" => "block_and_blockarg",
        "argument_formal_constant" => "argument_const",
        "argument_formal_class" => "argument_cvar",
        "argument_formal_global" => "argument_gvar",
        "argument_formal_ivar" => "argument_ivar",
        "argument_no_forwarding_amp" => "no_anonymous_blockarg",
        "argument_no_forwarding_star" => "no_anonymous_restarg",
        "argument_no_forwarding_star_star" => "no_anonymous_kwrestarg",
        "begin_lonely_else" => {
            bloc = (e.start, e.start + 4);
            "useless_else"
        }
        "class_name" | "module_name" => "module_name_const",
        "class_in_method" => "class_in_def",
        "def_endless_setter" => "endless_setter",
        "embdoc_term" => "embedded_document",
        "incomplete_variable_class" | "incomplete_variable_class_3_3" => {
            bloc = (e.start, e.end + 1);
            args.push(("name", ArgVal::Str(slice_string(ctx, bloc))));
            "cvar_name"
        }
        "incomplete_variable_instance" | "incomplete_variable_instance_3_3" => {
            bloc = (e.start, e.end + 1);
            args.push(("name", ArgVal::Str(slice_string(ctx, bloc))));
            "ivar_name"
        }
        "invalid_variable_global" | "invalid_variable_global_3_3" => {
            args.push(("name", ArgVal::Str(slice_string(ctx, bloc))));
            "gvar_name"
        }
        "module_in_method" => "module_in_def",
        "numbered_parameter_ordinary" => "ordinary_param_defined",
        "numbered_parameter_outer_scope" => "numparam_used_in_outer_scope",
        "parameter_circular" => {
            args.push(("var_name", ArgVal::Str(slice_string(ctx, bloc))));
            "circular_argument_reference"
        }
        "parameter_name_repeat" => "duplicate_argument",
        "parameter_numbered_reserved" => {
            args.push(("name", ArgVal::Str(slice_string(ctx, bloc))));
            "reserved_for_numparam"
        }
        "regexp_unknown_options" => {
            let s = slice_string(ctx, bloc);
            args.push(("options", ArgVal::Str(s.get(1..).unwrap_or("").to_string())));
            "regexp_options"
        }
        "singleton_for_literals" => "singleton_literal",
        "string_literal_eof" => "string_eof",
        "unexpected_token_ignore" => {
            args.push(("token", ArgVal::Str(slice_string(ctx, bloc))));
            "unexpected_token"
        }
        "write_target_in_method" => "dynamic_const",
        _ => {
            return Ok(DiagRow {
                prism: true,
                level: "error",
                reason: ty.to_string(),
                message: Some(String::from_utf8_lossy(&e.message).into_owned()),
                args: vec![],
                loc: ctx.r(bloc),
                highlights: vec![],
            });
        }
    };
    Ok(DiagRow {
        prism: false,
        level: "error",
        reason: reason.to_string(),
        message: None,
        args,
        loc: ctx.r(bloc),
        highlights: vec![],
    })
}

pub(crate) fn warning_diagnostic(ctx: &mut Ctx<'_>, w: &PDiag) -> CRes<Option<DiagRow>> {
    let ty = diag_type(w);
    let bloc = (w.start, w.end);
    let mut args: Vec<(&'static str, ArgVal)> = vec![];

    let reason: &'static str = match ty {
        "ambiguous_first_argument_plus" => {
            args.push(("prefix", ArgVal::Str("+".to_string())));
            "ambiguous_prefix"
        }
        "ambiguous_first_argument_minus" => {
            args.push(("prefix", ArgVal::Str("-".to_string())));
            "ambiguous_prefix"
        }
        "ambiguous_prefix_ampersand" => {
            args.push(("prefix", ArgVal::Str("&".to_string())));
            "ambiguous_prefix"
        }
        "ambiguous_prefix_star" => {
            args.push(("prefix", ArgVal::Str("*".to_string())));
            "ambiguous_prefix"
        }
        "ambiguous_prefix_star_star" => {
            args.push(("prefix", ArgVal::Str("**".to_string())));
            "ambiguous_prefix"
        }
        "ambiguous_slash" => "ambiguous_regexp",
        "dot_dot_dot_eol" => "triple_dot_at_eol",
        "duplicated_hash_key" => return Ok(None), // parser does this on its own
        _ => {
            return Ok(Some(DiagRow {
                prism: true,
                level: "warning",
                reason: ty.to_string(),
                message: Some(String::from_utf8_lossy(&w.message).into_owned()),
                args: vec![],
                loc: ctx.r(bloc),
                highlights: vec![],
            }));
        }
    };
    Ok(Some(DiagRow {
        prism: false,
        level: "warning",
        reason: reason.to_string(),
        message: None,
        args,
        loc: ctx.r(bloc),
        highlights: vec![],
    }))
}
