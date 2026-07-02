//! Materialize the translated whitequark tree as the exact Ruby object graph
//! the interpreted path builds: `RuboCop::AST::*` node instances (class per
//! `RuboCop::AST::Builder::NODE_MAP`), `Parser::Source::Map*` maps with the
//! gem's ivar layout/order, `Parser::Source::Range`s tied to the caller's
//! buffer, `Parser::Source::Comment`s, parser-gem token triples, and
//! diagnostic rows for the Ruby hook to replay.

use std::cell::Cell;
use std::rc::Rc;

use crate::heap::{HashObj, HeapObj};
use crate::intern::SymId;
use crate::value::{Class, Instance, IvarTable, Value};
use crate::vm::Vm;

use super::builder::{Ch, Map, MK, WqNode};
use super::lexer::{OutTok, TokVal};
use super::{ArgVal, CRes, Ctx, Decline, DiagRow, PParse, R};

/// `RuboCop::AST::Builder::NODE_MAP` (rubocop-ast 1.49.1) — node type →
/// unqualified class name under RuboCop::AST. Everything else is Node.
fn node_class_name(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "and" => "AndNode",
        "and_asgn" => "AndAsgnNode",
        "alias" => "AliasNode",
        "arg" | "blockarg" | "forward_arg" | "kwarg" | "kwoptarg" | "kwrestarg" | "optarg"
        | "restarg" | "shadowarg" => "ArgNode",
        "args" => "ArgsNode",
        "array" => "ArrayNode",
        "lvasgn" | "ivasgn" | "cvasgn" | "gvasgn" => "AsgnNode",
        "block" | "numblock" | "itblock" => "BlockNode",
        "break" => "BreakNode",
        "case_match" => "CaseMatchNode",
        "casgn" => "CasgnNode",
        "case" => "CaseNode",
        "class" => "ClassNode",
        "complex" => "ComplexNode",
        "const" => "ConstNode",
        "def" | "defs" => "DefNode",
        "defined?" => "DefinedNode",
        "dstr" => "DstrNode",
        "ensure" => "EnsureNode",
        "for" => "ForNode",
        "forward_args" => "ForwardArgsNode",
        "forwarded_kwrestarg" => "KeywordSplatNode",
        "float" => "FloatNode",
        "hash" | "kwargs" => "HashNode",
        "if" => "IfNode",
        "in_pattern" => "InPatternNode",
        "int" => "IntNode",
        "index" => "IndexNode",
        "indexasgn" => "IndexasgnNode",
        "irange" | "erange" => "RangeNode",
        "kwbegin" => "KeywordBeginNode",
        "kwsplat" => "KeywordSplatNode",
        "lambda" => "LambdaNode",
        "masgn" => "MasgnNode",
        "mlhs" => "MlhsNode",
        "module" => "ModuleNode",
        "next" => "NextNode",
        "op_asgn" => "OpAsgnNode",
        "or_asgn" => "OrAsgnNode",
        "or" => "OrNode",
        "pair" => "PairNode",
        "procarg0" => "Procarg0Node",
        "rational" => "RationalNode",
        "regexp" => "RegexpNode",
        "rescue" => "RescueNode",
        "resbody" => "ResbodyNode",
        "return" => "ReturnNode",
        "csend" => "CsendNode",
        "send" => "SendNode",
        "str" | "xstr" => "StrNode",
        "sclass" => "SelfClassNode",
        "super" | "zsuper" => "SuperNode",
        "sym" => "SymbolNode",
        "until" | "until_post" => "UntilNode",
        "lvar" | "ivar" | "cvar" | "gvar" => "VarNode",
        "when" => "WhenNode",
        "while" | "while_post" => "WhileNode",
        "yield" => "YieldNode",
        _ => return None,
    })
}

