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

// `SelfExpr` tripping enum_variant_names is the variant `Self` would
// be — but `Self` is reserved by the language, so the `Expr` suffix
// disambiguates rather than echoes. The other "Expr"-shaped variants
// are non-suffixed.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub(crate) enum Expr {
    IntLit(i64),
    FloatLit(f64),
    StrLit(String),
    /// `/pattern/` literal — Ruby regular expression. Source is
    /// kept as a String for interning; compilation happens at the
    /// VM layer (with caching).
    RegexLit(String),
    SymbolLit(String),
    InterpolatedStr(Vec<SExpr>),
    BoolLit(bool),
    Nil,
    LVarRead(String),
    LVarWrite(String, Box<SExpr>),
    IVarRead(String),
    IVarWrite(String, Box<SExpr>),
    /// Multi-write destructuring: `a, b = arr`, `@x, @y = pt`,
    /// `a, b = 1, 2`. The RHS is always an Array — multiple
    /// right-side expressions get packed into an Array literal
    /// at translation time. Targets are extracted by index; if
    /// there are more targets than elements, the surplus get
    /// `nil`. Splat (`*rest`) and call-targets (`obj.x =`) are
    /// not supported yet — those nodes are dropped silently.
    MultiWrite {
        targets: Vec<MultiWriteTarget>,
        value: Box<SExpr>,
    },
    SelfExpr,
    ConstRead(String),
    /// Constant write — covers both the bare `FOO = expr`
    /// (ConstantWriteNode) and the path form `Foo::Bar = expr`
    /// (ConstantPathWriteNode). Both flatten into a single
    /// "A::B::C"-joined name and store into the same
    /// `Vm.constants` table (rubyrs has no real module nesting
    /// yet — the path form's segment-validation divergences from
    /// CRuby are noted at the ConstantPathWriteNode translation
    /// site below).
    ConstWrite(String, Box<SExpr>),
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
        /// `true` for the post-condition form
        /// `begin … end while cond` / `begin … end until cond`.
        /// Body runs once before the first cond check (CRuby
        /// semantics). `false` for the pre-condition form
        /// `while cond; …; end`.
        post: bool,
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
        /// `Some(name)` for `def foo(a, b, *rest)`. Args past
        /// the last positional slot collapse into a fresh Array
        /// bound to this name. `None` means no rest param.
        rest: Option<String>,
        /// Keyword parameters: `def foo(name:, age: 0)` collects
        /// `("name", None)` and `("age", Some(IntLit(0)))`.
        /// Order is source order. None default = required.
        kw_params: Vec<(String, Option<SExpr>)>,
        /// `Some(name)` for `def foo(a, **opts)` — the leftover
        /// keyword args (those not bound by a named `kw_params`
        /// entry) collect into a fresh Hash bound to `name`.
        /// `Some("")` for the anonymous form `def foo(**)`
        /// (currently unused but reserved). `None` means no
        /// kw-rest capture; trailing-Hash callers with
        /// unrecognised keys raise ArgumentError.
        kw_rest: Option<String>,
        /// `Some(name)` for `def foo(&blk)` — the block-as-data
        /// parameter. Captures the BlockHandle the caller passed
        /// (or nil if no block) into a local of this name. `None`
        /// for plain `def foo`. Lives after kw_rest in the slot
        /// layout (see Proto.block_param).
        block_param: Option<String>,
        /// `def receiver.name; ...; end` — singleton method
        /// definition. `Some(SelfExpr)` is the class-body
        /// `def self.foo` form (compiles to
        /// `Op::DefSingletonMethod`, installs on the class's
        /// `singleton_methods` table). `Some(other)` is the
        /// general instance form `def obj.foo` (compiles to
        /// `Op::DefObjectSingletonMethod`, installs on the
        /// receiver Object's lazily-allocated eigenclass).
        /// `None` for the regular `def name; ...; end`.
        receiver: Option<Box<SExpr>>,
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
        block_params: Vec<BlockParam>,
        block_body: Vec<SExpr>,
    },
    /// `foo(&proc_value)` — block argument forwarding. The
    /// `block_arg` expression must evaluate to a `Value::Block`
    /// at runtime; that block is passed to the call as if it
    /// were a literal `do…end`. Synthesised from
    /// `BlockArgumentNode { expression: <non-symbol> }`.
    /// Symbol-to-proc (`&:foo`) takes the regular CallWithBlock
    /// path with a synthesised one-arg block — see the AST
    /// translator for the two branches.
    CallWithBlockArg {
        receiver: Option<Box<SExpr>>,
        name: String,
        args: Vec<SExpr>,
        block_arg: Box<SExpr>,
    },
    Yield(Vec<SExpr>),
    /// `foo(*arr)` — single-splat call. The compiler emits an
    /// `Op::ApplyCall` / `Op::ApplyCallNoRecv` that takes one
    /// Array on top of the stack and uses its elements as
    /// positional args. Mixed forms like `foo(a, *b, c)` are
    /// not yet supported.
    Apply {
        receiver: Option<Box<SExpr>>,
        name: String,
        splat: Box<SExpr>,
    },
    /// `->(params) { body }` — lambda literal. Compiles to the
    /// same `CreateBlock` opcode as a regular `{ |x| ... }` block,
    /// but stays on the stack as a Value::Block instead of being
    /// consumed by a method call. We don't distinguish Lambda
    /// from Proc at runtime; the strict-arity check that CRuby's
    /// Lambda enforces is missing — documented in SUBSET.md.
    Lambda { params: Vec<BlockParam>, body: Vec<SExpr> },
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
    /// `super` (forwarding all of the enclosing method's args)
    /// or `super(arg1, arg2)` (explicit args). `super()` with
    /// empty parens passes no args and is `Some(vec![])`;
    /// bare `super` is `None`.
    Super(Option<Vec<SExpr>>),
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

