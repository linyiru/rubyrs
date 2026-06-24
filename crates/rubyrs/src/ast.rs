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
    /// Monotonic counter that hands out unique synthesised local
    /// names for safe-navigation desugaring (`recv&.foo` → temp
    /// local + nil-test). `Cell<usize>` so the existing `&mut
    /// TranslationCtx` doesn't need to thread a write borrow
    /// through callers that just bump the counter.
    pub(crate) safe_nav_count: std::cell::Cell<usize>,
    /// Stack of "the enclosing method uses `(...)` argument
    /// forwarding". Pushed per `def` body (true only for
    /// `def m(...)`), read by bare `super`: inside a `(...)` method,
    /// bare `super` must forward the anonymous rest/kwrest/block the
    /// SAME way `super(...)` does (splat `*`, kwsplat `__kw_rest_anon`,
    /// `&block`) — not slot-dump the `*` rest array as one positional.
    /// Blocks don't push, so `super` in a block sees the method's flag.
    pub(crate) method_forward_stack: Vec<bool>,
}

impl<'src> TranslationCtx<'src> {
    pub(crate) fn new(source: Option<&'src [u8]>) -> Self {
        Self {
            errors: Vec::new(),
            source,
            safe_nav_count: std::cell::Cell::new(0),
            method_forward_stack: Vec::new(),
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
    /// `/pattern/imx` — the `u8` is the Ruby flag bitmask
    /// (IGNORECASE=1 | EXTENDED=2 | MULTILINE=4), threaded to the
    /// compiler so the runtime can apply the flags + answer
    /// `#options`.
    #[cfg(feature = "regex")]
    RegexLit(String, u8),
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
    /// `/#{...}/imx` — parts plus the Ruby flag bitmask (same
    /// encoding as `RegexLit`).
    #[cfg(feature = "regex")]
    InterpolatedRegex(Vec<SExpr>, u8),
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
    /// Assignment-SYNTAX method call: `recv.attr = v` / `recv[k] = v`
    /// (and the write half of the `||=` / `&&=` / op-assign
    /// desugars). CRuby evaluates the EXPRESSION to the final
    /// positional argument (the RHS) and discards the method's
    /// return value — `send(:foo=, v)` keeps the return value, so
    /// the marker is purely syntactic (prism `is_attribute_write`).
    /// Compiles to `Op::CallAset`, which swaps the dispatch result
    /// for the RHS. Always has a receiver, never a block; splat /
    /// kwargs-trailing shapes stay on the plain `Call` path
    /// (documented leak for those exotic forms).
    AssignCall {
        receiver: Box<SExpr>,
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
        /// Parent expression for `class Foo < <expr>` syntax.
        /// `None` for `class Foo; end` (defaults to Object at
        /// runtime). Was `Option<String>` historically (constant
        /// names only); generalised so dynamic shapes like
        /// `class Sub < some_local_var` or
        /// `class Sub < DelegateClass(Hash)` resolve their
        /// superclass by evaluating the expression and reading the
        /// Value::Class off the operand stack at `DefClass` time.
        superclass: Option<Box<SExpr>>,
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
        /// `true` when the constant path was ABSOLUTE (`class ::Foo` /
        /// `module ::Bar`) — the definition lands at TOP LEVEL, ignoring
        /// the enclosing lexical scope (`class C; module ::M; end; end`
        /// defines top-level `M`, not `C::M`). The compiler forces the
        /// qualified-name slot to the no-prefix sentinel for these.
        absolute: bool,
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
    /// `class << <expr>; body; end` run as a REAL eigenclass body
    /// (self = the metaclass), as opposed to the def/attr/alias
    /// desugar `tr_singleton_class` applies to the simple cases.
    /// Emitted only for bodies the desugar can't express
    /// faithfully — `include Mod`, nested `module`/`class`, and
    /// `internal def` / `private def` keyword-wrapped defs, where
    /// the body's statements (or runtime-indirected helpers like
    /// zeitwerk's `internal`) depend on `self` being the
    /// eigenclass. Compiles `body` into its own proto and emits
    /// `Op::OpenSingletonClass`; `recv` is evaluated once in the
    /// surrounding scope and left on the stack for the op to pop.
    SingletonClassBody {
        recv: Box<SExpr>,
        body: Vec<SExpr>,
    },
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
        /// `true` when the final arg came from a trailing
        /// `KeywordHashNode` (`foo(k: v, &blk)` / `foo(**kw, &blk)`)
        /// — drives the `CallKwBlock*` emit so an empty `**{}` is
        /// dropped and a non-empty kwargs Hash binds as kwargs.
        kwargs_trailing: bool,
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
        /// As `CallWithBlock::kwargs_trailing` — `foo(**kw, &proc)`.
        kwargs_trailing: bool,
    },
    Yield(Vec<SExpr>),
    /// `yield(*arr)` / `yield(a, *b, c)` — yield with a splat. The
    /// inner expr evaluates to the combined args Array (built by the
    /// same splat-chunking `collect_multi_return_value` the call/return
    /// paths use); the compiler emits `Op::ApplyYield`, which expands
    /// that Array's elements onto the stack and drives the block with
    /// the dynamic argc — the yield analogue of `Op::ApplyCall`.
    YieldSplat(Box<SExpr>),
    /// `foo(*arr)` — single-splat call. The compiler emits an
    /// `Op::ApplyCall` / `Op::ApplyCallNoRecv` that takes one
    /// Array on top of the stack and uses its elements as
    /// positional args. The optional `block_arg` carries the
    /// `&block` slot when both splat and block are present
    /// (`foo(*args, &block)` — exercised by Sinatra::Base.use's
    /// middleware `klass.new(inner_app, *args, &block)` chain).
    /// Mixed positional forms like `foo(a, *b, c)` collapse to
    /// a `+`-chain Array build BEFORE reaching this node, so
    /// the splat is always the only positional channel.
    Apply {
        receiver: Option<Box<SExpr>>,
        name: String,
        splat: Box<SExpr>,
        block_arg: Option<Box<SExpr>>,
        /// Keyword-splat carried SEPARATELY from `splat` (the positional
        /// array) so the VM can drop an empty `**{}` and keep a trailing
        /// positional brace-hash positional. `None` = no kwsplat (the
        /// classic `f(*args)` shape). Only set for the no-block
        /// `f(*args, **kw)` path; with a block the kwsplat stays folded
        /// into `splat` (the older path). Emits `Op::ApplyCallKw`.
        kwsplat: Option<Box<SExpr>>,
    },
    /// `->(params) { body }` — lambda literal. Compiles to the
    /// same `CreateBlock` opcode as a regular `{ |x| ... }` block,
    /// but stays on the stack as a Value::Block instead of being
    /// consumed by a method call. We don't distinguish Lambda
    /// from Proc at runtime; the strict-arity check that CRuby's
    /// Lambda enforces is missing — documented in SUBSET.md.
    Lambda { params: Vec<BlockParam>, body: Vec<SExpr>, is_lambda: bool },
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
    /// `redo` — re-run the current loop iteration / block body WITHOUT
    /// re-checking the condition or advancing the iterator. Targets the
    /// innermost enclosing `while`/`until` (via `loop_redo_jumps`) or,
    /// in a block, the block body start (`block_redo_target`).
    Redo,
    /// `super` (forwarding all of the enclosing method's args)
    /// or `super(arg1, arg2)` (explicit args). `super()` with
    /// empty parens passes no args and is `Some(vec![])`;
    /// bare `super` is `None`.
    Super(Option<Vec<SExpr>>),
    /// `super(*args)` / `super(a, *rest, b)` — splat in the
    /// super argument list. The inner SExpr evaluates to an
    /// Array containing the fully-assembled call args (the
    /// same shape `Expr::Apply` uses for regular splat-call
    /// dispatch). Compiles to `Op::ApplySuper(name_id)`,
    /// or `Op::ApplySuperBlock(name_id)` when `block_arg` is
    /// present (`super(*args, &block)` — sinatra-contrib's
    /// MultiRoute uses this shape across every HTTP verb
    /// method to forward both args + block to the inherited
    /// Sinatra::Base entry point).
    /// Rack `lib/rack/headers.rb`'s `super(*a.map!{...})`
    /// shape surfaces this; previously raised
    /// `unsupported node: SplatNode` at AST translation.
    SuperApply { args: Box<SExpr>, block_arg: Option<Box<SExpr>> },
    /// `super do |...| ... end` / `super(args) { ... }` — super with a
    /// block LITERAL (distinct from `super(&proc)`, which is
    /// `SuperApply.block_arg`). `args` is `None` for the bare
    /// arg-forwarding form (`super do ... end`) or `Some(exprs)` for an
    /// explicit list. The block is compiled inline and forwarded to the
    /// parent method via `Op::ApplySuperBlock`. Discovery: P3 Jekyll
    /// spike — liquid's `Document#parse` does `super do |tag, …| … end`.
    SuperWithBlock {
        args: Option<Vec<SExpr>>,
        block_params: Vec<BlockParam>,
        block_body: Vec<SExpr>,
    },
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
    /// An OPTIONAL positional param (`|a, b = 1|`). Takes a real slot
    /// like `Single` (so it binds positionally and is counted in
    /// `n_params`), but the compiler also tallies these into
    /// `Proto::n_optional_params` so `Proc#arity` reports the
    /// `-(required + 1)` variadic form. The default is desugared into
    /// the body prologue (`b = default if b.nil?`) by the AST walker.
    Optional(String),
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
    /// `|**opts|` keyword-rest parameter — binds the trailing
    /// keyword arguments as a Hash (`{}` when none were passed).
    /// Empty name is the anonymous `|**|`. compile_block reserves
    /// a slot + sets `proto.kw_rest_param`; invoke_block extracts
    /// the trailing kwargs Hash and binds it. Mustermann's
    /// `def_delegator`-generated blocks and many gems use
    /// `do |*a, **o| ... end`; pre-fix `**o` bound nil.
    KwRest(String),
    /// `|k1:, k2:|` named keyword parameter. `bool` is the
    /// required flag: `true` for `k:` (missing at call time →
    /// ArgumentError), `false` for `k: default` (the default is
    /// desugared at translation time into a body-prologue
    /// `k = default if k.nil?`, so the binder just writes Nil on
    /// miss — documented divergence: an explicit `k: nil` at the
    /// call site also re-evaluates the default). Translation
    /// pushes Keyword entries LAST in the param vec (after
    /// BlockArg / KwRest) so the kw slots land past every slot
    /// the `define_method`-as-method binder locates by position
    /// in `proto.params`; ordinary block invocation binds them
    /// by the absolute slots in `Proto::block_kw_params`.
    Keyword(String, bool),
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
    /// `*recv.attr` — splat into an attribute writer. Hit by
    /// Mustermann's `self.head, *self.payload = Array(payload)`
    /// (mustermann/ast/node.rb:216). The collected rest Array
    /// is passed through `recv.attr=(array)`; same dispatch
    /// shape as `MWT::Call` (which handles non-splat `recv.attr
    /// = val`). Receiver evaluated AFTER the RHS — documented
    /// Tier-1 divergence shared with `MWT::Call`.
    SplatCall { receiver: Box<SExpr>, name: String },
    /// `obj.attr = …` — method-call setter target. Surfaced by
    /// `obj.x, obj.y = a, b` shapes (sinatra-param uses this
    /// for `exception.param, exception.options = name, options`
    /// when re-raising InvalidParameterError). The compiler
    /// emits the receiver expression + `Op::Swap` to land
    /// `[..., recv, val]` on the stack, then dispatches the
    /// `name=` setter with arity 1 and pops the return.
    Call { receiver: Box<SExpr>, name: String },
    /// `CONST` on the LHS of a multi-write — `MAJOR, MINOR, *REST =
    /// ...` (rake/version.rb:6). Emits the same `Op::StoreConst`
    /// (plus the class-path-prefixed alias) a plain `CONST = ...`
    /// would; only bare ConstantTargetNode is handled (not the
    /// `Foo::BAR` ConstantPath form — rarer; would need the parent
    /// cref resolved).
    Const(String),
    /// `*REST` splat into a constant. Same store as `Const`, fed the
    /// gathered rest Array.
    SplatConst(String),
    /// `obj[idx, ...] = …` — `[]=` index-write target.
    /// Surfaced by `arr[0], arr[1] = a, b` / `h[k1], h[k2] = a, b`
    /// shapes. Args can be empty (`arr[] = v` — append) but
    /// usually one or two index expressions. The compiler stores
    /// the RHS into a synthetic local, builds the
    /// `[recv, idx1, ..., idxN, val]` stack via local load, then
    /// dispatches `[]=` with arity `args.len() + 1`. Mirrors the
    /// `MWT::Call` shape for symmetry; the dedicated `Index`
    /// variant keeps the arity-N argument list distinct from
    /// `Call`'s always-arity-1 setter dispatch.
    Index { receiver: Box<SExpr>, args: Vec<SExpr> },
    /// Nested / parenthesized destructuring target — `(a, b)` in
    /// `(a, b), c = ...` or `a, (b, *c) = ...`. The element the outer
    /// massign assigns to this position is itself destructured into the
    /// inner target list (recursively, including its own splat). Prism
    /// models it as a `MultiTargetNode`. Surfaced by parser/current
    /// (`(line, _), col = ...` shapes in its generated lexer).
    Nested(Vec<MultiWriteTarget>),
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
/// Translate a Prism `BlockNode` (`{ |params| body }` / `do |params|
/// body end`) into the rubyrs `(block_params, block_body)` pair.
/// Shared by the regular `CallWithBlock` path and `super do … end`
/// (`Expr::SuperWithBlock`). Mirrors the param subset the call path
/// models: required + destructure + `*rest` + `&blk` + `**kwrest`
/// (optionals / explicit keywords aren't modelled).
fn tr_block_node(
    ctx: &mut TranslationCtx<'_>,
    bn: &ruby_prism::BlockNode<'_>,
) -> (Vec<BlockParam>, Vec<SExpr>) {
    fn parse_one(n: &ruby_prism::Node<'_>) -> Option<BlockParam> {
        if let Some(rp) = n.as_required_parameter_node() {
            return Some(BlockParam::Single(cid_to_string(rp.name())));
        }
        if let Some(mt) = n.as_multi_target_node() {
            let inners: Vec<BlockParam> = mt
                .lefts()
                .iter()
                .filter_map(|inner| parse_one(&inner))
                .collect();
            return Some(BlockParam::Destructure(inners));
        }
        None
    }
    // A block's parameters node is one of three shapes: explicit
    // `|a, b|` (BlockParametersNode), implicit numbered `_1`/`_2`
    // (NumberedParametersNode), or the Ruby 3.4 implicit `it`
    // (ItParametersNode). The latter two carry no named param list —
    // synthesize the implicit slots so invoke_block binds the yielded
    // args to them (and auto-splats for `_2`+ just like an explicit
    // two-param block). Without this they were dropped and `_1` / `it`
    // read back as nil.
    // Optional-keyword defaults (`|k: expr|`) collected during the
    // params walk, desugared into a body prologue below.
    let mut kw_defaults: Vec<(String, SExpr)> = Vec::new();
    let block_params: Vec<BlockParam> = match bn.parameters() {
        None => Vec::new(),
        Some(pn) => {
            if let Some(np) = pn.as_numbered_parameters_node() {
                // `maximum()` is the highest `_N` used; `_1`..`_N`.
                (1..=np.maximum())
                    .map(|i| BlockParam::Single(format!("_{i}")))
                    .collect()
            } else if pn.as_it_parameters_node().is_some() {
                vec![BlockParam::Single("it".to_string())]
            } else {
                pn.as_block_parameters_node()
                    .and_then(|bp| bp.parameters())
                    .map(|p| {
                        let mut out: Vec<BlockParam> =
                            p.requireds().iter().filter_map(|r| parse_one(&r)).collect();
                        // Optional positionals come AFTER requireds and
                        // BEFORE rest (`|a, b = 1, *c|`).
                        walk_block_optionals(ctx, &p, &mut out, &mut kw_defaults);
                        if let Some(rest) = p.rest() {
                            if let Some(rp) = rest.as_rest_parameter_node() {
                                let name = rp.name().map(cid_to_string).unwrap_or_default();
                                out.push(BlockParam::Rest(name));
                            } else if rest.as_implicit_rest_node().is_some() {
                                // `|name,|` — the trailing comma is an
                                // ImplicitRestNode. Its presence makes the
                                // block multi-arg, so a single yielded Array
                                // auto-splats (`name` binds arr[0], the rest
                                // is discarded). Without an explicit rest slot
                                // the block stayed single-arg and `name` got
                                // the whole Array — rss's iTunes parser does
                                // `[["name"],["email"]].each { |name,| … }`.
                                out.push(BlockParam::Rest(String::new()));
                            }
                        }
                        if let Some(b) = p.block() {
                            let name = b.name().map(cid_to_string).unwrap_or_else(|| "&".to_string());
                            out.push(BlockParam::BlockArg(name));
                        }
                        if let Some(kr) = p.keyword_rest()
                            && let Some(krp) = kr.as_keyword_rest_parameter_node()
                        {
                            let name = krp.name().map(cid_to_string).unwrap_or_default();
                            out.push(BlockParam::KwRest(name));
                        }
                        // Keyword params LAST (after BlockArg/KwRest) so
                        // their slots don't shift the by-position slot
                        // math the define_method-as-method binder does
                        // over `proto.params` — see BlockParam::Keyword.
                        walk_block_keywords(ctx, &p, &mut out, &mut kw_defaults);
                        out
                    })
                    .unwrap_or_default()
            }
        }
    };
    let mut block_body: Vec<SExpr> = match bn.body() {
        Some(b) => {
            if let Some(stmts) = b.as_statements_node() {
                stmts.body().iter().map(|c| tr(ctx, &c)).collect()
            } else {
                vec![tr(ctx, &b)]
            }
        }
        None => vec![],
    };
    prepend_kw_default_prologue(&mut block_body, kw_defaults);
    (block_params, block_body)
}

/// Shared optionals()-walk for block + lambda param lists: an
/// optional POSITIONAL parameter (`|a, b = 10|` / `->(a, b = 10)`)
/// takes a normal positional slot (so binding fills it from args
/// left-to-right) and its default is desugared into the body prologue
/// as `b = <default> if b.nil?` — reusing the keyword-default
/// mechanism. Must be called RIGHT AFTER the requireds and BEFORE the
/// rest param so slot order matches the source. Pre-fix these were
/// dropped entirely (`->(a, b=10){}.call(1)` → `[1, nil]`).
/// Documented divergence (shared with kw defaults): an explicit `nil`
/// argument also triggers the default (CRuby keeps the nil).
fn walk_block_optionals(
    ctx: &mut TranslationCtx<'_>,
    p: &ruby_prism::ParametersNode<'_>,
    out: &mut Vec<BlockParam>,
    kw_defaults: &mut Vec<(String, SExpr)>,
) {
    for opt in p.optionals().iter() {
        if let Some(op) = opt.as_optional_parameter_node() {
            let name = cid_to_string(op.name());
            kw_defaults.push((name.clone(), tr(ctx, &op.value())));
            out.push(BlockParam::Optional(name));
        }
    }
}

/// Shared keywords()-walk for block + lambda param lists: pushes a
/// `BlockParam::Keyword` per `RequiredKeywordParameterNode` /
/// `OptionalKeywordParameterNode`, collecting optional defaults
/// (any expression) for the body-prologue desugar.
fn walk_block_keywords(
    ctx: &mut TranslationCtx<'_>,
    p: &ruby_prism::ParametersNode<'_>,
    out: &mut Vec<BlockParam>,
    kw_defaults: &mut Vec<(String, SExpr)>,
) {
    for kw in p.keywords().iter() {
        if let Some(rk) = kw.as_required_keyword_parameter_node() {
            out.push(BlockParam::Keyword(cid_to_string(rk.name()), true));
        } else if let Some(ok) = kw.as_optional_keyword_parameter_node() {
            let name = cid_to_string(ok.name());
            kw_defaults.push((name.clone(), tr(ctx, &ok.value())));
            out.push(BlockParam::Keyword(name, false));
        }
    }
}

/// Desugar optional-keyword defaults into a body prologue: each
/// `|k: expr|` becomes `k = expr if k.nil?` at the head of the
/// block body. The binder writes Nil into an optional kw slot the
/// caller didn't supply (kw slots are param slots, BELOW
/// `block_body_local_start`, so without the explicit Nil-write a
/// stale value would leak across invocations — invoke_block handles
/// that; this prologue only turns that Nil into the default).
/// Documented divergence: an explicit `k: nil` argument also
/// triggers the default (CRuby keeps the nil).
fn prepend_kw_default_prologue(body: &mut Vec<SExpr>, kw_defaults: Vec<(String, SExpr)>) {
    for (name, default) in kw_defaults.into_iter().rev() {
        let span = default.span;
        let nil_check = Spanned::new(span, Expr::Call {
            receiver: Some(Box::new(Spanned::new(span, Expr::LVarRead(name.clone())))),
            name: "nil?".to_string(),
            args: vec![],
            kwargs_trailing: false,
        });
        let assign = Spanned::new(span, Expr::LVarWrite(name, Box::new(default)));
        body.insert(0, Spanned::new(span, Expr::If {
            cond: Box::new(nil_check),
            then_body: vec![assign],
            else_body: vec![],
        }));
    }
}

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
            buf.push((tr(ctx, &an.key()), tr_assoc_value(ctx, &an)));
        } else if let Some(spn) = el.as_assoc_splat_node() {
            // `**h` keyword splat, OR anonymous `**` forwarding
            // (Ruby 3.2+: `def m(**); n(**); end`) where the splat
            // carries no value — read the kwrest slot the enclosing
            // `def m(**)` bound. The compiler maps an anonymous (empty
            // name) kwrest to the reserved `__kw_rest_anon` slot
            // (compiler.rs `slot_name` remap), so read that. `**nil`
            // has a value (a NilNode), handled by the Some arm.
            let inner_expr = match spn.value() {
                Some(inner) => tr(ctx, &inner),
                None => sp(kh_anchor, Expr::LVarRead("__kw_rest_anon".to_string())),
            };
            if !buf.is_empty() {
                chunks.push(sp(kh_anchor, Expr::HashLit(std::mem::take(&mut buf))));
            }
            chunks.push(inner_expr);
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

/// Wrap a splat-call's trailing keyword hash so an EMPTY one
/// contributes nothing. Builds `[hash].reject { |__kws| __kws.empty? }`
/// — `[]` when the hash is empty (`n(*a, **{})`), `[hash]` otherwise.
/// The splat path concatenates this chunk, so an empty kwargs hash
/// never lands as a phantom positional arg (it can't be peeled later —
/// the assembled args Array is opaque by dispatch time). A non-empty
/// hash survives and `invoke_method` peels it as kwargs exactly as
/// before. Mirrors `do_call_kw`'s empty-`**` drop for the non-splat
/// path.
fn kwsplat_chunk(anchor: &Node<'_>, kwhash: SExpr) -> SExpr {
    let arr = sp(anchor, Expr::ArrayLit(vec![kwhash]));
    sp(anchor, Expr::CallWithBlock {
        receiver: Some(Box::new(arr)),
        name: "reject".into(),
        args: vec![],
        block_params: vec![BlockParam::Single("__kws".into())],
        kwargs_trailing: false,
        block_body: vec![sp(anchor, Expr::Call {
            receiver: Some(Box::new(sp(anchor, Expr::LVarRead("__kws".into())))),
            name: "empty?".into(),
            args: vec![],
            kwargs_trailing: false,
        })],
    })
}

// ===== Pattern matching (`case/in`, `expr => pat`, `expr in pat`) =====
//
// Desugared entirely at AST translation: each pattern compiles to a
// BOOLEAN expression that, evaluated against a subject already bound to a
// simple local, returns truthy on a match and binds any pattern variables
// as a side effect (so a later body / the surrounding scope sees them).
// Structural patterns call CRuby's `deconstruct` / `deconstruct_keys`
// protocol; value patterns use `===`. No new opcodes — the desugar is
// `&&`/`||` short-circuit chains over method calls and assignments.

// `name = value` that always yields truthy, for `&&` chaining.
fn pm_bind(anchor: &Node<'_>, name: &str, value: SExpr) -> SExpr {
    sp(anchor, seq_inner(vec![
        sp(anchor, Expr::LVarWrite(name.to_string(), Box::new(value))),
        sp(anchor, Expr::BoolLit(true)),
    ]))
}

/// Fixed expected length of a top-level array pattern WITHOUT a
/// rest splat (`[Integer, Integer]` → Some(2); `[a, *r]` → None).
/// Feeds the CRuby-shaped "length mismatch (given N, expected M)"
/// tail of NoMatchingPatternError via __rubyrs_pm_fail_msg —
/// minitest's assert_pattern asserts /length mismatch/.
fn pm_fixed_array_len(pat: &Node<'_>) -> Option<usize> {
    let ap = pat.as_array_pattern_node()?;
    if ap.rest().is_some() || !ap.posts().is_empty() {
        return None;
    }
    Some(ap.requireds().len())
}

fn pm_fail_msg_args(anchor: &Node<'_>, subj: &str, pat: &Node<'_>) -> Vec<SExpr> {
    let mut args = vec![
        sp(anchor, Expr::ConstRead("NoMatchingPatternError".into())),
    ];
    let msg = match pm_fixed_array_len(pat) {
        Some(n) => sp(anchor, Expr::Call {
            receiver: None,
            name: "__rubyrs_pm_fail_msg".into(),
            args: vec![
                pm_lvar(anchor, subj),
                sp(anchor, Expr::IntLit(n as i64)),
            ],
            kwargs_trailing: false,
        }),
        None => pm_meth(anchor, pm_lvar(anchor, subj), "inspect", vec![]),
    };
    args.push(msg);
    args
}

fn pm_lvar(anchor: &Node<'_>, name: &str) -> SExpr {
    sp(anchor, Expr::LVarRead(name.to_string()))
}

fn pm_meth(anchor: &Node<'_>, recv: SExpr, name: &str, args: Vec<SExpr>) -> SExpr {
    sp(anchor, Expr::Call {
        receiver: Some(Box::new(recv)),
        name: name.to_string(),
        args,
        kwargs_trailing: false,
    })
}

fn pm_and(anchor: &Node<'_>, a: SExpr, b: SExpr) -> SExpr {
    sp(anchor, Expr::And(Box::new(a), Box::new(b)))
}

// Fold a list of boolean checks into a single short-circuit `&&` chain;
// an empty list is the literal `true`.
fn pm_all(anchor: &Node<'_>, checks: Vec<SExpr>) -> SExpr {
    let mut it = checks.into_iter();
    match it.next() {
        Some(first) => it.fold(first, |a, b| pm_and(anchor, a, b)),
        None => sp(anchor, Expr::BoolLit(true)),
    }
}

impl TranslationCtx<'_> {
    fn fresh_pm(&self) -> String {
        let c = self.safe_nav_count.get();
        self.safe_nav_count.set(c + 1);
        format!("__pm_{c}")
    }
}

/// `a..b` / `a...b` in boolean context — a flip-flop. Stateful: stays
/// "off" until `a` is truthy (turns on), then "on" until `b` is truthy
/// (turns off); a 2-dot also checks `b` on the same eval that `a` flips
/// it on, a 3-dot defers that to the next eval. State is held in a hidden
/// global `$__pm_N` (initial read = nil = off). A global — not a local —
/// because CRuby keeps the state across iterations of an enclosing block
/// (`(1..8).each { print _1 if (_1==2)..(_1==4) }` → `234`), and rubyrs's
/// per-invocation block locals would reset each time. Tier-1 divergence:
/// the global isn't reset on method re-entry the way CRuby's scope-local
/// flip-flop state is — flip-flops in a re-called method keep prior state.
#[inline(never)]
fn tr_flip_flop(ctx: &mut TranslationCtx<'_>, node: &Node<'_>, ff: &ruby_prism::FlipFlopNode<'_>) -> SExpr {
    let g = format!("${}", ctx.fresh_pm());
    let set = |v: bool| sp(node, Expr::GVarWrite(g.clone(), Box::new(sp(node, Expr::BoolLit(v)))));
    let a = ff.left().map(|n| tr(ctx, &n)).unwrap_or_else(|| sp(node, Expr::BoolLit(false)));
    // `if b then $g = false end` — translate `b` fresh at each use site.
    let off_if_b = |ctx: &mut TranslationCtx<'_>| {
        let b = ff.right().map(|n| tr(ctx, &n)).unwrap_or_else(|| sp(node, Expr::BoolLit(false)));
        sp(node, Expr::If { cond: Box::new(b), then_body: vec![set(false)], else_body: vec![] })
    };
    // State already on: maybe turn off, but this eval is still true.
    let on_branch = sp(node, seq_inner(vec![off_if_b(ctx), sp(node, Expr::BoolLit(true))]));
    // Off → on transition (a truthy): turn on; a 2-dot checks b now.
    let mut trans = vec![set(true)];
    if !ff.is_exclude_end() {
        trans.push(off_if_b(ctx));
    }
    trans.push(sp(node, Expr::BoolLit(true)));
    let transition = sp(node, seq_inner(trans));
    sp(node, Expr::If {
        cond: Box::new(sp(node, Expr::GVarRead(g.clone()))),
        then_body: vec![on_branch],
        else_body: vec![sp(node, Expr::If {
            cond: Box::new(a),
            then_body: vec![transition],
            else_body: vec![sp(node, Expr::BoolLit(false))],
        })],
    })
}

/// Handle the three pattern-matching node families (`case/in`,
/// `expr => pat`, `expr in pat`). Returns `None` for any other node so
/// `tr` falls through. `#[inline(never)]` keeps its large local set off
/// `tr`'s recursive frame (see the call site).
#[inline(never)]
fn tr_pattern_construct(ctx: &mut TranslationCtx<'_>, node: &Node<'_>) -> Option<SExpr> {
    if let Some(cm) = node.as_case_match_node() {
        // `case subj; in pat [if guard]; body; ...; [else body]; end`.
        // Bind the subject to a fresh local (evaluated once), then build
        // an if/elsif chain: arm N's else-branch is arm N+1's test. With
        // no `else` and nothing matched, raise NoMatchingPatternError.
        let subj = ctx.fresh_pm();
        let subj_expr = cm.predicate().map(|s| tr(ctx, &s)).unwrap_or_else(|| sp(node, Expr::Nil));
        // Innermost else: the `else` clause body, or the no-match raise.
        let mut else_body: Vec<SExpr> = match cm.else_clause() {
            Some(e) => e.statements().map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect()).unwrap_or_default(),
            None => {
                // Single-arm `case/in` gets the length-aware
                // message (the failing pattern is unambiguous);
                // multi-arm keeps the bare inspect form.
                let arms: Vec<_> = cm.conditions().iter().collect();
                let single_pat = if arms.len() == 1 {
                    arms[0].as_in_node().map(|inn| inn.pattern())
                } else {
                    None
                };
                let args = match &single_pat {
                    Some(p) => pm_fail_msg_args(node, &subj, p),
                    None => vec![
                        sp(node, Expr::ConstRead("NoMatchingPatternError".into())),
                        pm_meth(node, pm_lvar(node, &subj), "inspect", vec![]),
                    ],
                };
                vec![sp(node, Expr::Call {
                    receiver: None,
                    name: "raise".into(),
                    args,
                    kwargs_trailing: false,
                })]
            }
        };
        // Fold the `in` arms in reverse so each becomes the else of the
        // previous If.
        let arms: Vec<_> = cm.conditions().iter().collect();
        for arm in arms.iter().rev() {
            let Some(inn) = arm.as_in_node() else { continue };
            let pat_node = inn.pattern();
            // A guard (`in pat if cond` / `unless cond`) parses as an
            // If/UnlessNode wrapping the real pattern in its statements.
            // Prism `Node` isn't Clone, so compile inline per branch
            // rather than extracting a `real_pat` variable.
            let cond = if let Some(ifn) = pat_node.as_if_node() {
                let inner = ifn.statements().and_then(|s| s.body().iter().next());
                let base = match &inner {
                    Some(p) => compile_pattern(ctx, &subj, p),
                    None => compile_pattern(ctx, &subj, &pat_node),
                };
                pm_and(node, base, tr(ctx, &ifn.predicate()))
            } else if let Some(un) = pat_node.as_unless_node() {
                let inner = un.statements().and_then(|s| s.body().iter().next());
                let base = match &inner {
                    Some(p) => compile_pattern(ctx, &subj, p),
                    None => compile_pattern(ctx, &subj, &pat_node),
                };
                // `unless cond` — match requires `!cond`.
                let g = sp(node, Expr::Call {
                    receiver: Some(Box::new(tr(ctx, &un.predicate()))),
                    name: "!".into(), args: vec![], kwargs_trailing: false });
                pm_and(node, base, g)
            } else {
                compile_pattern(ctx, &subj, &pat_node)
            };
            let body: Vec<SExpr> = inn.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
                .unwrap_or_else(|| vec![sp(node, Expr::Nil)]);
            let if_expr = sp(node, Expr::If {
                cond: Box::new(cond),
                then_body: body,
                else_body: std::mem::take(&mut else_body),
            });
            else_body = vec![if_expr];
        }
        // `else_body` now holds the head of the chain.
        let mut seq = vec![sp(node, Expr::LVarWrite(subj.clone(), Box::new(subj_expr)))];
        seq.extend(else_body);
        return Some(sp(node, seq_inner(seq)));
    }
    if let Some(mr) = node.as_match_required_node() {
        // `value => pattern` — raises NoMatchingPatternError on no match,
        // binds on success, evaluates to nil.
        let subj = ctx.fresh_pm();
        let val = tr(ctx, &mr.value());
        let cond = compile_pattern(ctx, &subj, &mr.pattern());
        let raise = sp(node, Expr::Call {
            receiver: None,
            name: "raise".into(),
            args: pm_fail_msg_args(node, &subj, &mr.pattern()),
            kwargs_trailing: false,
        });
        let check = sp(node, Expr::If {
            cond: Box::new(cond),
            then_body: vec![sp(node, Expr::Nil)],
            else_body: vec![raise],
        });
        return Some(sp(node, seq_inner(vec![
            sp(node, Expr::LVarWrite(subj.clone(), Box::new(val))),
            check,
        ])));
    }
    if let Some(mp) = node.as_match_predicate_node() {
        // `value in pattern` — true/false, binds on success.
        let subj = ctx.fresh_pm();
        let val = tr(ctx, &mp.value());
        let cond = compile_pattern(ctx, &subj, &mp.pattern());
        return Some(sp(node, seq_inner(vec![
            sp(node, Expr::LVarWrite(subj.clone(), Box::new(val))),
            cond,
        ])));
    }
    None
}