struct M<'vm> {
    vm: &'vm mut Vm,
    buffer: Value,
    /// (begin, end) char offsets → materialized Range value. Ranges are
    /// value-compared everywhere (never identity), so sharing is safe.
    range_cache: crate::intern::FxHashMap<(u32, u32), Value>,
    range_class: Rc<Class>,
    node_default_class: Rc<Class>,
    // Interned symbols.
    s_source_buffer: SymId,
    s_begin_pos: SymId,
    s_end_pos: SymId,
    s_mutable_attributes: SymId,
    s_type: SymId,
    s_children: SymId,
    s_location: SymId,
    s_hash: SymId,
    s_node: SymId,
    s_expression: SymId,
    s_parent: SymId,
}

fn lookup_class(vm: &mut Vm, name: &str) -> CRes<Rc<Class>> {
    let id = vm.interner.intern(name);
    vm.classes.get(&id).cloned().ok_or(Decline("class missing"))
}

impl<'vm> M<'vm> {
    fn alloc_instance(&mut self, class: Rc<Class>, pairs: Vec<(SymId, Value)>, frozen: bool) -> CRes<Value> {
        let ivars = IvarTable::from_pairs(&class, pairs);
        self.vm.check_alloc().map_err(|_| Decline("alloc cap"))?;
        let id = self.vm.heap.alloc(HeapObj::Instance(Instance {
            class,
            ivars,
            singleton_class: None,
            frozen: Cell::new(frozen),
        }));
        Ok(Value::Object(id))
    }

    fn alloc_array(&mut self, elems: Vec<Value>, frozen: bool) -> CRes<Value> {
        self.vm.check_alloc().map_err(|_| Decline("alloc cap"))?;
        let id = self.vm.heap.alloc(HeapObj::Array(elems.into()));
        if frozen && let HeapObj::Array(a) = self.vm.heap.get(id) {
            a.frozen.set(true);
        }
        Ok(Value::Array(id))
    }

    fn range(&mut self, r: R) -> CRes<Value> {
        if let Some(v) = self.range_cache.get(&(r.b, r.e)) {
            return Ok(v.clone());
        }
        let mut ivars: Vec<(SymId, Value)> = Vec::new();
        ivars.push((self.s_source_buffer, self.buffer.clone()));
        ivars.push((self.s_begin_pos, Value::Int(r.b as i64)));
        ivars.push((self.s_end_pos, Value::Int(r.e as i64)));
        let class = self.range_class.clone();
        let v = self.alloc_instance(class, ivars, true)?;
        self.range_cache.insert((r.b, r.e), v.clone());
        Ok(v)
    }

    fn orange(&mut self, r: Option<R>) -> CRes<Value> {
        match r {
            Some(r) => self.range(r),
            None => Ok(Value::Nil),
        }
    }

