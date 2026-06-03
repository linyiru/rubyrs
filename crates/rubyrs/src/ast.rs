use ruby_prism::Node;

use crate::error::Span;

/// Per-translation context threaded through `tr` and helpers.
///
/// Holds the two pieces of state that the AST→SExpr translation
/// pass needs across recursion:
///   - `errors`: an accumulating Vec of unsupported-node /
///     SyntaxError-shape messages. `tr` and friends `push` here
///     instead of panicking; the public `tr_with_errors` /
///     `tr_with_errors_on_source` entry points drain it into
///     their return tuple.
///   - `source`: optional source bytes for the in-flight
///     translation. `Some(_)` when the caller is
///     `tr_with_errors_on_source`; `None` for test harnesses
///     that compile snippets without source access (line
///     numbers degrade to 0 — same stub value the previous
///     no-SourceGuard branch returned).
///
/// Replaces the previous `AST_ERRORS` / `AST_SOURCE` thread-
/// locals + `SourceGuard` RAII. Threading the context
/// explicitly makes the data flow visible at call sites (no
/// more "where does this error pop out of"); it also lets
/// future multi-threaded compilation work without TLS reset
/// gymnastics, and lets test harnesses build an isolated ctx
/// per case without stale-slot risk.
pub(crate) struct TranslationCtx<'src> {
    pub(crate) errors: Vec<String>,
    pub(crate) source: Option<&'src [u8]>,
}

impl<'src> TranslationCtx<'src> {
    pub(crate) fn new(source: Option<&'src [u8]>) -> Self {
        Self {
            errors: Vec::new(),
            source,
        }
    }

    /// Number of `\n` bytes in the source prefix ending at
    /// `loc_start_ptr` — i.e., the 1-based line number of the
    /// position the Prism Location's start pointer points at.
    /// Returns 0 when `source` is None (callers that invoke
    /// `tr` without `tr_with_errors_on_source`) — same stub
    /// value the previous SourceLine implementation returned.
    #[inline]
    pub(crate) fn line_of(&self, loc_start_ptr: *const u8) -> i64 {
        let source = match self.source {
            Some(s) => s,
            None => return 0,
        };
        // SAFETY: `loc_start_ptr` came from Prism's parse of the
        // same source `self.source` borrows, so it lies within
        // [source.as_ptr(), source.as_ptr() + source.len()].
        // Pointer arithmetic stays within the allocation.
        let offset = unsafe { loc_start_ptr.offset_from(source.as_ptr()) };
        if offset < 0 || (offset as usize) > source.len() {
            return 0;
        }
        let prefix = unsafe { std::slice::from_raw_parts(source.as_ptr(), offset as usize) };
        // 1-based line numbers: count newlines + 1.
        (prefix.iter().filter(|&&b| b == b'\n').count() + 1) as i64
    }
}

/// Translate a Prism root node, returning the SExpr plus any
/// unsupported-node messages collected along the way. Empty `errs`
/// means the whole tree was within the supported subset. If `errs`
/// is non-empty the returned SExpr may contain `Expr::Nil`
/// placeholders where translation failed — don't compile it.
///
/// The no-source variant is `#[cfg(test)]` only — every
/// non-test caller (`Runtime::eval`, `load_ruby_source_from_canon`)
/// has source bytes on hand and uses `tr_with_errors_on_source`
/// to get real line numbers. Keeping this variant for the test
/// harness avoids forcing every snippet test to pass a dummy
/// `b""` source.
#[cfg(test)]
pub(crate) fn tr_with_errors(node: &Node<'_>) -> (SExpr, Vec<String>) {
    let mut ctx = TranslationCtx::new(None);
    let prog = tr(&mut ctx, node);
    (prog, ctx.errors)
}

/// Same as `tr_with_errors` but also threads the source bytes
/// through `ctx.source` so `Expr::SourceLine` resolves real
/// line numbers. Callers that have the source on hand
/// (`Runtime::eval`, `kernel::load_ruby_source_from_canon`)
/// use this variant. The non-source variant is kept for
/// test harnesses that compile snippets without source
/// access.
pub(crate) fn tr_with_errors_on_source(node: &Node<'_>, source: &[u8]) -> (SExpr, Vec<String>) {
    let mut ctx = TranslationCtx::new(Some(source));
    let prog = tr(&mut ctx, node);
    (prog, ctx.errors)
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
    /// String literal whose Prism-unescaped bytes aren't valid
    /// UTF-8. Holds the raw bytes so high-byte escapes
    /// (`"\xFF\xFF"`) survive lossless — the previous
    /// `from_utf8_lossy` route substituted invalid sequences with
    /// U+FFFD, expanding each `\xFF` to 3 bytes and breaking
    /// binary-protocol use. The valid-UTF-8 path still uses
    /// `StrLit(String)`.
    StrLitBytes(Vec<u8>),
    /// `/pattern/` literal — Ruby regular expression. Source is
    /// kept as a String for interning; compilation happens at the
    /// VM layer (with caching). Cfg-gated on the `regex` feature
    /// (ADR 0017 Rule 3) — when the feature is off, AST
    /// translation emits a clear "regex feature not enabled"
    /// error instead of producing this variant.
    #[cfg(feature = "regex")]
    RegexLit(String),
    SymbolLit(String),
    /// Integer literal that overflows `i64`. The string is the
    /// canonical decimal representation built from Prism's
    /// arbitrary-precision integer digits. The compiler interns
    /// the string and emits `Op::LoadBigInt`; the runtime parses
    /// then caches it. Cfg-gated on `bignum` — without the
    /// feature the AST arm saturates to `i64::MIN` / `i64::MAX`
    /// instead. ADR 0018 covers the BigInt placement.
    #[cfg(feature = "bignum")]
    BigIntLit(String),
    /// Rational literal — `1/2r`, `0.5r`, `1000.0r`. Stored as a
    /// canonical-form (signed) num / (positive) den decimal-string
    /// pair so the compiler can intern + cache + emit through the
    /// same SymId pipeline used by `BigIntLit`. Phase C.4.4 wires
    /// this to a real `Value::Rational` at load time (replacing the
    /// pre-C.4.4 lowering to `FloatLit(num / den)`).
    RationalLit { num: String, den: String },
    InterpolatedStr(Vec<SExpr>),
    /// `/pre #{x} post/` — interpolated regex literal. Lowered
    /// like `InterpolatedStr` (concat all parts into a String via
    /// `to_s` + `+`), then handed to `Op::CompileRegex`. The
    /// interpolated parts are *re-evaluated* on every reach (the
    /// content can change per call), but pattern compilation hits
    /// `Vm::regex_cache` keyed by the assembled pattern's SymId,
    /// so identical expansions only compile once. Cfg-gated on the
    /// `regex` feature alongside `RegexLit`.
    #[cfg(feature = "regex")]
    InterpolatedRegex(Vec<SExpr>),
    BoolLit(bool),
    Nil,
    LVarRead(String),
    LVarWrite(String, Box<SExpr>),
    IVarRead(String),
    IVarWrite(String, Box<SExpr>),
    /// `$foo` global-variable read. Name includes the leading `$`.
    /// Unknown user globals resolve to Nil at runtime (CRuby
    /// semantics — uninitialized global is silently nil); a small
    /// set of "special globals" (`$$` for pid, `$0` for script
    /// name) is intercepted by `Op::LoadGlobal`.
    GVarRead(String),
    /// `$foo = expr` global-variable write. Stores into
    /// `Vm.globals` keyed by the interned name (including `$`).
    /// Spike scope: only the plain-name form; the special
    /// globals' writes (`$~ = nil`, `$, = "|"`) are out of scope
    /// and silently store into the same table — observable as
    /// "set" but not honoured by any builtin's behaviour.
    GVarWrite(String, Box<SExpr>),
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
    /// Silent-nil variant of `ConstRead` — emitted ONLY for the
    /// `||=` read position (`FOO ||= default` / `Foo::Bar ||=
    /// default`). CRuby special-cases `||=` so the read returns
    /// nil rather than raising NameError on an undefined LHS,
    /// which is what makes the lazy-init idiom work. Every other
    /// op-write (`&&=`, `+=`, ...) uses strict `ConstRead` and
    /// raises NameError before the operator runs. Emits
    /// `Op::LoadConstOrNil`.
    ConstReadOrNil(String),
    /// Constant write — covers both the bare `FOO = expr`
    /// (ConstantWriteNode) and the path form `Foo::Bar = expr`
    /// (ConstantPathWriteNode). Both flatten into a single
    /// "A::B::C"-joined name and store into the same
    /// `Vm.constants` table (rubyrs has no real module nesting
    /// yet — the path form's segment-validation divergences from
    /// CRuby are noted at the ConstantPathWriteNode translation
    /// site below).
    /// Fields: name, absolute, value. `absolute` is true only for
    /// writes through a leading-`::` constant path (`::X = 1`,
    /// `::Foo::Bar = 2`); the compiler skips its lexical
    /// class_path alias for those so they stay at top-level only.
    /// Bare-name writes and relative-path writes pass false.
    ConstWrite(String, bool, Box<SExpr>),
    /// `__FILE__` — the current source file's path. Resolved at
    /// compile time to a string literal of the surrounding
    /// proto's `filename` (the loader / `Runtime::eval` sets
    /// that to whatever the host passed). Lets vendored Ruby
    /// helpers do `$LOAD_PATH.unshift __dir__` without needing
    /// AST-level filename plumbing.
    SourceFile,
    /// `__LINE__` — source line of the literal. Captured at AST
    /// translation from the Prism node's location.
    SourceLine(i64),
    /// `@@name` — class variable read. Looks up `name` in the
    /// surrounding class's `class_vars` table at runtime; missing
    /// names return `nil` (CRuby raises NameError, but lenient
    /// default matches our ivar / global behaviour and avoids
    /// breaking gem-shim probes). Compiles to `Op::LoadCvar`.
    CvarRead(String),
    /// `@@name = expr` — class variable write. Stores into the
    /// surrounding class's `class_vars` table. Tier 1 doesn't
    /// walk the class hierarchy (`@@foo` on a subclass is
    /// independent of parent's). Documented divergence; the
    /// mainstream "cache a default instance" use cases
    /// (Sinatra `@@eats_errors`) stay on a single class.
    /// Compiles to `Op::StoreCvar`.
    CvarWrite(String, Box<SExpr>),
    Call {
        receiver: Option<Box<SExpr>>,
        name: String,
        args: Vec<SExpr>,
        /// `true` when the final entry in `args` originated from a
        /// `KeywordHashNode` (CRuby's `foo(k: v, ...)` sugar) vs.
        /// from an explicit positional `HashLit` (`foo({k: v})`).
        /// Survives to bytecode via the `Op::CallKw*` variants so
        /// the dispatcher can split the trailing Hash into a
        /// dedicated kwargs channel for `primitive_call` / kw_param
        /// binding. AST consumers that synthesise `Call` nodes
        /// (operator desugaring, attr-write rewriting, etc.) should
        /// default to `false` — only the call-site argument walker
        /// in ast.rs sets it `true`.
        kwargs_trailing: bool,
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
        /// M27 A4: count of required positional params that come
        /// AFTER the rest splat (`def mid(a, *b, c, d)` → 2).
        /// Appended to `params` after the optionals; CRuby grammar
        /// requires them only when `rest` is `Some`. Plumbed to
        /// the binder so the trailing args go to the post slots
        /// before the rest gathers the middle.
        n_required_post: u16,
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
        /// `true` when the AST node was a `module X; end` rather
        /// than `class X; end`. Drives `Class.is_module` at
        /// runtime so `is_a?(Class)` / `is_a?(Module)` /
        /// `class_of` distinguish the two shapes. `module` and
        /// `class` are still translated to the same Expr
        /// variant because their body-compilation steps are
        /// identical — only the constructed Class struct's
        /// flag differs.
        is_module: bool,
    },
    /// `alias new old` keyword form encountered INSIDE a
    /// `class << X` body. Compiles to `Op::AliasSingletonMethod`
    /// so the alias lands in the surrounding class's
    /// singleton_methods, not its instance methods.
    /// (Top-level / normal-class-body `alias` translates to
    /// `Expr::Call(alias_method)` which routes through the
    /// existing intercept emitting `Op::AliasMethod`.)
    AliasSingletonMethod(String, String),
    /// `prepend Mod` encountered INSIDE a `class << self` body
    /// (only the self-receiver case — `class << OtherConst;
    /// prepend M; end` still surfaces a SyntaxError). Compiles
    /// to `Op::SingletonChainPrepend` which pushes the popped
    /// module onto `class_stack.last().singleton_prepends`.
    ///
    /// Motivating case: tilt.rb's `finalize!` does
    /// `class << self; prepend(Module.new { ... }); end` to
    /// install an after-freeze guard layer in front of the
    /// class's own singleton methods.
    SingletonChainPrepend(Box<SExpr>),
    /// Push a new `Visibility::Public` entry onto the runtime
    /// class_visibility_stack. Emitted by the SingletonClassNode
    /// translator at body start for EVERY `class << <expr>`
    /// shape (receiver-independent — `class << self`,
    /// `class << obj`, `class << Const`) so bare `private` /
    /// `public` / `protected` mutations inside don't leak into
    /// the enclosing class body. Pairs with `PopClassVisibility`.
    PushClassVisibilityPublic,
    /// Pop one entry from class_visibility_stack. Pair with
    /// `PushClassVisibilityPublic` at the boundary of a
    /// `class << <expr>` body — emitted in the body's
    /// `Begin { ensure: [...] }` so the pop runs on both
    /// normal exit and exception unwind.
    PopClassVisibility,
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
    /// `retry` — re-executes the surrounding `begin` block from
    /// the start, re-evaluating the rescue clauses. Only legal
    /// inside a `rescue` clause body. CRuby raises SyntaxError
    /// at parse time when used outside; rubyrs catches the
    /// out-of-context case at compile time and emits a runtime
    /// raise (RuntimeError) instead — a Tier-1 divergence on the
    /// error class for an error-only path. (TRY_RUNS pass-10
    /// layer #9 — rackup-2.2.1/lib/rackup/server.rb:439 uses
    /// the canonical retry-on-EADDRINUSE pattern.)
    Retry,
    /// `super` (forwarding all of the enclosing method's args)
    /// or `super(arg1, arg2)` (explicit args). `super()` with
    /// empty parens passes no args and is `Some(vec![])`;
    /// bare `super` is `None`.
    Super(Option<Vec<SExpr>>),
    /// `super(*args)` / `super(a, *rest, b)` — splat in the
    /// super argument list. The inner SExpr evaluates to an
    /// Array containing the fully-assembled call args (the
    /// same shape `Expr::Apply` uses for regular splat-call
    /// dispatch). Compiles to `Op::ApplySuper(name_id)`.
    /// Rack `lib/rack/headers.rb`'s `super(*a.map!{...})`
    /// shape surfaces this; previously raised
    /// `unsupported node: SplatNode` at AST translation.
    SuperApply(Box<SExpr>),
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
    /// M27 A1: `|&blk|` named block parameter — captures the
    /// caller's block as a `Value::Block` (or `Nil` when none was
    /// passed). The compiler reserves a slot and sets
    /// `proto.block_param`, so the existing method dispatch path's
    /// trailing-slot binder (`invoke_method_with_block`) populates
    /// it automatically when the block is installed AS A METHOD
    /// via `define_method` and that method is later called with a
    /// block. For ordinary block invocation (each, map, etc. — no
    /// caller block) the slot stays `Nil`. Matches the CRuby idiom
    /// `define_method(:foo) do |arg, &blk| blk.call(arg) end` that
    /// Sinatra's route table uses heavily.
    BlockArg(String),
}

