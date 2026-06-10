//! Track C-1b: the prism AST → Conditional-Action-IR compiler for
//! rouge rule blocks.
//!
//! Given a rule proc's `source_location`, this parses the lexer FILE
//! with prism, locates the smallest block containing that line, and
//! translates the block body into carmine's IR (tuple-array JSON, see
//! `carmine::ir`). Every AST shape is whitelisted; anything outside
//! the subset returns `None` and the rule stays a `callback` (the
//! session protocol) — the decline boundary is THIS compiler, which is
//! what makes the mechanism sound across all lexers instead of
//! per-lexer handwork.
//!
//! Token constants are emitted as CONSTANT PATHS (`"Str::Heredoc"`),
//! not qualnames — rouge token constants alias (`Str` IS
//! `Literal::String`), so the shim resolves paths through the live
//! `Rouge::Token::Tokens` tree before handing the table to the engine.
//!
//! Recognized vocabulary (the census of rouge rule procs):
//!   token T            token T, EXPR        groups T1, T2, …
//!   push / push :s     pop! / pop!(n)       goto :s
//!   @ivar = EXPR       @ivar << [EXPR, …]
//!   STMT if COND / unless COND / if … end / unless … end
//!   EXPR:  "lit"  "a#{m[i]}b"  m[i]  true/false  [..].include?(m[i])
//!   COND:  @ivar  state?(:s)  m[i] == "lit"  [..].include?(m[i])  !c

#![cfg(feature = "_rouge_native")]

use ruby_prism::{BlockNode, CallNode, Node, Visit};
use serde_json::{Value as J, json};

use crate::ast::cid_to_string;

/// Compile the rule block starting at `line` (1-based) of `source`.
/// Returns the IR ops as a JSON array string, or `None` (decline).
pub(crate) fn compile_block_at(source: &str, line: u32) -> Option<String> {
    // Line-start offsets for offset→line mapping.
    let mut line_starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    if line == 0 || (line as usize) > line_starts.len() {
        return None;
    }

    let result = ruby_prism::parse(source.as_bytes());
    // Proc#source_location reports the block's FIRST OP line: for a
    // single-line brace block that's the `{` line itself; for the
    // rouge `do …` style the op sits on the line right after `do`.
    // Select the smallest block that STARTS on the target line or
    // the one above — never "any block containing the offset", which
    // mis-picks an enclosing state block (or an unrelated inner
    // brace block sharing the op line).
    let mut finder = BlockFinder {
        line_starts,
        line: line as usize,
        best: None,
    };
    finder.visit(&result.node());
    let (start, end) = finder.best?;

    // Re-find the chosen block node (we can't store the node across
    // the visitor borrow generically, so we stash its span and fetch
    // it again).
    let mut fetch = BlockFetch {
        span: (start, end),
        ops: None,
    };
    fetch.visit(&result.node());
    fetch.ops.map(|ops| J::Array(ops).to_string())
}

/// Pass 1: smallest BlockNode starting on the target line (or the
/// line above — the `do` of a do-block whose first op is the target).
struct BlockFinder {
    line_starts: Vec<usize>,
    /// 1-based target line.
    line: usize,
    best: Option<(usize, usize)>,
}

impl BlockFinder {
    fn line_of(&self, offset: usize) -> usize {
        // partition_point gives the count of line starts <= offset,
        // which IS the 1-based line number.
        self.line_starts.partition_point(|&s| s <= offset)
    }
}

impl<'pr> Visit<'pr> for BlockFinder {
    fn visit_block_node(&mut self, node: &BlockNode<'pr>) {
        let loc = node.location();
        let (s, e) = (loc.start_offset(), loc.end_offset());
        let start_line = self.line_of(s);
        if start_line == self.line || start_line + 1 == self.line {
            let better = match self.best {
                Some((bs, be)) => (e - s) < (be - bs),
                None => true,
            };
            if better {
                self.best = Some((s, e));
            }
        }
        ruby_prism::visit_block_node(self, node);
    }
}