/// Compile `pat` against the value held in local `subj`. Returns a
/// boolean SExpr (truthy = match, binds variables as a side effect).
fn compile_pattern(ctx: &mut TranslationCtx<'_>, subj: &str, pat: &Node<'_>) -> SExpr {
    // Variable binding: `x` (a lowercase identifier in pattern position
    // parses as LocalVariableTargetNode). Always matches, binding subj.
    if let Some(t) = pat.as_local_variable_target_node() {
        let name = cid_to_string(t.name());
        // `_` and `_name` bind too (CRuby allows repeated `_`).
        return pm_bind(pat, &name, pm_lvar(pat, subj));
    }
    // `pat => name` — match pat, then bind the whole subject to name.
    if let Some(cp) = pat.as_capture_pattern_node() {
        let inner = compile_pattern(ctx, subj, &cp.value());
        let name = cid_to_string(cp.target().name());
        return pm_and(pat, inner, pm_bind(pat, &name, pm_lvar(pat, subj)));
    }
    // `a | b` — alternation (no binding, per Ruby). Match either side.
    if let Some(alt) = pat.as_alternation_pattern_node() {
        let l = compile_pattern(ctx, subj, &alt.left());
        let r = compile_pattern(ctx, subj, &alt.right());
        return sp(pat, Expr::Or(Box::new(l), Box::new(r)));
    }
    // `^x` / `^(expr)` — pinned value; match by `=== subj` against the
    // pinned local / expression (evaluated in the surrounding scope).
    if let Some(pv) = pat.as_pinned_variable_node() {
        let val = tr(ctx, &pv.variable());
        return pm_meth(pat, val, "===", vec![pm_lvar(pat, subj)]);
    }
    if let Some(pe) = pat.as_pinned_expression_node() {
        let val = tr(ctx, &pe.expression());
        return pm_meth(pat, val, "===", vec![pm_lvar(pat, subj)]);
    }
    if let Some(ap) = pat.as_array_pattern_node() {
        return compile_array_pattern(ctx, subj, &ap, pat);
    }
    if let Some(hp) = pat.as_hash_pattern_node() {
        return compile_hash_pattern(ctx, subj, &hp, pat);
    }
    if let Some(fp) = pat.as_find_pattern_node() {
        return compile_find_pattern(ctx, subj, &fp, pat);
    }
    // Everything else is a value pattern: literal, range, regexp, a bare
    // `Constant` / `nil` / `true` / `false`, etc. Match with `===`.
    let val = tr(ctx, pat);
    pm_meth(pat, val, "===", vec![pm_lvar(pat, subj)])
}

