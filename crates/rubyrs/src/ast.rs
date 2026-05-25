use std::cell::RefCell;

use ruby_prism::Node;

use crate::error::Span;

// AST translation collects unsupported-node messages here instead of
// panicking. `tr_with_errors` clears + drains; bare `tr` (kept for
// the recursive internal API) still walks the whole tree, leaving a
// `Expr::Nil` placeholder wherever it bailed. The caller is
// responsible for checking the collected errors and surfacing a
// SyntaxError Trap before any compile/exec happens.
thread_local! {
    static AST_ERRORS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Translate a Prism root node, returning the SExpr plus any
/// unsupported-node messages collected along the way. Empty `errs`
/// means the whole tree was within the supported subset. If `errs`
/// is non-empty the returned SExpr may contain `Expr::Nil`
/// placeholders where translation failed — don't compile it.
pub(crate) fn tr_with_errors(node: &Node<'_>) -> (SExpr, Vec<String>) {
    AST_ERRORS.with(|cell| cell.borrow_mut().clear());
    let prog = tr(node);
    let errs = AST_ERRORS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    (prog, errs)
}

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
    FloatLit(f64),
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
        /// All formal parameters in source order — required ones
        /// first, then optionals. (Splat / keyword / block params
        /// aren't supported yet.)
        params: Vec<String>,
        /// Parallel to `params`. `None` = required (must be passed
        /// by the caller). `Some(SExpr)` = optional with a default
        /// expression. The default is restricted at AST-translate
        /// time to literal values (Int / Str / Sym / Bool / Nil) —
        /// arbitrary default expressions can reference earlier
        /// params and need a per-callsite prologue, which is more
        /// invasive than what this minimal pass handles.
        defaults: Vec<Option<SExpr>>,
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
        /// Zero or more `rescue` clauses, in source order. Empty
        /// vector means `begin ... end` without any rescue.
        /// Multiple clauses chain via Prism's `subsequent()`.
        rescue: Vec<RescueClause>,
        ensure: Option<Vec<SExpr>>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RescueClause {
    /// Class names to filter on. Empty = bare `rescue` (treated as
    /// `rescue StandardError` per Ruby semantics — see ADR 0008).
    /// Names that fail to resolve as classes at run-time
    /// (e.g. `rescue UndefinedConst`) make the clause never fire,
    /// matching CRuby's "skip handlers whose class isn't loaded"
    /// behaviour for our subset. ConstantPath names like
    /// `Foo::Bar` aren't yet supported and fall back to the
    /// last segment (`Bar`).
    pub(crate) classes: Vec<String>,
    pub(crate) body: Vec<SExpr>,
    pub(crate) var: Option<String>,
}

// ---------- Translate prism AST to Expr ----------

pub(crate) fn cid_to_string(id: ruby_prism::ConstantId<'_>) -> String {
    String::from_utf8_lossy(id.as_slice()).into_owned()
}

/// Flatten a Prism `ConstantPathNode` into a single `"A::B::C"`
/// string. Returns `None` if any segment is dynamic (e.g. a
/// method-call result in const position) — callers should fall
/// back to last-segment-only behaviour in that case.
fn flatten_constant_path(node: &Node<'_>) -> Option<String> {
    let cp = node.as_constant_path_node()?;
    let name = cid_to_string(cp.name()?);
    match cp.parent() {
        None => Some(name), // leading `::Bar` — treat as top-level `Bar`
        Some(parent) => {
            let head = if let Some(cr) = parent.as_constant_read_node() {
                cid_to_string(cr.name())
            } else if parent.as_constant_path_node().is_some() {
                flatten_constant_path(&parent)?
            } else {
                return None; // dynamic head
            };
            Some(format!("{}::{}", head, name))
        }
    }
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
    if let Some(n) = node.as_float_node() {
        return sp(node, Expr::FloatLit(n.value()));
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
    if let Some(n) = node.as_constant_path_node() {
        // Spike scope: a `Foo::Bar::Baz` ConstantPath translates to
        // a single ConstRead with the joined name. No real
        // module nesting; C extensions and `class` definitions that
        // wire up "BCrypt::Engine"-style classes must register them
        // under the joined name for this lookup to find them.
        // Real module scope resolution lands when we add the
        // `module` keyword to the language.
        if let Some(joined) = flatten_constant_path(&node) {
            return sp(node, Expr::ConstRead(joined));
        }
        // Dynamic path (rare): trailing-name fallback, matches the
        // existing rescue-clause behaviour at line ~378.
        if let Some(name_id) = n.name() {
            return sp(node, Expr::ConstRead(cid_to_string(name_id)));
        }
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
        let mut params: Vec<String> = Vec::new();
        let mut defaults: Vec<Option<SExpr>> = Vec::new();
        if let Some(p) = n.parameters() {
            for r in p.requireds().iter() {
                if let Some(rp) = r.as_required_parameter_node() {
                    params.push(cid_to_string(rp.name()));
                    defaults.push(None);
                }
            }
            for o in p.optionals().iter() {
                if let Some(op) = o.as_optional_parameter_node() {
                    params.push(cid_to_string(op.name()));
                    let val = tr(&op.value());
                    // Restrict defaults to literal values. Anything
                    // else (a method call, a reference to an earlier
                    // param, etc.) needs a per-callsite prologue we
                    // don't generate yet — surface as a SyntaxError
                    // via the AST_ERRORS thread-local rather than
                    // silently miscompiling.
                    match &val.node {
                        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::SymbolLit(_)
                        | Expr::BoolLit(_) | Expr::Nil => {
                            defaults.push(Some(val));
                        }
                        _ => {
                            AST_ERRORS.with(|cell| cell.borrow_mut().push(
                                format!("default value for parameter `{}` must be a literal (Int/Str/Sym/true/false/nil)", cid_to_string(op.name()))
                            ));
                            defaults.push(Some(sp(&o, Expr::Nil)));
                        }
                    }
                }
            }
        }
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Def { name, params, defaults, body });
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
        // Prism chains rescue clauses via `subsequent()`. Walk the
        // chain and flatten to a Vec so the compiler can emit one
        // PushRescue per clause in the right order.
        let mut rescue: Vec<RescueClause> = Vec::new();
        let mut cur = n.rescue_clause();
        while let Some(rc) = cur {
            let body: Vec<SExpr> = rc.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                .unwrap_or_default();
            let var = rc.reference().and_then(|r| {
                r.as_local_variable_target_node().map(|lvt| cid_to_string(lvt.name()))
            });
            // Extract class filter names. We accept ConstantReadNode
            // (`MyError`) directly. ConstantPathNode (`Foo::Bar`)
            // is a follow-up — for now we resolve it to the last
            // segment so `rescue Gem::LoadError` at least matches
            // a top-level `LoadError` class if defined.
            let mut classes: Vec<String> = Vec::new();
            for exc in rc.exceptions().iter() {
                if let Some(c) = exc.as_constant_read_node() {
                    classes.push(cid_to_string(c.name()));
                } else if let Some(cp) = exc.as_constant_path_node() {
                    // Use the trailing name. Better than nothing
                    // until P1-10b adds proper qualified-class
                    // resolution. `cp.name()` is `Option<ConstantId>`
                    // because Prism allows dynamic constant paths.
                    if let Some(name_id) = cp.name() {
                        classes.push(cid_to_string(name_id));
                    }
                }
                // Anything else (dynamic expression in rescue
                // position) is dropped silently for now.
            }
            rescue.push(RescueClause { classes, body, var });
            cur = rc.subsequent();
        }
        let ensure = n.ensure_clause().map(|ec| {
            ec.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect::<Vec<SExpr>>())
                .unwrap_or_default()
        });
        return sp(node, Expr::Begin { body, rescue, ensure });
    }
    // Unsupported Prism node — record the message and return a
    // placeholder. The eval entry point checks `AST_ERRORS` after
    // tr returns and surfaces a SyntaxError Trap, so the
    // placeholder never reaches the compiler in practice.
    AST_ERRORS.with(|cell| cell.borrow_mut().push(format!("unsupported node: {:?}", node)));
    sp(node, Expr::Nil)
}

fn seq_inner(stmts: Vec<SExpr>) -> Expr {
    Expr::Call { receiver: None, name: "__seq__".to_string(), args: stmts }
}

#[allow(dead_code)]
pub(crate) fn seq(stmts: Vec<SExpr>) -> SExpr {
    Spanned::new(Span::ZERO, seq_inner(stmts))
}