/// Pass 2: translate the block with that exact span.
struct BlockFetch {
    span: (usize, usize),
    ops: Option<Vec<J>>,
}

impl<'pr> Visit<'pr> for BlockFetch {
    fn visit_block_node(&mut self, node: &BlockNode<'pr>) {
        let loc = node.location();
        if (loc.start_offset(), loc.end_offset()) == self.span && self.ops.is_none() {
            self.ops = translate_block(node);
            return; // don't descend into our own body
        }
        ruby_prism::visit_block_node(self, node);
    }
}

fn translate_block(block: &BlockNode<'_>) -> Option<Vec<J>> {
    // Parameters: none, or exactly `|m|`.
    if let Some(params) = block.parameters() {
        let bp = params.as_block_parameters_node()?;
        if let Some(inner) = bp.parameters() {
            let req: Vec<_> = inner.requireds().iter().collect();
            if !(req.len() == 1
                && inner.optionals().iter().count() == 0
                && inner.rest().is_none()
                && inner.keywords().iter().count() == 0
                && inner.block().is_none())
            {
                return None;
            }
            let p = req[0].as_required_parameter_node()?;
            if cid_to_string(p.name()) != "m" {
                return None;
            }
        }
    }
    let body = block.body()?;
    let stmts = body.as_statements_node()?;
    let mut ops = Vec::new();
    for stmt in stmts.body().iter() {
        if is_debug_print_stmt(&stmt) {
            continue;
        }
        ops.push(translate_stmt(&stmt)?);
    }
    Some(ops)
}

/// `puts "…" if @debug` (and bare puts/p/print of literals/groups) —
/// stdout diagnostics that don't touch the token stream; elided, like
/// every existing native path.
fn is_debug_print_stmt(node: &Node<'_>) -> bool {
    fn is_print_call(node: &Node<'_>) -> bool {
        let Some(call) = node.as_call_node() else {
            return false;
        };
        call.receiver().is_none()
            && matches!(
                String::from_utf8_lossy(call.name().as_slice()).as_ref(),
                "puts" | "p" | "print" | "pp"
            )
    }
    if let Some(ifn) = node.as_if_node() {
        // modifier-if guarded by @debug with a print-only body
        if ifn.predicate().as_instance_variable_read_node().is_some()
            && let Some(stmts) = ifn.statements()
        {
            let body: Vec<_> = stmts.body().iter().collect();
            return !body.is_empty()
                && body.iter().all(is_print_call)
                && ifn.subsequent().is_none();
        }
        return false;
    }
    false
}

fn translate_stmt(node: &Node<'_>) -> Option<J> {
    if let Some(call) = node.as_call_node() {
        return translate_call(&call);
    }
    if let Some(ifn) = node.as_if_node() {
        // No elsif chains; else only as a plain ElseNode.
        let cond = translate_cond(&ifn.predicate())?;
        let then_ops = translate_stmts_opt(ifn.statements())?;
        let else_ops = match ifn.subsequent() {
            None => Vec::new(),
            Some(sub) => {
                let els = sub.as_else_node()?;
                match els.statements() {
                    None => Vec::new(),
                    Some(s) => translate_stmt_list(&s)?,
                }
            }
        };
        return Some(json!(["if", cond, then_ops, else_ops]));
    }
    if let Some(un) = node.as_unless_node() {
        let cond = translate_cond(&un.predicate())?;
        let then_ops = translate_stmts_opt(un.statements())?;
        let else_ops = match un.else_clause() {
            None => Vec::new(),
            Some(els) => match els.statements() {
                None => Vec::new(),
                Some(s) => translate_stmt_list(&s)?,
            },
        };
        return Some(json!(["if", ["not", cond], then_ops, else_ops]));
    }
    if let Some(iw) = node.as_instance_variable_write_node() {
        let name = cid_to_string(iw.name());
        let name = name.strip_prefix('@')?;
        let expr = translate_expr(&iw.value())?;
        return Some(json!(["iset", name, expr]));
    }
    None
}