    /// Build the map instance (ivars in the gem's construction order:
    /// subclass fields, @expression, @node placeholder, then @operator when
    /// `with_operator` applied). Returns the value; the caller assigns @node
    /// and freezes once the owning node exists.
    fn map_obj(&mut self, map: &Map) -> CRes<Value> {
        let i = |m: &mut M<'vm>, name: &str| m.vm.interner.intern(name);
        let mut ivars: Vec<(SymId, Value)> = Vec::new();
        let mut late_operator: Option<(SymId, Value)> = None;

        let class_name: &'static str = match &map.k {
            MK::Bare => "Parser::Source::Map",
            MK::Collection { b, e } => {
                let (b, e) = (*b, *e);
                let bv = self.orange(b)?;
                let ev = self.orange(e)?;
                let sb = i(self, "@begin");
                let se = i(self, "@end");
                ivars.push((sb, bv));
                ivars.push((se, ev));
                "Parser::Source::Map::Collection"
            }
            MK::Constant { dc, name, op } => {
                let (dc, name, op) = (*dc, *name, *op);
                let dcv = self.orange(dc)?;
                let nv = self.range(name)?;
                let s1 = i(self, "@double_colon");
                let s2 = i(self, "@name");
                ivars.push((s1, dcv));
                ivars.push((s2, nv));
                if let Some(op) = op {
                    let ov = self.range(op)?;
                    late_operator = Some((i(self, "@operator"), ov));
                }
                "Parser::Source::Map::Constant"
            }
            MK::Variable { name, op } => {
                let (name, op) = (*name, *op);
                let nv = self.orange(name)?;
                let s1 = i(self, "@name");
                ivars.push((s1, nv));
                if let Some(op) = op {
                    let ov = self.range(op)?;
                    late_operator = Some((i(self, "@operator"), ov));
                }
                "Parser::Source::Map::Variable"
            }
            MK::Operator { op } => {
                let op = *op;
                let ov = self.orange(op)?;
                let s1 = i(self, "@operator");
                ivars.push((s1, ov));
                "Parser::Source::Map::Operator"
            }
            MK::Send { dot, sel, b, e, op } => {
                let (dot, sel, b, e, op) = (*dot, *sel, *b, *e, *op);
                let dv = self.orange(dot)?;
                let sv = self.orange(sel)?;
                let bv = self.orange(b)?;
                let ev = self.orange(e)?;
                let s1 = i(self, "@dot");
                let s2 = i(self, "@selector");
                let s3 = i(self, "@begin");
                let s4 = i(self, "@end");
                ivars.push((s1, dv));
                ivars.push((s2, sv));
                ivars.push((s3, bv));
                ivars.push((s4, ev));
                if let Some(op) = op {
                    let ov = self.range(op)?;
                    late_operator = Some((i(self, "@operator"), ov));
                }
                "Parser::Source::Map::Send"
            }
            MK::Condition { kw, b, els, e } => {
                let (kw, b, els, e) = (*kw, *b, *els, *e);
                let kv = self.orange(kw)?;
                let bv = self.orange(b)?;
                let lv = self.orange(els)?;
                let ev = self.orange(e)?;
                let s1 = i(self, "@keyword");
                let s2 = i(self, "@begin");
                let s3 = i(self, "@else");
                let s4 = i(self, "@end");
                ivars.push((s1, kv));
                ivars.push((s2, bv));
                ivars.push((s3, lv));
                ivars.push((s4, ev));
                "Parser::Source::Map::Condition"
            }
            MK::Keyword { kw, b, e } => {
                let (kw, b, e) = (*kw, *b, *e);
                let kv = self.orange(kw)?;
                let bv = self.orange(b)?;
                let ev = self.orange(e)?;
                let s1 = i(self, "@keyword");
                let s2 = i(self, "@begin");
                let s3 = i(self, "@end");
                ivars.push((s1, kv));
                ivars.push((s2, bv));
                ivars.push((s3, ev));
                "Parser::Source::Map::Keyword"
            }
            MK::Ternary { q, c } => {
                let (q, c) = (*q, *c);
                let qv = self.range(q)?;
                let cv = self.range(c)?;
                let s1 = i(self, "@question");
                let s2 = i(self, "@colon");
                ivars.push((s1, qv));
                ivars.push((s2, cv));
                "Parser::Source::Map::Ternary"
            }
            MK::For { kw, inn, b, e } => {
                let (kw, inn, b, e) = (*kw, *inn, *b, *e);
                let kv = self.range(kw)?;
                let iv = self.range(inn)?;
                let bv = self.orange(b)?;
                let ev = self.range(e)?;
                let s1 = i(self, "@keyword");
                let s2 = i(self, "@in");
                let s3 = i(self, "@begin");
                let s4 = i(self, "@end");
                ivars.push((s1, kv));
                ivars.push((s2, iv));
                ivars.push((s3, bv));
                ivars.push((s4, ev));
                "Parser::Source::Map::For"
            }
            MK::Definition { kw, op, name, e } => {
                let (kw, op, name, e) = (*kw, *op, *name, *e);
                let kv = self.range(kw)?;
                let ov = self.orange(op)?;
                let nv = self.orange(name)?;
                let ev = self.orange(e)?;
                let s1 = i(self, "@keyword");
                let s2 = i(self, "@operator");
                let s3 = i(self, "@name");
                let s4 = i(self, "@end");
                ivars.push((s1, kv));
                ivars.push((s2, ov));
                ivars.push((s3, nv));
                ivars.push((s4, ev));
                "Parser::Source::Map::Definition"
            }
            MK::MethodDefinition { kw, op, name, e, assign } => {
                let (kw, op, name, e, assign) = (*kw, *op, *name, *e, *assign);
                let kv = self.range(kw)?;
                let ov = self.orange(op)?;
                let nv = self.range(name)?;
                let ev = self.orange(e)?;
                let av = self.orange(assign)?;
                let s1 = i(self, "@keyword");
                let s2 = i(self, "@operator");
                let s3 = i(self, "@name");
                let s4 = i(self, "@end");
                let s5 = i(self, "@assignment");
                ivars.push((s1, kv));
                ivars.push((s2, ov));
                ivars.push((s3, nv));
                ivars.push((s4, ev));
                ivars.push((s5, av));
                "Parser::Source::Map::MethodDefinition"
            }
            MK::RescueBody { kw, assoc, b } => {
                let (kw, assoc, b) = (*kw, *assoc, *b);
                let kv = self.range(kw)?;
                let av = self.orange(assoc)?;
                let bv = self.orange(b)?;
                let s1 = i(self, "@keyword");
                let s2 = i(self, "@assoc");
                let s3 = i(self, "@begin");
                ivars.push((s1, kv));
                ivars.push((s2, av));
                ivars.push((s3, bv));
                "Parser::Source::Map::RescueBody"
            }
            MK::Heredoc { body, hd_end } => {
                let (body, hd_end) = (*body, *hd_end);
                let bv = self.range(body)?;
                let ev = self.range(hd_end)?;
                let s1 = i(self, "@heredoc_body");
                let s2 = i(self, "@heredoc_end");
                ivars.push((s1, bv));
                ivars.push((s2, ev));
                "Parser::Source::Map::Heredoc"
            }
        };

