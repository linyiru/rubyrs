use ruby_prism::Node;

// ---------- IR ----------

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    IntLit(i64),
    StrLit(String),
    SymbolLit(String),
    InterpolatedStr(Vec<Expr>),
    BoolLit(bool),
    Nil,
    LVarRead(String),
    LVarWrite(String, Box<Expr>),
    IVarRead(String),
    IVarWrite(String, Box<Expr>),
    SelfExpr,
    ConstRead(String),
    Call {
        receiver: Option<Box<Expr>>,
        name: String,
        args: Vec<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_body: Vec<Expr>,
        else_body: Vec<Expr>,
    },
    While {
        cond: Box<Expr>,
        body: Vec<Expr>,
    },
    Def {
        name: String,
        params: Vec<String>,
        body: Vec<Expr>,
    },
    Class {
        name: String,
        body: Vec<Expr>,
    },
    ArrayLit(Vec<Expr>),
    HashLit(Vec<(Expr, Expr)>),
    CallWithBlock {
        receiver: Option<Box<Expr>>,
        name: String,
        args: Vec<Expr>,
        block_params: Vec<String>,
        block_body: Vec<Expr>,
    },
    Yield(Vec<Expr>),
    Begin {
        body: Vec<Expr>,
        rescue: Option<RescueClause>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RescueClause {
    pub(crate) body: Vec<Expr>,
    pub(crate) var: Option<String>,
}

// ---------- Translate prism AST to Expr ----------

pub(crate) fn cid_to_string(id: ruby_prism::ConstantId<'_>) -> String {
    String::from_utf8_lossy(id.as_slice()).into_owned()
}

pub(crate) fn tr(node: &Node<'_>) -> Expr {
    if let Some(n) = node.as_program_node() {
        let stmts: Vec<Expr> = n.statements().body().iter().map(|c| tr(&c)).collect();
        return if stmts.len() == 1 {
            stmts.into_iter().next().unwrap()
        } else {
            seq(stmts)
        };
    }
    if let Some(n) = node.as_statements_node() {
        let stmts: Vec<Expr> = n.body().iter().map(|c| tr(&c)).collect();
        return seq(stmts);
    }
    if let Some(n) = node.as_integer_node() {
        let v: i32 = n.value().try_into().unwrap_or(0);
        return Expr::IntLit(v as i64);
    }
    if let Some(n) = node.as_string_node() {
        return Expr::StrLit(String::from_utf8_lossy(n.unescaped()).into_owned());
    }
    if let Some(n) = node.as_symbol_node() {
        return Expr::SymbolLit(String::from_utf8_lossy(n.unescaped()).into_owned());
    }
    if let Some(n) = node.as_interpolated_string_node() {
        let parts: Vec<Expr> = n.parts().iter().map(|p| {
            if let Some(es) = p.as_embedded_statements_node() {
                let stmts: Vec<Expr> = es.statements()
                    .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                    .unwrap_or_default();
                if stmts.len() == 1 { stmts.into_iter().next().unwrap() } else { seq(stmts) }
            } else if let Some(ev) = p.as_embedded_variable_node() {
                tr(&ev.variable())
            } else {
                tr(&p)
            }
        }).collect();
        return Expr::InterpolatedStr(parts);
    }
    if node.as_true_node().is_some() { return Expr::BoolLit(true); }
    if node.as_false_node().is_some() { return Expr::BoolLit(false); }
    if node.as_nil_node().is_some() { return Expr::Nil; }
    if node.as_self_node().is_some() { return Expr::SelfExpr; }
    if let Some(n) = node.as_constant_read_node() {
        return Expr::ConstRead(cid_to_string(n.name()));
    }
    if let Some(n) = node.as_local_variable_read_node() {
        return Expr::LVarRead(cid_to_string(n.name()));
    }
    if let Some(n) = node.as_local_variable_write_node() {
        return Expr::LVarWrite(cid_to_string(n.name()), Box::new(tr(&n.value())));
    }
    if let Some(n) = node.as_instance_variable_read_node() {
        return Expr::IVarRead(cid_to_string(n.name()));
    }
    if let Some(n) = node.as_instance_variable_write_node() {
        return Expr::IVarWrite(cid_to_string(n.name()), Box::new(tr(&n.value())));
    }
    if let Some(n) = node.as_call_node() {
        let receiver = n.receiver().map(|r| Box::new(tr(&r)));
        let name = cid_to_string(n.name());
        let args: Vec<Expr> = n
            .arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        if let Some(bnode) = n.block() {
            if let Some(bn) = bnode.as_block_node() {
                let block_params: Vec<String> = bn.parameters().and_then(|pn| pn.as_block_parameters_node()).and_then(|bp| bp.parameters())
                    .map(|p| p.requireds().iter().filter_map(|r| r.as_required_parameter_node().map(|rp| cid_to_string(rp.name()))).collect())
                    .unwrap_or_default();
                let block_body: Vec<Expr> = match bn.body() {
                    Some(b) => {
                        if let Some(stmts) = b.as_statements_node() {
                            stmts.body().iter().map(|c| tr(&c)).collect()
                        } else { vec![tr(&b)] }
                    }
                    None => vec![],
                };
                return Expr::CallWithBlock { receiver, name, args, block_params, block_body };
            }
        }
        return Expr::Call { receiver, name, args };
    }
    if let Some(n) = node.as_yield_node() {
        let args: Vec<Expr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return Expr::Yield(args);
    }
    if let Some(n) = node.as_if_node() {
        let cond = Box::new(tr(&n.predicate()));
        let then_body: Vec<Expr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let else_body: Vec<Expr> = match n.subsequent() {
            Some(sub) => {
                if let Some(en) = sub.as_else_node() {
                    en.statements().map(|s| s.body().iter().map(|c| tr(&c)).collect()).unwrap_or_default()
                } else {
                    vec![tr(&sub)]
                }
            }
            None => vec![],
        };
        return Expr::If { cond, then_body, else_body };
    }
    if let Some(n) = node.as_while_node() {
        let cond = Box::new(tr(&n.predicate()));
        let body: Vec<Expr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return Expr::While { cond, body };
    }
    if let Some(n) = node.as_def_node() {
        let name = cid_to_string(n.name());
        let params: Vec<String> = n.parameters().map(|p| {
            p.requireds().iter()
                .filter_map(|r| r.as_required_parameter_node().map(|rp| cid_to_string(rp.name())))
                .collect()
        }).unwrap_or_default();
        let body: Vec<Expr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return Expr::Def { name, params, body };
    }
    if let Some(n) = node.as_array_node() {
        let elems: Vec<Expr> = n.elements().iter().map(|e| tr(&e)).collect();
        return Expr::ArrayLit(elems);
    }
    if let Some(n) = node.as_hash_node() {
        let pairs: Vec<(Expr, Expr)> = n.elements().iter().filter_map(|e| {
            e.as_assoc_node().map(|a| (tr(&a.key()), tr(&a.value())))
        }).collect();
        return Expr::HashLit(pairs);
    }
    if let Some(n) = node.as_class_node() {
        let name = if let Some(cr) = n.constant_path().as_constant_read_node() {
            cid_to_string(cr.name())
        } else { "?".to_string() };
        let body: Vec<Expr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return Expr::Class { name, body };
    }
    if let Some(n) = node.as_begin_node() {
        let body: Vec<Expr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let rescue = n.rescue_clause().map(|rc| {
            let body: Vec<Expr> = rc.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                .unwrap_or_default();
            let var = rc.reference().and_then(|r| {
                r.as_local_variable_target_node().map(|lvt| cid_to_string(lvt.name()))
            });
            RescueClause { body, var }
        });
        return Expr::Begin { body, rescue };
    }
    panic!("unsupported node: {:?}", node);
}

pub(crate) fn seq(stmts: Vec<Expr>) -> Expr {
    Expr::Call { receiver: None, name: "__seq__".to_string(), args: stmts }
}