/// `[req…, *rest, post…]` (optionally `Const[…]`). Uses `deconstruct`.
fn compile_array_pattern(
    ctx: &mut TranslationCtx<'_>,
    subj: &str,
    ap: &ruby_prism::ArrayPatternNode<'_>,
    anchor: &Node<'_>,
) -> SExpr {
    let mut checks: Vec<SExpr> = Vec::new();
    // `Const[...]` — the value must also be a `Const` (=== check first).
    if let Some(c) = ap.constant() {
        let cexpr = tr(ctx, &c);
        checks.push(pm_meth(anchor, cexpr, "===", vec![pm_lvar(anchor, subj)]));
    }
    // Must respond to `deconstruct`; bind its result to a fresh temp.
    let d = ctx.fresh_pm();
    checks.push(pm_meth(anchor, pm_lvar(anchor, subj), "respond_to?",
        vec![sp(anchor, Expr::SymbolLit("deconstruct".into()))]));
    checks.push(pm_bind(anchor, &d, pm_meth(anchor, pm_lvar(anchor, subj), "deconstruct", vec![])));
    checks.push(pm_meth(anchor, pm_lvar(anchor, &d), "is_a?",
        vec![sp(anchor, Expr::ConstRead("Array".into()))]));
    let reqs: Vec<_> = ap.requireds().iter().collect();
    let posts: Vec<_> = ap.posts().iter().collect();
    let has_rest = ap.rest().is_some();
    let len = pm_meth(anchor, pm_lvar(anchor, &d), "length", vec![]);
    let min = (reqs.len() + posts.len()) as i64;
    if has_rest {
        checks.push(pm_meth(anchor, len, ">=", vec![sp(anchor, Expr::IntLit(min))]));
    } else {
        checks.push(pm_meth(anchor, len, "==", vec![sp(anchor, Expr::IntLit(min))]));
    }
    // Pre-rest requireds: d[i].
    for (i, req) in reqs.iter().enumerate() {
        let e = ctx.fresh_pm();
        checks.push(pm_bind(anchor, &e,
            pm_meth(anchor, pm_lvar(anchor, &d), "[]", vec![sp(anchor, Expr::IntLit(i as i64))])));
        checks.push(compile_pattern(ctx, &e, req));
    }
    // Rest: `*name` binds the middle slice `d[reqs.len ... d.length-posts.len]`.
    if let Some(rest) = ap.rest()
        && let Some(sn) = rest.as_splat_node()
        && let Some(expr) = sn.expression()
        && let Some(t) = expr.as_local_variable_target_node()
    {
        let name = cid_to_string(t.name());
        // d[reqs.len .. -(posts.len+1)] — inclusive end index via a Range.
        let from = sp(anchor, Expr::IntLit(reqs.len() as i64));
        // d[from, count] where count = length - reqs - posts.
        let count = pm_meth(anchor, pm_meth(anchor, pm_lvar(anchor, &d), "length", vec![]),
            "-", vec![sp(anchor, Expr::IntLit((reqs.len() + posts.len()) as i64))]);
        let slice = pm_meth(anchor, pm_lvar(anchor, &d), "[]", vec![from, count]);
        checks.push(pm_bind(anchor, &name, slice));
    }
    // Post-rest requireds: indexed from the end, d[-(posts.len-j)].
    for (j, post) in posts.iter().enumerate() {
        let idx = -((posts.len() - j) as i64);
        let e = ctx.fresh_pm();
        checks.push(pm_bind(anchor, &e,
            pm_meth(anchor, pm_lvar(anchor, &d), "[]", vec![sp(anchor, Expr::IntLit(idx))])));
        checks.push(compile_pattern(ctx, &e, post));
    }
    pm_all(anchor, checks)
}

/// `{k:, l: pat, **rest}` (optionally `Const(…)`). Uses `deconstruct_keys`.
fn compile_hash_pattern(
    ctx: &mut TranslationCtx<'_>,
    subj: &str,
    hp: &ruby_prism::HashPatternNode<'_>,
    anchor: &Node<'_>,
) -> SExpr {
    let mut checks: Vec<SExpr> = Vec::new();
    if let Some(c) = hp.constant() {
        let cexpr = tr(ctx, &c);
        checks.push(pm_meth(anchor, cexpr, "===", vec![pm_lvar(anchor, subj)]));
    }
    let h = ctx.fresh_pm();
    checks.push(pm_meth(anchor, pm_lvar(anchor, subj), "respond_to?",
        vec![sp(anchor, Expr::SymbolLit("deconstruct_keys".into()))]));
    checks.push(pm_bind(anchor, &h, pm_meth(anchor, pm_lvar(anchor, subj), "deconstruct_keys",
        vec![sp(anchor, Expr::Nil)])));
    checks.push(pm_meth(anchor, pm_lvar(anchor, &h), "is_a?",
        vec![sp(anchor, Expr::ConstRead("Hash".into()))]));
    let mut matched_keys: Vec<String> = Vec::new();
    for el in hp.elements().iter() {
        if let Some(an) = el.as_assoc_node() {
            // Key is a SymbolNode (`k:`); extract its name.
            let key_name = an.key().as_symbol_node()
                .map(|s| String::from_utf8_lossy(s.unescaped()).into_owned());
            let Some(key_name) = key_name else {
                ctx.errors.push("unsupported node: non-symbol hash pattern key".to_string());
                continue;
            };
            matched_keys.push(key_name.clone());
            let key_sym = sp(anchor, Expr::SymbolLit(key_name.clone()));
            // Key must be present.
            checks.push(pm_meth(anchor, pm_lvar(anchor, &h), "key?", vec![key_sym.clone()]));
            // Bind the value to a temp, then match the sub-pattern.
            let v = ctx.fresh_pm();
            checks.push(pm_bind(anchor, &v, pm_meth(anchor, pm_lvar(anchor, &h), "[]", vec![key_sym])));
            // Shorthand `{k:}` — Prism's AssocNode value is an
            // ImplicitNode wrapping the binding; bind the key name.
            if an.value().as_implicit_node().is_some() {
                checks.push(pm_bind(anchor, &key_name, pm_lvar(anchor, &v)));
            } else {
                checks.push(compile_pattern(ctx, &v, &an.value()));
            }
        }
    }
    // Keyword rest: `**rest` binds the leftover keys; `**nil` asserts none.
    if let Some(rest) = hp.rest() {
        if rest.as_no_keywords_parameter_node().is_some() {
            // `**nil` — the value must have EXACTLY the matched keys.
            checks.push(pm_meth(anchor, pm_meth(anchor, pm_lvar(anchor, &h), "length", vec![]),
                "==", vec![sp(anchor, Expr::IntLit(matched_keys.len() as i64))]));
        } else if let Some(ar) = rest.as_assoc_splat_node()
            && let Some(val) = ar.value()
            && let Some(t) = val.as_local_variable_target_node()
        {
            // `**rest` — bind a Hash of the unmatched keys: h.reject { |k,_| matched.include?(k) }.
            let name = cid_to_string(t.name());
            let key_list = sp(anchor, Expr::ArrayLit(
                matched_keys.iter().map(|k| sp(anchor, Expr::SymbolLit(k.clone()))).collect()));
            let rest_hash = sp(anchor, Expr::CallWithBlock {
                receiver: Some(Box::new(pm_lvar(anchor, &h))),
                name: "reject".into(),
                args: vec![],
                block_params: vec![BlockParam::Single("__pk".into()), BlockParam::Single("__pv".into())],
                block_body: vec![pm_meth(anchor, key_list, "include?", vec![pm_lvar(anchor, "__pk")])],
                kwargs_trailing: false,
            });
            checks.push(pm_bind(anchor, &name, rest_hash));
        }
    }
    pm_all(anchor, checks)
}

/// `[*pre, m…, *post]` — find a consecutive run matching the middle
/// patterns; `pre`/`post` bind the slices before/after the FIRST such
/// run. Two-phase to keep bindings in the arm's scope: phase 1 finds the
/// start index in a `find` block (bindings there are block-local, fine);
/// phase 2 re-runs the middle matches at that fixed index in the outer
/// `&&` chain so the variables leak to the arm body, then slices pre/post.
fn compile_find_pattern(
    ctx: &mut TranslationCtx<'_>,
    subj: &str,
    fp: &ruby_prism::FindPatternNode<'_>,
    anchor: &Node<'_>,
) -> SExpr {
    let mut checks: Vec<SExpr> = Vec::new();
    if let Some(c) = fp.constant() {
        let cexpr = tr(ctx, &c);
        checks.push(pm_meth(anchor, cexpr, "===", vec![pm_lvar(anchor, subj)]));
    }
    let d = ctx.fresh_pm();
    checks.push(pm_meth(anchor, pm_lvar(anchor, subj), "respond_to?",
        vec![sp(anchor, Expr::SymbolLit("deconstruct".into()))]));
    checks.push(pm_bind(anchor, &d, pm_meth(anchor, pm_lvar(anchor, subj), "deconstruct", vec![])));
    checks.push(pm_meth(anchor, pm_lvar(anchor, &d), "is_a?",
        vec![sp(anchor, Expr::ConstRead("Array".into()))]));
    let mids: Vec<_> = fp.requireds().iter().collect();
    let k = mids.len() as i64;
    let len = pm_meth(anchor, pm_lvar(anchor, &d), "length", vec![]);
    checks.push(pm_meth(anchor, len, ">=", vec![sp(anchor, Expr::IntLit(k))]));
    // Phase 1: locate the start index. find over 0..(len-k); the block
    // tests the middle patterns at d[i+j] (its bindings are block-local).
    let fi = ctx.fresh_pm();
    let iparam = ctx.fresh_pm();
    let mut detect: Vec<SExpr> = Vec::new();
    for (j, m) in mids.iter().enumerate() {
        let e = ctx.fresh_pm();
        let idx = pm_meth(anchor, pm_lvar(anchor, &iparam), "+", vec![sp(anchor, Expr::IntLit(j as i64))]);
        detect.push(pm_bind(anchor, &e, pm_meth(anchor, pm_lvar(anchor, &d), "[]", vec![idx])));
        detect.push(compile_pattern(ctx, &e, m));
    }
    let upper = pm_meth(anchor, pm_meth(anchor, pm_lvar(anchor, &d), "length", vec![]),
        "-", vec![sp(anchor, Expr::IntLit(k))]);
    let range = sp(anchor, Expr::RangeLit {
        begin: Box::new(sp(anchor, Expr::IntLit(0))),
        end: Box::new(upper),
        exclusive: false,
    });
    let find_call = sp(anchor, Expr::CallWithBlock {
        receiver: Some(Box::new(range)),
        name: "find".into(),
        args: vec![],
        block_params: vec![BlockParam::Single(iparam.clone())],
        block_body: vec![pm_all(anchor, detect)],
        kwargs_trailing: false,
    });
    checks.push(pm_bind(anchor, &fi, find_call));
    // `find` returns nil on no match (0 is truthy in Ruby, so a hit at
    // index 0 still passes); require non-nil explicitly.
    checks.push(sp(anchor, Expr::Call {
        receiver: Some(Box::new(pm_meth(anchor, pm_lvar(anchor, &fi), "nil?", vec![]))),
        name: "!".into(), args: vec![], kwargs_trailing: false,
    }));
    // Phase 2: re-run the middle matches at the found index so the
    // variables bind in the arm's scope.
    for (j, m) in mids.iter().enumerate() {
        let e = ctx.fresh_pm();
        let idx = pm_meth(anchor, pm_lvar(anchor, &fi), "+", vec![sp(anchor, Expr::IntLit(j as i64))]);
        checks.push(pm_bind(anchor, &e, pm_meth(anchor, pm_lvar(anchor, &d), "[]", vec![idx])));
        checks.push(compile_pattern(ctx, &e, m));
    }
    // `*pre` / `*post` slices (named only).
    if let Some(t) = fp.left().expression().and_then(|e| e.as_local_variable_target_node()) {
        let name = cid_to_string(t.name());
        let slice = pm_meth(anchor, pm_lvar(anchor, &d), "[]",
            vec![sp(anchor, Expr::IntLit(0)), pm_lvar(anchor, &fi)]);
        checks.push(pm_bind(anchor, &name, slice));
    }
    if let Some(sn) = fp.right().as_splat_node()
        && let Some(t) = sn.expression().and_then(|e| e.as_local_variable_target_node())
    {
        let name = cid_to_string(t.name());
        let start = pm_meth(anchor, pm_lvar(anchor, &fi), "+", vec![sp(anchor, Expr::IntLit(k))]);
        let cnt = pm_meth(anchor, pm_lvar(anchor, &d), "length", vec![]);
        let slice = pm_meth(anchor, pm_lvar(anchor, &d), "[]", vec![start, cnt]);
        checks.push(pm_bind(anchor, &name, slice));
    }
    pm_all(anchor, checks)
}