        let ev = self.orange(map.expr)?;
        ivars.push((self.s_expression, ev));
        ivars.push((self.s_node, Value::Nil));
        if let Some((sym, v)) = late_operator {
            ivars.push((sym, v));
        }

        let class = lookup_class(self.vm, class_name)?;
        self.alloc_instance(class, ivars, false)
    }

    fn node_class(&mut self, ty: &str) -> CRes<Rc<Class>> {
        match node_class_name(ty) {
            Some(name) => lookup_class(self.vm, &format!("RuboCop::AST::{}", name)),
            None => Ok(self.node_default_class.clone()),
        }
    }

    /// Materialize one node (bottom-up).
    fn node(&mut self, node: WqNode) -> CRes<Value> {
        let WqNode { ty, children, map } = node;

        let mut child_vals: Vec<Value> = Vec::with_capacity(children.len());
        let mut child_nodes: Vec<Value> = Vec::new();
        for ch in children {
            match ch {
                Ch::N(child) => {
                    let v = self.node(*child)?;
                    child_nodes.push(v.clone());
                    child_vals.push(v);
                }
                Ch::V(v) => child_vals.push(v),
            }
        }

        let type_sym = self.vm.interner.intern(ty);
        let children_val = self.alloc_array(child_vals, true)?;
        let class = self.node_class(ty)?;

        // @hash = [@type, @children, self.class].hash — computed with the
        // VM's own hashing so it matches what the interpreted initialize
        // would have produced for this exact object graph.
        let hash_val = {
            let triple = self.alloc_array(
                vec![Value::Sym(type_sym), children_val.clone(), Value::Class(class.clone())],
                false,
            )?;
            crate::vm::dispatch::object_hash(&triple, &self.vm.heap)
        };

        let map_val = match map {
            Some(m) => Some(self.map_obj(&m)?),
            None => None,
        };

        self.vm.check_alloc().map_err(|_| Decline("alloc cap"))?;
        let mattrs = Value::Hash(self.vm.heap.alloc(HeapObj::Hash(HashObj::with_pairs(vec![]))));

        let mut ivars: Vec<(SymId, Value)> = Vec::new();
        ivars.push((self.s_mutable_attributes, mattrs));
        ivars.push((self.s_type, Value::Sym(type_sym)));
        ivars.push((self.s_children, children_val));
        if let Some(mv) = &map_val {
            ivars.push((self.s_location, mv.clone()));
        }
        ivars.push((self.s_hash, Value::Int(hash_val)));

        let node_val = self.alloc_instance(class, ivars, true)?;

        // location.node = self (freezes the map).
        if let Some(Value::Object(map_id)) = map_val {
            let inst = self.vm.heap.instance_mut(map_id);
            inst.ivar_set(self.s_node, node_val.clone());
            inst.frozen.set(true);
        }

        // each_child_node { |child| child.parent = self }.
        for child in child_nodes {
            let Value::Object(cid) = child else { continue };
            let mattr = {
                let inst = self.vm.heap.instance_mut(cid);
                inst.ivar_get(self.s_mutable_attributes).cloned()
            };
            if let Some(Value::Hash(hid)) = mattr
                && let HeapObj::Hash(h) = self.vm.heap.get_mut(hid)
            {
                // #parent= may be invoked once per (re)construction; only
                // the final tree exists here, so a plain upsert suffices.
                let key = Value::Sym(self.s_parent);
                if let Some(pair) = h.pairs.iter_mut().find(|(k, _)| matches!(k, Value::Sym(s) if *s == self.s_parent)) {
                    pair.1 = node_val.clone();
                } else {
                    h.pairs.push((key, node_val.clone()));
                }
                h.index = None;
                h.user_index = None;
            }
        }

        Ok(node_val)
    }
}

