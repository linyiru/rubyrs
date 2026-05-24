use ruby_prism::Node;

use crate::error::Span;

// ---------- IR ----------

#[derive(Debug, Clone)]
pub(crate) struct Spanned<T> {
    pub(crate) span: Span,
    pub(crate) node: T,
}

impl<T> Spanned<T> {
    pub(crate) fn new(span: Span, node: T) -> Self { Spanned { span, node } }
}

pub(crate) type SExpr = Spanned<Expr>;

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    IntLit(i64),
    StrLit(String),
    SymbolLit(String),
    InterpolatedStr(Vec<SExpr>),
    BoolLit(bool),
    Nil,
    LVarRead(String),
    LVarWrite(String, Box<SExpr>),
    IVarRead(String),
    IVarWrite(String, Box<SExpr>),
    SelfExpr,
    ConstRead(String),
    Call {
        receiver: Option<Box<SExpr>>,
        name: String,
        args: Vec<SExpr>,
    },
    If {
        cond: Box<SExpr>,
        then_body: Vec<SExpr>,
        else_body: Vec<SExpr>,
    },
    While {
        cond: Box<SExpr>,
        body: Vec<SExpr>,
    },
    Def {
        name: String,
        params: Vec<String>,
        body: Vec<SExpr>,
    },
    Class {
        name: String,
        /// Name of the parent class, if `class Foo < Bar` syntax was used.
        superclass: Option<String>,
        body: Vec<SExpr>,
    },
    ArrayLit(Vec<SExpr>),
    HashLit(Vec<(SExpr, SExpr)>),
    /// `begin..end` (exclusive=false) or `begin...end` (exclusive=true).
    /// Both endpoints must be present in our subset.
    RangeLit { begin: Box<SExpr>, end: Box<SExpr>, exclusive: bool },
    CallWithBlock {
        receiver: Option<Box<SExpr>>,
        name: String,
        args: Vec<SExpr>,
        block_params: Vec<String>,
        block_body: Vec<SExpr>,
    },
    Yield(Vec<SExpr>),
    /// `return [val]` — exits the current method/block frame with `val`.
    Return(Option<Box<SExpr>>),
    /// `next [val]` — exits the current block iteration with `val`.
    /// Outside a block CRuby raises LocalJumpError; we treat it as Return
    /// of the current frame (acceptable for the niches we serve).
    Next(Option<Box<SExpr>>),
    /// `break [val]` — exits the current block AND terminates the
    /// iteration in the calling driver (e.g. `arr.each`), making the
    /// driver's return value `val`.
    Break(Option<Box<SExpr>>),
    /// `a || b` — short-circuit: returns `a` if truthy, else `b`.
    Or(Box<SExpr>, Box<SExpr>),
    /// `a && b` — short-circuit: returns `b` if `a` truthy, else `a`.
    And(Box<SExpr>, Box<SExpr>),
    Begin {
        body: Vec<SExpr>,
        rescue: Option<RescueClause>,
        ensure: Option<Vec<SExpr>>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RescueClause {
    pub(crate) body: Vec<SExpr>,
    pub(crate) var: Option<String>,
}

// ---------- Translate prism AST to Expr ----------

pub(crate) fn cid_to_string(id: ruby_prism::ConstantId<'_>) -> String {
    String::from_utf8_lossy(id.as_slice()).into_owned()
}

fn node_span(node: &Node<'_>) -> Span {
    Span::at(node.location().start_offset())
}

fn sp(node: &Node<'_>, e: Expr) -> SExpr {
    Spanned::new(node_span(node), e)
}

pub(crate) fn tr(node: &Node<'_>) -> SExpr {
    let span = node_span(node);
    if let Some(n) = node.as_program_node() {
        let stmts: Vec<SExpr> = n.statements().body().iter().map(|c| tr(&c)).collect();
        return if stmts.len() == 1 {
            stmts.into_iter().next().unwrap()
        } else {
            Spanned::new(span, seq_inner(stmts))
        };
    }
    if let Some(n) = node.as_statements_node() {
        let stmts: Vec<SExpr> = n.body().iter().map(|c| tr(&c)).collect();
        return Spanned::new(span, seq_inner(stmts));
    }
    if let Some(n) = node.as_integer_node() {
        let v: i32 = n.value().try_into().unwrap_or(0);
        return sp(node, Expr::IntLit(v as i64));
    }
    if let Some(n) = node.as_string_node() {
        return sp(node, Expr::StrLit(String::from_utf8_lossy(n.unescaped()).into_owned()));
    }
    if let Some(n) = node.as_symbol_node() {
        return sp(node, Expr::SymbolLit(String::from_utf8_lossy(n.unescaped()).into_owned()));
    }
    if let Some(n) = node.as_interpolated_string_node() {
        let parts: Vec<SExpr> = n.parts().iter().map(|p| {
            if let Some(es) = p.as_embedded_statements_node() {
                let stmts: Vec<SExpr> = es.statements()
                    .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                    .unwrap_or_default();
                if stmts.len() == 1 { stmts.into_iter().next().unwrap() }
                else { Spanned::new(node_span(&p), seq_inner(stmts)) }
            } else if let Some(ev) = p.as_embedded_variable_node() {
                tr(&ev.variable())
            } else {
                tr(&p)
            }
        }).collect();
        return sp(node, Expr::InterpolatedStr(parts));
    }
    if node.as_true_node().is_some() { return sp(node, Expr::BoolLit(true)); }
    if node.as_false_node().is_some() { return sp(node, Expr::BoolLit(false)); }
    if node.as_nil_node().is_some() { return sp(node, Expr::Nil); }
    if node.as_self_node().is_some() { return sp(node, Expr::SelfExpr); }
    if let Some(n) = node.as_constant_read_node() {
        return sp(node, Expr::ConstRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_local_variable_read_node() {
        return sp(node, Expr::LVarRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_local_variable_write_node() {
        return sp(node, Expr::LVarWrite(cid_to_string(n.name()), Box::new(tr(&n.value()))));
    }
    if let Some(n) = node.as_instance_variable_read_node() {
        return sp(node, Expr::IVarRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_instance_variable_write_node() {
        return sp(node, Expr::IVarWrite(cid_to_string(n.name()), Box::new(tr(&n.value()))));
    }
    if let Some(n) = node.as_call_node() {
        let receiver = n.receiver().map(|r| Box::new(tr(&r)));
        let name = cid_to_string(n.name());
        let args: Vec<SExpr> = n
            .arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        if let Some(bnode) = n.block() {
            if let Some(bn) = bnode.as_block_node() {
                let block_params: Vec<String> = bn.parameters().and_then(|pn| pn.as_block_parameters_node()).and_then(|bp| bp.parameters())
                    .map(|p| p.requireds().iter().filter_map(|r| r.as_required_parameter_node().map(|rp| cid_to_string(rp.name()))).collect())
                    .unwrap_or_default();
                let block_body: Vec<SExpr> = match bn.body() {
                    Some(b) => {
                        if let Some(stmts) = b.as_statements_node() {
                            stmts.body().iter().map(|c| tr(&c)).collect()
                        } else { vec![tr(&b)] }
                    }
                    None => vec![],
                };
                return sp(node, Expr::CallWithBlock { receiver, name, args, block_params, block_body });
            }
        }
        return sp(node, Expr::Call { receiver, name, args });
    }
    if let Some(n) = node.as_return_node() {
        let val = n.arguments().and_then(|a| {
            a.arguments().iter().next().map(|first| Box::new(tr(&first)))
        });
        return sp(node, Expr::Return(val));
    }
    if let Some(n) = node.as_next_node() {
        let val = n.arguments().and_then(|a| {
            a.arguments().iter().next().map(|first| Box::new(tr(&first)))
        });
        return sp(node, Expr::Next(val));
    }
    if let Some(n) = node.as_break_node() {
        let val = n.arguments().and_then(|a| {
            a.arguments().iter().next().map(|first| Box::new(tr(&first)))
        });
        return sp(node, Expr::Break(val));
    }
    if let Some(n) = node.as_yield_node() {
        let args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return sp(node, Expr::Yield(args));
    }
    if let Some(n) = node.as_if_node() {
        let cond = Box::new(tr(&n.predicate()));
        let then_body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let else_body: Vec<SExpr> = match n.subsequent() {
            Some(sub) => {
                if let Some(en) = sub.as_else_node() {
                    en.statements().map(|s| s.body().iter().map(|c| tr(&c)).collect()).unwrap_or_default()
                } else {
                    vec![tr(&sub)]
                }
            }
            None => vec![],
        };
        return sp(node, Expr::If { cond, then_body, else_body });
    }
    if let Some(n) = node.as_or_node() {
        return sp(node, Expr::Or(Box::new(tr(&n.left())), Box::new(tr(&n.right()))));
    }
    if let Some(n) = node.as_and_node() {
        return sp(node, Expr::And(Box::new(tr(&n.left())), Box::new(tr(&n.right()))));
    }
    if let Some(n) = node.as_while_node() {
        let cond = Box::new(tr(&n.predicate()));
        let body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return sp(node, Expr::While { cond, body });
    }
    if let Some(n) = node.as_def_node() {
        let name = cid_to_string(n.name());
        let params: Vec<String> = n.parameters().map(|p| {
            p.requireds().iter()
                .filter_map(|r| r.as_required_parameter_node().map(|rp| cid_to_string(rp.name())))
                .collect()
        }).unwrap_or_default();
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Def { name, params, body });
    }
    if let Some(n) = node.as_range_node() {
        // Beginless / endless ranges (`..3`, `1..`) are not yet supported;
        // we treat the missing endpoint as `nil` which will fail at runtime
        // when something tries to iterate. For our subset, both ends should
        // be present.
        let begin = n.left().map(|c| tr(&c)).unwrap_or_else(|| sp(node, Expr::Nil));
        let end = n.right().map(|c| tr(&c)).unwrap_or_else(|| sp(node, Expr::Nil));
        return sp(node, Expr::RangeLit {
            begin: Box::new(begin),
            end: Box::new(end),
            exclusive: n.is_exclude_end(),
        });
    }
    if let Some(n) = node.as_array_node() {
        let elems: Vec<SExpr> = n.elements().iter().map(|e| tr(&e)).collect();
        return sp(node, Expr::ArrayLit(elems));
    }
    if let Some(n) = node.as_hash_node() {
        let pairs: Vec<(SExpr, SExpr)> = n.elements().iter().filter_map(|e| {
            e.as_assoc_node().map(|a| (tr(&a.key()), tr(&a.value())))
        }).collect();
        return sp(node, Expr::HashLit(pairs));
    }
    if let Some(n) = node.as_class_node() {
        let name = if let Some(cr) = n.constant_path().as_constant_read_node() {
            cid_to_string(cr.name())
        } else { "?".to_string() };
        let superclass = n.superclass().and_then(|s| {
            s.as_constant_read_node().map(|cr| cid_to_string(cr.name()))
        });
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Class { name, superclass, body });
    }
    if let Some(n) = node.as_parentheses_node() {
        // `(expr)` — just unwrap to the inner expression / statements.
        if let Some(body) = n.body() {
            if let Some(stmts) = body.as_statements_node() {
                let v: Vec<SExpr> = stmts.body().iter().map(|c| tr(&c)).collect();
                return if v.len() == 1 { v.into_iter().next().unwrap() }
                       else { Spanned::new(span, seq_inner(v)) };
            }
            return tr(&body);
        }
        return sp(node, Expr::Nil);
    }
    if let Some(n) = node.as_begin_node() {
        let body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let rescue = n.rescue_clause().map(|rc| {
            let body: Vec<SExpr> = rc.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                .unwrap_or_default();
            let var = rc.reference().and_then(|r| {
                r.as_local_variable_target_node().map(|lvt| cid_to_string(lvt.name()))
            });
            RescueClause { body, var }
        });
        let ensure = n.ensure_clause().map(|ec| {
            ec.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect::<Vec<SExpr>>())
                .unwrap_or_default()
        });
        return sp(node, Expr::Begin { body, rescue, ensure });
    }
    panic!("unsupported node: {:?}", node);
}

fn seq_inner(stmts: Vec<SExpr>) -> Expr {
    Expr::Call { receiver: None, name: "__seq__".to_string(), args: stmts }
}

#[allow(dead_code)]
pub(crate) fn seq(stmts: Vec<SExpr>) -> SExpr {
    Spanned::new(Span::ZERO, seq_inner(stmts))
}