/// Decide whether a `class << recv` body must run as a REAL
/// eigenclass body (`Expr::SingletonClassBody`, self = metaclass)
/// rather than via the per-statement desugar. See the call site in
/// `tr_singleton_class` for the rationale behind each shape.
fn singleton_body_needs_real_eval(body_nodes: &[Node<'_>], recv_is_self: bool) -> bool {
    body_nodes.iter().any(|bn| {
        // Nested namespace definition — the desugar has no way to
        // place a `module`/`class` inside the metaclass.
        if bn.as_module_node().is_some() || bn.as_class_node().is_some() {
            return true;
        }
        // Control-flow wrapping defs — `if/elsif/else def …`,
        // `unless`, `case/when def …`. The per-statement desugar only
        // admits a single `if`/`else` of pure defs and BAILS on an
        // `elsif` chain (or `case`); the real-body path compiles the
        // whole body into its own proto and runs it with self = the
        // metaclass, so every def lands on the singleton table no
        // matter how it's nested. Surfaced by listen's
        // `MonotonicTime` (`class << self; if defined?(...) … elsif …
        // else … end`) on the Bridgetown boot path.
        if bn.as_if_node().is_some()
            || bn.as_unless_node().is_some()
            || bn.as_case_node().is_some()
        {
            return true;
        }
        // `alias new old` inside `class << <Const>` / `class << obj`
        // (NON-self receiver only). The per-statement desugar admits
        // `alias` when the receiver is the literal `self` — and that
        // self-path carries special logic (builtin-method alias
        // forwarders + the Module-`new` fence the
        // `class_self_alias_builtin` fixture pins), so leave it alone.
        // For a non-self receiver the desugar BAILS ("`alias` only
        // supported when receiver is `self`"); `class << HTTP; alias
        // is_version_1_1? version_1_1?` (HTTP = the enclosing class — i.e.
        // `class << self` written as a constant) hit that bail. Route it
        // to the real eigenclass body, which runs with self = the
        // metaclass so the alias lands on the singleton-method table
        // regardless of how the receiver was spelled. Surfaced by stdlib
        // net/http.rb (ADR 0028).
        if !recv_is_self && bn.as_alias_method_node().is_some() {
            return true;
        }
        if let Some(call) = bn.as_call_node()
            && call.receiver().is_none()
        {
            let nm = cid_to_string(call.name());
            // `include Mod` in the eigenclass body — must land on
            // the metaclass, which the desugar misroutes.
            if nm == "include" {
                return true;
            }
            // A call wrapping a `def` — `internal def foo`,
            // `private def foo`, `public def foo`, … The wrapper
            // runs at runtime with self = the metaclass; only the
            // real-body path keeps `def` and the wrapper's
            // `private`/`alias_method` on the same (singleton) table.
            if let Some(args) = call.arguments()
                && args.arguments().iter().any(|a| a.as_def_node().is_some())
            {
                return true;
            }
        }
        // A `class << <expr>` (NON-self) whose body has ANY node the
        // desugar can't handle — an arbitrary value-returning call
        // (`(class << obj; ancestors; end)`), a literal, an assignment,
        // an ivar read, etc. — is used for its VALUE (the last
        // expression). The desugar only knows the def / attr_* / alias /
        // prepend / reflective-table-call / bare-`self` shapes; route
        // everything else to the real eigenclass body, which runs with
        // self = the metaclass and yields the last expression.
        // rspec-mocks' `(class << object; ancestors; end).map { … }`
        // (space.rb) hits this. Scoped to non-self so a `class << self`
        // body's @@cvar/const desugar interplay (class_self_cvar) is
        // untouched. `self` stays on the desugar (it has a dedicated
        // arm rewriting to `recv.singleton_class`).
        if !recv_is_self
            && bn.as_def_node().is_none()
            && bn.as_alias_method_node().is_none()
            && bn.as_self_node().is_none()
            && bn.as_constant_write_node().is_none()
            && bn.as_constant_path_write_node().is_none()
            && !bn.as_call_node().map(|c| c.receiver().is_none()
                && matches!(cid_to_string(c.name()).as_str(),
                    "attr_reader" | "attr_writer" | "attr_accessor" | "prepend"
                    | "define_method" | "undef_method" | "remove_method"
                    | "alias_method" | "method_defined?" | "public_method_defined?"
                    | "private_method_defined?" | "protected_method_defined?"
                    | "instance_method")).unwrap_or(false)
        {
            return true;
        }
        // Constant assignment in the body (`PATCH_MAP = {…}`): the
        // constant belongs to the eigenclass, and the body's methods
        // reference it BARE — which only resolves when those methods
        // carry the eigenclass cref. The compile-time desugar rewrites
        // them to `def Recv.m` with the SURROUNDING cref (wrong), so a
        // bare reference can't find the const. The real eigenclass-body
        // path runs the whole body with self = the metaclass, giving
        // both the const and the methods the right scope. Surfaced by
        // diff-lcs (`class << Diff::LCS; PATCH_MAP = {…}; def …
        // PATCH_MAP[dir] … end; end`).
        //
        // NON-self only: a `class << self` body's const write is handled
        // by the desugar's dedicated arm (which keeps it coexisting with
        // class-variable writes in the SAME body — the real-eigenclass
        // path re-roots `@@cvar` on the metaclass and breaks that
        // interplay, as `class_self_cvar`'s MixedBody pins). A non-self
        // `class << Const` body has no such desugar arm and needs the
        // real path for the bare-const-reference cref anyway.
        if !recv_is_self
            && (bn.as_constant_write_node().is_some()
                || bn.as_constant_path_write_node().is_some())
        {
            return true;
        }
        false
    })
}

/// Extracted body of the `class << recv` (singleton-class) AST
/// translation. Lives in its own function so its large local set
/// (the per-body `out` / `then_body` / `else_body` Vecs, the
/// `mk_singleton_def` closure, etc.) does NOT inflate the stack
/// frame of the recursive `tr` hot path — preamble compilation
/// recurses through `tr` deeply, and on a 2 MB test thread the
/// combined frame previously overflowed in debug / coverage
/// builds. Returns `Expr::Nil` if the node isn't a singleton
/// class (unreachable via the guarded call site).
fn tr_singleton_class(ctx: &mut TranslationCtx<'_>, node: &Node<'_>) -> SExpr {
    let Some(n) = node.as_singleton_class_node() else { return sp(node, Expr::Nil); };
        let recv_expr = tr(ctx, &n.expression());
        let body_nodes: Vec<_> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().collect::<Vec<_>>()
                } else { vec![b] }
            }
            None => vec![],
        };
        // Route the WHOLE body to the real eigenclass-body op
        // (`Expr::SingletonClassBody`, self = the metaclass) when it
        // contains a shape the per-statement desugar can't express
        // faithfully:
        //   - `include Mod` — must land on the metaclass (the real
        //     class's `singleton_includes`), not its instance chain.
        //   - a nested `module`/`class` definition.
        //   - a call wrapping a `def`: `internal def foo`,
        //     `private def foo`, `public def foo`, etc. The wrapper
        //     (zeitwerk's `internal`, or the visibility keywords)
        //     runs at RUNTIME with `self` = the metaclass and calls
        //     `private`/`alias_method` on it — which only lands on
        //     the same table the `def` did when `self` really is the
        //     eigenclass. The desugar runs the body with `self` =
        //     the enclosing module, so the def goes to the singleton
        //     table but the wrapper's `private`/`alias_method` hit
        //     the instance table — an unfixable mismatch.
        // The def/attr/alias-only fast cases stay on the desugar
        // (preserving the large body of sinatra/minitest/tilt tests
        // that exercise it). Receiver-independent: works for
        // `class << self`, `class << Const`, and `class << obj`.
        if singleton_body_needs_real_eval(&body_nodes, matches!(&recv_expr.node, Expr::SelfExpr)) {
            let body: Vec<SExpr> = body_nodes.iter().map(|bn| tr(ctx, bn)).collect();
            return sp(node, Expr::SingletonClassBody {
                recv: Box::new(recv_expr),
                body,
            });
        }
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
        let body_ends_with_self = body_nodes
            .last()
            .map(|n| n.as_self_node().is_some())
            .unwrap_or(false);
        let recv_is_self_outer = matches!(&recv_expr.node, Expr::SelfExpr);
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
            // Reflective method-table calls inside `class << X` —
            // define_method / undef_method / remove_method /
            // method_defined? / alias_method and the visibility-
            // filtered probes. CRuby runs the body with self = the
            // eigenclass, so these calls operate on X's singleton
            // table. Desugar to an explicit-receiver call on
            // `RECV.singleton_class` (the dispatch arms for
            // eigenclass receivers — define_method redirect,
            // alias shells, undef tombstones — already exist).
            // minitest's i_suck_and_my_tests_are_order_dependent!
            // is `class << self; undef_method :test_order if
            // method_defined? :test_order; define_method(:test_order)
            // { :alpha }; end` inside a class method.
            if let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
                && matches!(cid_to_string(call.name()).as_str(),
                    "define_method" | "undef_method" | "remove_method"
                    | "alias_method" | "method_defined?"
                    | "public_method_defined?" | "private_method_defined?"
                    | "protected_method_defined?" | "instance_method"
                )
            {
                let receiver_expr = if needs_local {
                    sp(bn, Expr::LVarRead(synth_local.clone()))
                } else {
                    recv_expr.clone()
                };
                let sc_expr = sp(bn, Expr::Call {
                    receiver: Some(Box::new(receiver_expr)),
                    name: "singleton_class".to_string(),
                    args: vec![],
                    kwargs_trailing: false,
                });
                let translated = tr(ctx, bn);
                let rewritten = match translated.node {
                    Expr::Call { receiver: None, name, args, kwargs_trailing } => Some(sp(bn, Expr::Call {
                        receiver: Some(Box::new(sc_expr)),
                        name,
                        args,
                        kwargs_trailing,
                    })),
                    Expr::CallWithBlock {
                        receiver: None, name, args, block_params, block_body, kwargs_trailing,
                    } => Some(sp(bn, Expr::CallWithBlock {
                        receiver: Some(Box::new(sc_expr)),
                        name,
                        args,
                        block_params,
                        block_body,
                        kwargs_trailing,
                    })),
                    other => {
                        // e.g. `undef_method :x if cond` arrives as a
                        // ConditionalNode wrapping the call — handled
                        // by the if/unless admission arm below, which
                        // re-enters this loop body shape. Fall through
                        // by re-wrapping the translated node untouched.
                        out.push(sp(bn, other));
                        continue;
                    }
                };
                if let Some(r) = rewritten {
                    out.push(r);
                }
                continue;
            }
            // `private :m` / `public :m` (with explicit method-name args)
            // inside `class << X` — set the visibility of X's SINGLETON
            // method `m`. Desugar to `X.private_class_method(:m)` /
            // `X.public_class_method(:m)` (the dispatch arms that flip
            // singleton-method visibility). diff/lcs's
            // `class << Diff::LCS::Internals; … private :diff_traversal;`.
            // Bare `private` (no args, the default-visibility toggle) and
            // `protected` (no `protected_class_method` exists) aren't
            // modelled — they fall through to the error below.
            if let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
                && matches!(cid_to_string(call.name()).as_str(), "private" | "public")
                && call.arguments().map(|a| {
                    let args: Vec<_> = a.arguments().iter().collect();
                    // Only the explicit-name form (`private :a, :b`). A
                    // SPLAT arg (`public(*FileUtils::METHODS)`, rake/
                    // fileutils) must fall through to the regular call
                    // handling — translating the splat here would trip
                    // the unsupported-SplatNode path.
                    !args.is_empty() && args.iter().all(|n| n.as_splat_node().is_none())
                }).unwrap_or(false)
            {
                // Set the visibility on the receiver's singleton methods.
                // Two cases, distinguished at RUNTIME by the receiver's
                // type:
                //   - `recv` is a Module/Class (`class << self` in a class
                //     body, `class << Const`): use `private_class_method`,
                //     which flips the class's singleton-method visibility
                //     (those methods live in the real class's table, not
                //     the eigenclass shell's, so `singleton_class.private`
                //     wouldn't see them).
                //   - `recv` is an ordinary object (`class << self` inside
                //     an INSTANCE method — regexp_parser's scanner.rb:48):
                //     instances have no `private_class_method`, so flip via
                //     `recv.singleton_class.send(:private, …)`.
                let vis = cid_to_string(call.name()); // "private" / "public"
                let cm_name = if vis == "private" { "private_class_method" } else { "public_class_method" };
                let receiver_expr = if needs_local {
                    sp(bn, Expr::LVarRead(synth_local.clone()))
                } else {
                    recv_expr.clone()
                };
                let arg_exprs: Vec<SExpr> = call.arguments()
                    .map(|a| a.arguments().iter().map(|n| tr(ctx, &n)).collect())
                    .unwrap_or_default();
                let cond = sp(bn, Expr::Call {
                    receiver: Some(Box::new(receiver_expr.clone())),
                    name: "is_a?".to_string(),
                    args: vec![sp(bn, Expr::ConstRead("Module".to_string()))],
                    kwargs_trailing: false,
                });
                let then_call = sp(bn, Expr::Call {
                    receiver: Some(Box::new(receiver_expr.clone())),
                    name: cm_name.to_string(),
                    args: arg_exprs.clone(),
                    kwargs_trailing: false,
                });
                let sing = sp(bn, Expr::Call {
                    receiver: Some(Box::new(receiver_expr)),
                    name: "singleton_class".to_string(),
                    args: vec![], kwargs_trailing: false,
                });
                let mut send_args: Vec<SExpr> = vec![sp(bn, Expr::SymbolLit(vis))];
                send_args.extend(arg_exprs);
                let else_call = sp(bn, Expr::Call {
                    receiver: Some(Box::new(sing)),
                    name: "send".to_string(),
                    args: send_args,
                    kwargs_trailing: false,
                });
                out.push(sp(bn, Expr::If {
                    cond: Box::new(cond),
                    then_body: vec![then_call],
                    else_body: vec![else_call],
                }));
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
                if let Some((_do_reader, _do_writer)) = attr_reader_writer_flags(&name) {
                    // `class << self; attr_reader(*ATTRIBUTES); end` — the
                    // attr names come from a runtime splat (an Array
                    // constant), so the compile-time per-name def
                    // expansion below can't see them. Desugar a
                    // single-splat call to a RUNTIME
                    // `self.singleton_class.send(:attr_reader, *ATTRIBUTES)`
                    // — `send` reaches the private Module method and the
                    // singleton class is the right target (class-level
                    // readers, matching CRuby). mail's
                    // multibyte/unicode.rb does `attr_reader(*ATTRIBUTES)`
                    // / `attr_writer(*ATTRIBUTES)` inside `class << self`.
                    let recv_is_self_attr = matches!(&recv_expr.node, Expr::SelfExpr);
                    let single_splat = call.arguments().and_then(|a| {
                        let v: Vec<_> = a.arguments().iter().collect();
                        if v.len() == 1 { v[0].as_splat_node() } else { None }
                    });
                    if recv_is_self_attr
                        && let Some(sn) = single_splat
                        && let Some(inner) = sn.expression()
                    {
                        let inner_expr = tr(ctx, &inner);
                        let name_sym = sp(bn, Expr::ArrayLit(vec![
                            sp(bn, Expr::SymbolLit(name.clone())),
                        ]));
                        let args_array = sp(bn, Expr::Call {
                            receiver: Some(Box::new(name_sym)),
                            name: "+".into(),
                            args: vec![inner_expr],
                            kwargs_trailing: false,
                        });
                        let sing = sp(bn, Expr::Call {
                            receiver: Some(Box::new(sp(bn, Expr::SelfExpr))),
                            name: "singleton_class".into(),
                            args: vec![], kwargs_trailing: false,
                        });
                        out.push(sp(bn, Expr::Apply {
                            receiver: Some(Box::new(sing)),
                            name: "send".into(),
                            splat: Box::new(args_array),
                            block_arg: None,
                            kwsplat: None,
                        }));
                        continue;
                    }
                }
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
            // `class << self; alias_method :new, :old; end` — the
            // method-call form of `alias` (vs. the keyword arm
            // above). Same `self`-receiver gate; routes to
            // `Op::AliasSingletonMethod` so the alias lands on X's
            // singleton_methods table. Both operands must be plain
            // Symbols (the common case). addressable's uri.rb does
            // `class << self; alias_method :escape_component,
            // :encode_component; end` (and three more), which the
            // Jekyll require chain hits.
            if recv_is_self
                && let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
                && cid_to_string(call.name()) == "alias_method"
                && let Some(args) = call.arguments()
            {
                let arg_vec: Vec<_> = args.arguments().iter().collect();
                if arg_vec.len() == 2
                    && let (Some(new_sym), Some(old_sym)) =
                        (arg_vec[0].as_symbol_node(), arg_vec[1].as_symbol_node())
                {
                    let new_name = String::from_utf8_lossy(new_sym.unescaped()).into_owned();
                    let old_name = String::from_utf8_lossy(old_sym.unescaped()).into_owned();
                    out.push(sp(bn, Expr::AliasSingletonMethod(new_name, old_name)));
                    continue;
                }
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
            // `class << self; private :new; end` — visibility
            // modifier WITH method-name args at body top level.
            // Equivalent to `private_class_method :new`: it sets the
            // named SINGLETON method's visibility. rubyrs doesn't
            // model singleton-method visibility (same documented
            // Tier-1 trade-off as `private_class_method` / the bare
            // form's effect on later defs), so this is a no-op — the
            // method stays callable. Motivating case: Liquid's
            // tag.rb does `class << self; def parse(...); ...; end;
            // private :new; end` to push callers toward `Tag.parse`.
            if recv_is_self
                && let Some(call) = bn.as_call_node()
                && call.receiver().is_none()
                && call.arguments().is_some_and(|a| a.arguments().iter().next().is_some())
                && matches!(cid_to_string(call.name()).as_str(),
                    "private" | "public" | "protected"
                )
            {
                out.push(sp(bn, Expr::Nil));
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
            // `class << self; if cond; def a; ...; else; def a; ...;
            // end; end` — conditional method definitions. Each branch
            // must contain ONLY `def`s; they become singleton defs
            // (class methods) wrapped in the runtime `if`. The
            // condition translates through the regular path. A plain
            // `else` is admitted; `elsif` (a nested IfNode subsequent)
            // is not. Motivating case: i18n's utils.rb guards
            // `def except(hash, *keys)` on
            // `Hash.method_defined?(:except)` to pick the native vs.
            // polyfill implementation.
            if recv_is_self
                && let Some(if_n) = bn.as_if_node()
            {
                // Shape-validate first (no translation side effects):
                // every then-stmt is a def, and the subsequent is
                // either absent or an ElseNode of only defs.
                let all_defs = |stmts: Option<ruby_prism::StatementsNode<'_>>| -> bool {
                    match stmts {
                        Some(s) => {
                            let mut it = s.body().iter().peekable();
                            if it.peek().is_none() { return false; }
                            s.body().iter().all(|n| n.as_def_node().is_some())
                        }
                        None => false,
                    }
                };
                let then_ok = all_defs(if_n.statements());
                let else_node = if_n.subsequent().and_then(|s| s.as_else_node());
                let else_ok = match (if_n.subsequent(), &else_node) {
                    (None, _) => true,                       // no else
                    (Some(_), Some(en)) => all_defs(en.statements()),
                    (Some(_), None) => false,                // elsif — bail
                };
                if then_ok && else_ok {
                    let mut then_body: Vec<SExpr> = Vec::new();
                    if let Some(stmts) = if_n.statements() {
                        for s in stmts.body().iter() {
                            let t = tr(ctx, &s);
                            if let Some(sd) = mk_singleton_def(&s, t.node) {
                                then_body.push(sd);
                            }
                        }
                    }
                    let mut else_body: Vec<SExpr> = Vec::new();
                    if let Some(en) = &else_node
                        && let Some(stmts) = en.statements() {
                        for s in stmts.body().iter() {
                            let t = tr(ctx, &s);
                            if let Some(sd) = mk_singleton_def(&s, t.node) {
                                else_body.push(sd);
                            }
                        }
                    }
                    let cond = tr(ctx, &if_n.predicate());
                    out.push(sp(bn, Expr::If {
                        cond: Box::new(cond),
                        then_body,
                        else_body,
                    }));
                    continue;
                }
            }
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
            // Explicit-receiver statement at body top level —
            // `Template.default_exception_renderer = lambda { … }`,
            // `Other.configure(...)`, etc. These don't depend on the
            // singleton-class `self` at all (the receiver is named
            // explicitly), so translating them through the regular
            // `tr()` path and running them in the surrounding context
            // (where `self` is the enclosing class) is observably
            // identical to CRuby. Only BARE-receiver statements need
            // the singleton-class self, and those are handled by the
            // def / attr_* / alias / prepend / visibility arms above
            // (or fall through to NotImplementedError). Prism models
            // `Foo.bar = x` as a CallNode with name `bar=` and an
            // explicit receiver, so attribute-assignment is covered.
            // Motivating case: Liquid's template.rb sets
            // `Template.default_exception_renderer = lambda { … }`
            // inside `class << self`.
            if recv_is_self
                && let Some(call) = bn.as_call_node()
                && call.receiver().is_some()
            {
                out.push(tr(ctx, bn));
                continue;
            }
            // Bare-receiver method call at body top level — `extend
            // Gem::Deprecate`, `deprecate :x, …`, `ruby2_keywords
            // :foo`, etc. Translated through the regular path and run
            // in the surrounding context (self = the enclosing
            // class). For `extend M` this matches CRuby's observable
            // effect: M's instance methods become callable as class
            // methods, and a following bare call to one of them (e.g.
            // addressable idna's `deprecate` after `extend
            // Gem::Deprecate`) then dispatches to it. The `attr_*` /
            // `prepend` names that WOULD be silently misdirected are
            // already consumed by their dedicated arms above. Known
            // divergence: `include M` here installs M on the
            // surrounding class's instance methods rather than its
            // singleton (rubyrs's flat per-class model) — rare and
            // documented. Motivating case: addressable's idna/pure.rb
            // `class << self; …; extend Gem::Deprecate; deprecate
            // :unicode_normalize_kc, …; end`.
            if recv_is_self
                && bn.as_call_node().is_some()
            {
                out.push(tr(ctx, bn));
                continue;
            }
            // Bare `self` in the body — the metaclass-expression
            // idiom `(class << self; self; end)`, whose VALUE is
            // the eigenclass (minitest's cattr_accessor, mock.rb's
            // `metaclass = class << self; self; end`). The runtime
            // already reifies eigenclass shells via
            // `singleton_class`, so the statement desugars to
            // `self.singleton_class` — as the last body statement
            // it becomes the construct's value, matching CRuby.
            if recv_is_self && bn.as_self_node().is_some() {
                out.push(sp(bn, Expr::Call {
                    receiver: Some(Box::new(sp(bn, Expr::SelfExpr))),
                    name: "singleton_class".into(),
                    args: vec![],
                    kwargs_trailing: false,
                }));
                continue;
            }
            // Bare `self` in a NON-self singleton body —
            // `mc = class << Time; self; end`, the ubiquitous
            // grab-the-eigenclass idiom (common_logger mocks
            // `Time.now` this way). `self` inside the body IS the
            // receiver's eigenclass, so desugar to
            // `RECV.singleton_class`. The per-statement value is
            // discarded (the construct's value is re-derived from the
            // outer wrapper below), but emitting the faithful
            // expression also keeps a non-final `self` statement
            // correct. Reached only when `!recv_is_self` — the
            // `recv_is_self` self-node case is consumed just above.
            if bn.as_self_node().is_some() {
                let receiver_expr = if needs_local {
                    sp(bn, Expr::LVarRead(synth_local.clone()))
                } else {
                    recv_expr.clone()
                };
                out.push(sp(bn, Expr::Call {
                    receiver: Some(Box::new(receiver_expr)),
                    name: "singleton_class".into(),
                    args: vec![],
                    kwargs_trailing: false,
                }));
                continue;
            }
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
        if body_ends_with_self {
            // Metaclass-expression idiom: the construct's value is
            // the eigenclass when the body's LAST statement is the
            // bare `self` (CRuby: `class << X` evaluates to its
            // last body expression). The inner Begin's per-statement
            // values are discarded by the wrapper, so re-derive the
            // eigenclass here as the outer value. The receiver is
            // `self` for `class << self`, otherwise the synthetic
            // local (side-effectful recv, evaluated once) or the
            // literal pure receiver (`Time` in `class << Time`).
            let receiver_expr = if recv_is_self_outer {
                sp(node, Expr::SelfExpr)
            } else if needs_local {
                sp(node, Expr::LVarRead(synth_local.clone()))
            } else {
                recv_expr.clone()
            };
            outer.push(sp(node, Expr::Call {
                receiver: Some(Box::new(receiver_expr)),
                name: "singleton_class".into(),
                args: vec![],
                kwargs_trailing: false,
            }));
        } else {
            outer.push(sp(node, Expr::Nil));
        }
        sp(node, Expr::Begin {
            body: outer,
            rescue: vec![],
            ensure: None,
        })
}

/// Translate an `AssocNode`'s value, unwrapping the Ruby 3.1
/// value-shorthand form. In `{x:, y:}` / `foo(x:, y:)` Prism gives the
/// AssocNode an `ImplicitNode` value wrapping the synthesized binding
/// read (`LocalVariableReadNode` / `CallNode`). Unwrap it so the value
/// is the variable/method read, mirroring CRuby's desugar to
/// `{x: x, y: y}`. Surfaced by bridgetown-core, which leans on the
/// shorthand heavily.
pub(crate) fn tr_assoc_value(ctx: &mut TranslationCtx<'_>, an: &ruby_prism::AssocNode<'_>) -> SExpr {
    if let Some(imp) = an.value().as_implicit_node() {
        return tr(ctx, &imp.value());
    }
    tr(ctx, &an.value())
}

/// Minimum native stack (bytes) that must remain before `tr`
/// recursion is allowed to continue on the current stack; below this
/// `maybe_grow` switches to a fresh heap segment. It must exceed the
/// stack one `tr_impl` invocation consumes between two recursive `tr`
/// calls in a debug build (its own frame — every match arm's locals —
/// plus the iterator/collect/closure frames), so the guard trips with
/// headroom rather than overshooting into an overflow. 128 KB clears
/// that with margin: measured startup high-water (preamble compile)
/// drops from ~2 MB to ~380 KB, well under the 1 MB default
/// main-thread stack on Windows (issue #356). The residual floor is
/// the prologue plus Prism's own C parser recursion (source→AST,
/// which this guard doesn't cover) — both far below any realistic
/// main-thread stack, so guarding only `tr` is sufficient. Raising
/// the red zone doesn't lower that floor; it just trips the guard
/// sooner. On a roomy 8 MB main thread the guard never trips and
/// costs only a stack-pointer compare per node.
#[cfg(not(target_family = "wasm"))]
const TR_RED_ZONE: usize = 128 * 1024;
/// Size of each fresh stack segment `maybe_grow` allocates when the
/// red zone is hit.
#[cfg(not(target_family = "wasm"))]
const TR_STACK_GROW: usize = 2 * 1024 * 1024;

/// Stack-growth guard around the recursive AST→IR translator.
///
/// `tr_impl` recurses structurally over the Prism AST; in unoptimised
/// (debug) builds every frame carries the locals of *all* match arms,
/// so each level of nesting costs several KB of native stack. The
/// always-on preamble is compiled through this path at
/// `Runtime::new`, and a deeply nested expression there could blow a
/// small thread stack — notably the 1 MB default *main-thread* stack
/// on Windows, where debug embedders overflowed at startup while
/// release (smaller frames) ran fine. See issue #356.
///
/// `stacker::maybe_grow` is a cheap stack-pointer check on the common
/// path; only when fewer than `TR_RED_ZONE` bytes remain does it
/// heap-allocate a fresh `TR_STACK_GROW`-byte segment and continue the
/// recursion there, so the native thread stack stays bounded
/// regardless of AST depth. wasm has a link-time-fixed stack and psm
/// can't switch stacks, so there we fall back to plain recursion
/// (unchanged behaviour).
pub(crate) fn tr(ctx: &mut TranslationCtx<'_>, node: &Node<'_>) -> SExpr {
    #[cfg(not(target_family = "wasm"))]
    {
        stacker::maybe_grow(TR_RED_ZONE, TR_STACK_GROW, move || tr_impl(ctx, node))
    }
    #[cfg(target_family = "wasm")]
    {
        tr_impl(ctx, node)
    }
}

fn tr_impl(ctx: &mut TranslationCtx<'_>, node: &Node<'_>) -> SExpr {
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
    // Imaginary literal — `3i`, `2.5i`, `1ri`. Prism wraps the
    // numeric in an ImaginaryNode; desugar to `Complex(0, <numeric>)`
    // so it routes through the preamble's pure-Ruby Complex factory
    // (and its component types follow the inner literal — `3i` has an
    // Integer imaginary part, `2.5i` a Float).
    if let Some(n) = node.as_imaginary_node() {
        let inner = tr_impl(ctx, &n.numeric());
        return sp(node, Expr::Call {
            receiver: None,
            name: "Complex".to_string(),
            args: vec![sp(node, Expr::IntLit(0)), inner],
            kwargs_trailing: false,
        });
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
            // Ruby flag bitmask from Prism: i=1, x=2, m=4 (CRuby's
            // Regexp::IGNORECASE/EXTENDED/MULTILINE). Ruby /m is
            // dot-matches-newline (engine `(?s)`), applied at
            // runtime by `apply_ruby_flags`.
            let flags = (_n.is_ignore_case() as u8)
                | ((_n.is_extended() as u8) << 1)
                | ((_n.is_multi_line() as u8) << 2);
            return sp(node, Expr::RegexLit(String::from_utf8_lossy(_n.unescaped()).into_owned(), flags));
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
    // `:"#{x}=…"` — interpolated symbol. Same `parts()` shape as an
    // interpolated string; build the string then `.to_sym`. Discovery:
    // P3 Jekyll spike — jekyll builds setter symbols dynamically, e.g.
    // `:"#{key}="`.
    if let Some(n) = node.as_interpolated_symbol_node() {
        let parts: Vec<SExpr> = n.parts().iter().map(|p| {
            if let Some(es) = p.as_embedded_statements_node() {
                let stmts: Vec<SExpr> = es.statements()
                    .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
                    .unwrap_or_default();
                if stmts.len() == 1 {
                    stmts.into_iter().next().unwrap_or_else(|| sp(&p, Expr::Nil))
                } else {
                    Spanned::new(node_span(&p), seq_inner(stmts))
                }
            } else if let Some(ev) = p.as_embedded_variable_node() {
                tr(ctx, &ev.variable())
            } else {
                tr(ctx, &p)
            }
        }).collect();
        let interp = sp(node, Expr::InterpolatedStr(parts));
        return sp(node, Expr::Call {
            receiver: Some(Box::new(interp)),
            name: "to_sym".into(),
            args: vec![],
            kwargs_trailing: false,
        });
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
            let flags = (_n.is_ignore_case() as u8)
                | ((_n.is_extended() as u8) << 1)
                | ((_n.is_multi_line() as u8) << 2);
            return sp(node, Expr::InterpolatedRegex(parts, flags));
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
            let name = crate::const_marker::tag_absolute(joined, is_constant_path_absolute(node));
            return sp(node, Expr::ConstRead(name));
        }
        // Dynamic path: `expr::CONST` where `expr` is a RUNTIME value
        // (e.g. `self.class::FOO`, `k::FOO` for a local `k`). CRuby
        // resolves FOO on the runtime value's own class/ancestry — NOT
        // in the lexical scope. Desugar to `expr.const_get(:FOO)` so
        // the dynamic base is honoured. The previous trailing-name-only
        // `ConstRead(FOO)` discarded the base entirely and resolved FOO
        // in the surrounding lexical scope, so a `self.class::FOO` from
        // a base-class method returned the BASE's `FOO` even when the
        // runtime subclass redefined it (e.g. kramdown-gfm's
        // `self.class::FENCED_CODEBLOCK_MATCH` resolved to the
        // tilde-only base constant, breaking ``` ``` ``` code fences).
        if let Some(parent) = n.parent()
            && let Some(name_id) = n.name()
        {
            return sp(node, Expr::Call {
                receiver: Some(Box::new(tr(ctx, &parent))),
                name: "const_get".into(),
                args: vec![sp(node, Expr::SymbolLit(cid_to_string(name_id)))],
                kwargs_trailing: false,
            });
        }
        // No parent (shouldn't reach here): trailing-name fallback.
        if let Some(name_id) = n.name() {
            return sp(node, Expr::ConstRead(cid_to_string(name_id)));
        }
    }
    if let Some(n) = node.as_local_variable_read_node() {
        return sp(node, Expr::LVarRead(cid_to_string(n.name())));
    }
    // Ruby 3.4 implicit `it` block param. `tr_block_node` synthesizes
    // a `Single("it")` slot for an ItParametersNode block, so the body
    // reference reads that local exactly like `_1`.
    if node.as_it_local_variable_read_node().is_some() {
        return sp(node, Expr::LVarRead("it".to_string()));
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
    // `# shareable_constant_value: ...` magic comment — Prism wraps the
    // constant write it governs in a `ShareableConstantNode`. rubyrs has
    // no Ractor-shareability model, so the frozen-ness it would enforce
    // is a no-op here; translate the inner write directly. Surfaced by
    // stdlib time.rb (`# shareable_constant_value: literal`).
    if let Some(n) = node.as_shareable_constant_node() {
        return tr(ctx, &n.write());
    }
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
    if let Some(n) = node.as_call_or_write_node() {
        // `recv.attr ||= val` → `recv.attr || (recv.attr=(val))`.
        // CRuby semantics: read via `recv.attr` (no args), and on
        // falsy write via the writer method `recv.attr=(val)`. The
        // writer's name is the read name plus `=`. Receiver is
        // evaluated TWICE here (once per branch) — same shape as
        // IndexOrWriteNode below; CRuby's version evaluates once
        // and stashes in a temp, but the doubled eval matches
        // observed CRuby behaviour for the simple receiver shapes
        // (`self`, ConstRead, local var) the spike hits.
        let recv = n.receiver().map(|r| tr(ctx, &r)).expect(
            "CallOrWriteNode without receiver is unrepresentable",
        );
        let read_name = cid_to_string(n.read_name());
        let write_name = format!("{}=", read_name);
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: read_name,
            args: vec![], kwargs_trailing: false });
        let write = sp(node, Expr::AssignCall {
            receiver: Box::new(recv),
            name: write_name,
            args: vec![tr(ctx, &n.value())] });
        return sp(node, Expr::Or(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_call_and_write_node() {
        // `recv.attr &&= val` → `recv.attr && (recv.attr=(val))`.
        // Same shape as CallOrWrite above, just `&&` instead of
        // `||`. Hit by attribute-and-write idioms inside attr_
        // accessor-heavy classes.
        let recv = n.receiver().map(|r| tr(ctx, &r)).expect(
            "CallAndWriteNode without receiver is unrepresentable",
        );
        let read_name = cid_to_string(n.read_name());
        let write_name = format!("{}=", read_name);
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: read_name,
            args: vec![], kwargs_trailing: false });
        let write = sp(node, Expr::AssignCall {
            receiver: Box::new(recv),
            name: write_name,
            args: vec![tr(ctx, &n.value())] });
        return sp(node, Expr::And(Box::new(read), Box::new(write)));
    }
    if let Some(n) = node.as_call_operator_write_node() {
        // `recv.attr += val` → `recv.attr=(recv.attr + val)`.
        // Binary operator from `n.binary_operator()` (e.g. "+",
        // "-", "*", "<<"); writer method is the read name + `=`.
        // Receiver evaluated twice — same shape as the
        // IndexOperatorWrite arm below.
        let recv = n.receiver().map(|r| tr(ctx, &r)).expect(
            "CallOperatorWriteNode without receiver is unrepresentable",
        );
        let read_name = cid_to_string(n.read_name());
        let write_name = format!("{}=", read_name);
        let op = cid_to_string(n.binary_operator());
        let read = sp(node, Expr::Call {
            receiver: Some(Box::new(recv.clone())),
            name: read_name,
            args: vec![], kwargs_trailing: false });
        let new_val = sp(node, Expr::Call {
            receiver: Some(Box::new(read)),
            name: op,
            args: vec![tr(ctx, &n.value())], kwargs_trailing: false });
        let write = sp(node, Expr::AssignCall {
            receiver: Box::new(recv),
            name: write_name,
            args: vec![new_val] });
        return write;
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
        let write = sp(node, Expr::AssignCall {
            receiver: Box::new(recv),
            name: "[]=".into(),
            args: write_args });
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
        let write = sp(node, Expr::AssignCall {
            receiver: Box::new(recv),
            name: "[]=".into(),
            args: write_args });
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
        return sp(node, Expr::AssignCall {
            receiver: Box::new(recv),
            name: "[]=".into(),
            args: write_args });
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
        // undefined constant. The read's name carries the `::`
        // marker when the path is absolute so the compiler's
        // ConstRead fast path emits a flat top-level LoadConst
        // (no cref-walk); the write side keeps the bare joined
        // name + `abs` flag because ConstWrite handles the
        // class_path-alias decision separately.
        let mut make = |name: String, abs: bool| {
            let read_name = crate::const_marker::tag_absolute(name.clone(), abs);
            let read = sp(node, Expr::ConstRead(read_name));
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
        // See ConstantPathOperatorWriteNode arm above for the
        // read-name vs write-name split rationale (read carries
        // the `::` marker for absolute paths so `||=` short-
        // circuits on the TOP-LEVEL value, not on a same-named
        // inner shadow).
        let mut make = |name: String, abs: bool| {
            let read_name = crate::const_marker::tag_absolute(name.clone(), abs);
            let read = sp(node, Expr::ConstReadOrNil(read_name));
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
        // Same read/write split as the OperatorWrite arm: read's
        // name carries the `::` marker for absolute paths so
        // `&&=` short-circuits on the top-level constant rather
        // than on a cref-walked inner shadow.
        let mut make = |name: String, abs: bool| {
            let read_name = crate::const_marker::tag_absolute(name.clone(), abs);
            let read = sp(node, Expr::ConstRead(read_name));
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
            } else if let Some(ct) = tgt.as_constant_target_node() {
                targets.push(MultiWriteTarget::Const(cid_to_string(ct.name())));
            } else if let Some(call_tgt) = tgt.as_call_target_node() {
                // `obj.attr = …` setter target. Prism's
                // CallTargetNode carries the receiver + name
                // (without the trailing `=`); the compiler
                // appends `=` and dispatches as a 1-arg
                // method call.
                let receiver = Box::new(tr(ctx, &call_tgt.receiver()));
                let name = cid_to_string(call_tgt.name());
                targets.push(MultiWriteTarget::Call { receiver, name });
            } else if let Some(idx_tgt) = tgt.as_index_target_node() {
                // `obj[idx, ...] = …` index-write target. Prism's
                // IndexTargetNode carries the receiver + an
                // arguments node holding the index expressions.
                // Compiler routes through `[]=` with arity =
                // args.len() + 1 (the RHS occupies the last slot).
                let receiver = Box::new(tr(ctx, &idx_tgt.receiver()));
                let args: Vec<SExpr> = idx_tgt
                    .arguments()
                    .map(|a| a.arguments().iter().map(|n| tr(ctx, &n)).collect())
                    .unwrap_or_default();
                targets.push(MultiWriteTarget::Index { receiver, args });
            } else if let Some(mt) = tgt.as_multi_target_node() {
                // Nested / parenthesized target `(a, b)` — recurse.
                targets.push(MultiWriteTarget::Nested(gather_nested(ctx, &mt)));
            } else {
                ctx.errors.push(
                    format!("unsupported multi-write target: {:?}", tgt)
                );
            }
        }
        // Gather a nested `MultiTargetNode`'s own lefts / `*rest` /
        // rights into a target list (mutually recursive with
        // `push_positional` for deeper nesting). The splat slot supports
        // the common into-local / into-ivar forms; rarer splat-into-
        // const/call/global inside a NESTED target errors (the top-level
        // arm below still handles those at depth 0).
        fn gather_nested(
            ctx: &mut TranslationCtx<'_>,
            mt: &ruby_prism::MultiTargetNode<'_>,
        ) -> Vec<MultiWriteTarget> {
            let mut sub: Vec<MultiWriteTarget> = Vec::new();
            for t in mt.lefts().iter() {
                push_positional(ctx, &mut sub, &t);
            }
            if let Some(rest) = mt.rest() {
                if let Some(splat) = rest.as_splat_node() {
                    match splat.expression() {
                        None => sub.push(MultiWriteTarget::SplatLocal(None)),
                        Some(expr) => {
                            if let Some(lvt) = expr.as_local_variable_target_node() {
                                sub.push(MultiWriteTarget::SplatLocal(Some(cid_to_string(lvt.name()))));
                            } else if let Some(ivt) = expr.as_instance_variable_target_node() {
                                sub.push(MultiWriteTarget::SplatIvar(cid_to_string(ivt.name())));
                            } else {
                                ctx.errors.push(format!("unsupported nested splat target: {:?}", expr));
                            }
                        }
                    }
                } else if rest.as_implicit_rest_node().is_some() {
                    sub.push(MultiWriteTarget::SplatLocal(None));
                }
            }
            for t in mt.rights().iter() {
                push_positional(ctx, &mut sub, &t);
            }
            sub
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
                        } else if let Some(ct) = expr.as_constant_target_node() {
                            // `MAJOR, MINOR, BUILD, *OTHER = ...`
                            // (rake/version.rb:6) — splat into a const.
                            targets.push(MultiWriteTarget::SplatConst(
                                cid_to_string(ct.name()),
                            ));
                        } else if let Some(call_tgt) = expr.as_call_target_node() {
                            // `*recv.attr` — splat into an attribute
                            // writer. Mustermann's
                            // `self.head, *self.payload = ...` shape;
                            // pre-fix this hit the "unsupported splat
                            // target" arm despite the non-splat
                            // CallTargetNode case (~line 1505) being
                            // wired. Same dispatch as positional
                            // `MWT::Call`: receiver evaluated +
                            // swapped, setter dispatched with arity 1.
                            let receiver = Box::new(tr(ctx, &call_tgt.receiver()));
                            let name = cid_to_string(call_tgt.name());
                            targets.push(MultiWriteTarget::SplatCall {
                                receiver, name,
                            });
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
        // Safe-navigation desugaring: `recv&.method(args)` evaluates
        // `recv` ONCE; if it's nil the whole expression is nil,
        // otherwise the regular call fires. We capture the original
        // receiver into a fresh synthetic local (`__sn_N`), swap the
        // call's receiver to a read of that local, and — at every
        // return site below — wrap the call SExpr in a
        // `Begin { LVarWrite(local, raw_recv); if local.nil? then nil
        // else <call> end }` envelope. The single eval is what makes
        // this different from the naive `recv.nil? ? nil :
        // recv.method` rewrite — `recv` might be `expensive_call`,
        // and we mustn't trigger its side effects twice. CRuby's
        // `&.` triggers ONLY on `nil` (`false&.foo` calls), which is
        // exactly the semantics `local.nil?` gives us.
        let is_safe_nav = n.is_safe_navigation();
        let raw_recv: Option<SExpr> = n.receiver().map(|r| tr(ctx, &r));
        let safe_nav_local: Option<String> = if is_safe_nav && raw_recv.is_some() {
            let c = ctx.safe_nav_count.get();
            ctx.safe_nav_count.set(c + 1);
            Some(format!("__sn_{c}"))
        } else {
            None
        };
        let receiver: Option<Box<SExpr>> = if let Some(name) = &safe_nav_local {
            Some(Box::new(sp(node, Expr::LVarRead(name.clone()))))
        } else {
            raw_recv.clone().map(Box::new)
        };
        // Helper: wrap the call expression with the safe-nav
        // envelope when active. No-op when this isn't a safe-nav
        // call site — keeps every return site below uniform.
        let wrap_sn = |call_expr: SExpr| -> SExpr {
            match (&safe_nav_local, &raw_recv) {
                (Some(local), Some(orig_recv)) => {
                    let assign = sp(node, Expr::LVarWrite(local.clone(), Box::new(orig_recv.clone())));
                    let nil_check = sp(node, Expr::Call {
                        receiver: Some(Box::new(sp(node, Expr::LVarRead(local.clone())))),
                        name: "nil?".into(),
                        args: vec![],
                        kwargs_trailing: false,
                    });
                    let nil_lit = sp(node, Expr::Nil);
                    let if_expr = sp(node, Expr::If {
                        cond: Box::new(nil_check),
                        then_body: vec![nil_lit],
                        else_body: vec![call_expr],
                    });
                    sp(node, Expr::Begin {
                        body: vec![assign, if_expr],
                        rescue: vec![],
                        ensure: None,
                    })
                }
                _ => call_expr,
            }
        };
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
        // Detect a `&block_arg` co-existing with a splat. The
        // BlockArgumentNode-with-expression case (i.e. `&proc`,
        // not the anonymous `&` or `&:symbol` shapes) needs to be
        // routed through `Apply.block_arg` because the regular
        // CallWithBlockArg path doesn't expand splats. The
        // anonymous and symbol-to-proc shapes still go via the
        // legacy path below — they don't combine with splats in
        // any gem we've vendored, and giving them their own arm
        // would duplicate the synthesis code without benefit.
        let early_block_arg: Option<Box<SExpr>> = n.block().and_then(|bnode| {
            // A brace/do block on a SPLAT call (`foo(*a) { … }`):
            // the splat forces the Apply path, which the non-splat
            // CallWithBlock arm never reaches, so the block was dropped
            // ("no block given" / silently ignored). Convert it to a
            // Lambda so the Apply path forwards it as the call's block —
            // `Struct.new(*KEYS) { attr_accessor :previous, :next }`
            // (regexp_parser's Token). Consumed ONLY by the splat
            // returns below; non-splat calls fall through to the
            // CallWithBlock arm, which handles the block itself.
            if let Some(bn) = bnode.as_block_node() {
                let (params, body) = tr_block_node(ctx, &bn);
                // NOT a real lambda — this reuses Expr::Lambda purely to
                // forward a brace/do block through the splat-call Apply
                // path, so `is_lambda: false` (it's an ordinary block).
                return Some(Box::new(sp(node, Expr::Lambda { params, body, is_lambda: false })));
            }
            bnode.as_block_argument_node().and_then(|ba| match ba.expression() {
                // Skip the symbol-to-proc shape (`&:method`); the
                // existing CallWithBlock arm has a richer
                // expansion (synthesises a one-arg block body) we
                // don't want to bypass.
                Some(expr) if expr.as_symbol_node().is_some() => None,
                Some(expr) => Some(Box::new(tr(ctx, &expr))),
                // Anonymous `&` forwarding combined with a splat
                // (`def m(*, &); n(*, &); end`): read the reserved
                // `&` block sentinel the enclosing def bound. Without
                // this the splat path dropped the block (the legacy
                // non-splat block arm never runs once a splat forces
                // the Apply path), turning it into "no block given".
                None => Some(Box::new(sp(node, Expr::LVarRead("&".to_string())))),
            })
        });
        // `n(...)` / `n(x, ...)` — Ruby 3.0 argument forwarding.
        // ForwardingArgumentsNode stands in for `*<rest>, **<kw>,
        // &<blk>` all at once, reading the reserved sentinels the
        // enclosing `def m(...)` bound (rest `*`, kwrest
        // `__kw_rest_anon`, block `&`). Desugar to a splat call:
        // leading positionals + Array(`*`) + the kwsplat chunk (drops
        // an empty kwrest), with the block forwarded via Apply.block_arg.
        if arg_nodes.iter().any(|c| c.as_forwarding_arguments_node().is_some()) {
            let mut chunks: Vec<SExpr> = Vec::new();
            let mut buf: Vec<SExpr> = Vec::new();
            for c in &arg_nodes {
                if c.as_forwarding_arguments_node().is_some() {
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                    }
                    // positional rest: Array(`*` sentinel)
                    chunks.push(sp(node, Expr::Call {
                        receiver: None,
                        name: "Array".into(),
                        args: vec![sp(node, Expr::LVarRead("*".to_string()))],
                        kwargs_trailing: false,
                    }));
                    // keyword rest: `[__kw_rest_anon].reject(&:empty?)`
                    chunks.push(kwsplat_chunk(node, sp(node, Expr::LVarRead("__kw_rest_anon".to_string()))));
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
            return wrap_sn(sp(node, Expr::Apply {
                receiver,
                name,
                splat: Box::new(acc),
                block_arg: Some(Box::new(sp(node, Expr::LVarRead("&".to_string())))),
                kwsplat: None,
            }));
        }
        if arg_nodes.len() == 1
            && let Some(sn) = arg_nodes[0].as_splat_node()
                && let Some(splat_expr) = sn.expression() {
                    // Wrap the splat'd expression in `Array(x)` so
                    // the call-splat obeys CRuby's coerce-to-array
                    // contract (Array→unchanged, nil→[], scalar→
                    // [scalar]) — same as the array-literal splat
                    // path below. Without it `foo(*5)` reached
                    // `Op::ApplyCall` with a bare Integer and tripped
                    // "no implicit conversion of Integer into Array".
                    return wrap_sn(sp(node, Expr::Apply {
                        receiver,
                        name,
                        splat: Box::new(sp(node, Expr::Call {
                            receiver: None,
                            name: "Array".into(),
                            args: vec![tr(ctx, &splat_expr)],
                            kwargs_trailing: false,
                        })),
                        block_arg: early_block_arg,
                        kwsplat: None,
                    }));
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
            // Keyword-splat carried separately from the positional array
            // for the no-block `f(*args, **kw)` path (see below).
            let mut kwsplat_expr: Option<SExpr> = None;
            for c in &arg_nodes {
                let cn: &ruby_prism::Node<'_> = c;
                if let Some(sn) = cn.as_splat_node() {
                        // `*x` splat, OR anonymous `*` forwarding
                        // (`def m(*); n(*); end`) where the splat has
                        // no expression — read the reserved `"*"` rest
                        // sentinel the enclosing `def m(*)` bound.
                        let inner_expr = match sn.expression() {
                            Some(inner) => tr(ctx, &inner),
                            None => sp(node, Expr::LVarRead("*".to_string())),
                        };
                        if !buf.is_empty() {
                            chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                        }
                        // `Array(inner)` coerce — keeps the `+`-chain
                        // valid for scalar/nil splats (`foo(a, *5)`)
                        // and matches the single-splat + array-literal
                        // paths' CRuby coerce-to-array contract.
                        chunks.push(sp(node, Expr::Call {
                            receiver: None,
                            name: "Array".into(),
                            args: vec![inner_expr],
                            kwargs_trailing: false,
                        }));
                    } else if let Some(kh) = cn.as_keyword_hash_node() {
                    // Trailing kwarg-hash (`**opts` / `k: v`). Flush any
                    // pending positionals first.
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                    }
                    let kwhash = tr_kwhash(ctx, node, cn, &kh);
                    // Carry the kwsplat SEPARATELY (Op::ApplyCallKw, or the
                    // *Block variant when a block is also present) so the VM
                    // can drop an empty `**{}` and keep a trailing positional
                    // brace-hash positional (`f({a:1}, **{})` → value={a:1}).
                    // Folding it into the array — the old `kwsplat_chunk`
                    // path — made an empty kwsplat vanish, after which the
                    // binder peeled the real positional hash as kwargs.
                    kwsplat_expr = Some(kwhash);
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
            return wrap_sn(sp(node, Expr::Apply {
                receiver,
                name,
                splat: Box::new(acc),
                block_arg: early_block_arg,
                kwsplat: kwsplat_expr.map(Box::new),
            }));
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
                // Block params + body via the shared translator
                // (`|a, (b, c)|`, `|*rest|`, `|&blk|`, `|**opts|`).
                let (block_params, block_body) = tr_block_node(ctx, &bn);
                return wrap_sn(sp(node, Expr::CallWithBlock { receiver, name, args, block_params, block_body, kwargs_trailing }));
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
                    return wrap_sn(sp(node, Expr::CallWithBlockArg {
                        receiver, name, args, block_arg: Box::new(block_arg), kwargs_trailing,
                    }));
                }
            }
            if let Some(ba) = bnode.as_block_argument_node()
                && let Some(expr) = ba.expression() {
                    if let Some(sn) = expr.as_symbol_node() {
                        let method_name: String = String::from_utf8_lossy(sn.unescaped()).into_owned();
                        // `&:sym` is `sym.to_proc` — it forwards ALL the
                        // yielded args: `recv.sym(*rest)`, where `recv` is
                        // the first arg and `rest` the others. So
                        // `reduce(&:+)` → `acc.+(x)` works (a binary op
                        // gets its operand). Desugar to a REST-ONLY block
                        // `{ |*__sp_a| __sp_a[0].sym(*__sp_a.drop(1)) }`:
                        // rest-only means it does NOT auto-splat (so
                        // `[[1,2]].map(&:first)` keeps the pair as the
                        // single arg → `[1,2].first` == 1), yet still
                        // forwards extra args. The old 1-param desugar
                        // `{ |x| x.sym }` dropped them (broke `&:+`).
                        let arr = "__sp_a".to_string();
                        let arr_read = || sp(node, Expr::LVarRead(arr.clone()));
                        let recv0 = sp(node, Expr::Call {
                            receiver: Some(Box::new(arr_read())),
                            name: "[]".to_string(),
                            args: vec![sp(node, Expr::IntLit(0))],
                            kwargs_trailing: false,
                        });
                        let rest = sp(node, Expr::Call {
                            receiver: Some(Box::new(arr_read())),
                            name: "drop".to_string(),
                            args: vec![sp(node, Expr::IntLit(1))],
                            kwargs_trailing: false,
                        });
                        let body_call = sp(node, Expr::Apply {
                            receiver: Some(Box::new(recv0)),
                            name: method_name,
                            splat: Box::new(rest),
                            block_arg: None,
                            kwsplat: None,
                        });
                        return wrap_sn(sp(node, Expr::CallWithBlock {
                            receiver, name, args,
                            block_params: vec![BlockParam::Rest(arr)],
                            block_body: vec![body_call], kwargs_trailing,
                        }));
                    }
                    // Fall-through: any other expression becomes
                    // the block arg via CallWithBlockArg. CRuby
                    // requires the value to respond to `to_proc` —
                    // for our subset we only accept Value::Block
                    // directly (no implicit coercion).
                    let block_arg = tr(ctx, &expr);
                    return wrap_sn(sp(node, Expr::CallWithBlockArg {
                        receiver, name, args, block_arg: Box::new(block_arg), kwargs_trailing,
                    }));
                }
        }
        // Assignment-syntax call (`recv.attr = v` / `recv[k] = v` —
        // prism marks these CallNodes ATTRIBUTE_WRITE): route to
        // AssignCall so the expression evaluates to the RHS even
        // when a user writer's return value differs (CRuby rule;
        // `send(:attr=, v)` is NOT flagged and keeps the return).
        // kwargs-trailing shapes stay on the plain path — the CallKw
        // kwargs split doesn't apply to assignment args. Safe-nav
        // (`recv&.attr = v`) composes through wrap_sn unchanged.
        return match (n.is_attribute_write() && !kwargs_trailing, receiver) {
            (true, Some(recv)) => {
                wrap_sn(sp(node, Expr::AssignCall { receiver: recv, name, args }))
            }
            (_, receiver) => {
                wrap_sn(sp(node, Expr::Call { receiver, name, args, kwargs_trailing }))
            }
        };
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
                if let Some(sn) = only.as_splat_node() {
                    // `*val`, OR anonymous `*` forwarding (`def m(*);
                    // yield(*); end`) where the splat has no expression
                    // — read the reserved `"*"` rest sentinel `def m(*)`
                    // bound. erb_templates.rb's `def capture(*);
                    // yield(*); end` hits this.
                    let inner_expr = match sn.expression() {
                        Some(inner) => tr(ctx, &inner),
                        None => sp(span_node, Expr::LVarRead("*".to_string())),
                    };
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
                    let elems: Vec<SExpr> = arg_nodes.iter().map(|n| {
                        if let Some(kh) = n.as_keyword_hash_node() {
                            tr_kwhash(ctx, span_node, n, &kh)
                        } else {
                            tr(ctx, n)
                        }
                    }).collect();
                    return Some(Box::new(sp(span_node, Expr::ArrayLit(elems))));
                }
                let mut chunks: Vec<SExpr> = Vec::new();
                let mut buf: Vec<SExpr> = Vec::new();
                for n in &arg_nodes {
                    if let Some(sn) = n.as_splat_node() {
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
                        // Array RHS. Anonymous `*` (no expression) reads
                        // the reserved `"*"` rest sentinel.
                        let inner_expr = match sn.expression() {
                            Some(inner) => tr(ctx, &inner),
                            None => sp(span_node, Expr::LVarRead("*".to_string())),
                        };
                        chunks.push(sp(span_node, Expr::Call {
                            receiver: None,
                            name: "Array".into(),
                            args: vec![inner_expr], kwargs_trailing: false }));
                    } else if let Some(kh) = n.as_keyword_hash_node() {
                        // Trailing `**h` / `k: v` in a splat arg list
                        // (`yield(*v, **h)`, `return a, *b, **h`): route
                        // through `tr_kwhash` like the call / super splat
                        // paths so the AssocSplat merges into a Hash that
                        // rides as the assembled array's trailing element
                        // (ApplyYield expands it; the block peels it as
                        // kwargs). Without this, `tr` on the
                        // KeywordHashNode trips the unsupported-node trap
                        // — pp's `yield(*v, **kwsplat)` (pp.rb:277).
                        buf.push(tr_kwhash(ctx, span_node, n, &kh));
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
    if node.as_redo_node().is_some() {
        // `redo` — re-run the current loop body. Target resolution
        // (innermost `while` vs enclosing block) happens at compile
        // time; an out-of-loop `redo` emits a runtime raise.
        return sp(node, Expr::Redo);
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
        // `defined?(super)` — "super" when the enclosing method has a
        // super-chain method of the same name, else nil. The runtime
        // probe reads the current frame's method + ancestry (host fns
        // run inline, so the frame is still the method containing the
        // `defined?`). Pre-fix this fell through to the catch-all
        // `"expression"`, so `if defined?(super); super; end` (sorbet's
        // T::Helpers#abstract!) always ran the `super` and tripped
        // "no superclass method".
        if inner.as_super_node().is_some() || inner.as_forwarding_super_node().is_some() {
            return Spanned::new(span, Expr::Call {
                receiver: None,
                name: "__defined_super?".into(),
                args: vec![], kwargs_trailing: false });
        }
        // `defined?(yield)` — "yield" when the enclosing method has a
        // block, else nil. Same catch-all-"expression" pre-fix bug as
        // super: sequel's `if defined?(yield); return yield(db); end`
        // ran the yield with no block ("no block given").
        if inner.as_yield_node().is_some() {
            return Spanned::new(span, Expr::Call {
                receiver: None,
                name: "__defined_yield?".into(),
                args: vec![], kwargs_trailing: false });
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
            // Receiver-bearing `defined?(recv.m)`. CRuby evaluates
            // the receiver (NameError → the whole defined? is nil)
            // and then checks the method's existence. Full fidelity
            // needs exception plumbing around the receiver eval; we
            // cover the side-effect-free receiver shapes gems
            // actually use for feature detection and stay
            // optimistic for the rest:
            //   - Const / Const::Path receiver → guard with
            //     `__defined_const?` first, then a runtime respond
            //     check on the (now known-resolvable) receiver.
            //     rack/utils.rb's
            //     `defined?(OpenSSL.fixed_length_secure_compare)`
            //     needs the nil here to pick its pure-Ruby
            //     secure_compare branch when openssl is absent —
            //     the old unconditional "method" sent it down the
            //     OpenSSL path and NameError'd at call time.
            //   - self / lvar / ivar receiver → zero-side-effect
            //     reads; runtime respond check directly (an unset
            //     ivar reads as nil and the check runs against
            //     NilClass, same as CRuby).
            //   - anything else (chained calls, receivers with
            //     side effects, ...) → "method" optimistically
            //     (documented divergence, unchanged).
            if let Some(recv) = cn.receiver() {
                let mname = cid_to_string(cn.name());
                let const_key = if let Some(cr) = recv.as_constant_read_node() {
                    Some(cid_to_string(cr.name()))
                } else if recv.as_constant_path_node().is_some() {
                    flatten_constant_path(&recv)
                } else {
                    None
                };
                let probe = sp(node, Expr::Call {
                    receiver: None,
                    name: "__defined_recv_method?".into(),
                    args: vec![tr(ctx, &recv), sp(node, Expr::SymbolLit(mname))],
                    kwargs_trailing: false,
                });
                if let Some(key) = const_key {
                    let cond = sp(node, Expr::Call {
                        receiver: None,
                        name: "__defined_const?".into(),
                        args: vec![sp(node, Expr::SymbolLit(key))],
                        kwargs_trailing: false,
                    });
                    return Spanned::new(span, Expr::If {
                        cond: Box::new(cond),
                        then_body: vec![probe],
                        else_body: vec![sp(node, Expr::Nil)],
                    });
                }
                if recv.as_self_node().is_some()
                    || recv.as_local_variable_read_node().is_some()
                {
                    return probe;
                }
                // ivar receiver: CRuby checks the RECEIVER's
                // definedness first — `defined?(@unset.to_s)` is
                // nil even though nil responds to to_s. Guard
                // with `__defined_ivar?`, mirroring the const
                // shape.
                if let Some(iv) = recv.as_instance_variable_read_node() {
                    let ivname = cid_to_string(iv.name());
                    let cond = sp(node, Expr::Call {
                        receiver: None,
                        name: "__defined_ivar?".into(),
                        args: vec![sp(node, Expr::SymbolLit(ivname))],
                        kwargs_trailing: false,
                    });
                    return Spanned::new(span, Expr::If {
                        cond: Box::new(cond),
                        then_body: vec![probe],
                        else_body: vec![sp(node, Expr::Nil)],
                    });
                }
            }
            return s("method");
        }
        return s("expression");
    }
    if let Some(n) = node.as_lambda_node() {
        // `->(x, *rest) { body }` — same param shape as block
        // literals: requireds + optional rest. Lambda body is
        // a `Vec<SExpr>` evaluated in the block proto.
        // Lambda literals take implicit `_1`/`it` params too
        // (`-> { _1 + 1 }`, `-> { it * 3 }`), same three parameter-node
        // shapes a block has — synthesize the implicit slots.
        let mut kw_defaults: Vec<(String, SExpr)> = Vec::new();
        let params: Vec<BlockParam> = match n.parameters() {
            None => Vec::new(),
            Some(pn) => {
                if let Some(np) = pn.as_numbered_parameters_node() {
                    (1..=np.maximum())
                        .map(|i| BlockParam::Single(format!("_{i}")))
                        .collect()
                } else if pn.as_it_parameters_node().is_some() {
                    vec![BlockParam::Single("it".to_string())]
                } else {
                    pn.as_block_parameters_node()
                        .and_then(|bp| bp.parameters())
                        .map(|p| {
                            let mut out: Vec<BlockParam> = p.requireds().iter()
                                .filter_map(|r| r.as_required_parameter_node()
                                    .map(|rp| BlockParam::Single(cid_to_string(rp.name()))))
                                .collect();
                            // Optional positionals (`->(a, b = 1, *c)`) —
                            // after requireds, before rest.
                            walk_block_optionals(ctx, &p, &mut out, &mut kw_defaults);
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
                            // `->(**opts) { }` keyword-rest, same as block form.
                            if let Some(kr) = p.keyword_rest()
                                && let Some(krp) = kr.as_keyword_rest_parameter_node() {
                                    let name = krp.name().map(cid_to_string).unwrap_or_default();
                                    out.push(BlockParam::KwRest(name));
                                }
                            // `->(k1:, k2: default) { }` — same keyword walk
                            // + body-prologue desugar as block literals.
                            walk_block_keywords(ctx, &p, &mut out, &mut kw_defaults);
                            out
                        })
                        .unwrap_or_default()
                }
            }
        };
        let mut body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        prepend_kw_default_prologue(&mut body, kw_defaults);
        // A real `->(){}` lambda literal — flagged so Proc#lambda? is true.
        return sp(node, Expr::Lambda { params, body, is_lambda: true });
    }
    if let Some(n) = node.as_yield_node() {
        // `yield(*x)` / `yield(a, *b)` — a splat in the args needs the
        // dynamic-argc path. Reuse the splat-chunking array builder, then
        // emit `Op::ApplyYield` (expands the Array, yields its elements).
        let has_splat = n.arguments()
            .map(|a| a.arguments().iter().any(|c| c.as_splat_node().is_some()))
            .unwrap_or(false);
        if has_splat
            && let Some(arr) = collect_multi_return_value(ctx, n.arguments(), node)
        {
            return sp(node, Expr::YieldSplat(arr));
        }
        // A trailing `yield(a: 1, b: 2)` / `yield a: 1` reaches Prism
        // as a `KeywordHashNode` (the same `k: v` sugar as a call
        // site). CRuby yields it as a single trailing Hash, so route
        // it through `tr_kwhash` like the call path does — otherwise
        // `tr` hits the KeywordHashNode unsupported-node arm and the
        // whole file fails to compile. The block's `|h|` / `|**o|`
        // binding extracts it from the trailing Hash exactly as it
        // does for `yield({a: 1})` (which already worked).
        let args: Vec<SExpr> = n.arguments()
            .map(|a| a.arguments().iter().map(|c| {
                if let Some(kh) = c.as_keyword_hash_node() {
                    tr_kwhash(ctx, node, &c, &kh)
                } else {
                    tr(ctx, &c)
                }
            }).collect())
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
    if let Some(fs) = node.as_forwarding_super_node() {
        // Bare `super` — forwards all of the enclosing method's
        // args. The arg list is filled in at compile time by
        // emitting LoadLocal for each param slot, so the AST
        // just stores `None` here. `super do … end` attaches a block
        // literal that must be forwarded to the parent method.
        if let Some(bn) = fs.block() {
            let (block_params, block_body) = tr_block_node(ctx, &bn);
            return sp(node, Expr::SuperWithBlock { args: None, block_params, block_body });
        }
        // Inside a `def m(...)` method, bare `super` forwards the
        // anonymous rest/kwrest/block EXACTLY like `super(...)` —
        // splat the `*` rest, kwsplat `__kw_rest_anon`, pass `&`. The
        // plain `Super(None)` slot-dump otherwise passed the `*` rest
        // ARRAY as a single positional arg (signalize's
        // `def signal_accessor(...); super; end` then saw `names ==
        // [[...]]`). Mirror the `super(...)` desugar below.
        if matches!(ctx.method_forward_stack.last(), Some(true)) {
            let star = sp(node, Expr::Call {
                receiver: None,
                name: "Array".into(),
                args: vec![sp(node, Expr::LVarRead("*".to_string()))],
                kwargs_trailing: false,
            });
            let kw = kwsplat_chunk(node, sp(node, Expr::LVarRead("__kw_rest_anon".to_string())));
            let acc = sp(node, Expr::Call {
                receiver: Some(Box::new(star)),
                name: "+".into(),
                args: vec![kw],
                kwargs_trailing: false,
            });
            return sp(node, Expr::SuperApply {
                args: Box::new(acc),
                block_arg: Some(Box::new(sp(node, Expr::LVarRead("&".to_string())))),
            });
        }
        return sp(node, Expr::Super(None));
    }
    if let Some(n) = node.as_super_node() {
        let arg_nodes: Vec<ruby_prism::Node<'_>> = n.arguments()
            .map(|args| args.arguments().iter().collect())
            .unwrap_or_default();
        // Detect explicit `&block` on the super call. Same shape
        // as the CallNode block-arg detection — anonymous `&` and
        // `&:sym` shapes are skipped (their richer expansions
        // would need their own Apply variants; no vendored gem
        // we ship hits those for super yet).
        let super_block_arg: Option<Box<SExpr>> = n.block().and_then(|bnode| {
            bnode.as_block_argument_node()
                .and_then(|ba| ba.expression())
                .and_then(|expr| {
                    if expr.as_symbol_node().is_some() {
                        None
                    } else {
                        Some(Box::new(tr(ctx, &expr)))
                    }
                })
        });
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
        // Per-arg translator that mirrors the regular Call args
        // walk (~line 1681): a trailing `KeywordHashNode` (Prism's
        // shape for `k: v, **opts` sugar inside a call) routes
        // through `tr_kwhash` so `**opts` AssocSplats merge via
        // the `.merge(opts)` chain. Without this, super-with-kw
        // shapes like `super(s, **options) { options }` (Mustermann
        // pattern.rb:59) tripped the `unsupported node:
        // KeywordHashNode` trap. Plain nodes fall through to `tr`.
        let tr_super_arg = |ctx: &mut TranslationCtx<'_>, c: &ruby_prism::Node<'_>| -> SExpr {
            if let Some(kh) = c.as_keyword_hash_node() {
                tr_kwhash(ctx, node, c, &kh)
            } else {
                tr(ctx, c)
            }
        };
        // `super(key, ...)` — Ruby 3.0 argument forwarding in an
        // EXPLICIT-args super call (distinct from bare `super`, which
        // forwards the caller's own args implicitly). Same desugar as
        // the regular-call forwarding path: leading positionals +
        // Array(`*` rest sentinel) + the kwsplat chunk, assembled into
        // one Array and routed through `Op::ApplySuper`, with the block
        // (`&` sentinel) forwarded. Surfaced by faraday's
        // `Utils::Headers#fetch` (`def fetch(key, ...); super(key, ...); end`).
        if arg_nodes.iter().any(|c| c.as_forwarding_arguments_node().is_some()) {
            let mut chunks: Vec<SExpr> = Vec::new();
            let mut buf: Vec<SExpr> = Vec::new();
            for c in &arg_nodes {
                if c.as_forwarding_arguments_node().is_some() {
                    if !buf.is_empty() {
                        chunks.push(sp(node, Expr::ArrayLit(std::mem::take(&mut buf))));
                    }
                    chunks.push(sp(node, Expr::Call {
                        receiver: None,
                        name: "Array".into(),
                        args: vec![sp(node, Expr::LVarRead("*".to_string()))],
                        kwargs_trailing: false,
                    }));
                    chunks.push(kwsplat_chunk(node, sp(node, Expr::LVarRead("__kw_rest_anon".to_string()))));
                } else {
                    buf.push(tr_super_arg(ctx, c));
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
            return sp(node, Expr::SuperApply {
                args: Box::new(acc),
                block_arg: super_block_arg.or_else(|| Some(Box::new(sp(node, Expr::LVarRead("&".to_string()))))),
            });
        }
        let has_splat = arg_nodes.iter().any(|c| c.as_splat_node().is_some());
        // `super(args) do … end` — a block LITERAL (not `&proc`). The
        // splat-free arg form is the common shape; a splat combined
        // with a literal block falls through to the plain paths below.
        if super_block_arg.is_none()
            && !has_splat
            && let Some(bnode) = n.block()
            && let Some(bn) = bnode.as_block_node()
        {
            let (block_params, block_body) = tr_block_node(ctx, &bn);
            let args: Vec<SExpr> = arg_nodes.iter().map(|c| tr_super_arg(ctx, c)).collect();
            return sp(node, Expr::SuperWithBlock {
                args: Some(args),
                block_params,
                block_body,
            });
        }
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
                    buf.push(tr_super_arg(ctx, c));
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
            return sp(node, Expr::SuperApply { args: Box::new(acc), block_arg: super_block_arg });
        }
        // Non-splat with `&block` still routes through SuperApply
        // — wrap args in an ArrayLit so the splat-shaped opcode
        // (ApplySuperBlock) sees a uniform `[block, array]` stack
        // layout. The cost is one extra Array build, but it avoids
        // a fourth Op::Super variant just to carry a block slot.
        if super_block_arg.is_some() {
            let args_arr: Vec<SExpr> = arg_nodes.iter().map(|n| tr_super_arg(ctx, n)).collect();
            let array = sp(node, Expr::ArrayLit(args_arr));
            return sp(node, Expr::SuperApply {
                args: Box::new(array),
                block_arg: super_block_arg,
            });
        }
        let args: Vec<SExpr> = arg_nodes.iter().map(|n| tr_super_arg(ctx, n)).collect();
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
    //
    // The subject is bound to a fresh local so it's evaluated EXACTLY
    // ONCE (CRuby evaluates the case subject a single time, then matches
    // each `when` against it with `===`). Each `when` comparison reads
    // that local. Previously the translated subject was cloned into
    // every `when`, re-evaluating a side-effecting predicate per
    // condition — e.g. rack's multipart `case consume_boundary` advanced
    // the StringScanner once per `when`, mis-parsing the body.
    if let Some(n) = node.as_case_node() {
        let subj_local = n.predicate().map(|_| ctx.fresh_pm());
        let subj_value = n.predicate().map(|p| tr(ctx, &p));
        let predicate: Option<SExpr> = subj_local
            .as_ref()
            .map(|name| sp(node, Expr::LVarRead(name.clone())));
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
                                kwargs_trailing: false,
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
        let chain = if acc.is_empty() {
            sp(node, Expr::LVarRead("nil".into()))
        } else {
            acc.into_iter().next().unwrap()
        };
        // Prepend the once-only subject binding (when there is a
        // predicate) so the if-chain reads the bound local.
        return match (subj_local, subj_value) {
            (Some(name), Some(val)) => {
                let seq = vec![
                    sp(node, Expr::LVarWrite(name, Box::new(val))),
                    chain,
                ];
                sp(node, seq_inner(seq))
            }
            _ => chain,
        };
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
        let mut is_dotdotdot_forward = false;
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
            // `def m(...)` — Ruby 3.0 argument forwarding. Prism puts a
            // ForwardingParameterNode in the keyword_rest slot; it
            // stands in for an anonymous rest + kwrest + block all at
            // once. Bind the same reserved sentinels the standalone
            // anonymous `*` / `**` / `&` forms use (rest `*`, kwrest
            // `""` → compiler's `__kw_rest_anon` slot, block `&`); the
            // matching `inner(...)` call site reads them back.
            if let Some(r) = p.keyword_rest()
                && r.as_forwarding_parameter_node().is_some()
            {
                rest = Some("*".to_string());
                kw_rest = Some(String::new());
                block_param = Some("&".to_string());
                is_dotdotdot_forward = true;
            }
            for kw in p.keywords().iter() {
                if let Some(rk) = kw.as_required_keyword_parameter_node() {
                    kw_params.push((cid_to_string(rk.name()), None));
                } else if let Some(ok) = kw.as_optional_keyword_parameter_node() {
                    let name = cid_to_string(ok.name());
                    let val = tr(ctx, &ok.value());
                    // Accept any expression as a kwarg default —
                    // mirrors positional defaults. The compiler
                    // routes literals (`Int`, `Float`, `Str`, `Symbol`,
                    // `Bool`, `Nil`) into `Proto::kw_param_defaults`
                    // for the binder's fast path; non-literals
                    // (`ConstRead`, method call, prior-param ref,
                    // ...) get a `JumpIfKwArgGiven` prologue at
                    // method-body entry — same shape as the
                    // positional default-arg prologue.
                    kw_params.push((name, Some(val)));
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
        // Track `(...)` forwarding across the body so bare `super`
        // forwards the anonymous args like `super(...)` (see
        // `method_forward_stack`).
        ctx.method_forward_stack.push(is_dotdotdot_forward);
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        ctx.method_forward_stack.pop();
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
        // A trailing `k: v` inside an array literal (`[:public, max_age:
        // 0]`) parses as a KeywordHashNode element — it's a plain Hash
        // element (`[:public, {max_age: 0}]`), not call kwargs. Route it
        // through `tr_kwhash`; bare `tr` would hit the unsupported-node
        // trap. Sinatra's `set :static_cache_control, [:public, max_age:
        // 0]`.
        let tr_elem = |ctx: &mut TranslationCtx<'_>, e: &Node<'_>| -> SExpr {
            if let Some(kh) = e.as_keyword_hash_node() {
                tr_kwhash(ctx, node, e, &kh)
            } else {
                tr(ctx, e)
            }
        };
        if !has_splat {
            let elems: Vec<SExpr> = raw_elems.iter().map(|e| tr_elem(ctx, e)).collect();
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
                    // Wrap the splat'd expression with `Array(x)` so
                    // CRuby's coerce-to-array contract holds:
                    //   - Array → unchanged
                    //   - nil   → []
                    //   - other → [other] (`[*"foo"]` → `["foo"]`)
                    // Without this, `[*scalar]` collapsed to `scalar`
                    // (the chained `Array#+` reducer relied on every
                    // chunk being an Array). Surfaced by
                    // sinatra-contrib/MultiRoute's
                    // `routes = [*args.pop]` idiom — when `args.pop`
                    // returned a String the routes loop tripped
                    // `String#each`.
                    chunks.push(sp(node, Expr::Call {
                        receiver: None,
                        name: "Array".into(),
                        args: vec![tr(ctx, &inner)],
                        kwargs_trailing: false,
                    }));
                } else {
                buf.push(tr_elem(ctx, en));
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
                e.as_assoc_node().map(|a| (tr(ctx, &a.key()), tr_assoc_value(ctx, &a)))
            }).collect();
            return sp(node, Expr::HashLit(pairs));
        }
        let mut chunks: Vec<SExpr> = Vec::new();
        let mut buf: Vec<(SExpr, SExpr)> = Vec::new();
        for el in n.elements().iter() {
            if let Some(an) = el.as_assoc_node() {
                buf.push((tr(ctx, &an.key()), tr_assoc_value(ctx, &an)));
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
        // Absolute-path handling: callers that need to preserve
        // leading `::` (this site + the ConstantPathNode →
        // ConstRead lowering at line ~915) consult
        // `is_constant_path_absolute` and prefix the flattened
        // name with `::` so the compiler emits a flat LoadConst
        // and skips cref-walk. `flatten_constant_path` itself
        // still drops the marker — that's intentional, since
        // most other consumers want the bare joined name. Each
        // caller decides whether absolute info matters.
        // Generic superclass — any expression. Translate via the
        // standard `tr` recursive walker; the compiler will emit
        // bytecode that pushes a Value onto the stack and DefClass
        // will pop it.
        let superclass = n.superclass().map(|s| Box::new(tr(ctx, &s)));
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        let absolute = is_constant_path_absolute(&cp);
        return sp(node, Expr::Class { name, superclass, body, is_module: false, absolute });
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
        let absolute = is_constant_path_absolute(&cp);
        let body: Vec<SExpr> = match n.body() {
            Some(b) => {
                if let Some(stmts) = b.as_statements_node() {
                    stmts.body().iter().map(|c| tr(ctx, &c)).collect()
                } else { vec![tr(ctx, &b)] }
            }
            None => vec![],
        };
        return sp(node, Expr::Class { name, superclass: None, body, is_module: true, absolute });
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
    // `undef foo, bar` keyword form — desugar into a synthetic
    // `undef_method :foo, :bar` Call so the existing class-intrinsic
    // `undef_method` arm (dispatch.rs) handles it. Removal itself is
    // a Tier-1 no-op there (only the `method_undefined` hook fires),
    // so `undef` carries the same semantics. All names must be plain
    // `SymbolNode`s (the common case, `undef freeze`); a dynamic /
    // interpolated name (`undef :"a#{i}"`) is exotic and falls
    // through to the unsupported-node trail unchanged. Motivating
    // consumer: concurrent-ruby's `undef freeze` (pulled by i18n).
    // Backtick / `%x{…}` command execution (`XStringNode` and its
    // interpolated form). rubyrs is a Tier-1 sandbox with no
    // subprocess capability, so rather than reject the syntax at
    // compile time (which fails the whole file load), we COMPILE it
    // to a runtime `raise` of a StandardError. A bare `rescue`
    // catches it — matching how CRuby's `Errno::ENOENT` (raised when
    // the command isn't found) is caught — so guarded probes degrade
    // gracefully. Discovery: P3 Jekyll spike — safe_yaml's
    // libyaml_checker.rb does `(`which dpkg` rescue '').empty?` at
    // (deferred) runtime; compiling the backtick lets safe_yaml load,
    // and the rescue yields '' so its libyaml probe reports "absent".
    if let Some(xs) = node.as_x_string_node() {
        // Plain backtick: dispatch to the capability-gated builtin
        // (off → the same catchable RuntimeError the old
        // compile-time raise produced; on → captured stdout).
        let cmd = String::from_utf8_lossy(xs.unescaped()).into_owned();
        return sp(node, Expr::Call {
            receiver: None,
            name: "__rubyrs_backtick".into(),
            args: vec![sp(node, Expr::StrLit(cmd))],
            kwargs_trailing: false,
        });
    }
    if let Some(n) = node.as_interpolated_x_string_node() {
        // Interpolated backtick — build the command string with the
        // same parts walk as interpolated strings, then dispatch to
        // the capability-gated builtin. minitest's diff pipeline is
        // exactly `\`#{diff_tool} #{a.path} #{b.path}\``.
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
        let cmd = sp(node, Expr::InterpolatedStr(parts));
        return sp(node, Expr::Call {
            receiver: None,
            name: "__rubyrs_backtick".into(),
            args: vec![cmd],
            kwargs_trailing: false,
        });
    }
    // `for x in coll; body; end` — desugar to `coll.each { |x| body
    // }`. The loop target is a `LocalVariableTargetNode` (the common
    // `for x in …` shape); a multi-target (`for a, b in …`) maps to a
    // destructuring block param. Other target shapes (ivar/cvar
    // targets) fall through to the unsupported-node trail.
    //
    // DIVERGENCE: CRuby's `for` does NOT introduce a new scope (the
    // loop var and any vars first-assigned in the body leak to the
    // surrounding scope); the `.each` block does scope its params.
    // Bodies that rely on post-loop leakage diverge — a documented
    // Tier-1 trade-off, same family as the block-scope notes in
    // SUBSET.md. Discovery: P3 Jekyll spike — kramdown's html.rb
    // uses `for element in @stack … end` at load time.
    if let Some(n) = node.as_for_node() {
        let index = n.index();
        let block_params: Option<Vec<BlockParam>> =
            if let Some(lt) = index.as_local_variable_target_node() {
                Some(vec![BlockParam::Single(cid_to_string(lt.name()))])
            } else if let Some(mt) = index.as_multi_target_node() {
                let mut inner = Vec::new();
                let mut ok = true;
                for t in mt.lefts().iter() {
                    if let Some(lt) = t.as_local_variable_target_node() {
                        inner.push(BlockParam::Single(cid_to_string(lt.name())));
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok { Some(vec![BlockParam::Destructure(inner)]) } else { None }
            } else {
                None
            };
        if let Some(block_params) = block_params {
            let receiver = Some(Box::new(tr(ctx, &n.collection())));
            let block_body: Vec<SExpr> = match n.statements() {
                Some(stmts) => stmts.body().iter().map(|c| tr(ctx, &c)).collect(),
                None => vec![],
            };
            return sp(node, Expr::CallWithBlock {
                receiver,
                name: "each".into(),
                args: vec![],
                block_params,
                block_body,
                kwargs_trailing: false,
            });
        }
    }
    if let Some(n) = node.as_undef_node() {
        let name_nodes: Vec<_> = n.names().iter().collect();
        let args: Vec<SExpr> = name_nodes
            .iter()
            .filter_map(|nm| {
                nm.as_symbol_node().map(|sym| {
                    let name = String::from_utf8_lossy(sym.unescaped()).into_owned();
                    sp(node, Expr::SymbolLit(name))
                })
            })
            .collect();
        if !name_nodes.is_empty() && args.len() == name_nodes.len() {
            return sp(node, Expr::Call {
                receiver: None,
                name: "undef_method".into(),
                args,
                kwargs_trailing: false,
            });
        }
    }
    if node.as_singleton_class_node().is_some() {
        return tr_singleton_class(ctx, node);
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
                    // Absolute paths (`rescue ::Foo::Bar`) carry a
                    // leading `::` marker so the PushRescue handler
                    // can skip the lex-walk and look up the joined
                    // name at top level only. Without this, inside
                    // `module Wrapper` that also defines `TopErr`,
                    // `rescue ::TopErr` would lex-walk and match
                    // `Wrapper::TopErr` instead of the intended
                    // top-level class.
                    classes.push(crate::const_marker::tag_absolute(joined, is_constant_path_absolute(&exc)));
                } else if let Some(sp) = exc.as_splat_node() {
                    // `rescue *CONST` — minitest's
                    // `rescue *PASSTHROUGH_EXCEPTIONS` idiom. The
                    // constant NAME travels with a splat marker;
                    // PushRescue resolves it to an Array of classes
                    // at run time and matches any element. Only
                    // const-shaped splat operands are supported;
                    // anything else falls to the drop note below.
                    // (Before this arm existed the splat was
                    // silently dropped → empty class list → bare
                    // `rescue` → StandardError matched EVERYTHING,
                    // which made minitest's passthrough arm re-raise
                    // every test error and kill the whole run.)
                    if let Some(inner) = sp.expression() {
                        if let Some(c) = inner.as_constant_read_node() {
                            classes.push(crate::const_marker::tag_splat(cid_to_string(c.name())));
                        } else if inner.as_constant_path_node().is_some()
                            && let Some(joined) = flatten_constant_path(&inner)
                        {
                            classes.push(crate::const_marker::tag_splat(
                                crate::const_marker::tag_absolute(joined, is_constant_path_absolute(&inner)),
                            ));
                        } else if let Some(lv) = inner.as_local_variable_read_node() {
                            // `rescue *exp` on a LOCAL — minitest's
                            // `assert_raises *exp` shape. The compiler
                            // resolves the name to a slot and emits
                            // `Op::PushRescueSplatLocal`. Caveat: slot
                            // resolution uses the CURRENT proto's
                            // table, so a captured outer local inside
                            // a block would mis-slot — that reads Nil
                            // and matches nothing (fail-closed), it
                            // can't wrong-catch.
                            classes.push(crate::const_marker::tag_splat_local(cid_to_string(lv.name())));
                        }
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
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect::<Vec<SExpr>>())
                .unwrap_or_default()
        });
        // `begin … rescue … else E … ensure … end`. The `else`
        // body runs ONLY when the protected body completes WITHOUT
        // an exception; its value becomes the begin's value; and —
        // unlike the body — an exception raised inside `else` is
        // NOT caught by the rescue clauses (it propagates, with
        // ensure still running). Prism exposes it as an ElseNode.
        if let Some(en) = n.else_clause() {
            let else_body: Vec<SExpr> = en.statements()
                .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
                .unwrap_or_default();
            if rescue.is_empty() {
                // No rescue clause: `else` is just sequenced after
                // the body (CRuby warns "else without rescue is
                // useless" but still runs it). Appending preserves
                // both the value and the ensure semantics.
                let mut merged = body;
                merged.extend(else_body);
                return sp(node, Expr::Begin { body: merged, rescue, ensure });
            }
            // Desugar by nesting. An inner begin/rescue sets a
            // hidden ok-flag true as its FIRST body statement (so
            // `retry`, which re-runs the body, re-arms it) and
            // false as the first statement of each rescue clause.
            // An outer rescue-free layer — which carries the
            // ensure — then runs `else` only when the flag
            // survived, so an exception inside `else` escapes the
            // rescue chain yet still triggers ensure.
            let ok = ctx.fresh_pm();
            let res = ctx.fresh_pm();
            let mut inner_body = vec![
                sp(node, Expr::LVarWrite(ok.clone(), Box::new(sp(node, Expr::BoolLit(true))))),
            ];
            inner_body.extend(body);
            let rescue: Vec<RescueClause> = rescue.into_iter().map(|mut rc| {
                let mut rb = vec![
                    sp(node, Expr::LVarWrite(ok.clone(), Box::new(sp(node, Expr::BoolLit(false))))),
                ];
                rb.append(&mut rc.body);
                rc.body = rb;
                rc
            }).collect();
            let inner = sp(node, Expr::Begin { body: inner_body, rescue, ensure: None });
            let assign = sp(node, Expr::LVarWrite(res.clone(), Box::new(inner)));
            let branch = sp(node, Expr::If {
                cond: Box::new(sp(node, Expr::LVarRead(ok))),
                then_body: else_body,
                else_body: vec![sp(node, Expr::LVarRead(res))],
            });
            return sp(node, Expr::Begin { body: vec![assign, branch], rescue: vec![], ensure });
        }
        return sp(node, Expr::Begin { body, rescue, ensure });
    }
    if let Some(n) = node.as_post_execution_node() {
        // `END { ... }` — runs the body at program exit, LIFO across
        // multiple ENDs. That's exactly `at_exit`'s contract (verified
        // LIFO-equivalent), so desugar to `at_exit { ... }` and reuse
        // the existing Kernel#at_exit machinery.
        let block_body: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        return sp(node, Expr::CallWithBlock {
            receiver: None,
            name: "at_exit".to_string(),
            args: vec![],
            block_params: vec![],
            block_body,
            kwargs_trailing: false,
        });
    }
    if let Some(n) = node.as_pre_execution_node() {
        // `BEGIN { ... }` — CRuby hoists it to run before the rest of
        // the program regardless of textual position. Tier-1 runs the
        // body inline at this position instead; correct for the
        // conventional top-of-file placement (the only one CRuby's
        // grammar really encourages), a documented divergence when a
        // BEGIN is written after code that should run later.
        let stmts: Vec<SExpr> = n.statements()
            .map(|s| s.body().iter().map(|c| tr(ctx, &c)).collect())
            .unwrap_or_default();
        return sp(node, seq_inner(stmts));
    }
    // Pattern matching (`case/in`, `expr => pat`, `expr in pat`). The
    // bodies carry a large local set (subject temp, else-chain Vec, the
    // per-arm guard locals); kept in a separate `#[inline(never)]`
    // function so they don't inflate the recursive `tr` stack frame —
    // a bloated `tr` frame overflows the 2 MB test thread on a deeply
    // nested AST under the debug + llvm-cov Coverage build (same reason
    // `tr_singleton_class` is extracted).
    if let Some(r) = tr_pattern_construct(ctx, node) {
        return r;
    }
    if let Some(ff) = node.as_flip_flop_node() {
        return tr_flip_flop(ctx, node, &ff);
    }
    // `MatchWriteNode` — a regexp-LITERAL `=~` whose pattern has named
    // captures (`/(?<y>\d+)-(?<m>\d+)/ =~ str`). CRuby evaluates the
    // `=~`, then binds a local variable PER named capture to that
    // group's text (or nil when the match failed). Desugar into:
    //   __mw_N = (re =~ str)         # the =~ call sets $~
    //   y = $~ ? $~[:y] : nil
    //   m = $~ ? $~[:m] : nil
    //   __mw_N                        # whole expr value is the =~ result
    // The synthetic temp carries the match index/nil so the sequence's
    // value matches the bare `=~`. The `$~ ? …` guard mirrors CRuby
    // setting the locals to nil (not erroring) on no-match.
    if let Some(mw) = node.as_match_write_node() {
        let call_node = mw.call();
        let tmp = format!("__mw_{}", node_span(node).byte_offset);
        let mut body: Vec<SExpr> = Vec::new();
        body.push(sp(
            node,
            Expr::LVarWrite(tmp.clone(), Box::new(tr(ctx, &call_node.as_node()))),
        ));
        for t in mw.targets().iter() {
            if let Some(lt) = t.as_local_variable_target_node() {
                let name = cid_to_string(lt.name());
                let index = sp(
                    node,
                    Expr::Call {
                        receiver: Some(Box::new(sp(node, Expr::GVarRead("$~".to_string())))),
                        name: "[]".to_string(),
                        args: vec![sp(node, Expr::SymbolLit(name.clone()))],
                        kwargs_trailing: false,
                    },
                );
                let guarded = sp(
                    node,
                    Expr::If {
                        cond: Box::new(sp(node, Expr::GVarRead("$~".to_string()))),
                        then_body: vec![index],
                        else_body: vec![sp(node, Expr::Nil)],
                    },
                );
                body.push(sp(node, Expr::LVarWrite(name, Box::new(guarded))));
            }
        }
        body.push(sp(node, Expr::LVarRead(tmp)));
        return sp(node, Expr::Begin { body, rescue: vec![], ensure: None });
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
        // `alias $new $old` (AliasGlobalVariableNode) — global-variable
        // aliasing, vanishingly rare and outside the subset. The
        // translator should collect a message instead of panicking.
        // (Canary keeps moving to a still-unsupported node as the gap
        // closes — was `BEGIN { }`, then `case/in`; both gained support.
        // Global-var alias mirrors the embed/error_handling.rs canary.)
        let (_, errs) = translate("alias $new_g $old_g");
        assert!(!errs.is_empty(), "alias-gvar should produce AST errors");
        assert!(
            errs.iter().any(|e| e.contains("unsupported")),
            "expected 'unsupported' wording, got: {errs:?}"
        );
    }

    #[test]
    fn ast_errors_buffer_resets_between_calls() {
        // First call has unsupported nodes — leaves errors in the
        // buffer (which tr_with_errors drains on the way out).
        let (_, e1) = translate("alias $new_g $old_g");
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