/// One top-level block parameter as seen at the block-call ABI.
/// `|a, (b, c)|` produces two `BlockParam`s: `Single("a")` and
/// `Destructure([Single("b"), Single("c")])`. The destructure
/// stores its inner params (which may themselves be nested
/// destructures, supporting `|((a, b), c)|` and deeper) alongside
/// an anonymous receiving slot the compile path reads from to
/// populate the named inner slots via a prologue.
#[derive(Debug, Clone)]
pub(crate) enum BlockParam {
    Single(String),
    Destructure(Vec<BlockParam>),
    /// `|*args|` rest parameter — collects all positional args
    /// past the last `Single` / `Destructure` slot into a fresh
    /// Array bound to this name. At most one Rest per param list
    /// (Prism enforces source-level uniqueness). Empty name is
    /// the anonymous form `|*|` (reserve the slot, drop the
    /// data — analogous to `**` for kwargs).
    Rest(String),
}

#[derive(Debug, Clone)]
pub(crate) enum MultiWriteTarget {
    Local(String),
    Ivar(String),
    /// `*rest` — receives a fresh Array of the middle slice.
    /// `None` is the anonymous form `*` which discards the slice
    /// but still anchors the post-splat counting.
    SplatLocal(Option<String>),
    /// `*@rest` — splat into an ivar. Same slicing as SplatLocal.
    SplatIvar(String),
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

/// Translate a Prism `KeywordHashNode` into a single SExpr that
/// evaluates to a Hash. Pairs like `a: 1` build into HashLit
/// chunks; `**opts` splats interrupt the chunk and chain
/// `.merge(opts)` against the accumulated hash. The final
/// expression has shape `{...}.merge(opts).merge({...})...`
/// — same Hash that CRuby would build for the same source.
fn tr_kwhash(parent: &Node<'_>, kh_anchor: &Node<'_>, kh: &ruby_prism::KeywordHashNode<'_>) -> SExpr {
    let mut chunks: Vec<SExpr> = Vec::new();
    let mut buf: Vec<(SExpr, SExpr)> = Vec::new();
    for el in kh.elements().iter() {
        if let Some(an) = el.as_assoc_node() {
            buf.push((tr(&an.key()), tr(&an.value())));
        } else if let Some(spn) = el.as_assoc_splat_node()
            && let Some(inner) = spn.value() {
                if !buf.is_empty() {
                    chunks.push(sp(kh_anchor, Expr::HashLit(std::mem::take(&mut buf))));
                }
                chunks.push(tr(&inner));
            }
    }
    if !buf.is_empty() {
        chunks.push(sp(kh_anchor, Expr::HashLit(buf)));
    }
    if chunks.is_empty() {
        return sp(parent, Expr::HashLit(vec![]));
    }
    let mut it = chunks.into_iter();
    let first = it.next().unwrap();
    it.fold(first, |lhs, rhs| sp(parent, Expr::Call {
        receiver: Some(Box::new(lhs)),
        name: "merge".into(),
        args: vec![rhs],
    }))
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
    if let Some(n) = node.as_regular_expression_node() {
        return sp(node, Expr::RegexLit(String::from_utf8_lossy(n.unescaped()).into_owned()));
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
        if let Some(joined) = flatten_constant_path(node) {
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
    // Bare constant assignment: `FOO = expr` (top level or inside a
    // class/module body). Storage is a separate `Vm.constants` map
    // keyed by SymId — class names continue to live in `Vm.classes`,
    // and class lookup wins on read. This is a deliberate rubyrs
    // divergence from CRuby (CRuby warns "already initialized" and
    // reassigns); see `Vm::constants` for the precedence rationale.
    if let Some(n) = node.as_constant_write_node() {
        return sp(node, Expr::ConstWrite(cid_to_string(n.name()), Box::new(tr(&n.value()))));
    }
    // `Foo::Bar = expr` — ConstantPathWriteNode. Same spike-scope
    // model as ConstantPathNode read: flatten the LHS path into a
    // joined "A::B::C" name and route through the existing
    // `Vm.constants` table (StoreConst opcode). No real module
    // nesting; the assignment binds the joined name, and a later
    // `Foo::Bar` read picks it up via `ConstRead("Foo::Bar")`.
    //
    // Two known CRuby divergences inherited from this spike-scope
    // model (symmetric with the way ConstantPathNode read also
    // skips module-nesting validation):
    //   - `Missing::X = 1` succeeds silently here; CRuby raises
    //     `NameError: uninitialized constant Missing`.
    //   - `Foo = 1; Foo::X = 2` succeeds here; CRuby raises
    //     `TypeError: Foo is not a class/module`.
    // A future PR would walk each prefix segment via the existing
    // class/constants lookup and require Class/Module — and the
    // same fix would apply to the READ side. Out of this PR's
    // scope (the AST translation alone can't see runtime types).
    if let Some(n) = node.as_constant_path_write_node() {
        let target = n.target();
        // target is a ConstantPathNode; flatten via the same helper
        // the read path uses.
        if let Some(joined) = flatten_constant_path(&target.as_node()) {
            return sp(node, Expr::ConstWrite(joined, Box::new(tr(&n.value()))));
        }
        // Dynamic-path fallback (rare): use the trailing name only,
        // matching the ConstantPathNode read fallback at line ~415.
        if let Some(name_id) = target.name() {
            return sp(node, Expr::ConstWrite(cid_to_string(name_id), Box::new(tr(&n.value()))));
        }
    }
    // Op-assign desugaring: `a += b` is translated to
    // `a = a + b`. The receiver / index path is re-evaluated,
    // which costs one extra read but is observably equivalent
    // for the side-effect-free targets we encounter in
    // practice. Re-evaluating `arr[i] += v` calls Array#[]
    // twice (read then write); this is the same as
    // CRuby's literal rewrite — Ruby does NOT eval the
    // receiver/index once and cache it for `[]=`.
    if let Some(n) = node.as_local_variable_operator_write_node() {
        let name = cid_to_string(n.name());
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::LVarRead(name.clone()));
        let rhs = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(&n.value())],
        });
        return sp(node, Expr::LVarWrite(name, Box::new(rhs)));
    }
    if let Some(n) = node.as_instance_variable_operator_write_node() {
        let name = cid_to_string(n.name());
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::IVarRead(name.clone()));
        let rhs = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(&n.value())],
        });
        return sp(node, Expr::IVarWrite(name, Box::new(rhs)));
    }
    // `a ||= b` → `a || (a = b)`; `a &&= b` → `a && (a = b)`.
    // Reading an uninitialised local returns nil (the frame slot
    // is zeroed at entry), so `a ||= b` on a fresh `a` correctly
    // assigns. Same for ivars — unset ivar reads as nil.
    if let Some(n) = node.as_local_variable_or_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::LVarRead(name.clone()));
        let write = sp(node, Expr::LVarWrite(name, Box::new(tr(&n.value()))));
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_local_variable_and_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::LVarRead(name.clone()));
        let write = sp(node, Expr::LVarWrite(name, Box::new(tr(&n.value()))));
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_instance_variable_or_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::IVarRead(name.clone()));
        let write = sp(node, Expr::IVarWrite(name, Box::new(tr(&n.value()))));
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_instance_variable_and_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::IVarRead(name.clone()));
        let write = sp(node, Expr::IVarWrite(name, Box::new(tr(&n.value()))));
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_index_or_write_node() {
        // `recv[idx] ||= val` → `recv[idx] || (recv[idx] = val)`.
        let recv = n.receiver().map(|r| tr(&r)).expect(
            "IndexOrWriteNode without receiver is unrepresentable",
        );
        let idx_args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: "[]".into(),
            args: idx_args.clone(),
        });
        let mut write_args = idx_args;
        write_args.push(tr(&n.value()));
        let write = sp(node, Expr::Call {
            receiver: Some(Box::new(recv)),
            name: "[]=".into(),
            args: write_args,
        });
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_index_and_write_node() {
        // `recv[idx] &&= val` → `recv[idx] && (recv[idx] = val)`.
        let recv = n.receiver().map(|r| tr(&r)).expect(
            "IndexAndWriteNode without receiver is unrepresentable",
        );
        let idx_args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: "[]".into(),
            args: idx_args.clone(),
        });
        let mut write_args = idx_args;
        write_args.push(tr(&n.value()));
        let write = sp(node, Expr::Call {
            receiver: Some(Box::new(recv)),
            name: "[]=".into(),
            args: write_args,
        });
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_index_operator_write_node() {
        // `recv[idx] += val` → `recv.[]=(idx, recv.[](idx) + val)`.
        // Multi-arg subscripts (`m[i, j]`) are flattened: every
        // index node becomes a positional arg in both the read
        // and write calls. Block arg is not supported here
        // (`m[i, &b] += ...` is exotic; pass through as
        // unsupported).
        let recv = n.receiver().map(|r| tr(&r)).expect(
            "IndexOperatorWriteNode without receiver is unrepresentable in our subset",
        );
        let idx_args: Vec<SExpr> = n
            .arguments()
            .map(|a| a.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: "[]".into(),
            args: idx_args.clone(),
        });
        let new_val = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(&n.value())],
        });
        let mut write_args = idx_args;
        write_args.push(new_val);
        return sp(node, Expr::Call {
            receiver: Some(Box::new(recv)),
            name: "[]=".into(),
            args: write_args,
        });
    }
    if let Some(n) = node.as_multi_write_node() {
        // `a, b = expr`, `a, *r, b = expr`, `@x, @y = expr`,
        // `a, b = 1, 2`. Targets come from `lefts` (pre-splat),
        // `rest` (the splat slot itself), and `rights`
        // (post-splat). If Prism got multiple right-side values
        // with no array literal in source, they're packed into
        // an ArrayNode at the `value` slot.
        let mut targets: Vec<MultiWriteTarget> = Vec::new();
        let push_positional = |targets: &mut Vec<MultiWriteTarget>, tgt: &Node<'_>| {
            if let Some(lvt) = tgt.as_local_variable_target_node() {
                targets.push(MultiWriteTarget::Local(cid_to_string(lvt.name())));
            } else if let Some(ivt) = tgt.as_instance_variable_target_node() {
                targets.push(MultiWriteTarget::Ivar(cid_to_string(ivt.name())));
            } else {
                AST_ERRORS.with(|cell| cell.borrow_mut().push(
                    format!("unsupported multi-write target: {:?}", tgt)
                ));
            }
        };
        for tgt in n.lefts().iter() {
            push_positional(&mut targets, &tgt);
        }
        if let Some(rest) = n.rest() {
            if let Some(splat) = rest.as_splat_node() {
                match splat.expression() {
                    None => targets.push(MultiWriteTarget::SplatLocal(None)),
                    Some(expr) => {
                        if let Some(lvt) = expr.as_local_variable_target_node() {
                            targets.push(MultiWriteTarget::SplatLocal(
                                Some(cid_to_string(lvt.name())),
                            ));
                        } else if let Some(ivt) = expr.as_instance_variable_target_node() {
                            targets.push(MultiWriteTarget::SplatIvar(
                                cid_to_string(ivt.name()),
                            ));
                        } else {
                            AST_ERRORS.with(|cell| cell.borrow_mut().push(
                                format!("unsupported splat target: {:?}", expr)
                            ));
                        }
                    }
                }
            } else if rest.as_implicit_rest_node().is_some() {
                // `a, = arr` form — Prism uses ImplicitRestNode to
                // mark the trailing comma. Treat as anonymous splat.
                targets.push(MultiWriteTarget::SplatLocal(None));
            } else {
                AST_ERRORS.with(|cell| cell.borrow_mut().push(
                    format!("unsupported multi-write rest: {:?}", rest)
                ));
            }
        }
        for tgt in n.rights().iter() {
            push_positional(&mut targets, &tgt);
        }
        let value = tr(&n.value());
        return sp(node, Expr::MultiWrite {
            targets,
            value: Box::new(value),
        });
    }
    if let Some(n) = node.as_call_node() {
        let receiver = n.receiver().map(|r| Box::new(tr(&r)));
        let name = cid_to_string(n.name());
        // Detect single-splat call `foo(*arr)` — args is a
        // single SplatNode wrapping an Array-shaped expression.
        // Splat detection. Two paths:
        //   1. Single splat as the sole arg (`foo(*arr)`): use the
        //      existing `Expr::Apply` opcode — most efficient.
        //   2. Mixed splats (`foo(a, *b, c)`): synthesise an array
        //      literal with the same shape, then route through
        //      `Expr::Apply` against that constructed array. The
        //      array-literal-with-splat handler above translates
        //      this into a `+`-chain of Array#+ calls; the Apply
        //      op spreads the resulting Array as positional args.
        let arg_nodes: Vec<_> = n
            .arguments()
            .map(|a| a.arguments().iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if arg_nodes.len() == 1
            && let Some(sn) = arg_nodes[0].as_splat_node()
                && let Some(splat_expr) = sn.expression() {
                    return sp(node, Expr::Apply {
                        receiver,
                        name,
                        splat: Box::new(tr(&splat_expr)),
                    });
                }
        // Detect any splat anywhere in the args; if present and
        // multiple args exist, build a synthetic array expression
        // from the args (preserving order, splats interleaved) and
        // dispatch as a single-splat Apply.
        //
        // KeywordHashNode (the trailing `k: v, **opts` hash) is
        // handled by the args-walk below and stays a regular
        // positional arg (HashLit). For now we don't recombine
        // multiple KeywordHash nodes — only the standard trailing
        // form Prism emits.
        let has_splat = arg_nodes.iter().any(|c| c.as_splat_node().is_some());
        if has_splat {
            // Walk and group: build the array from the elements.
            let mut chunks: Vec<SExpr> = Vec::new();
            let mut buf: Vec<SExpr> = Vec::new();
            for c in &arg_nodes {
                let cn: &ruby_prism::Node<'_> = c;
                if let Some(sn) = cn.as_splat_node()
                    && let Some(inner) = sn.expression() {
                        if !buf.is_empty() {
                            chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                        }
                        chunks.push(tr(&inner));
                    } else if let Some(kh) = cn.as_keyword_hash_node() {
                    // Trailing kwarg-hash retains its sugar shape;
                    // **opts merges via tr_kwhash's `.merge` chain.
                    buf.push(tr_kwhash(node, cn, &kh));
                } else {
                    buf.push(tr(cn));
                }
            }
            if !buf.is_empty() {
                chunks.push(sp(node, Expr::ArrayLit(buf)));
            }
            let mut it = chunks.into_iter();
            let first = it.next().unwrap_or_else(|| sp(node, Expr::ArrayLit(vec![])));
            let acc = it.fold(first, |lhs, rhs| sp(node, Expr::Call {
                receiver: Some(Box::new(lhs)),
                name: "+".into(),
                args: vec![rhs],
            }));
            return sp(node, Expr::Apply {
                receiver,
                name,
                splat: Box::new(acc),
            });
        }
        // KeywordHashNode at the tail of an argument list — Prism
        // emits this for the `name: value, ...` sugar at call
        // sites. Translate to a HashLit so the callee receives
        // it as the trailing Hash arg; invoke_method splits
        // keyword bindings out of it. NB: only the trailing
        // position is conventional; CRuby allows interleaving
        // but flags it `1.9 hash` style. We accept either spot
        // but always normalize to a HashLit Expr.
        let args: Vec<SExpr> = arg_nodes.iter().map(|c| {
            if let Some(kh) = c.as_keyword_hash_node() {
                tr_kwhash(node, c, &kh)
            } else {
                tr(c)
            }
        }).collect();
        if let Some(bnode) = n.block() {
            if let Some(bn) = bnode.as_block_node() {
                // Block params. Each top-level param becomes a
                // `BlockParam`, recursively for nested destructures.
                // `RequiredParameterNode` → `Single(name)`;
                // `MultiTargetNode` → `Destructure(inner params)`
                // where each inner is itself parsed via the same
                // recursion. Supports `|a, (b, c)|`, `|((a, b), c)|`,
                // and deeper nestings.
                fn parse_one(n: &ruby_prism::Node<'_>) -> Option<BlockParam> {
                    if let Some(rp) = n.as_required_parameter_node() {
                        return Some(BlockParam::Single(cid_to_string(rp.name())));
                    }
                    if let Some(mt) = n.as_multi_target_node() {
                        let inners: Vec<BlockParam> = mt.lefts().iter()
                            .filter_map(|inner| parse_one(&inner))
                            .collect();
                        return Some(BlockParam::Destructure(inners));
                    }
                    None
                }
                let block_params: Vec<BlockParam> = bn.parameters()
                    .and_then(|pn| pn.as_block_parameters_node())
                    .and_then(|bp| bp.parameters())
                    .map(|p| {
                        let mut out: Vec<BlockParam> = p.requireds().iter()
                            .filter_map(|r| parse_one(&r))
                            .collect();
                        // `|*rest|` — Prism reports the rest param
                        // separately from requireds. Append as a
                        // Rest BlockParam; the compiler's prologue
                        // will gather overflow args here.
                        if let Some(rest) = p.rest()
                            && let Some(rp) = rest.as_rest_parameter_node() {
                                let name = rp.name().map(cid_to_string).unwrap_or_default();
                                out.push(BlockParam::Rest(name));
                            }
                        out
                    })
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
            // `&...` block argument. Two sub-cases:
            //   - `&:method` — symbol-to-proc. Synthesize a one-
            //     arg block `{ |__sp_x| __sp_x.method_name }`.
            //   - `&proc_value` — block-argument forwarding.
            //     Evaluate the expression to a Value::Block at
            //     runtime and pass it as the block.
            if let Some(ba) = bnode.as_block_argument_node()
                && let Some(expr) = ba.expression() {
                    if let Some(sn) = expr.as_symbol_node() {
                        let method_name: String = String::from_utf8_lossy(sn.unescaped()).into_owned();
                        let param_name = "__sp_x".to_string();
                        let body_call = sp(node, Expr::Call {
                            receiver: Some(Box::new(sp(node, Expr::LVarRead(param_name.clone())))),
                            name: method_name,
                            args: vec![],
                        });
                        return sp(node, Expr::CallWithBlock {
                            receiver, name, args,
                            block_params: vec![BlockParam::Single(param_name)],
                            block_body: vec![body_call],
                        });
                    }
                    // Fall-through: any other expression becomes
                    // the block arg via CallWithBlockArg. CRuby
                    // requires the value to respond to `to_proc` —
                    // for our subset we only accept Value::Block
                    // directly (no implicit coercion).
                    let block_arg = tr(&expr);
                    return sp(node, Expr::CallWithBlockArg {
                        receiver, name, args, block_arg: Box::new(block_arg),
                    });
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
    // `defined?(expr)` — returns a string describing the kind
    // of `expr`, or nil if it's not defined. Resolved at AST
    // translation: literals collapse to a static string ("expr",
    // "true", "false", "nil"); local-variable references are
    // "local-variable" by parse-time guarantee (Prism only emits
    // LocalVariableReadNode when a local is in scope); ivars,
    // methods (zero-arg, no-receiver Calls), and constants
    // resolve through Kernel `__defined_ivar?` / `__defined_method?`
    // / `__defined_const?` builtins so the check happens at
    // runtime against `self` / class table / methods.
    if let Some(n) = node.as_defined_node() {
        let inner = n.value();
        let span = node_span(node);
        let s = |label: &str| -> SExpr { sp(node, Expr::StrLit(label.into())) };
        let to_nil = sp(node, Expr::Nil);
        let _ = to_nil; // suppress unused; kept for shape symmetry
        if inner.as_integer_node().is_some()
            || inner.as_float_node().is_some()
            || inner.as_string_node().is_some()
            || inner.as_symbol_node().is_some()
            || inner.as_interpolated_string_node().is_some()
            || inner.as_array_node().is_some()
            || inner.as_hash_node().is_some()
            || inner.as_range_node().is_some()
            || inner.as_regular_expression_node().is_some()
            || inner.as_lambda_node().is_some()
        {
            return s("expression");
        }
        if inner.as_true_node().is_some() { return s("true"); }
        if inner.as_false_node().is_some() { return s("false"); }
        if inner.as_nil_node().is_some() { return s("nil"); }
        if inner.as_self_node().is_some() { return s("self"); }
        if inner.as_local_variable_read_node().is_some() {
            return s("local-variable");
        }
        if let Some(iv) = inner.as_instance_variable_read_node() {
            let name = cid_to_string(iv.name());
            return Spanned::new(span, Expr::Call {
                receiver: None,
                name: "__defined_ivar?".into(),
                args: vec![sp(node, Expr::SymbolLit(name))],
            });
        }
        if let Some(cr) = inner.as_constant_read_node() {
            let name = cid_to_string(cr.name());
            return Spanned::new(span, Expr::Call {
                receiver: None,
                name: "__defined_const?".into(),
                args: vec![sp(node, Expr::SymbolLit(name))],
            });
        }
        if let Some(cn) = inner.as_call_node() {
            // No-receiver, no-args call → runtime method check on
            // self / toplevel / builtin. With a receiver, CRuby
            // would dispatch on the receiver's class; we can't
            // do that without evaluating the receiver (which has
            // its own side-effect concerns). Pragmatic
            // approximation: literal-arithmetic shapes (`1 + 2`)
            // and any explicit-receiver call return "method"
            // optimistically. Documented divergence from CRuby
            // for receivers that genuinely lack the method.
            if cn.receiver().is_none() {
                let name = cid_to_string(cn.name());
                return Spanned::new(span, Expr::Call {
                    receiver: None,
                    name: "__defined_method?".into(),
                    args: vec![sp(node, Expr::SymbolLit(name))],
                });
            }
            return s("method");
        }
        return s("expression");
    }
    if let Some(n) = node.as_lambda_node() {
        // `->(x, *rest) { body }` — same param shape as block
        // literals: requireds + optional rest. Lambda body is
        // a `Vec<SExpr>` evaluated in the block proto.
        let params: Vec<BlockParam> = n.parameters()
            .and_then(|pn| pn.as_block_parameters_node())
            .and_then(|bp| bp.parameters())
            .map(|p| {
                let mut out: Vec<BlockParam> = p.requireds().iter()
                    .filter_map(|r| r.as_required_parameter_node()
                        .map(|rp| BlockParam::Single(cid_to_string(rp.name()))))
                    .collect();
                if let Some(rest) = p.rest()
                    && let Some(rp) = rest.as_rest_parameter_node() {
                        let name = rp.name().map(cid_to_string).unwrap_or_default();
                        out.push(BlockParam::Rest(name));
                    }
                out
            })
            .unwrap_or_default();
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Lambda { params, body });
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
    if node.as_forwarding_super_node().is_some() {
        // Bare `super` — forwards all of the enclosing method's
        // args. The arg list is filled in at compile time by
        // emitting LoadLocal for each param slot, so the AST
        // just stores `None` here.
        return sp(node, Expr::Super(None));
    }
    if let Some(n) = node.as_super_node() {
        let args: Vec<SExpr> = n.arguments()
            .map(|args| args.arguments().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        return sp(node, Expr::Super(Some(args)));
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
        // `begin … end while cond` — Prism marks this with the
        // `begin_modifier` flag. Body runs once before the first
        // cond check, matching CRuby semantics.
        return sp(node, Expr::While { cond, body, post: n.is_begin_modifier() });
    }
    // `unless cond; then; else else; end` and modifier
    // `expr unless cond` both desugar to `if cond; else_body;
    // else then_body; end` — swap the branches. The modifier
    // form has no else clause; the swap leaves an empty
    // else (CRuby's behaviour: result is `nil` when the
    // unless block doesn't run).
    // `X rescue Y` modifier — semantically `begin; X; rescue
    // StandardError; Y; end`. CRuby's bare-rescue-modifier
    // contract: only StandardError (and its subclasses) is caught,
    // not Exception. Translate to a Begin with one anonymous
    // RescueClause (empty `classes` list, which our Begin compiler
    // already treats as "filter on StandardError").
    if let Some(n) = node.as_rescue_modifier_node() {
        let body = vec![tr(&n.expression())];
        let rescue = vec![RescueClause {
            classes: vec![],
            body: vec![tr(&n.rescue_expression())],
            var: None,
        }];
        return sp(node, Expr::Begin { body, rescue, ensure: None });
    }
    // `case x; when a, b; body1; when c; body2; else body3; end`
    // desugars to nested if/elsif using `===`:
    //   if a === x || b === x then body1
    //   elsif c === x then body2
    //   else body3
    //   end
    // Without a predicate (`case; when cond; ...; end`) each
    // condition is evaluated as a plain boolean (no === call).
    // The predicate is re-evaluated per condition, which is fine
    // for side-effect-free predicates (the common case).
    if let Some(n) = node.as_case_node() {
        let predicate = n.predicate().map(|p| tr(&p));
        let conditions: Vec<_> = n.conditions().iter().collect();
        let else_body: Vec<SExpr> = match n.else_clause() {
            Some(en) => en.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                .unwrap_or_default(),
            None => vec![],
        };
        // Build the chain from the inside out so the last `when`
        // wraps the else, the one before it wraps that, and so on.
        let mut acc: Vec<SExpr> = else_body;
        for cond_node in conditions.iter().rev() {
            let when = match cond_node.as_when_node() {
                Some(w) => w,
                None => continue,
            };
            // Per-condition, with a flag noting whether the
            // condition is a splat. Splats `when *arr` translate
            // to `arr.any? { |__sp_v| __sp_v === predicate }` —
            // already a boolean against the predicate, so the
            // === wrap below must be skipped for them. Non-
            // splat conditions follow the standard
            // `<wc> === predicate` path.
            //
            // No-predicate case forms (`case; when *arr ...`)
            // collapse the body to a bare `arr.any?` truthy
            // check on elements.
            let when_conditions: Vec<(SExpr, bool /* is_splat */)> = when.conditions()
                .iter()
                .map(|c| {
                    let cn: &Node<'_> = &c;
                    if let Some(sn) = cn.as_splat_node()
                        && let Some(inner) = sn.expression() {
                            let arr = tr(&inner);
                            let sp_name = "__sp_v".to_string();
                            let body_expr = match &predicate {
                                Some(pred) => sp(cn, Expr::Call {
                                    receiver: Some(Box::new(sp(cn, Expr::LVarRead(sp_name.clone())))),
                                    name: "===".into(),
                                    args: vec![pred.clone()],
                                }),
                                None => sp(cn, Expr::LVarRead(sp_name.clone())),
                            };
                            return (sp(cn, Expr::CallWithBlock {
                                receiver: Some(Box::new(arr)),
                                name: "any?".into(),
                                args: vec![],
                                block_params: vec![BlockParam::Single(sp_name)],
                                block_body: vec![body_expr],
                            }), true);
                        }
                    (tr(cn), false)
                })
                .collect();
            let when_body: Vec<SExpr> = when.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                .unwrap_or_default();
            // Combine multiple `when a, b, c` conditions with
            // short-circuit `||`. Each `expr` becomes
            // `expr === predicate` when there's a predicate.
            let mut cond_expr: Option<SExpr> = None;
            for (wc, is_splat) in when_conditions {
                let one = if is_splat {
                    // Splat-derived `any?` block already
                    // encodes the predicate-check internally;
                    // wrapping it in `=== predicate` would
                    // double-apply (the outer call would
                    // compare a Bool against predicate).
                    wc
                } else {
                    match &predicate {
                        Some(pred) => sp(node, Expr::Call {
                            receiver: Some(Box::new(wc)),
                            name: "===".into(),
                            args: vec![pred.clone()],
                        }),
                        None => wc,
                    }
                };
                cond_expr = Some(match cond_expr {
                    None => one,
                    Some(prev) => sp(node, Expr::Or(Box::new(prev), Box::new(one))),
                });
            }
            let cond_expr = cond_expr.unwrap_or_else(|| sp(node, Expr::LVarRead("nil".into())));
            let if_node = sp(node, Expr::If {
                cond: Box::new(cond_expr),
                then_body: when_body,
                else_body: acc,
            });
            acc = vec![if_node];
        }
        // If the chain is empty (no when clauses at all), just
        // produce nil. Otherwise the single accumulated If is
        // the result.
        if acc.is_empty() {
            return sp(node, Expr::LVarRead("nil".into()));
        }
        return acc.into_iter().next().unwrap();
    }
    if let Some(n) = node.as_unless_node() {
        let cond = Box::new(tr(&n.predicate()));
        let then_body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        let else_body: Vec<SExpr> = match n.else_clause() {
            Some(en) => en.statements()
                .map(|s| s.body().iter().map(|c| tr(&c)).collect())
                .unwrap_or_default(),
            None => vec![],
        };
        // Swap: if cond runs `else_body`, else runs `then_body`.
        return sp(node, Expr::If { cond, then_body: else_body, else_body: then_body });
    }
    // `until cond; body; end` and modifier `expr until cond`
    // desugar to `while !cond; body; end`. We synthesise the
    // negation as a Call to `!` on the original cond — the
    // Unary-Bang primitive arm handles all value types.
    if let Some(n) = node.as_until_node() {
        let raw_cond = tr(&n.predicate());
        let cond = Box::new(sp(node, Expr::Call {
            receiver: Some(Box::new(raw_cond)),
            name: "!".into(),
            args: vec![],
        }));
        let body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(&c)).collect())
            .unwrap_or_default();
        // `begin … end until cond` — same begin-modifier flag.
        // Translates to a negated-cond do-while via the post flag.
        return sp(node, Expr::While { cond, body, post: n.is_begin_modifier() });
    }
    if let Some(n) = node.as_def_node() {
        let name = cid_to_string(n.name());
        let mut params: Vec<String> = Vec::new();
        let mut defaults: Vec<Option<SExpr>> = Vec::new();
        let mut rest: Option<String> = None;
        let mut kw_params: Vec<(String, Option<SExpr>)> = Vec::new();
        let mut kw_rest: Option<String> = None;
        let mut block_param: Option<String> = None;
        if let Some(p) = n.parameters() {
            if let Some(b) = p.block() {
                // `def foo(&blk)`: capture the caller's block into
                // the named slot. Anonymous form `def foo(&)` would
                // have `b.name() == None`; CRuby uses it for
                // forward-the-block-only, which we don't model yet
                // — treat as no-name (skip the bind). Prism returns
                // `BlockParameterNode` directly from `p.block()`
                // (it's an alternation node, not a generic Node);
                // no `as_*_node` cast needed.
                block_param = b.name().map(cid_to_string);
            }
            if let Some(r) = p.rest()
                && let Some(rp) = r.as_rest_parameter_node() {
                    rest = rp.name().map(|n| cid_to_string(n));
                }
            if let Some(r) = p.keyword_rest()
                && let Some(kr) = r.as_keyword_rest_parameter_node() {
                    kw_rest = Some(kr.name().map(cid_to_string).unwrap_or_default());
                }
            for kw in p.keywords().iter() {
                if let Some(rk) = kw.as_required_keyword_parameter_node() {
                    kw_params.push((cid_to_string(rk.name()), None));
                } else if let Some(ok) = kw.as_optional_keyword_parameter_node() {
                    let name = cid_to_string(ok.name());
                    let val = tr(&ok.value());
                    // Same literal-only restriction as positional
                    // defaults: anything else needs a per-callsite
                    // prologue we don't generate. Surface as a
                    // SyntaxError via AST_ERRORS.
                    match &val.node {
                        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::SymbolLit(_)
                        | Expr::BoolLit(_) | Expr::Nil => {
                            kw_params.push((name, Some(val)));
                        }
                        _ => {
                            AST_ERRORS.with(|cell| cell.borrow_mut().push(
                                format!("default value for keyword parameter `{}` must be a literal", name)
                            ));
                            kw_params.push((name, Some(sp(&kw, Expr::Nil))));
                        }
                    }
                }
            }
            for r in p.requireds().iter() {
                if let Some(rp) = r.as_required_parameter_node() {
                    params.push(cid_to_string(rp.name()));
                    defaults.push(None);
                }
            }
            for o in p.optionals().iter() {
                if let Some(op) = o.as_optional_parameter_node() {
                    params.push(cid_to_string(op.name()));
                    // Any expression is allowed as a positional
                    // default — the compiler emits a per-optional
                    // entry prologue (`Op::JumpIfArgGiven(slot, skip)
                    // + <expr> + Op::StoreLocal(slot)`) that runs
                    // before the body, so the default can reference
                    // earlier params, call methods, look up
                    // constants, etc.
                    defaults.push(Some(tr(&op.value())));
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
        // `def receiver.name; ...; end` — Prism reports the
        // receiver expression on DefNode when there is one.
        // Box the full expression rather than collapsing to a
        // bool: the compiler distinguishes `self` (class-body
        // class-level singleton — master `844530f`'s path) from
        // any other expression (instance-level singleton on a
        // Value::Object) at compile time.
        let receiver = n.receiver().map(|r| Box::new(tr(&r)));
        return sp(node, Expr::Def { name, params, defaults, rest, kw_params, kw_rest, block_param, receiver, body });
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
        // Detect splats in the array literal: `[a, *b, c]`. When
        // present, synthesise `[a] + b + [c]` via chained Array#+
        // calls — no new opcode needed, since Array#+ is already
        // a primitive. Splats in array literals are the building
        // block for splat-in-call-args (K3 below).
        let raw_elems: Vec<_> = n.elements().iter().collect();
        let has_splat = raw_elems.iter().any(|e| e.as_splat_node().is_some());
        if !has_splat {
            let elems: Vec<SExpr> = raw_elems.iter().map(|e| tr(e)).collect();
            return sp(node, Expr::ArrayLit(elems));
        }
        // Walk the elements building (group of consecutive non-splats
        // → ArrayLit, splat → bare expression). Chain all results
        // with `+`. The first chunk becomes the receiver; subsequent
        // chunks are args to `+`.
        let mut chunks: Vec<SExpr> = Vec::new();
        let mut buf: Vec<SExpr> = Vec::new();
        for e in &raw_elems {
            let en: &ruby_prism::Node<'_> = e;
            if let Some(sn) = en.as_splat_node()
                && let Some(inner) = sn.expression() {
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                    }
                    chunks.push(tr(&inner));
                } else {
                buf.push(tr(en));
            }
        }
        if !buf.is_empty() {
            chunks.push(sp(node, Expr::ArrayLit(buf)));
        }
        // Reduce left: chunk0 + chunk1 + chunk2 + ...
        let mut it = chunks.into_iter();
        let first = it.next().unwrap_or_else(|| sp(node, Expr::ArrayLit(vec![])));
        let acc = it.fold(first, |lhs, rhs| sp(node, Expr::Call {
            receiver: Some(Box::new(lhs)),
            name: "+".into(),
            args: vec![rhs],
        }));
        return acc;
    }
    if let Some(n) = node.as_hash_node() {
        // Detect `**splat` inside the literal. Without one we
        // take the fast path; with one we route through the
        // same `.merge` chain shape as kwarg-hash call args.
        let has_splat = n.elements().iter().any(|e| e.as_assoc_splat_node().is_some());
        if !has_splat {
            let pairs: Vec<(SExpr, SExpr)> = n.elements().iter().filter_map(|e| {
                e.as_assoc_node().map(|a| (tr(&a.key()), tr(&a.value())))
            }).collect();
            return sp(node, Expr::HashLit(pairs));
        }
        let mut chunks: Vec<SExpr> = Vec::new();
        let mut buf: Vec<(SExpr, SExpr)> = Vec::new();
        for el in n.elements().iter() {
            if let Some(an) = el.as_assoc_node() {
                buf.push((tr(&an.key()), tr(&an.value())));
            } else if let Some(spn) = el.as_assoc_splat_node()
                && let Some(inner) = spn.value() {
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::HashLit(std::mem::take(&mut buf))));
                    }
                    chunks.push(tr(&inner));
                }
        }
        if !buf.is_empty() {
            chunks.push(sp(node, Expr::HashLit(buf)));
        }
        let mut it = chunks.into_iter();
        let first = it.next().unwrap_or_else(|| sp(node, Expr::HashLit(vec![])));
        return it.fold(first, |lhs, rhs| sp(node, Expr::Call {
            receiver: Some(Box::new(lhs)),
            name: "merge".into(),
            args: vec![rhs],
        }));
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
    // `module Foo; ... end` — our subset doesn't distinguish
    // modules from classes (Comparable was already a stub class
    // in the preamble). Reusing Expr::Class lets `include`,
    // method definitions, and constant lookups inside the
    // module body all work via the existing class machinery.
    // What's missing vs CRuby: `Module#instance_methods`
    // introspection, and the strict "can't `.new` a module"
    // check. Acceptable for the subset.
    if let Some(n) = node.as_module_node() {
        let name = if let Some(cr) = n.constant_path().as_constant_read_node() {
            cid_to_string(cr.name())
        } else { "?".to_string() };
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(&c)).collect()
                } else { vec![tr(&b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Class { name, superclass: None, body });
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

#[cfg(test)]
mod tests {
    use super::*;
    use ruby_prism::parse;

    /// Drive `tr_with_errors` over a source string. Returns the
    /// (SExpr, errors) pair; we want both directions tested:
    /// supported sources should produce no errors and a non-Nil
    /// root, unsupported ones should accumulate messages without
    /// panicking.
    fn translate(src: &str) -> (SExpr, Vec<String>) {
        let result = parse(src.as_bytes());
        tr_with_errors(&result.node())
    }

    #[test]
    fn supported_source_produces_no_errors() {
        let (root, errs) = translate("puts 1 + 2");
        assert!(errs.is_empty(), "expected no AST errors, got: {errs:?}");
        // Root is a non-Nil program — the puts call lives inside
        // the program-node wrapping.
        assert!(!matches!(root.node, Expr::Nil));
    }

    #[test]
    fn defined_keyword_supported() {
        // `defined?` is a supported node — sanity-check that the
        // supported path round-trips without false positives.
        let (_, errs) = translate("defined?(x)");
        assert!(errs.is_empty(), "defined? should be supported, got: {errs:?}");
    }

    #[test]
    fn ast_errors_collected_for_unsupported_node() {
        // `BEGIN { ... }` (pre-execution block) is outside the
        // subset. The translator should collect a message instead
        // of panicking.
        let (_, errs) = translate("BEGIN { puts 1 }");
        assert!(!errs.is_empty(), "BEGIN should produce AST errors");
        assert!(
            errs.iter().any(|e| e.contains("unsupported")),
            "expected 'unsupported' wording, got: {errs:?}"
        );
    }

    #[test]
    fn ast_errors_buffer_resets_between_calls() {
        // First call has unsupported nodes — leaves errors in the
        // buffer (which tr_with_errors drains on the way out).
        let (_, e1) = translate("BEGIN { puts 1 }");
        assert!(!e1.is_empty());
        // Second call on supported source must see an empty buffer
        // — proves drain works.
        let (_, e2) = translate("puts 1");
        assert!(e2.is_empty(), "buffer leaked between calls: {e2:?}");
    }

    #[test]
    fn empty_source_produces_no_errors() {
        let (_, errs) = translate("");
        assert!(errs.is_empty());
    }

    #[test]
    fn whitespace_only_source_produces_no_errors() {
        let (_, errs) = translate("   \n\t  ");
        assert!(errs.is_empty());
    }

    #[test]
    fn comment_only_source_produces_no_errors() {
        let (_, errs) = translate("# just a comment\n");
        assert!(errs.is_empty());
    }
}