fn translate_stmts_opt(stmts: Option<ruby_prism::StatementsNode<'_>>) -> Option<Vec<J>> {
    match stmts {
        None => Some(Vec::new()),
        Some(s) => translate_stmt_list(&s),
    }
}

fn translate_stmt_list(stmts: &ruby_prism::StatementsNode<'_>) -> Option<Vec<J>> {
    let mut out = Vec::new();
    for stmt in stmts.body().iter() {
        if is_debug_print_stmt(&stmt) {
            continue;
        }
        out.push(translate_stmt(&stmt)?);
    }
    Some(out)
}

fn translate_call(call: &CallNode<'_>) -> Option<J> {
    let name = String::from_utf8_lossy(call.name().as_slice()).into_owned();

    if let Some(recv) = call.receiver() {
        // `@ivar << [a, b, …]`
        if name == "<<" {
            let ivar = recv.as_instance_variable_read_node()?;
            let ivar_name = cid_to_string(ivar.name());
            let ivar_name = ivar_name.strip_prefix('@')?;
            let args = call_args(call)?;
            if args.len() != 1 {
                return None;
            }
            let arr = args[0].as_array_node()?;
            let mut tuple = Vec::new();
            for el in arr.elements().iter() {
                tuple.push(translate_expr(&el)?);
            }
            return Some(json!(["lpush", ivar_name, tuple]));
        }
        return None;
    }
    if call.block().is_some() {
        // `push do … end` (dynamic state synthesis) and friends.
        return None;
    }

    match name.as_str() {
        "token" => {
            let args = call_args(call)?;
            if args.is_empty() || args.len() > 2 {
                return None;
            }
            let tok = const_path(&args[0])?;
            let value = match args.get(1) {
                None => J::Null,
                Some(v) => translate_expr(v)?,
            };
            Some(json!(["token", tok, value]))
        }
        "groups" => {
            let args = call_args(call)?;
            if args.is_empty() {
                return None;
            }
            let toks = args.iter().map(const_path).collect::<Option<Vec<_>>>()?;
            Some(json!(["groups", toks]))
        }
        "push" => {
            let args = call_args(call)?;
            match args.len() {
                0 => Some(json!(["push", J::Null])),
                1 => Some(json!(["push", symbol_name(&args[0])?])),
                _ => None,
            }
        }
        "pop!" => {
            let args = call_args(call)?;
            match args.len() {
                0 => Some(json!(["pop", 1])),
                1 => Some(json!(["pop", int_value(&args[0])?])),
                _ => None,
            }
        }
        "goto" => {
            let args = call_args(call)?;
            if args.len() != 1 {
                return None;
            }
            Some(json!(["goto", symbol_name(&args[0])?]))
        }
        _ => None,
    }
}

fn translate_cond(node: &Node<'_>) -> Option<J> {
    if let Some(iv) = node.as_instance_variable_read_node() {
        let name = cid_to_string(iv.name());
        return Some(json!(["ivar", name.strip_prefix('@')?]));
    }
    if let Some(call) = node.as_call_node() {
        let name = String::from_utf8_lossy(call.name().as_slice()).into_owned();
        match (call.receiver(), name.as_str()) {
            (None, "state?") => {
                let args = call_args(&call)?;
                if args.len() != 1 {
                    return None;
                }
                return Some(json!(["instate", symbol_name(&args[0])?]));
            }
            (Some(recv), "include?") => {
                let lits = string_array(&recv)?;
                let args = call_args(&call)?;
                if args.len() != 1 {
                    return None;
                }
                let g = group_index(&args[0])?;
                return Some(json!(["gin", g, lits]));
            }
            (Some(recv), "==") => {
                let g = group_index(&recv)?;
                let args = call_args(&call)?;
                if args.len() != 1 {
                    return None;
                }
                let lit = string_lit(&args[0])?;
                return Some(json!(["geq", g, lit]));
            }
            (Some(recv), "!") => {
                let inner = translate_cond(&recv)?;
                return Some(json!(["not", inner]));
            }
            _ => return None,
        }
    }
    None
}