/// Assemble the host-fn result: `[ast, comments, tokens, diags]`.
pub(crate) fn materialize(
    ctx: Ctx<'_>,
    parse: PParse,
    ast: Option<Box<WqNode>>,
    tokens: Vec<OutTok>,
    buffer: Value,
) -> CRes<Value> {
    let Ctx { vm, src, enc, diags, off, .. } = ctx;

    let range_class = lookup_class(vm, "Parser::Source::Range")?;
    let node_default_class = lookup_class(vm, "RuboCop::AST::Node")?;

    let mut m = M {
        s_source_buffer: vm.interner.intern("@source_buffer"),
        s_begin_pos: vm.interner.intern("@begin_pos"),
        s_end_pos: vm.interner.intern("@end_pos"),
        s_mutable_attributes: vm.interner.intern("@mutable_attributes"),
        s_type: vm.interner.intern("@type"),
        s_children: vm.interner.intern("@children"),
        s_location: vm.interner.intern("@location"),
        s_hash: vm.interner.intern("@hash"),
        s_node: vm.interner.intern("@node"),
        s_expression: vm.interner.intern("@expression"),
        s_parent: vm.interner.intern("parent"),
        vm,
        buffer,
        range_cache: crate::intern::FxHashMap::default(),
        range_class,
        node_default_class,
    };

    // AST.
    let ast_val = match ast {
        Some(node) => m.node(*node)?,
        None => Value::Nil,
    };

    // Comments: Parser::Source::Comment.new(range) — @location is a BARE map
    // (never frozen, no @node), @text the frozen source slice.
    let map_class = lookup_class(m.vm, "Parser::Source::Map")?;
    let comment_class = lookup_class(m.vm, "Parser::Source::Comment")?;
    let s_location = m.s_location;
    let s_text = m.vm.interner.intern("@text");
    let mut comments_out = Vec::with_capacity(parse.comments.len());
    for c in &parse.comments {
        let (start, end) = (c.start, c.end);
        // Offsets: byte → char.
        let cr = R { b: off.c(start), e: off.c(end) };
        let range_val = m.range(cr)?;
        let mut map_ivars: Vec<(SymId, Value)> = Vec::new();
        map_ivars.push((m.s_expression, range_val));
        let map_val = m.alloc_instance(map_class.clone(), map_ivars, false)?;
        let text_bytes = src.get(start as usize..end as usize).unwrap_or(&[]).to_vec();
        let text_rs = crate::value::RStr::from_bytes(text_bytes);
        text_rs.encoding.set(enc);
        text_rs.frozen.set(true);
        let mut ivars: Vec<(SymId, Value)> = Vec::new();
        ivars.push((s_location, map_val));
        ivars.push((s_text, Value::Str(Rc::new(text_rs))));
        comments_out.push(m.alloc_instance(comment_class.clone(), ivars, true)?);
    }
    let comments_val = m.alloc_array(comments_out, false)?;

    // Tokens: [[type_sym, [value, range]], ...].
    let mut token_vals = Vec::with_capacity(tokens.len());
    for t in tokens {
        let ty_sym = m.vm.interner.intern(t.ty);
        let val = match t.val {
            TokVal::Bytes(b) => {
                let rs = crate::value::RStr::from_bytes(b);
                rs.encoding.set(enc);
                Value::Str(Rc::new(rs))
            }
            TokVal::BytesF(b) => {
                let rs = crate::value::RStr::from_bytes(b);
                rs.encoding.set(enc);
                rs.frozen.set(true);
                Value::Str(Rc::new(rs))
            }
            TokVal::Nil => Value::Nil,
            TokVal::V(v) => v,
        };
        let range_val = m.range(t.r)?;
        let pair = m.alloc_array(vec![val, range_val], false)?;
        let row = m.alloc_array(vec![Value::Sym(ty_sym), pair], false)?;
        token_vals.push(row);
    }
    let tokens_val = m.alloc_array(token_vals, false)?;

    // Diagnostic rows for the hook:
    // [prism?, level, reason, message, args_flat, begin, end, highlights_flat]
    let mut diag_vals = Vec::with_capacity(diags.len());
    for d in diags {
        let DiagRow { prism, level, reason, message, args, loc, highlights } = d;
        let level_sym = m.vm.interner.intern(level);
        let reason_sym = m.vm.interner.intern(&reason);
        let message_val = match message {
            Some(s) => {
                let rs = crate::value::RStr::from_bytes(s.into_bytes());
                rs.encoding.set(enc);
                Value::Str(Rc::new(rs))
            }
            None => Value::Nil,
        };
        let mut args_flat = Vec::with_capacity(args.len() * 2);
        for (k, v) in args {
            let ks = m.vm.interner.intern(k);
            args_flat.push(Value::Sym(ks));
            match v {
                ArgVal::Str(s) => {
                    let rs = crate::value::RStr::from_bytes(s.into_bytes());
                    rs.encoding.set(enc);
                    args_flat.push(Value::Str(Rc::new(rs)));
                }
                ArgVal::Sym(s) => {
                    let sym = m.vm.interner.intern(&s);
                    args_flat.push(Value::Sym(sym));
                }
            }
        }
        let args_val = m.alloc_array(args_flat, false)?;
        let mut hl_flat = Vec::with_capacity(highlights.len() * 2);
        for h in highlights {
            hl_flat.push(Value::Int(h.b as i64));
            hl_flat.push(Value::Int(h.e as i64));
        }
        let hl_val = m.alloc_array(hl_flat, false)?;
        let row = m.alloc_array(
            vec![
                Value::Bool(prism),
                Value::Sym(level_sym),
                Value::Sym(reason_sym),
                message_val,
                args_val,
                Value::Int(loc.b as i64),
                Value::Int(loc.e as i64),
                hl_val,
            ],
            false,
        )?;
        diag_vals.push(row);
    }
    let diags_val = m.alloc_array(diag_vals, false)?;

    m.alloc_array(vec![ast_val, comments_val, tokens_val, diags_val], false)
}