#[derive(Debug, Clone)]
pub(crate) enum MultiWriteTarget {
    Local(String),
    Ivar(String),
    /// `$foo` on the LHS of a multi-write. Threaded through to
    /// `Op::StoreGlobal` so e.g. `verbose, $VERBOSE = $VERBOSE,
    /// nil` (rackup.rb:13 — the "silence Ruby 3.4 deprecation
    /// warning" idiom) compiles. (TRY_RUNS pass-10 layer #8.)
    Global(String),
    /// `*rest` — receives a fresh Array of the middle slice.
    /// `None` is the anonymous form `*` which discards the slice
    /// but still anchors the post-splat counting.
    SplatLocal(Option<String>),
    /// `*@rest` — splat into an ivar. Same slicing as SplatLocal.
    SplatIvar(String),
    /// `*$rest` — splat into a global. Same slicing as
    /// SplatLocal. Added in code-review #301 for symmetry with
    /// the positional `Global` variant; pre-fix `*$g = …` still
    /// hit the legacy "unsupported splat target" error path.
    SplatGlobal(String),
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

/// Decode an `attr_*` method name into `(do_reader, do_writer)`
/// flags. Returns `None` for any other name. Shared by the
/// compiler's class-body intercept (compiler.rs, normal `class
/// Foo; attr_*; end`) and the AST-level `class << X; attr_*; end`
/// expansion below, so both sites agree on which methods get
/// synthesised. The actual reader/writer body shape
/// (`def name; @name; end` / `def name=(v); @name = v; end`) is
/// still spelled out at each call site — bytecode emission and
/// SExpr construction don't share a representation. If the
/// desugar EVER changes (e.g. to add type checks), update both
/// sites in lockstep.
pub(crate) fn attr_reader_writer_flags(name: &str) -> Option<(bool, bool)> {
    match name {
        "attr_reader"   => Some((true,  false)),
        "attr_writer"   => Some((false, true)),
        "attr_accessor" => Some((true,  true)),
        // `attr :name` (single or multi-symbol form) is the
        // pre-1.9 legacy alias for `attr_reader`. This match
        // returns the reader-only flags; the 1.8-only
        // `attr :name, true` accessor form is dispatched in
        // the compiler intercept (and the `class << X` body
        // desugar) by a dedicated `(SymbolLit, BoolLit)` arm
        // that runs BEFORE this helper is consulted. The
        // all-symbols gate downstream of this helper would
        // otherwise reject the `BoolLit` second arg as
        // unsupported. rackup-2.2.1/lib/rackup/stream.rb and
        // rack-3.1.10/lib/rack/builder.rb use the bare
        // single-symbol form; sinatra-4 transitively requires
        // both. (TRY_RUNS pass-10 layer #10; Copilot review
        // #313 round 1.)
        "attr"          => Some((true,  false)),
        _ => None,
    }
}

/// True iff a `ConstantPathNode` chain is rooted at top-level
/// (leading `::`). `Foo::Bar` is relative, `::Bar` and
/// `::Foo::Bar` are absolute. Used by `ConstantPathWriteNode`
/// arms to suppress the lexical-class-path alias the compiler
/// emits for relative writes — `::X = 1` inside `module Foo`
/// must store ONLY at top-level `X`, not also at `Foo::X`.
fn is_constant_path_absolute(node: &Node<'_>) -> bool {
    let Some(cp) = node.as_constant_path_node() else { return false; };
    match cp.parent() {
        None => true,                       // `::Bar` or root of `::Foo::Bar`
        Some(parent) => {
            if parent.as_constant_path_node().is_some() {
                is_constant_path_absolute(&parent)
            } else {
                false                       // hit `Foo` in `Foo::Bar` — relative
            }
        }
    }
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
fn tr_kwhash(
    ctx: &mut TranslationCtx<'_>,
    parent: &Node<'_>,
    kh_anchor: &Node<'_>,
    kh: &ruby_prism::KeywordHashNode<'_>,
) -> SExpr {
    let mut chunks: Vec<SExpr> = Vec::new();
    let mut buf: Vec<(SExpr, SExpr)> = Vec::new();
    for el in kh.elements().iter() {
        if let Some(an) = el.as_assoc_node() {
            buf.push((tr(ctx, &an.key()), tr(ctx, &an.value())));
        } else if let Some(spn) = el.as_assoc_splat_node()
            && let Some(inner) = spn.value() {
                if !buf.is_empty() {
                    chunks.push(sp(kh_anchor, Expr::HashLit(std::mem::take(&mut buf))));
                }
                chunks.push(tr(ctx, &inner));
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
        args: vec![rhs], kwargs_trailing: false }))
}

pub(crate) fn tr(ctx: &mut TranslationCtx<'_>, node: &Node<'_>) -> SExpr {
    let span = node_span(node);
    if let Some(n) = node.as_program_node() {
        let stmts: Vec<SExpr> = n.statements().body().iter().map(|c| tr(ctx, &c)).collect();
        return if stmts.len() == 1 {
            stmts.into_iter().next().unwrap()
        } else {
            Spanned::new(span, seq_inner(stmts))
        };
    }
    if let Some(n) = node.as_statements_node() {
        let stmts: Vec<SExpr> = n.body().iter().map(|c| tr(ctx, &c)).collect();
        return Spanned::new(span, seq_inner(stmts));
    }
    if let Some(n) = node.as_integer_node() {
        // Prism's `IntegerNode::value()` exposes the digits as
        // LSB-first u32 chunks + sign. Most literals fit in i64;
        // those that don't promote to BigInt when the `bignum`
        // feature is on, and saturate to `i64::MIN`/`i64::MAX`
        // when it's off (matching the no-bignum `wrapping_*`
        // arithmetic discipline).
        let int_value = n.value();
        let (negative, digits) = int_value.to_u32_digits();
        // i64-fast-path check: at most two u32 chunks AND the
        // 64-bit magnitude fits the signed range. Above that
        // boundary we hand off to BigInt (or saturate).
        let fits_i64_fast = digits.len() <= 2 && {
            let mut magnitude: u64 = 0;
            for (i, d) in digits.iter().enumerate() {
                magnitude |= (*d as u64) << (i * 32);
            }
            if negative { magnitude <= (i64::MAX as u64) + 1 }
            else        { magnitude <= i64::MAX as u64 }
        };
        if fits_i64_fast {
            let mut magnitude: u64 = 0;
            for (i, d) in digits.iter().enumerate() {
                magnitude |= (*d as u64) << (i * 32);
            }
            let v: i64 = if negative {
                if magnitude == (i64::MAX as u64) + 1 { i64::MIN }
                else { -(magnitude as i64) }
            } else {
                magnitude as i64
            };
            return sp(node, Expr::IntLit(v));
        }
        // Out-of-i64 literal. Build a BigInt from Prism's LSB-first
        // u32 digits, format to decimal, emit a `BigIntLit`. The
        // compiler interns the decimal string; the runtime parses
        // + caches on first execution.
        #[cfg(feature = "bignum")]
        {
            use num_bigint::{BigInt, Sign};
            let sign = if negative { Sign::Minus } else { Sign::Plus };
            let big = BigInt::from_slice(sign, digits);
            return sp(node, Expr::BigIntLit(big.to_string()));
        }
        #[cfg(not(feature = "bignum"))]
        {
            // No bignum feature → saturate (legacy behaviour).
            let v: i64 = if negative { i64::MIN } else { i64::MAX };
            return sp(node, Expr::IntLit(v));
        }
    }
    // Rational literal — `1000.0r`, `1/3r`, `0.5r`. Phase C.4.4
    // wires this to a real `Value::Rational` via the canonical-form
    // num / den decimal-string pair. CRuby parity for class,
    // arithmetic, and display (`1000.0r.class == Rational`,
    // `(1/3r) + (1/6r) == (1/2r)`, etc.). Pre-C.4.4 lowered to
    // `FloatLit(num / den)` — see the `feat/rational-c4-3-...`
    // PR series for the BigInt-backed `RationalRepr` machinery
    // this op relies on at the VM side.
    //
    // Under `bignum`, both num and den are arbitrary-precision
    // BigInt (via `num_bigint::BigInt::from_slice` on Prism's
    // LSB-first u32 digits, then gcd-reduced + sign-normalized
    // here so the canonical decimal strings hit the cache cleanly).
    // Without `bignum`, the literal must fit i64 — `LoadRational`
    // raises RangeError at load time if the parsed value overflows.
    if let Some(n) = node.as_rational_node() {
        #[cfg(feature = "bignum")]
        {
            use num_bigint::{BigInt, Sign};
            use num_integer::Integer;
            use num_traits::One;
            let to_bigint = |int_value: ruby_prism::Integer<'_>| -> BigInt {
                let (negative, digits) = int_value.to_u32_digits();
                let sign = if negative { Sign::Minus } else { Sign::Plus };
                BigInt::from_slice(sign, digits)
            };
            let mut num = to_bigint(n.numerator());
            let mut den = to_bigint(n.denominator());
            // Prism rejects `/0r` at lex time; this guard is
            // defensive against a future Prism build relaxing
            // the rule. ZeroDivisionError emerges at load time.
            if den.sign() != Sign::NoSign {
                if den.sign() == Sign::Minus {
                    num = -num;
                    den = -den;
                }
                let g = num.gcd(&den);
                if !g.is_one() {
                    num /= &g;
                    den /= &g;
                }
            }
            return sp(node, Expr::RationalLit {
                num: num.to_string(),
                den: den.to_string(),
            });
        }
        #[cfg(not(feature = "bignum"))]
        {
            // No-bignum path: convert each Prism integer to a u128
            // accumulator, then format as a signed decimal. No
            // gcd-reduction here — VM-side `make_rational` does it
            // at load time. Components beyond u128 substitute a
            // `u128::MAX` sentinel so `LoadRational` reliably raises
            // RangeError (the i64 parse fails on the sentinel value).
            // The emitted decimal text matches the original literal
            // for any source-realistic magnitude; only the rare
            // > u128 case diverges.
            let signed_str = |int_value: ruby_prism::Integer<'_>| -> String {
                let (negative, digits) = int_value.to_u32_digits();
                let mut mag: u128 = 0;
                for (i, d) in digits.iter().enumerate() {
                    if i < 4 {
                        mag |= (*d as u128) << (i * 32);
                    } else {
                        // Overflow past u128 — write a sentinel
                        // that won't fit i64 so LoadRational
                        // raises RangeError cleanly.
                        mag = u128::MAX;
                        break;
                    }
                }
                if negative { format!("-{}", mag) } else { mag.to_string() }
            };
            return sp(node, Expr::RationalLit {
                num: signed_str(n.numerator()),
                den: signed_str(n.denominator()),
            });
        }
    }
    if let Some(n) = node.as_float_node() {
        return sp(node, Expr::FloatLit(n.value()));
    }
    if let Some(n) = node.as_string_node() {
        // Prism's `unescaped()` returns the raw post-escape byte
        // sequence — `\xFF` produces a single 0xFF byte. We try
        // UTF-8 first (the overwhelmingly common case for
        // source-level string literals); if validation fails the
        // literal carries high-byte content that the interner
        // (UTF-8 only) can't hold, so we take the binary-literal
        // path and preserve raw bytes via `StrLitBytes`.
        let raw = n.unescaped();
        return match std::str::from_utf8(raw) {
            Ok(s) => sp(node, Expr::StrLit(s.to_string())),
            Err(_) => sp(node, Expr::StrLitBytes(raw.to_vec())),
        };
    }
    if let Some(n) = node.as_symbol_node() {
        return sp(node, Expr::SymbolLit(String::from_utf8_lossy(n.unescaped()).into_owned()));
    }
    if let Some(_n) = node.as_regular_expression_node() {
        #[cfg(feature = "regex")]
        {
            return sp(node, Expr::RegexLit(String::from_utf8_lossy(_n.unescaped()).into_owned()));
        }
        #[cfg(not(feature = "regex"))]
        {
            // ADR 0017 Rule 3: regex moves to the `regex` Cargo
            // feature. Without it, `/pattern/` literals reject at
            // AST-translation time with a clear pointer at the
            // feature flag. ctx.errors is the standard channel for
            // unsupported nodes; the bare `Expr::Nil` placeholder
            // keeps downstream compilation walking even though the
            // collected error will trap before any compiled body
            // runs.
            ctx.errors.push(
                    "/pattern/ regex literal: rubyrs was built without the \
                     `regex` Cargo feature; rebuild with --features regex to \
                     enable Regexp support (ADR 0017 Rule 3 / Tier 2)".to_string(),
                );
            return sp(node, Expr::Nil);
        }
    }
    if let Some(n) = node.as_interpolated_string_node() {
        let parts: Vec<SExpr> = n.parts().iter().map(|p| {
            if let Some(es) = p.as_embedded_statements_node() {
                let stmts: Vec<SExpr> = es.statements()
                    .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
                    .unwrap_or_default();
                if stmts.len() == 1 { stmts.into_iter().next().unwrap() }
                else { Spanned::new(node_span(&p), seq_inner(stmts)) }
            } else if let Some(ev) = p.as_embedded_variable_node() {
                tr(ctx, &ev.variable())
            } else {
                tr(ctx, &p)
            }
        }).collect();
        return sp(node, Expr::InterpolatedStr(parts));
    }
    // `/pre #{x} post/` — same `parts()` shape as InterpolatedString;
    // the per-part `to_s + +` build runs in the compiler, then
    // `Op::CompileRegex` turns the resulting String into a Regex.
    // Without the regex feature, emit the standard "regex feature
    // not enabled" AST error (matching `RegexLit`'s behaviour).
    if let Some(_n) = node.as_interpolated_regular_expression_node() {
        #[cfg(feature = "regex")]
        {
            let parts: Vec<SExpr> = _n.parts().iter().map(|p| {
                if let Some(es) = p.as_embedded_statements_node() {
                    let stmts: Vec<SExpr> = es.statements()
                        .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
                        .unwrap_or_default();
                    if stmts.len() == 1 { stmts.into_iter().next().unwrap() }
                    else { Spanned::new(node_span(&p), seq_inner(stmts)) }
                } else if let Some(ev) = p.as_embedded_variable_node() {
                    tr(ctx, &ev.variable())
                } else {
                    tr(ctx, &p)
                }
            }).collect();
            return sp(node, Expr::InterpolatedRegex(parts));
        }
        #[cfg(not(feature = "regex"))]
        {
            ctx.errors.push(
                    "/#{...}/ interpolated regex literal: rubyrs was built without the \
                     `regex` Cargo feature; rebuild with --features regex to \
                     enable Regexp support (ADR 0017 Rule 3 / Tier 2)".to_string(),
                );
            return sp(node, Expr::Nil);
        }
    }
    if node.as_true_node().is_some() { return sp(node, Expr::BoolLit(true)); }
    if node.as_false_node().is_some() { return sp(node, Expr::BoolLit(false)); }
    if node.as_nil_node().is_some() { return sp(node, Expr::Nil); }
    if node.as_self_node().is_some() { return sp(node, Expr::SelfExpr); }
    if let Some(n) = node.as_constant_read_node() {
        return sp(node, Expr::ConstRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_constant_path_node() {
        // A `Foo::Bar::Baz` ConstantPath translates to a single
        // ConstRead with the joined name. Real module scope
        // resolution lives in `build_const_chain` at compile time:
        // for relative paths, it cref-walks the first segment; for
        // absolute paths (`::Foo::Bar`), we keep a leading `::`
        // marker so the compiler can skip the cref walk and look up
        // exactly the joined name at top level (CRuby semantics).
        if let Some(joined) = flatten_constant_path(node) {
            let name = if is_constant_path_absolute(node) {
                format!("::{}", joined)
            } else {
                joined
            };
            return sp(node, Expr::ConstRead(name));
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
        return sp(node, Expr::LVarWrite(cid_to_string(n.name()), Box::new(tr(ctx, &n.value()))));
    }
    if let Some(n) = node.as_instance_variable_read_node() {
        return sp(node, Expr::IVarRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_instance_variable_write_node() {
        return sp(node, Expr::IVarWrite(cid_to_string(n.name()), Box::new(tr(ctx, &n.value()))));
    }
    // `@@foo` read / `@@foo = expr` write — class variables.
    // Tier 1 stores them per-class without hierarchy walk; see
    // Class.class_vars + Op::Load/StoreCvar comments.
    // `__FILE__` / `__LINE__` — pseudo-keywords for source
    // location. SourceFileNode's content is empty; the actual
    // filename comes from the surrounding proto at compile time
    // (compile_proto threads `filename_rc` into ProtoBuilder).
    if node.as_source_file_node().is_some() {
        return sp(node, Expr::SourceFile);
    }
    if node.as_source_line_node().is_some() {
        // Derive the 1-based line number from the Prism
        // Location's start pointer + the source bytes the
        // SourceGuard threaded in. Without an active guard
        // (test harnesses that call `tr` directly), falls
        // back to `0` — matches the prior stub value, no
        // behaviour change for callers that don't pass
        // source.
        let loc = node.location();
        // Reach the raw start pointer via the public
        // `as_slice` shape: `Location::as_slice` panics
        // if end < start, but for a SourceLineNode start
        // == end (zero-length location at the keyword
        // position), so we can't rely on it. Use the
        // private start ptr via a 1-byte read offset.
        // SAFETY: Prism guarantees `loc.start` points into
        // the parsed source, so taking the raw `*const u8`
        // value through a zero-length slice is well-defined.
        let line = {
            let s = loc.as_slice();
            ctx.line_of(s.as_ptr())
        };
        return sp(node, Expr::SourceLine(line));
    }
    if let Some(n) = node.as_class_variable_read_node() {
        return sp(node, Expr::CvarRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_class_variable_write_node() {
        return sp(node, Expr::CvarWrite(cid_to_string(n.name()), Box::new(tr(ctx, &n.value()))));
    }
    // `@@x += y` etc. — desugar to `@@x = @@x + y` (or whichever
    // binary op). Same shape we use for the local-variable op-
    // write family.
    if let Some(n) = node.as_class_variable_operator_write_node() {
        let name = cid_to_string(n.name());
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::CvarRead(name.clone()));
        let rhs = tr(ctx, &n.value());
        let combined = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![rhs], kwargs_trailing: false });
        return sp(node, Expr::CvarWrite(name, Box::new(combined)));
    }
    // `@@x ||= y` — assign-if-falsy. CRuby: read; if truthy
    // return it, else assign rhs. Use Or-then-Write shape.
    if let Some(n) = node.as_class_variable_or_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::CvarRead(name.clone()));
        let rhs = tr(ctx, &n.value());
        let or_expr = sp(node, Expr::Or(Box::new(read), Box::new(rhs)));
        return sp(node, Expr::CvarWrite(name, Box::new(or_expr)));
    }
    // `@@x &&= y` — assign-if-truthy.
    if let Some(n) = node.as_class_variable_and_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::CvarRead(name.clone()));
        let rhs = tr(ctx, &n.value());
        let and_expr = sp(node, Expr::And(Box::new(read), Box::new(rhs)));
        return sp(node, Expr::CvarWrite(name, Box::new(and_expr)));
    }
    // `$foo` read / `$foo = expr` write — global variables.
    // Spike subset: plain user globals go through `Vm.globals`;
    // a small set of special globals (`$$`, `$0`) is intercepted
    // by `Op::LoadGlobal`. Unknown globals read as Nil.
    if let Some(n) = node.as_global_variable_read_node() {
        return sp(node, Expr::GVarRead(cid_to_string(n.name())));
    }
    if let Some(n) = node.as_global_variable_write_node() {
        return sp(node, Expr::GVarWrite(cid_to_string(n.name()), Box::new(tr(ctx, &n.value()))));
    }
    // `$1`, `$2`, ..., `$10`, `$11`, ... (Prism's
    // NumberedReferenceReadNode) — numbered capture references
    // derived from the last successful regex match. Lowered to
    // the same `GVarRead("$N")` shape as a regular global read;
    // the LoadGlobal arm in vm/step.rs parses all trailing digits
    // and reads from `Vm::last_match.caps[n - 1]`. Without the
    // `regex` feature, last_match is always None, so these reads
    // still resolve to nil (no need to AST-reject). Prism's
    // `number()` returns u32 — multi-digit indices are supported,
    // matching CRuby.
    if let Some(n) = node.as_numbered_reference_read_node() {
        return sp(node, Expr::GVarRead(format!("${}", n.number())));
    }
    // `$~`, `$&`, `` $` ``, `$'`, `$+` — special regex match
    // globals (Prism's BackReferenceReadNode). All five route
    // through the same GVarRead → LoadGlobal path as the
    // numbered backrefs; the actual values live on
    // `Vm::last_match` and are materialised in vm/step.rs.
    // `$+` is the one motivating real-world use (ERB's
    // detect_magic_comment uses it after a regex match).
    if let Some(n) = node.as_back_reference_read_node() {
        return sp(node, Expr::GVarRead(cid_to_string(n.name())));
    }
    // Bare constant assignment: `FOO = expr` (top level or inside a
    // class/module body). Storage is a separate `Vm.constants` map
    // keyed by SymId — class names continue to live in `Vm.classes`,
    // and class lookup wins on read. This is a deliberate rubyrs
    // divergence from CRuby (CRuby warns "already initialized" and
    // reassigns); see `Vm::constants` for the precedence rationale.
    if let Some(n) = node.as_constant_write_node() {
        return sp(node, Expr::ConstWrite(cid_to_string(n.name()), false, Box::new(tr(ctx, &n.value()))));
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
        let absolute = is_constant_path_absolute(&target.as_node());
        // target is a ConstantPathNode; flatten via the same helper
        // the read path uses.
        if let Some(joined) = flatten_constant_path(&target.as_node()) {
            return sp(node, Expr::ConstWrite(joined, absolute, Box::new(tr(ctx, &n.value()))));
        }
        // Dynamic-path fallback (rare): use the trailing name only,
        // matching the ConstantPathNode read fallback at line ~415.
        // Force `absolute = true` here even when the path wasn't
        // syntactically leading-`::` — the dynamic head means the
        // user's `obj::X = ...` was never meant to land in the
        // lexical scope, so the compiler-side alias would be wrong
        // (would create a spurious `OuterModule::X` when this
        // appears inside `module OuterModule`).
        if let Some(name_id) = target.name() {
            return sp(node, Expr::ConstWrite(cid_to_string(name_id), true, Box::new(tr(ctx, &n.value()))));
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
            args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
        return sp(node, Expr::LVarWrite(name, Box::new(rhs)));
    }
    if let Some(n) = node.as_instance_variable_operator_write_node() {
        let name = cid_to_string(n.name());
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::IVarRead(name.clone()));
        let rhs = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
        return sp(node, Expr::IVarWrite(name, Box::new(rhs)));
    }
    // `a ||= b` → `a || (a = b)`; `a &&= b` → `a && (a = b)`.
    // Reading an uninitialised local returns nil (the frame slot
    // is zeroed at entry), so `a ||= b` on a fresh `a` correctly
    // assigns. Same for ivars — unset ivar reads as nil.
    if let Some(n) = node.as_local_variable_or_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::LVarRead(name.clone()));
        let write = sp(node, Expr::LVarWrite(name, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_local_variable_and_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::LVarRead(name.clone()));
        let write = sp(node, Expr::LVarWrite(name, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_instance_variable_or_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::IVarRead(name.clone()));
        let write = sp(node, Expr::IVarWrite(name, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_instance_variable_and_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::IVarRead(name.clone()));
        let write = sp(node, Expr::IVarWrite(name, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_index_or_write_node() {
        // `recv[idx] ||= val` → `recv[idx] || (recv[idx] = val)`.
        let recv = n.receiver().map(|r| tr(ctx, &r)).expect(
            "IndexOrWriteNode without receiver is unrepresentable",
        );
        let idx_args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: "[]".into(),
            args: idx_args.clone(), kwargs_trailing: false });
        let mut write_args = idx_args;
        write_args.push(tr(ctx, &n.value()));
        let write = sp(node, Expr::Call {
            receiver: Some(Box::new(recv)),
            name: "[]=".into(),
            args: write_args, kwargs_trailing: false });
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_index_and_write_node() {
        // `recv[idx] &&= val` → `recv[idx] && (recv[idx] = val)`.
        let recv = n.receiver().map(|r| tr(ctx, &r)).expect(
            "IndexAndWriteNode without receiver is unrepresentable",
        );
        let idx_args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: "[]".into(),
            args: idx_args.clone(), kwargs_trailing: false });
        let mut write_args = idx_args;
        write_args.push(tr(ctx, &n.value()));
        let write = sp(node, Expr::Call {
            receiver: Some(Box::new(recv)),
            name: "[]=".into(),
            args: write_args, kwargs_trailing: false });
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_index_operator_write_node() {
        // `recv[idx] += val` → `recv.[]=(idx, recv.[](idx) + val)`.
        // Multi-arg subscripts (`m[i, j]`) are flattened: every
        // index node becomes a positional arg in both the read
        // and write calls. Block arg is not supported here
        // (`m[i, &b] += ...` is exotic; pass through as
        // unsupported).
        let recv = n.receiver().map(|r| tr(ctx, &r)).expect(
            "IndexOperatorWriteNode without receiver is unrepresentable in our subset",
        );
        let idx_args: Vec<SExpr> = n
            .arguments()
            .map(|a| a.arguments().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: "[]".into(),
            args: idx_args.clone(), kwargs_trailing: false });
        let new_val = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
        let mut write_args = idx_args;
        write_args.push(new_val);
        return sp(node, Expr::Call {
            receiver: Some(Box::new(recv)),
            name: "[]=".into(),
            args: write_args, kwargs_trailing: false });
    }
    // Global-variable op-writes — same desugar pattern as IVar.
    // Unknown globals read as nil (Op::LoadGlobal default), so
    // `$g ||= 1` on an unset `$g` correctly assigns.
    if let Some(n) = node.as_global_variable_operator_write_node() {
        let name = cid_to_string(n.name());
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::GVarRead(name.clone()));
        let rhs = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
        return sp(node, Expr::GVarWrite(name, Box::new(rhs)));
    }
    if let Some(n) = node.as_global_variable_or_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::GVarRead(name.clone()));
        let write = sp(node, Expr::GVarWrite(name, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_global_variable_and_write_node() {
        let name = cid_to_string(n.name());
        let read = sp(node, Expr::GVarRead(name.clone()));
        let write = sp(node, Expr::GVarWrite(name, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    // Constant op-writes — CRuby diverges by operator:
    //   - `FOO ||= default`: read is silent-nil if undefined
    //     (lazy-init idiom).
    //   - `FOO &&= "x"`: read raises NameError if undefined
    //     ("update if set" — no lazy-init shortcut).
    //   - `FOO += 1` / other operator-writes: read raises
    //     NameError (CRuby evaluates the read first; the
    //     `+` against an undefined name never runs).
    // We mirror exactly: only `||=` reads via `ConstReadOrNil`.
    if let Some(n) = node.as_constant_operator_write_node() {
        let name = cid_to_string(n.name());
        let op = cid_to_string(n.binary_operator());
        // Strict read — `FOO += 1` on undefined raises NameError
        // before the operator runs.
        let read = sp(node, Expr::ConstRead(name.clone()));
        let rhs = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
        return sp(node, Expr::ConstWrite(name, false, Box::new(rhs)));
    }
    if let Some(n) = node.as_constant_or_write_node() {
        let name = cid_to_string(n.name());
        // Silent-nil read — `UNSET ||= default` is CRuby's
        // canonical lazy-init idiom.
        let read = sp(node, Expr::ConstReadOrNil(name.clone()));
        let write = sp(node, Expr::ConstWrite(name, false, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_constant_and_write_node() {
        let name = cid_to_string(n.name());
        // Strict read — `MAYBE &&= "x"` on undefined raises
        // NameError, matching CRuby (no lazy-init special-case
        // for the and-form).
        let read = sp(node, Expr::ConstRead(name.clone()));
        let write = sp(node, Expr::ConstWrite(name, false, Box::new(tr(ctx, &n.value()))));
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    // ConstantPath op-writes — `Foo::Bar += 1`. Target is a
    // ConstantPathNode; flatten via the same helper used by
    // ConstantPathWriteNode. Dynamic-head paths
    // (`obj.const::Bar += 1`) fall back to trailing-name-only,
    // mirroring the existing ConstantPathRead / ConstantPathWrite
    // arms — keeps op-write behaviour consistent with the base
    // read/write so `obj.foo::BAR += 1` doesn't silently become
    // an unsupported-node error while `obj.foo::BAR = 1` works.
    if let Some(n) = node.as_constant_path_operator_write_node() {
        let target = n.target();
        let absolute = is_constant_path_absolute(&target.as_node());
        let op = cid_to_string(n.binary_operator());
        // `abs` parameter: leading-`::` form uses the computed
        // `absolute`; dynamic-head fallback passes `true` so the
        // collapsed bare-name write doesn't get class_path-aliased
        // (the dynamic `obj::X += 1` was never meant to land in
        // the lexical scope).
        //
        // Strict read — matches the bare `FOO += 1` arm above:
        // CRuby raises NameError before the operator runs on an
        // undefined constant.
        let mut make = |name: String, abs: bool| {
            let read = sp(node, Expr::ConstRead(name.clone()));
            let rhs = sp(node, Expr::Call {
                receiver: Some(Box::new(read)),
                name: op.clone(),
                args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
            sp(node, Expr::ConstWrite(name, abs, Box::new(rhs)))
        };
        if let Some(joined) = flatten_constant_path(&target.as_node()) {
            return make(joined, absolute);
        }
        if let Some(name_id) = target.name() {
            return make(cid_to_string(name_id), true);
        }
    }
    if let Some(n) = node.as_constant_path_or_write_node() {
        let target = n.target();
        let absolute = is_constant_path_absolute(&target.as_node());
        // See ConstantPathOperatorWriteNode arm for the `abs`
        // override rationale on the dynamic-head fallback.
        let mut make = |name: String, abs: bool| {
            let read = sp(node, Expr::ConstReadOrNil(name.clone()));
            let write = sp(node, Expr::ConstWrite(name, abs, Box::new(tr(ctx, &n.value()))));
            sp(node, Expr::Or(Box::new(read), Box::new(write)))
        };
        if let Some(joined) = flatten_constant_path(&target.as_node()) {
            return make(joined, absolute);
        }
        if let Some(name_id) = target.name() {
            return make(cid_to_string(name_id), true);
        }
    }
    if let Some(n) = node.as_constant_path_and_write_node() {
        let target = n.target();
        let absolute = is_constant_path_absolute(&target.as_node());
        // Strict read — matches the bare `FOO &&= ...` arm above:
        // CRuby has no lazy-init shortcut for `&&=`; undefined
        // constants raise NameError on the read.
        let mut make = |name: String, abs: bool| {
            let read = sp(node, Expr::ConstRead(name.clone()));
            let write = sp(node, Expr::ConstWrite(name, abs, Box::new(tr(ctx, &n.value()))));
            sp(node, Expr::And(Box::new(read), Box::new(write)))
        };
        if let Some(joined) = flatten_constant_path(&target.as_node()) {
            return make(joined, absolute);
        }
        if let Some(name_id) = target.name() {
            return make(cid_to_string(name_id), true);
        }
    }
    if let Some(n) = node.as_multi_write_node() {
        // `a, b = expr`, `a, *r, b = expr`, `@x, @y = expr`,
        // `a, b = 1, 2`. Targets come from `lefts` (pre-splat),
        // `rest` (the splat slot itself), and `rights`
        // (post-splat). If Prism got multiple right-side values
        // with no array literal in source, they're packed into
        // an ArrayNode at the `value` slot.
        let mut targets: Vec<MultiWriteTarget> = Vec::new();
        // Nested fn (not closure) so we don't keep a mutable
        // borrow of `ctx.errors` alive across the splat/rest
        // arms below, which also push errors directly.
        fn push_positional(
            ctx: &mut TranslationCtx<'_>,
            targets: &mut Vec<MultiWriteTarget>,
            tgt: &Node<'_>,
        ) {
            if let Some(lvt) = tgt.as_local_variable_target_node() {
                targets.push(MultiWriteTarget::Local(cid_to_string(lvt.name())));
            } else if let Some(ivt) = tgt.as_instance_variable_target_node() {
                targets.push(MultiWriteTarget::Ivar(cid_to_string(ivt.name())));
            } else if let Some(gvt) = tgt.as_global_variable_target_node() {
                targets.push(MultiWriteTarget::Global(cid_to_string(gvt.name())));
            } else {
                ctx.errors.push(
                    format!("unsupported multi-write target: {:?}", tgt)
                );
            }
        }
        for tgt in n.lefts().iter() {
            push_positional(ctx, &mut targets, &tgt);
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
                        } else if let Some(gvt) = expr.as_global_variable_target_node() {
                            // Symmetric with the positional `Global`
                            // arm at line 1274 — CRuby accepts
                            // `a, *$g = …` (sets `a=arr[0]; $g=arr[1..]`)
                            // and pre-fix this still raised
                            // "unsupported splat target". (Code-review
                            // #301.)
                            targets.push(MultiWriteTarget::SplatGlobal(
                                cid_to_string(gvt.name()),
                            ));
                        } else {
                            ctx.errors.push(
                                format!("unsupported splat target: {:?}", expr)
                            );
                        }
                    }
                }
            } else if rest.as_implicit_rest_node().is_some() {
                // `a, = arr` form — Prism uses ImplicitRestNode to
                // mark the trailing comma. Treat as anonymous splat.
                targets.push(MultiWriteTarget::SplatLocal(None));
            } else {
                ctx.errors.push(
                    format!("unsupported multi-write rest: {:?}", rest)
                );
            }
        }
        for tgt in n.rights().iter() {
            push_positional(ctx, &mut targets, &tgt);
        }
        let value = tr(ctx, &n.value());
        return sp(node, Expr::MultiWrite {
            targets,
            value: Box::new(value),
        });
    }
    if let Some(n) = node.as_call_node() {
        let receiver = n.receiver().map(|r| Box::new(tr(ctx, &r)));
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
                        splat: Box::new(tr(ctx, &splat_expr)),
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
                        chunks.push(tr(ctx, &inner));
                    } else if let Some(kh) = cn.as_keyword_hash_node() {
                    // Trailing kwarg-hash retains its sugar shape;
                    // **opts merges via tr_kwhash's `.merge` chain.
                    buf.push(tr_kwhash(ctx, node, cn, &kh));
                } else {
                    buf.push(tr(ctx, cn));
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
                args: vec![rhs], kwargs_trailing: false }));
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
        // Track whether the FINAL arg originated from a
        // KeywordHashNode — that's the only position CRuby treats
        // as a kwarg sugar (preceding positions in `1.9 hash` style
        // are accepted but interpreted as regular Hash literals).
        // The flag is what later `Op::CallKw*` emission consults to
        // signal "trailing arg is kwargs, not positional Hash" to
        // the dispatcher / primitive_call kwarg channel.
        let mut kwargs_trailing = false;
        let last_idx = arg_nodes.len().saturating_sub(1);
        let args: Vec<SExpr> = arg_nodes.iter().enumerate().map(|(i, c)| {
            if let Some(kh) = c.as_keyword_hash_node() {
                if i == last_idx { kwargs_trailing = true; }
                tr_kwhash(ctx, node, c, &kh)
            } else {
                tr(ctx, c)
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
                        // M27 A1: `|&blk|` named block-arg param.
                        // Prism returns BlockParameterNode directly
                        // from `p.block()` (alternation node — no
                        // `as_*` cast needed). Append as a BlockArg
                        // BlockParam; compile_block reserves a slot
                        // and sets proto.block_param so
                        // invoke_method_with_block's trailing-slot
                        // binder populates it when the block is
                        // installed as a method.
                        if let Some(b) = p.block() {
                            let name = b.name().map(cid_to_string).unwrap_or_else(|| "&".to_string());
                            out.push(BlockParam::BlockArg(name));
                        }
                        out
                    })
                    .unwrap_or_default();
                let block_body: Vec<SExpr> = match bn.body() {
                    Some(b) => {
                        if let Some(stmts) = b.as_statements_node() {
                            stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                        } else { vec![tr(ctx, &b)] }
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
            if let Some(ba) = bnode.as_block_argument_node() {
                // Anonymous `inner(&)` (Ruby 3.1+ block forwarding):
                // no expression on the BlockArgumentNode. Read the
                // sentinel local `&` populated by the enclosing
                // `def foo(&)` parameter and forward it as the
                // block arg.
                //
                // Divergence: if the enclosing def DIDN'T have
                // `(&)`, CRuby raises a parse-time SyntaxError
                // ("no anonymous block parameter"). rubyrs auto-
                // creates the local slot on read (resolving to nil),
                // so `inner(&)` degenerates to `inner(&nil)` — i.e.
                // the call proceeds without a block. The observable
                // runtime outcome depends on the callee:
                //   - ignores the block → silent success (the
                //     real behavioral divergence; CRuby would
                //     have caught this at parse time)
                //   - calls `blk.call` → NoMethodError on nil.call
                //   - uses `yield` → RuntimeError "no block given"
                // Documented in docs/SUBSET.md.
                if ba.expression().is_none() {
                    let block_arg = sp(node, Expr::LVarRead("&".to_string()));
                    return sp(node, Expr::CallWithBlockArg {
                        receiver, name, args, block_arg: Box::new(block_arg),
                    });
                }
            }
            if let Some(ba) = bnode.as_block_argument_node()
                && let Some(expr) = ba.expression() {
                    if let Some(sn) = expr.as_symbol_node() {
                        let method_name: String = String::from_utf8_lossy(sn.unescaped()).into_owned();
                        let param_name = "__sp_x".to_string();
                        let body_call = sp(node, Expr::Call {
                            receiver: Some(Box::new(sp(node, Expr::LVarRead(param_name.clone())))),
                            name: method_name,
                            args: vec![], kwargs_trailing: false });
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
                    let block_arg = tr(ctx, &expr);
                    return sp(node, Expr::CallWithBlockArg {
                        receiver, name, args, block_arg: Box::new(block_arg),
                    });
                }
        }
        return sp(node, Expr::Call { receiver, name, args, kwargs_trailing });
    }
    // `return`, `next`, `break` all collapse multi-arg forms
    // into a single value the same way CRuby does:
    //   - zero args: None (yields nil at the consumer)
    //   - one arg: that single value
    //   - two-or-more args: an Array literal wrapping them all
    // CRuby treats `return a, b` as `return [a, b]` (same shape
    // as a `[a, b]` literal). Without the Array-wrap, the
    // multi-arg form silently dropped everything past the first
    // value — broke destructuring assignments like
    // `x, y = some_method` where the method used `return a, b`.
    // Motivating case: MRI's `lib/erb/compiler.rb:466`
    // (`return enc, frozen` consumed by `*magic_comment` splat).
    fn collect_multi_return_value(ctx: &mut TranslationCtx<'_>, args: Option<ruby_prism::ArgumentsNode<'_>>, span_node: &Node<'_>) -> Option<Box<SExpr>> {
        let a = args?;
        let arg_nodes: Vec<_> = a.arguments().iter().collect();
        match arg_nodes.len() {
            0 => None,
            // Single arg, no splat: pass through directly.
            //
            // Single arg, splat form `return *val`: CRuby's
            // semantics is `return Array(val)` — wraps scalars
            // (`*5` → `[5]`), expands Array (`*[1,2]` → `[1,2]`),
            // empty-array for nil (`*nil` → `[]`). Lower to a
            // synthetic `Array(inner)` call so the runtime's
            // existing `Kernel#Array` impl produces the correct
            // shape. No new opcode needed.
            1 => {
                let only = &arg_nodes[0];
                if let Some(sn) = only.as_splat_node()
                    && let Some(inner) = sn.expression() {
                    let inner_expr = tr(ctx, &inner);
                    return Some(Box::new(sp(span_node, Expr::Call {
                        receiver: None,
                        name: "Array".into(),
                        args: vec![inner_expr], kwargs_trailing: false })));
                }
                Some(Box::new(tr(ctx, only)))
            }
            // 2+ args: build an Array. With splats, mirror the
            // `[a, *b, c]` handling in the array-literal arm —
            // chunk non-splats into ArrayLit groups, splats stay
            // bare, chain them with `Array#+`.
            _ => {
                let has_splat = arg_nodes.iter().any(|c| c.as_splat_node().is_some());
                if !has_splat {
                    let elems: Vec<SExpr> = arg_nodes.iter().map(|n| tr(ctx, n)).collect();
                    return Some(Box::new(sp(span_node, Expr::ArrayLit(elems))));
                }
                let mut chunks: Vec<SExpr> = Vec::new();
                let mut buf: Vec<SExpr> = Vec::new();
                for n in &arg_nodes {
                    if let Some(sn) = n.as_splat_node()
                        && let Some(inner) = sn.expression() {
                        if !buf.is_empty() {
                            chunks.push(sp(span_node, Expr::ArrayLit(std::mem::take(&mut buf))));
                        }
                        // Wrap splat inner in `Array(inner)` so the
                        // subsequent `Array#+` chain always concats
                        // Array against Array — `return :first, *5, :last`
                        // becomes `[:first] + Array(5) + [:last]` →
                        // `[:first, 5, :last]`, matching CRuby. Without
                        // the wrap, scalars/nil would push bare values
                        // and `Array#+` would TypeError on the non-
                        // Array RHS.
                        let inner_expr = tr(ctx, &inner);
                        chunks.push(sp(span_node, Expr::Call {
                            receiver: None,
                            name: "Array".into(),
                            args: vec![inner_expr], kwargs_trailing: false }));
                    } else {
                        buf.push(tr(ctx, n));
                    }
                }
                if !buf.is_empty() {
                    chunks.push(sp(span_node, Expr::ArrayLit(buf)));
                }
                let mut it = chunks.into_iter();
                let first = it.next().unwrap_or_else(|| sp(span_node, Expr::ArrayLit(vec![])));
                let acc = it.fold(first, |lhs, rhs| sp(span_node, Expr::Call {
                    receiver: Some(Box::new(lhs)),
                    name: "+".into(),
                    args: vec![rhs], kwargs_trailing: false }));
                Some(Box::new(acc))
            }
        }
    }
    if let Some(n) = node.as_return_node() {
        let val = collect_multi_return_value(ctx, n.arguments(), node);
        return sp(node, Expr::Return(val));
    }
    if let Some(n) = node.as_next_node() {
        let val = collect_multi_return_value(ctx, n.arguments(), node);
        return sp(node, Expr::Next(val));
    }
    if let Some(n) = node.as_break_node() {
        let val = collect_multi_return_value(ctx, n.arguments(), node);
        return sp(node, Expr::Break(val));
    }
    if node.as_retry_node().is_some() {
        // `retry` carries no value/args; legality (must be inside a
        // rescue clause body) is enforced at compile time via the
        // ProtoBuilder's `retry_targets` stack. (TRY_RUNS pass-10
        // layer #9.)
        return sp(node, Expr::Retry);
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
                args: vec![sp(node, Expr::SymbolLit(name))], kwargs_trailing: false });
        }
        if let Some(cr) = inner.as_constant_read_node() {
            let name = cid_to_string(cr.name());
            return Spanned::new(span, Expr::Call {
                receiver: None,
                name: "__defined_const?".into(),
                args: vec![sp(node, Expr::SymbolLit(name))], kwargs_trailing: false });
        }
        if inner.as_constant_path_node().is_some()
            && let Some(joined) = flatten_constant_path(&inner)
        {
            // `defined?(Foo::Bar)` — flatten the path to the same
            // qualified key the dual-write / class table uses, then
            // let `__defined_const?` report `"constant"` if either
            // `self.classes` or `self.constants` holds the entry.
            // Falls back to `"expression"` only if the path can't
            // be flattened (dynamic ConstantPath, rare; matches
            // CRuby's behaviour for `defined?(some_method::Foo)`).
            return Spanned::new(span, Expr::Call {
                receiver: None,
                name: "__defined_const?".into(),
                args: vec![sp(node, Expr::SymbolLit(joined))], kwargs_trailing: false });
        }
        if let Some(cn) = inner.as_call_node() {
            // `defined?(__dir__)` — bareword shape (no receiver /
            // args / block) short-circuits to `"method"` to match
            // CRuby. The value path is handled by `do_call`'s
            // dedicated `__dir__` arm in `vm/dispatch.rs`, which
            // pulls the dir from the current frame's proto filename
            // (sandbox-aware: canonicalize only when
            // `allow_filesystem_io` is on and no allowlist is set).
            // The `__defined_method?` host fn falls back to nil for
            // built-in dispatch-arm shortcuts because they don't
            // live in the method table, so this arm bridges the
            // reflection gap.
            if cn.receiver().is_none()
                && cn.arguments().is_none()
                && cn.block().is_none()
                && cid_to_string(cn.name()) == "__dir__"
            {
                return s("method");
            }
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
                    args: vec![sp(node, Expr::SymbolLit(name))], kwargs_trailing: false });
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
                // M27 A1: `|&blk|` capture in lambdas, same as for
                // blocks (see comment above).
                if let Some(b) = p.block() {
                    let name = b.name().map(cid_to_string).unwrap_or_else(|| "&".to_string());
                    out.push(BlockParam::BlockArg(name));
                }
                out
            })
            .unwrap_or_default();
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Lambda { params, body });
    }
    if let Some(n) = node.as_yield_node() {
        let args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        return sp(node, Expr::Yield(args));
    }
    if let Some(n) = node.as_if_node() {
        let cond = Box::new(tr(ctx, &n.predicate()));
        let then_body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        let else_body: Vec<SExpr> = match n.subsequent() {
            Some(sub) => {
                if let Some(en) = sub.as_else_node() {
                    en.statements().map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect()).unwrap_or_default()
                } else {
                    vec![tr(ctx, &sub)]
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
        let arg_nodes: Vec<ruby_prism::Node<'_>> = n.arguments()
            .map(|args| args.arguments().iter().collect())
            .unwrap_or_default();
        // Detect splat anywhere in the arg list. When present,
        // assemble the args into a single Array via the same
        // chunking strategy regular Call-with-splat uses
        // (concat the non-splat groups via `+`), and emit
        // `Expr::SuperApply` so the compiler routes through
        // `Op::ApplySuper` (which pops one Array and treats
        // its elements as positional args). Without this path,
        // `super(*a)` and `super(a, *rest, b)` shapes from
        // Rack / Sinatra inheritance chains tripped the
        // `unsupported node: SplatNode` trap.
        let has_splat = arg_nodes.iter().any(|c| c.as_splat_node().is_some());
        if has_splat {
            let mut chunks: Vec<SExpr> = Vec::new();
            let mut buf: Vec<SExpr> = Vec::new();
            for c in &arg_nodes {
                if let Some(sn) = c.as_splat_node()
                    && let Some(inner) = sn.expression() {
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                    }
                    chunks.push(tr(ctx, &inner));
                } else {
                    buf.push(tr(ctx, c));
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
                args: vec![rhs], kwargs_trailing: false }));
            return sp(node, Expr::SuperApply(Box::new(acc)));
        }
        let args: Vec<SExpr> = arg_nodes.iter().map(|n| tr(ctx, n)).collect();
        return sp(node, Expr::Super(Some(args)));
    }
    if let Some(n) = node.as_or_node() {
        return sp(node, Expr::Or(Box::new(tr(ctx, &n.left())), Box::new(tr(ctx, &n.right()))));
    }
    if let Some(n) = node.as_and_node() {
        return sp(node, Expr::And(Box::new(tr(ctx, &n.left())), Box::new(tr(ctx, &n.right()))));
    }
    if let Some(n) = node.as_while_node() {
        let cond = Box::new(tr(ctx, &n.predicate()));
        let body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
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
        let body = vec![tr(ctx, &n.expression())];
        let rescue = vec![RescueClause {
            classes: vec![],
            body: vec![tr(ctx, &n.rescue_expression())],
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
        let predicate = n.predicate().map(|p| tr(ctx, &p));
        let conditions: Vec<_> = n.conditions().iter().collect();
        let else_body: Vec<SExpr> = match n.else_clause() {
            Some(en) => en.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
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
                            let arr = tr(ctx, &inner);
                            let sp_name = "__sp_v".to_string();
                            let body_expr = match &predicate {
                                Some(pred) => sp(cn, Expr::Call {
                                    receiver: Some(Box::new(sp(cn, Expr::LVarRead(sp_name.clone())))),
                                    name: "===".into(),
                                    args: vec![pred.clone()], kwargs_trailing: false }),
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
                    (tr(ctx, cn), false)
                })
                .collect();
            let when_body: Vec<SExpr> = when.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
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
                            args: vec![pred.clone()], kwargs_trailing: false }),
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
        let cond = Box::new(tr(ctx, &n.predicate()));
        let then_body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        let else_body: Vec<SExpr> = match n.else_clause() {
            Some(en) => en.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
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
        let raw_cond = tr(ctx, &n.predicate());
        let cond = Box::new(sp(node, Expr::Call {
            receiver: Some(Box::new(raw_cond)),
            name: "!".into(),
            args: vec![], kwargs_trailing: false }));
        let body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
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
        let mut n_required_post: u16 = 0;
        let mut kw_params: Vec<(String, Option<SExpr>)> = Vec::new();
        let mut kw_rest: Option<String> = None;
        let mut block_param: Option<String> = None;
        if let Some(p) = n.parameters() {
            if let Some(b) = p.block() {
                // `def foo(&blk)`: capture the caller's block into
                // the named slot. Anonymous form `def foo(&)` (Ruby
                // 3.1+ block forwarding) has `b.name() == None`;
                // bind it to a reserved sentinel name `&` (invalid
                // as a user identifier) so the matching `inner(&)`
                // call site at this method level can read it via
                // LVarRead. CRuby surfaces the same anonymous block
                // as the Symbol `:&` in Method#parameters
                // (`[[:block, :&]]`), so the sentinel passes through
                // introspection unchanged — byte-for-byte parity
                // without any unwrap. Prism returns
                // `BlockParameterNode` directly from `p.block()`
                // (it's an alternation node, not a generic Node);
                // no `as_*_node` cast needed.
                block_param = Some(b.name().map(cid_to_string).unwrap_or_else(|| "&".to_string()));
            }
            if let Some(r) = p.rest()
                && let Some(rp) = r.as_rest_parameter_node() {
                    // Anonymous form `def foo(*)` (Ruby 2.0+
                    // anonymous rest forwarding) has
                    // `rp.name() == None`. Without a fallback the
                    // rest slot would be silently dropped — the
                    // method would compile with arity 0 and reject
                    // any positional args. Bind to the reserved
                    // sentinel `*` (invalid as a user identifier)
                    // so the binder still allocates a sink slot.
                    // CRuby surfaces the same anonymous rest as
                    // the Symbol `:*` in Method#parameters
                    // (`[[:rest, :*]]`), so the sentinel passes
                    // through introspection unchanged — mirroring
                    // the existing block-param `&` pattern at
                    // ~line 2109.
                    rest = Some(rp.name().map(cid_to_string).unwrap_or_else(|| "*".to_string()));
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
                    let val = tr(ctx, &ok.value());
                    // Same literal-only restriction as positional
                    // defaults: anything else needs a per-callsite
                    // prologue we don't generate. Surface as a
                    // SyntaxError via ctx.errors.
                    match &val.node {
                        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::SymbolLit(_)
                        | Expr::BoolLit(_) | Expr::Nil => {
                            kw_params.push((name, Some(val)));
                        }
                        _ => {
                            ctx.errors.push(
                                format!("default value for keyword parameter `{}` must be a literal", name)
                            );
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
                    defaults.push(Some(tr(ctx, &op.value())));
                }
            }
            // M27 A4: post-rest required params (`def mid(a, *b, c)`'s
            // `c`). Appended to `params` AFTER the optionals so the
            // binder can peel them off the tail of args. CRuby grammar
            // requires `*rest` to precede them; we don't enforce it
            // here (prism does at parse time).
            for r in p.posts().iter() {
                if let Some(rp) = r.as_required_parameter_node() {
                    params.push(cid_to_string(rp.name()));
                    defaults.push(None);
                    n_required_post += 1;
                }
            }
        }
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
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
        let receiver = n.receiver().map(|r| Box::new(tr(ctx, &r)));
        return sp(node, Expr::Def { name, params, defaults, rest, n_required_post, kw_params, kw_rest, block_param, receiver, body });
    }
    if let Some(n) = node.as_range_node() {
        // Beginless / endless ranges (`..3`, `1..`) are not yet supported;
        // we treat the missing endpoint as `nil` which will fail at runtime
        // when something tries to iterate. For our subset, both ends should
        // be present.
        let begin = n.left().map(|c| tr(ctx, &c)).unwrap_or_else(|| sp(node, Expr::Nil));
        let end = n.right().map(|c| tr(ctx, &c)).unwrap_or_else(|| sp(node, Expr::Nil));
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
            let elems: Vec<SExpr> = raw_elems.iter().map(|e| tr(ctx, e)).collect();
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
                    chunks.push(tr(ctx, &inner));
                } else {
                buf.push(tr(ctx, en));
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
            args: vec![rhs], kwargs_trailing: false }));
        return acc;
    }
    if let Some(n) = node.as_hash_node() {
        // Detect `**splat` inside the literal. Without one we
        // take the fast path; with one we route through the
        // same `.merge` chain shape as kwarg-hash call args.
        let has_splat = n.elements().iter().any(|e| e.as_assoc_splat_node().is_some());
        if !has_splat {
            let pairs: Vec<(SExpr, SExpr)> = n.elements().iter().filter_map(|e| {
                e.as_assoc_node().map(|a| (tr(ctx, &a.key()), tr(ctx, &a.value())))
            }).collect();
            return sp(node, Expr::HashLit(pairs));
        }
        let mut chunks: Vec<SExpr> = Vec::new();
        let mut buf: Vec<(SExpr, SExpr)> = Vec::new();
        for el in n.elements().iter() {
            if let Some(an) = el.as_assoc_node() {
                buf.push((tr(ctx, &an.key()), tr(ctx, &an.value())));
            } else if let Some(spn) = el.as_assoc_splat_node()
                && let Some(inner) = spn.value() {
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::HashLit(std::mem::take(&mut buf))));
                    }
                    chunks.push(tr(ctx, &inner));
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
            args: vec![rhs], kwargs_trailing: false }));
    }
    if let Some(n) = node.as_class_node() {
        // Class name shape:
        //   1. `class Foo`      — ConstantReadNode → "Foo"
        //   2. `class Foo::Bar` — ConstantPathNode → "Foo::Bar"
        //      (flatten via the same helper used by ConstRead's
        //      path arm, so reads of `Foo::Bar` from elsewhere hit
        //      the same joined-string key in `Vm.classes`).
        //   3. dynamic path or unrecognised — "?" fallback (keeps
        //      compilation alive; the resulting class is unusable
        //      but does not ICE).
        // Motivating use: MRI `lib/erb/compiler.rb:79`
        // (`class ERB::Compiler`) — without case (2), the body
        // executes against a class named "?" and `ERB::Compiler`
        // resolves to nil.
        let cp = n.constant_path();
        let name = if let Some(cr) = cp.as_constant_read_node() {
            cid_to_string(cr.name())
        } else if let Some(joined) = flatten_constant_path(&cp) {
            joined
        } else {
            "?".to_string()
        };
        // Superclass can be either a bare constant (`class C < P`)
        // or a constant path (`class C < Foo::Bar`). Without
        // accepting `ConstantPathNode` here, the path form silently
        // returned `None` (because `as_constant_read_node` rejects
        // it), so DefClass popped Nil and the child lost its
        // inheritance link — observable as "undefined method `m'
        // for Object" on any child instance. Surfaced as TRY_RUNS
        // pass-7 layer #6 (the `alias secure? ssl?` bug) but the
        // root cause was broader: nested-via-path superclass dropped
        // at AST translation time.
        //
        // Compiler downstream (compiler.rs ~line 1036-1044): for
        // path-shape names containing `::`, `build_const_chain`
        // bails out (returns `None` because `bare.contains("::")`,
        // compiler.rs:195), so the emitter falls through to
        // `Op::LoadConst` with the joined `"Foo::Bar"` SymId. That
        // works for relative paths because `Op::DefClass` keys the
        // class table by the qualified SymId (vm/step.rs:1520 uses
        // `qual_id` over `name_id` when set), so a `module M; class
        // MP; end; end` definition lands at the same
        // `interner.intern("M::MP")` slot that `LoadConst("M::MP")`
        // later reads.
        //
        // KNOWN GAP: `flatten_constant_path` (and its `None =>
        // Some(name)` arm) loses leading-`::` (absolute-path)
        // information across ALL callers, not just this one.
        // Effect on superclass: in a nested scope, `class C <
        // ::Bar` flattens to `"Bar"`, and the cref-walking
        // `LoadConstChain` built for bare-name lookups in
        // `compiler.rs` resolves it as `Wrapper::Bar` first,
        // instead of forcing top-level `Bar` per CRuby semantics.
        // Pre-existing gap shared with const reads / rescue classes
        // / etc. — not introduced by this PR; deferred until a
        // caller surfaces a real failure on it.
        let superclass = n.superclass().and_then(|s| {
            if let Some(cr) = s.as_constant_read_node() {
                Some(cid_to_string(cr.name()))
            } else if s.as_constant_path_node().is_some() {
                flatten_constant_path(&s)
            } else {
                None
            }
        });
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Class { name, superclass, body, is_module: false });
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
        // Same constant-path handling as `class Foo::Bar` above —
        // `module Foo::Bar` is the second-most-common shape in
        // real gems (e.g. `module Tilt::ERBTemplate::Helpers`).
        let cp = n.constant_path();
        let name = if let Some(cr) = cp.as_constant_read_node() {
            cid_to_string(cr.name())
        } else if let Some(joined) = flatten_constant_path(&cp) {
            joined
        } else {
            "?".to_string()
        };
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Class { name, superclass: None, body, is_module: true });
    }
    // `class << X; body; end` — singleton class body. Supported
    // body entries in the spike subset:
    //
    //   1. `def name(...) ... end` — rewritten with
    //      `receiver: Some(<translated X>)` so the existing
    //      `def X.foo` machinery (`Op::DefSingletonMethod` when
    //      X = self inside a `class Foo` body;
    //      `Op::DefObjectSingletonMethod` otherwise) lands each
    //      method on the right singleton table.
    //   2. `attr_reader` / `attr_writer` / `attr_accessor` with
    //      Symbol-literal args — expanded into one or two
    //      synthetic def-rewrites per symbol (see the helper
    //      `attr_reader_writer_flags` and the per-symbol loop
    //      below). Zero-arg form is a silent no-op matching
    //      CRuby 3.4.
    //
    // Other body entries (constant assignments, alias keyword,
    // `prepend`/`include` calls, embedded begin/end, etc.) still
    // surface as SyntaxError — they'd need either a real
    // singleton-class-as-class-stack-entry opcode or a
    // `class_eval` detour. Add support here as real targets
    // hit them; the error message at the fallthrough below
    // lists what's currently accepted.
    //
    // We don't translate to an `Expr::Class { ... }` because the
    // wrapping `class << X` doesn't introduce a NEW class with its
    // own name; the defs already address the existing
    // singleton_methods table on X's class chain.
    // `alias new old` keyword form (method-name aliasing) —
    // desugar into a synthetic `alias_method :new, :old` Call so
    // the existing compiler intercept (compiler.rs,
    // `Op::AliasMethod`) handles it. Both operands are parsed as
    // `SymbolNode` by Prism; non-Symbol shapes here are exotic
    // (dynamic dispatch via `alias` is uncommon) and fall
    // through to the ctx.errors trail.
    //
    // NOT THIS ARM: `alias $new $old` (global-variable aliasing)
    // is `AliasGlobalVariableNode` — a distinct Prism node, not
    // an `AliasMethodNode` with GlobalVariableNode operands. The
    // arm below only matches AliasMethodNode, so global-alias
    // forms naturally fall through to the unsupported-node trail
    // unchanged.
    //
    // OUT OF SCOPE: `alias` inside `class << X` body. The
    // existing intercept emits Op::AliasMethod targeting
    // `class_stack.last().methods` (instance methods), not the
    // singleton table. Tilt's `class << self; alias prefer
    // register; end` would silently alias on the wrong table.
    // Wrapped at the singleton-class body translation site below
    // — those cases still surface as the existing "only def and
    // attr_*" SyntaxError.
    if let Some(n) = node.as_alias_method_node()
        && let (Some(new_sym), Some(old_sym)) = (
            n.new_name().as_symbol_node(),
            n.old_name().as_symbol_node(),
        )
    {
        let new_name = String::from_utf8_lossy(new_sym.unescaped()).into_owned();
        let old_name = String::from_utf8_lossy(old_sym.unescaped()).into_owned();
        return sp(node, Expr::Call {
            receiver: None,
            name: "alias_method".into(),
            args: vec![
                sp(node, Expr::SymbolLit(new_name)),
                sp(node, Expr::SymbolLit(old_name)),
            ], kwargs_trailing: false });
    }
    if let Some(n) = node.as_singleton_class_node() {
        let recv_expr = tr(ctx, &n.expression());
        let body_nodes: Vec<_> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().collect::<Vec<_>>()
                } else { vec![b] }
            }
            None => vec![],
        };
        // CRuby evaluates the `class << expr` receiver exactly
        // ONCE for the whole body. Naive desugar `def expr.foo;
        // def expr.bar; ...` would re-evaluate expr per def —
        // fine for pure exprs (SelfExpr, ConstRead) but wrong
        // for anything side-effectful.
        //
        // For SelfExpr specifically, we MUST keep the literal
        // SelfExpr as the receiver — the compiler's special
        // case emits `Op::DefSingletonMethod` (lands on
        // class_stack.last().singleton_methods) only when it
        // sees `receiver: Some(SelfExpr)`. A synthetic-local
        // indirection would route to `Op::DefObjectSingletonMethod`
        // instead, which rejects Class receivers.
        //
        // For ConstRead the constant lookup is also pure and
        // can be re-evaluated cheaply, AND classes don't go
        // through DefObjectSingletonMethod's reject path because
        // the compiler routes them via the same special case.
        // Actually no — only `SelfExpr` hits the special case.
        // So for ConstRead we ALSO need to keep it literal so
        // the compiler can detect Class-shaped receivers at
        // dispatch time without going through the
        // Object-only path.
        //
        // Rule: only bind to a synthetic local for receivers
        // that are NEITHER SelfExpr NOR ConstRead — the
        // side-effectful / allocating cases. For pure receivers
        // the per-def re-evaluation is observably identical to
        // one-eval anyway.
        let needs_local = !matches!(
            &recv_expr.node,
            Expr::SelfExpr | Expr::ConstRead(_)
        );
        let synth_local = format!("__cls_lt_lt_recv_{}", node_span(node).byte_offset);
        let mut out: Vec<SExpr> = Vec::with_capacity(body_nodes.len() + 1);
        // Closure helper: make a `def recv.name(params) body` SExpr
        // rewriting the receiver-less Def into a singleton-method
        // form. Reused for both real DefNodes in the body and for
        // synthetic Defs we generate when expanding `attr_*` calls.
        let mk_singleton_def = |bn: &Node<'_>, def_translated: Expr| -> Option<SExpr> {
            if let Expr::Def {
                name, params, defaults, rest, n_required_post,
                kw_params, kw_rest, block_param, receiver: _, body,
            } = def_translated {
                let receiver = if needs_local {
                    sp(bn, Expr::LVarRead(synth_local.clone()))
                } else {
                    recv_expr.clone()
                };
                Some(sp(bn, Expr::Def {
                    name, params, defaults, rest, n_required_post,
                    kw_params, kw_rest, block_param,
                    receiver: Some(Box::new(receiver)),
                    body,
                }))
            } else {
                None
            }
        };
        for bn in &body_nodes {
            if bn.as_def_node().is_some() {
                let translated = tr(ctx, bn);
                if let Some(s) = mk_singleton_def(bn, translated.node) {
                    out.push(s);
                } else {
                    ctx.errors.push(
                        "class << X: internal — def translated unexpectedly".into()
                    );
                    out.push(sp(bn, Expr::Nil));
                }
                continue;
            }
            // `attr_reader :foo` / `attr_writer :foo` / `attr_accessor :foo`
            // inside `class << X` body. CRuby installs reader/writer
            // methods on X's singleton class. We desugar each symbol
            // arg into one or two synthetic `def X.foo; @foo; end` /
            // `def X.foo=(val); @foo = val; end` Defs and route them
            // through the existing singleton-rewrite path above.
            //
            // Caveat: ivar persistence on Class receivers diverges
            // from CRuby (class-level @foo on a Class value doesn't
            // currently round-trip across method calls — separate gap).
            // The reader returns nil instead of CRuby's last-written
            // value. Tilt's template.rb:503 only checks
            // `extract_fixed_locals.nil?`, so nil-vs-false is moot.
            if let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
            {
                let name = cid_to_string(call.name());
                // Pre-helper arm: legacy `attr :name, true/false`
                // accessor form. Single Symbol followed by a
                // BoolLit second arg. Mirrors compiler.rs's
                // dedicated `(SymbolLit, BoolLit)` intercept arm
                // for the normal class-body path. CRuby 3.4 still
                // accepts this with a suppressed warning; without
                // the special case the all-symbols gate further
                // down would reject it as unsupported.
                // (Copilot review #313 round 1.)
                if name == "attr" {
                    let raw_args: Vec<_> = call.arguments()
                        .map(|args| args.arguments().iter().collect())
                        .unwrap_or_default();
                    if raw_args.len() == 2
                        && let (Some(sym), Some(b)) = (
                            raw_args[0].as_symbol_node(),
                            raw_args[1].as_true_node().map(|_| true)
                                .or_else(|| raw_args[1].as_false_node().map(|_| false)),
                        )
                    {
                        let sym_name = String::from_utf8_lossy(sym.unescaped()).into_owned();
                        let ivar_name = format!("@{}", sym_name);
                        // Reader.
                        let body = vec![sp(bn, Expr::IVarRead(ivar_name.clone()))];
                        let def = Expr::Def {
                            name: sym_name.clone(),
                            params: vec![], defaults: vec![], rest: None,
                            n_required_post: 0,
                            kw_params: vec![], kw_rest: None, block_param: None,
                            receiver: None,
                            body,
                        };
                        if let Some(s) = mk_singleton_def(bn, def) { out.push(s); }
                        // Writer (only when arg is `true`).
                        if b {
                            let setter_name = format!("{sym_name}=");
                            let val_read = sp(bn, Expr::LVarRead("val".into()));
                            let body = vec![sp(
                                bn,
                                Expr::IVarWrite(ivar_name.clone(), Box::new(val_read)),
                            )];
                            let def = Expr::Def {
                                name: setter_name,
                                params: vec!["val".into()], defaults: vec![], rest: None,
                                n_required_post: 0,
                                kw_params: vec![], kw_rest: None, block_param: None,
                                receiver: None,
                                body,
                            };
                            if let Some(s) = mk_singleton_def(bn, def) { out.push(s); }
                        }
                        continue;
                    }
                }
                // Decode via the shared helper (paired with
                // compiler.rs's normal-class-body attr_* arm).
                // NOTE: zero-arg `attr_accessor` (etc.) is a SILENT
                // NO-OP in CRuby 3.4 (verified: no ArgumentError,
                // no methods defined). Our loop below handles that
                // case naturally — empty sym_names → no iterations
                // → nothing emitted. Don't add a guard rejecting
                // zero-arg; that would diverge from CRuby.
                if let Some((do_reader, do_writer)) = attr_reader_writer_flags(&name) {
                    let mut all_sym_args = true;
                    let sym_names: Vec<String> = call.arguments()
                        .map(|args| args.arguments().iter().filter_map(|a| {
                            a.as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
                        }).collect())
                        .unwrap_or_default();
                    let expected = call.arguments().map(|a| a.arguments().iter().count()).unwrap_or(0);
                    if sym_names.len() != expected { all_sym_args = false; }
                    if !all_sym_args {
                        ctx.errors.push(
                            "class << X body: attr_* with non-symbol args is not supported".into()
                        );
                        out.push(sp(bn, Expr::Nil));
                        continue;
                    }
                    for sym_name in sym_names {
                        let ivar_name = format!("@{}", sym_name);
                        if do_reader {
                            let body = vec![sp(bn, Expr::IVarRead(ivar_name.clone()))];
                            let def = Expr::Def {
                                name: sym_name.clone(),
                                params: vec![], defaults: vec![], rest: None,
                                n_required_post: 0,
                                kw_params: vec![], kw_rest: None, block_param: None,
                                receiver: None,
                                body,
                            };
                            if let Some(s) = mk_singleton_def(bn, def) { out.push(s); }
                        }
                        if do_writer {
                            let setter_name = format!("{sym_name}=");
                            let body = vec![sp(bn, Expr::IVarWrite(
                                ivar_name.clone(),
                                Box::new(sp(bn, Expr::LVarRead("val".into()))),
                            ))];
                            let def = Expr::Def {
                                name: setter_name,
                                params: vec!["val".into()], defaults: vec![], rest: None,
                                n_required_post: 0,
                                kw_params: vec![], kw_rest: None, block_param: None,
                                receiver: None,
                                body,
                            };
                            if let Some(s) = mk_singleton_def(bn, def) { out.push(s); }
                        }
                    }
                    continue;
                }
            }
            // `alias new old` keyword form INSIDE `class << X`
            // body. Routes to `Op::AliasSingletonMethod` so the
            // alias lands on X's singleton_methods rather than
            // its instance methods (which is what the regular
            // top-level translation would do). Tilt's tilt.rb:99
            // `class << self; alias prefer register; end` is the
            // motivating case — `register` is a class method of
            // Tilt, `prefer` should also be a class method.
            // IMPORTANT scope guard: `Op::AliasSingletonMethod`
            // installs on `class_stack.last().singleton_methods`,
            // which is only the correct target when the surrounding
            // shape is `class << self` inside a class body — the
            // existing class_stack entry IS X. For
            // `class << SomeConst` / `class << obj`, the body runs
            // in the same frame without any class_stack push, so
            // the op would silently alias on the wrong receiver
            // (or on toplevel). Those receivers still fall through
            // to the existing unsupported-node SyntaxError.
            let recv_is_self = matches!(&recv_expr.node, Expr::SelfExpr);
            if recv_is_self
                && let Some(alias_node) = bn.as_alias_method_node()
                && let (Some(new_sym), Some(old_sym)) = (
                    alias_node.new_name().as_symbol_node(),
                    alias_node.old_name().as_symbol_node(),
                )
            {
                let new_name = String::from_utf8_lossy(new_sym.unescaped()).into_owned();
                let old_name = String::from_utf8_lossy(old_sym.unescaped()).into_owned();
                out.push(sp(bn, Expr::AliasSingletonMethod(new_name, old_name)));
                continue;
            }
            // Tighter error if we declined to handle alias due to
            // the non-self receiver guard above — separate from
            // the general "only def / attr_* / alias" message.
            if bn.as_alias_method_node().is_some() && !recv_is_self {
                ctx.errors.push(
                    "class << <non-self>: `alias` is only supported when the receiver is `self` (inside a class body)".into()
                );
                out.push(sp(bn, Expr::Nil));
                continue;
            }
            // `class << self; prepend Mod; end` — install Mod on
            // X's singleton-class prepend chain. Same `self`-
            // receiver gate as `alias`. The recogniser is purely
            // syntactic: this arm matches any `class << self;
            // prepend Mod; end` regardless of enclosing scope.
            // The compiled `Op::SingletonChainPrepend` enforces
            // the install-target check at runtime — it uses
            // `class_stack.last()` when present, traps with
            // SyntaxError otherwise (covers toplevel and any
            // context where the surrounding self isn't a
            // class/module). Tilt's finalize! is the motivating
            // case (`prepend(Module.new { ... })`).
            //
            // Single-arg form only (matches CRuby's single-module
            // prepend grammar in practice — `prepend(A, B)` is
            // legal but rare).
            if recv_is_self
                && let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
                && cid_to_string(call.name()) == "prepend"
                && let Some(args) = call.arguments()
                && args.arguments().iter().count() == 1
            {
                let src = tr(ctx, &args.arguments().iter().next().unwrap());
                out.push(sp(bn, Expr::SingletonChainPrepend(Box::new(src))));
                continue;
            }
            // `class << self; FOO = expr; ...` — constant assignment
            // inside the singleton class body. CRuby places the
            // constant on the singleton class itself, accessible
            // via `Foo.singleton_class::FOO`. rubyrs's spike-scope
            // constants model is flatter — `Vm.constants` is a
            // single name-keyed table — so we route the assignment
            // through the regular toplevel `Expr::ConstWrite`. The
            // result: a bare `FOO` read inside the singleton class
            // resolves through the same table that a top-level
            // `FOO` would, which is the model rubyrs already uses
            // for all other constants in the spike scope.
            //
            // Motivating call site: sinatra/base.rb:1292's
            // `class << self; CALLERS_TO_IGNORE = [...].freeze;
            // attr_reader :routes, ...; def callers_to_ignore;
            // CALLERS_TO_IGNORE; end; end` — the constant is
            // assigned once and read from the singleton method
            // body that follows. (TRY_RUNS pass 9 layer #11.)
            if recv_is_self && bn.as_constant_write_node().is_some() {
                out.push(tr(ctx, bn));
                continue;
            }
            // `class << self; @@cvar = expr; ...` — class variable
            // assignment inside the singleton class body. Same
            // shape as the CWN arm above: the toplevel
            // `Expr::CvarWrite` path already exists; we just
            // admit it here so the spike-subset doesn't reject
            // the form. CRuby places class variables on the
            // enclosing class hierarchy regardless of whether the
            // write happens inside `class << self` (cvars are
            // hierarchy-keyed in CRuby, NOT singleton-class-
            // scoped). rubyrs's Tier-1 cvar model is per-class
            // (no hierarchy walk — see `Op::LoadCvar` /
            // `StoreCvar`); admitting this arm doesn't change
            // that pre-existing divergence either way. So the
            // write goes to the same table whether the write
            // syntactically appears at class-body top level or
            // inside `class << self`; what this arm fixes is
            // strictly the parse-time admission, not any
            // semantic alignment with CRuby's hierarchy-walking
            // cvar lookup.
            //
            // Motivating call site: sinatra/base.rb:1292's
            // `class << self; ...; @@mutex = Mutex.new; def
            // synchronize(&block); @@mutex.synchronize(&block);
            // ...; end; end` — cvar assigned once then read from
            // singleton methods defined in the same body.
            // (TRY_RUNS pass 9.5 layer #12.)
            if recv_is_self && bn.as_class_variable_write_node().is_some() {
                out.push(tr(ctx, bn));
                continue;
            }
            // `class << self; private; def secret; ...; end; ...` —
            // bare visibility modifier (`private` / `public` /
            // `protected`) at body top level. Translates as a
            // regular method call (Expr::Call with name="private"
            // and implicit receiver). At runtime self is the
            // surrounding class (= `class_stack.last()` —
            // singleton-class body shares the outer class's
            // class_stack entry), and do_call's
            // `visibility_from_name` arm at ~line 2417 mutates
            // `class_visibility_stack.last_mut()` accordingly.
            // Subsequent `def`s in the same body read that stack
            // when DefSingletonMethod runs, so the modifier flows
            // correctly to following method definitions.
            //
            // Scope: only the bare-receiver form. The args form
            // (`private :foo, :bar`) retroactively flips named
            // methods' visibility on the OUTER class — but
            // sinatra/base.rb:1690 uses the bare form, and the
            // args form's interaction with singleton methods is
            // a separate question we don't need to answer here.
            //
            // Motivating call site: sinatra/base.rb:1690's
            // `class << self; ...; private; ...; end` — bare
            // `private` precedes a block of helper methods that
            // sinatra hides from external callers.
            // (TRY_RUNS pass 9.7 layer #14.)
            if recv_is_self
                && let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
                && call.arguments().is_none_or(|a| a.arguments().iter().next().is_none())
                && matches!(cid_to_string(call.name()).as_str(),
                    "private" | "public" | "protected"
                )
            {
                out.push(tr(ctx, bn));
                continue;
            }
            // `class << self; <stmt> if cond` / `class << self;
            // <stmt> unless cond` — and structurally-equivalent
            // block forms `if cond; <stmt>; end` / `unless cond;
            // <stmt>; end` — at body top level wrapping a single
            // supported inner stmt. The recogniser admits ANY
            // `IfNode` / `UnlessNode` with exactly one statement
            // and no `subsequent` / `else_clause` (Prism's
            // names for the else/elsif tail): the modifier and one-stmt
            // block forms compile identically (modifier is just
            // sugar), so handling both is safe and gives a
            // slightly broader green path. Tightening to truly
            // modifier-form via `end_keyword_loc().is_none()`
            // would be the alternative; chose the broader
            // wording over the narrower recogniser since the
            // semantics are equivalent. (PR #218 Copilot
            // round 3 caught the previous "modifier-form only"
            // wording as inaccurate.)
            // Recognised inner shapes: bare-call (CallNode, e.g.
            // `ruby2_keywords(:use)`) and the `alias new old` form
            // (AliasMethodNode). Both are wrapped as
            // `Expr::If { cond, then_body: [<inner>], else_body: [] }`
            // (matches the rest of `tr()` — empty `else_body`
            // compiles to `LoadNil` at the codegen layer).
            // The condition is translated via the regular `tr()`
            // path (so `respond_to?(...)` / `method_defined? :foo`
            // dispatch through their usual builtins).
            //
            // Motivating call sites (TRY_RUNS pass 9.5 layers
            // #13 / #15):
            //   - `ruby2_keywords(:use) if respond_to?(:ruby2_keywords, true)`
            //   - `alias new! new unless method_defined? :new!`
            // The Ruby 2.7 guard pattern: try the call only when
            // the receiver advertises support. The guard's
            // truthiness is computed by the regular dispatch
            // path — whether the guarded call ultimately fires
            // depends on the same dispatch decisions
            // `respond_to?` / `method_defined?` make for any
            // other caller, not on something specific to this
            // arm.
            //
            // Scope deliberately narrow: only CallNode and
            // AliasMethodNode inner statements are admitted; other
            // shapes (def / attr_* / const-write / cvar-write
            // wrapped in if/unless) fall through to
            // NotImplementedError because sinatra/base.rb doesn't
            // surface them and the inner-form recogniser can be
            // widened on demand.
            if recv_is_self {
                let modifier_if = bn.as_if_node()
                    .and_then(|if_n| {
                        if if_n.subsequent().is_some() { return None; }
                        let stmts = if_n.statements()?;
                        // Exactly-one-stmt check via a single
                        // iterator walk: take one, error if there
                        // are more. Cheaper than `count() + next()`.
                        let mut it = stmts.body().iter();
                        let inner = it.next()?;
                        if it.next().is_some() { return None; }
                        Some((if_n.predicate(), inner, false))
                    });
                let modifier_unless = bn.as_unless_node()
                    .and_then(|un_n| {
                        if un_n.else_clause().is_some() { return None; }
                        let stmts = un_n.statements()?;
                        let mut it = stmts.body().iter();
                        let inner = it.next()?;
                        if it.next().is_some() { return None; }
                        Some((un_n.predicate(), inner, true))
                    });
                if let Some((cond_node, inner, negated)) = modifier_if.or(modifier_unless) {
                    // Translate the inner stmt only if it's one of
                    // the admitted shapes; otherwise fall through.
                    let inner_expr: Option<SExpr> = if let Some(alias_node) = inner.as_alias_method_node()
                        && let (Some(new_sym), Some(old_sym)) = (
                            alias_node.new_name().as_symbol_node(),
                            alias_node.old_name().as_symbol_node(),
                        )
                    {
                        let new_name = String::from_utf8_lossy(new_sym.unescaped()).into_owned();
                        let old_name = String::from_utf8_lossy(old_sym.unescaped()).into_owned();
                        Some(sp(&inner, Expr::AliasSingletonMethod(new_name, old_name)))
                    } else if let Some(call) = inner.as_call_node() {
                        // Bare-receiver CallNode admitted only
                        // when its name is NOT one of the forms
                        // the body translator would otherwise
                        // special-case for the singleton class.
                        // `attr_reader` / `attr_writer` /
                        // `attr_accessor` / `prepend` translated
                        // through the regular `tr()` path WITH
                        // IMPLICIT RECEIVER would install on the
                        // OUTER class (instance methods), not the
                        // singleton class — semantic divergence
                        // from the unconditional forms supported
                        // earlier in this loop. Calls with an
                        // EXPLICIT receiver (`Other.attr_reader(:x)
                        // if cond`) don't trigger the body
                        // special-casing and route to whatever
                        // method `Other` provides, so they're
                        // admitted regardless of name. Real
                        // sinatra cases (`ruby2_keywords(:use)`)
                        // are bare-receiver but with names that
                        // have no body-level special-case, so
                        // they pass uneventfully. PR #218
                        // code-review #4 / Copilot round 5.
                        let is_silently_misdirected = call.receiver().is_none()
                            && matches!(cid_to_string(call.name()).as_str(),
                                "attr_reader" | "attr_writer" | "attr_accessor" | "prepend"
                            );
                        if is_silently_misdirected {
                            None
                        } else {
                            Some(tr(ctx, &inner))
                        }
                    } else {
                        None
                    };
                    if let Some(inner_expr) = inner_expr {
                        let raw_cond = tr(ctx, &cond_node);
                        let cond = if negated {
                            sp(&cond_node, Expr::Call {
                                receiver: Some(Box::new(raw_cond)),
                                name: "!".into(),
                                args: vec![], kwargs_trailing: false })
                        } else {
                            raw_cond
                        };
                        out.push(sp(bn, Expr::If {
                            cond: Box::new(cond),
                            then_body: vec![inner_expr],
                            else_body: vec![],
                        }));
                        continue;
                    }
                }
            }
            // `class << self` body: any remaining unsupported form
            // compiles to a runtime `raise NotImplementedError`
            // rather than a parse-time SyntaxError. The raise fires
            // WHEN THE SURROUNDING SCOPE EXECUTES — not necessarily
            // at method-call time. Two distinct scenarios:
            //
            //   1. Inside a `def` body: the raise sits in the
            //      method body and only fires when the method is
            //      invoked. Scripts that never call it load fine.
            //
            //   2. At class-body top level (`class Foo; class <<
            //      self; unsupported; end; end`): the raise sits
            //      in the class body and fires at LOAD time when
            //      the enclosing `class` / `module` block executes.
            //      The file load fails — matching the pre-PR
            //      SyntaxError outcome ("file doesn't load") with
            //      two differences:
            //        - error class is NotImplementedError
            //          (catchable by explicit
            //          `rescue NotImplementedError`), not SyntaxError;
            //        - NotImplementedError < ScriptError < Exception,
            //          so a bare `rescue` does NOT catch it
            //          (matches CRuby).
            //
            // Case (1) is a strict improvement (file loads); case
            // (2) is roughly equivalent. The deferral is the right
            // trade-off only when the unsupported form sits inside
            // an infrequently-called method. Specific shapes already
            // handled above (def / attr_* / alias / prepend-Mod)
            // bypass this path; everything else (e.g. `include Mod`
            // inside `class << self`, generic method calls) lands
            // here.
            //
            // `class << <non-self>` still hard-errors at parse time —
            // that branch has no surrounding class_stack frame, so
            // even silently emitting nil would do the wrong thing
            // (the body's intended target receiver is lost).
            if recv_is_self {
                let msg = "class << self body: only `def`, `attr_reader`/`attr_writer`/`attr_accessor`, `alias`, `prepend Mod` (single Module arg, with `self` receiver), constant assignment (`FOO = expr`), and class variable assignment (`@@cvar = expr`) are supported in the spike subset";
                out.push(sp(bn, Expr::Call {
                    receiver: None,
                    name: "raise".into(),
                    args: vec![
                        sp(bn, Expr::ConstRead("NotImplementedError".into())),
                        sp(bn, Expr::StrLit(msg.into())),
                    ], kwargs_trailing: false }));
                continue;
            }
            ctx.errors.push(
                "class << <non-self> body: only `def`, `attr_reader`/`attr_writer`/`attr_accessor`, and `alias` are supported in the spike subset".into()
            );
            out.push(sp(bn, Expr::Nil));
        }
        // Pin the trailing value to nil so the synthetic receiver
        // LVarWrite (when `needs_local`) doesn't leak as the body's
        // result. Empty bodies (`class << X; end`) and zero-arg
        // attr_* (`class << X; attr_accessor; end`) would otherwise
        // return the receiver value — CRuby returns nil for the
        // empty case. We don't try to match CRuby's attr_*
        // return-Array shape (`[]` for zero-arg); nil is the
        // user-friendly common case and the spec for `class << X`
        // generally is "evaluates to the last expression in body";
        // every supported entry's last op is already nil-pushing
        // (Def → LoadNil, expanded attr_* → LoadNil), so this only
        // changes the rare zero-arg/empty edge.
        // Wrap the body in a `Begin { ensure: [Pop] }` so the
        // visibility scope pop runs on BOTH normal exit and
        // exception unwind. Without the ensure, a raise inside
        // the body (or rescued by an outer begin) would skip the
        // pop and leak an extra entry into
        // `class_visibility_stack`, corrupting default visibility
        // for later defs. The Push runs FIRST, OUTSIDE the inner
        // Begin, so it's not double-counted by any unwind path —
        // the inner Begin's ensure handles the pairing on every
        // exit. PR #233 code-review round 2 (#1 unwind safety,
        // #3 doc accuracy).
        let inner_begin = sp(node, Expr::Begin {
            body: out,
            rescue: vec![],
            ensure: Some(vec![sp(node, Expr::PopClassVisibility)]),
        });
        // Outer Begin runs: synthetic-local write (if needed),
        // Push, inner Begin (with ensure-Pop), final Nil.
        let mut outer: Vec<SExpr> = Vec::with_capacity(4);
        if needs_local {
            outer.push(sp(node, Expr::LVarWrite(synth_local.clone(), Box::new(recv_expr.clone()))));
        }
        outer.push(sp(node, Expr::PushClassVisibilityPublic));
        outer.push(inner_begin);
        outer.push(sp(node, Expr::Nil));
        return sp(node, Expr::Begin {
            body: outer,
            rescue: vec![],
            ensure: None,
        });
    }
    if let Some(n) = node.as_parentheses_node() {
        // `(expr)` — just unwrap to the inner expression / statements.
        if let Some(body) = n.body() {
            if let Some(stmts) = body.as_statements_node() {
                let v: Vec<SExpr> = stmts.body().iter().map(|c| tr(ctx, &c)).collect();
                return if v.len() == 1 { v.into_iter().next().unwrap() }
                       else { Spanned::new(span, seq_inner(v)) };
            }
            return tr(ctx, &body);
        }
        return sp(node, Expr::Nil);
    }
    if let Some(n) = node.as_begin_node() {
        let body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        // Prism chains rescue clauses via `subsequent()`. Walk the
        // chain and flatten to a Vec so the compiler can emit one
        // PushRescue per clause in the right order.
        let mut rescue: Vec<RescueClause> = Vec::new();
        let mut cur = n.rescue_clause();
        while let Some(rc) = cur {
            let body: Vec<SExpr> = rc.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
                .unwrap_or_default();
            let var = rc.reference().and_then(|r| {
                r.as_local_variable_target_node().map(|lvt| cid_to_string(lvt.name()))
            });
            // Extract class filter names. ConstantReadNode
            // (`MyError`) maps to its bare name. ConstantPathNode
            // (`Foo::Bar`) flattens to the dotted form via
            // `flatten_constant_path` — the same qualified key
            // that the lexical dual-write stamps on classes
            // defined inside a module/class body, so
            // `rescue Foo::Bar` resolves to exactly that nested
            // class (not to a top-level `Bar` that happens to
            // share the trailing segment).
            let mut classes: Vec<String> = Vec::new();
            for exc in rc.exceptions().iter() {
                if let Some(c) = exc.as_constant_read_node() {
                    classes.push(cid_to_string(c.name()));
                } else if exc.as_constant_path_node().is_some()
                    && let Some(joined) = flatten_constant_path(&exc)
                {
                    classes.push(joined);
                }
                // Anything else (dynamic expression in rescue
                // position) is dropped silently for now.
            }
            rescue.push(RescueClause { classes, body, var });
            cur = rc.subsequent();
        }
        let ensure = n.ensure_clause().map(|ec| {
            ec.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect::<Vec<SExpr>>())
                .unwrap_or_default()
        });
        return sp(node, Expr::Begin { body, rescue, ensure });
    }
    // Unsupported Prism node — record the message and return a
    // placeholder. The eval entry point checks `ctx.errors` after
    // tr returns and surfaces a SyntaxError Trap, so the
    // placeholder never reaches the compiler in practice.
    ctx.errors.push(format!("unsupported node: {:?}", node));
    sp(node, Expr::Nil)
}

fn seq_inner(stmts: Vec<SExpr>) -> Expr {
    Expr::Call { receiver: None, name: "__seq__".to_string(), args: stmts , kwargs_trailing: false }
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