fn translate_expr(node: &Node<'_>) -> Option<J> {
    if let Some(s) = string_lit(node) {
        return Some(json!(["lit", s]));
    }
    if let Some(g) = group_index(node) {
        return Some(json!(["g", g]));
    }
    if node.as_true_node().is_some() {
        return Some(json!(["bool", true]));
    }
    if node.as_false_node().is_some() {
        return Some(json!(["bool", false]));
    }
    if let Some(interp) = node.as_interpolated_string_node() {
        let mut parts = vec![J::String("cat".into())];
        for part in interp.parts().iter() {
            if let Some(s) = part.as_string_node() {
                parts.push(json!([
                    "lit",
                    String::from_utf8_lossy(s.unescaped()).into_owned()
                ]));
            } else if let Some(emb) = part.as_embedded_statements_node() {
                let stmts = emb.statements()?;
                let inner: Vec<_> = stmts.body().iter().collect();
                if inner.len() != 1 {
                    return None;
                }
                let g = group_index(&inner[0])?;
                parts.push(json!(["g", g]));
            } else {
                return None;
            }
        }
        return Some(J::Array(parts));
    }
    if let Some(call) = node.as_call_node() {
        let name = String::from_utf8_lossy(call.name().as_slice()).into_owned();
        if name == "include?"
            && let Some(recv) = call.receiver()
        {
            let lits = string_array(&recv)?;
            let args = call_args(&call)?;
            if args.len() == 1 {
                let g = group_index(&args[0])?;
                return Some(json!(["gin", g, lits]));
            }
        }
        return None;
    }
    None
}

// ---- leaf helpers ---------------------------------------------------------

fn call_args<'pr>(call: &CallNode<'pr>) -> Option<Vec<Node<'pr>>> {
    match call.arguments() {
        None => Some(Vec::new()),
        Some(args) => Some(args.arguments().iter().collect()),
    }
}

/// `m[i]` → capture index (the block param is checked to be `m`).
fn group_index(node: &Node<'_>) -> Option<u64> {
    let call = node.as_call_node()?;
    if String::from_utf8_lossy(call.name().as_slice()) != "[]" {
        return None;
    }
    let recv = call.receiver()?;
    let lv = recv.as_local_variable_read_node()?;
    if cid_to_string(lv.name()) != "m" {
        return None;
    }
    let args = call_args(&call)?;
    if args.len() != 1 {
        return None;
    }
    int_value(&args[0])
}

fn int_value(node: &Node<'_>) -> Option<u64> {
    let n = node.as_integer_node()?;
    let value = n.value();
    let (negative, digits) = value.to_u32_digits();
    if negative || digits.len() > 1 {
        return None;
    }
    Some(*digits.first().unwrap_or(&0) as u64)
}

fn string_lit(node: &Node<'_>) -> Option<String> {
    let s = node.as_string_node()?;
    Some(String::from_utf8_lossy(s.unescaped()).into_owned())
}

fn string_array(node: &Node<'_>) -> Option<Vec<String>> {
    let arr = node.as_array_node()?;
    let mut out = Vec::new();
    for el in arr.elements().iter() {
        out.push(string_lit(&el)?);
    }
    Some(out)
}

fn symbol_name(node: &Node<'_>) -> Option<String> {
    let sym = node.as_symbol_node()?;
    Some(String::from_utf8_lossy(sym.unescaped()).into_owned())
}

/// `Str::Heredoc` → `"Str::Heredoc"` (constant PATH; the shim resolves
/// it to a qualname through the live rouge token tree).
fn const_path(node: &Node<'_>) -> Option<String> {
    if let Some(c) = node.as_constant_read_node() {
        return Some(cid_to_string(c.name()));
    }
    if let Some(p) = node.as_constant_path_node() {
        let name = cid_to_string(p.name()?);
        let parent = p.parent()?;
        let prefix = const_path(&parent)?;
        return Some(format!("{prefix}::{name}"));
    }
    None
}
