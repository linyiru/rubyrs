use std::rc::Rc;

use crate::error::Span;
use crate::intern::SymId;
use crate::value::Value;

// ---------- Bytecode ----------

// `BinOp` + `BinOpInt` are paired variants — the suffix distinguishes
// the integer-fused fast path from the generic dispatch one; both
// match an existing convention in the rest of the codebase ("Op" is
// the bytecode-instruction nature, not a redundant tag).
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "preamble-cache", derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum Op {
    LoadConstInt(i64),
    /// Float literal — `5.0`, `3.14`, etc. f64 is Copy so the Op
    /// stays Copy. Float arithmetic dispatches through
    /// `primitive_call`'s Float arms; the BinOp fast path
    /// (Int + Int) doesn't fire on Float receivers.
    LoadConstFloat(f64),
    LoadConstStr(SymId),
    /// String literal whose Prism-unescaped bytes aren't valid
    /// UTF-8 (typically `\xNN` escapes producing high-byte
    /// sequences — `"\xFF\xFF"`, binary protocol literals,
    /// hex-string sentinels). Indexes into the current Proto's
    /// `byte_literals` table; runtime constructs a fresh
    /// `Value::Str` from the stored `Rc<[u8]>` so the raw bytes
    /// survive the round-trip. The valid-UTF-8 path keeps using
    /// `LoadConstStr(SymId)` so Symbol-shaped strings still hit
    /// the global interner.
    LoadConstStrBytes(u32),
    /// `/pattern/` literal — looks up an Rc<Regex> in the Vm's
    /// `regex_cache`, compiling it from the interned source the
    /// first time. A compile-time bad pattern surfaces as a
    /// SyntaxError trap at run-time (not at parse-time, since
    /// we lazy-compile). Cfg-gated on the `regex` feature
    /// (ADR 0017 Rule 3) — with the feature off the variant
    /// disappears, AST translation rejects `/.../` literals,
    /// and `Expr::RegexLit` never reaches the compiler arm.
    /// The `u8` is the Ruby flag bitmask (IGNORECASE=1 |
    /// EXTENDED=2 | MULTILINE=4); the runtime applies it as an
    /// inline `(?is)` prefix before compiling and folds it into
    /// the `regex_cache` key (so `/foo/` and `/foo/i` don't
    /// collide).
    #[cfg(feature = "regex")]
    LoadRegex(SymId, u8),
    /// Load an integer literal that overflows i64. The SymId is
    /// the interned decimal representation of the value; the
    /// runtime parses to `BigInt` on first load and caches in
    /// `Vm.bigint_lit_cache` keyed by SymId. Same shape as
    /// `LoadRegex`. Cfg-gated on `bignum`; without the feature
    /// the variant disappears and the AST translator emits a
    /// saturated `IntLit` instead.
    #[cfg(feature = "bignum")]
    LoadBigInt(SymId),
    /// Materializes a `Value::Rational` from interned decimal-string
    /// `num` and `den` components (Phase C.4.4). Under bignum both
    /// strings parse into `BigInt` and route through
    /// `make_rational_bigint`; under no-bignum into i64 (RangeError
    /// on overflow) and route through `make_rational(i64, i64)`.
    ///
    /// The bignum AST lowering does gcd-reduction and sign-
    /// normalization at parse time so the strings hit `make_rational_bigint`
    /// already canonical (the redundant gcd is then ~free). The
    /// no-bignum lowering formats each component via a u128
    /// accumulator with a `u128::MAX` sentinel fallback for the
    /// (rare) > u128 case, then relies on `make_rational` to
    /// gcd-reduce + sign-normalize at load time. Per-component
    /// parse cache reuses `bigint_lit_cache` (no new map).
    LoadRational(SymId, SymId),
    /// Pop a Value::Str, compile it as a Regex pattern, push
    /// Value::Regex. Emitted by `Expr::InterpolatedRegex` after
    /// the same `to_s + +` build sequence used by InterpolatedStr.
    /// Pattern reuse hits the same `regex_cache` keyed by SymId
    /// of the assembled pattern. Compile errors surface as
    /// SyntaxError traps at runtime (same shape as `LoadRegex`,
    /// since the pattern is unknown until interpolation runs).
    /// The `u8` carries the Ruby flag bitmask for the
    /// interpolated pattern (same encoding as `LoadRegex`).
    #[cfg(feature = "regex")]
    CompileRegex(u8),
    /// String-interpolation part conversion (`"#{x}"`): if the
    /// top-of-stack is already a String, leave it (CRuby's
    /// `rb_obj_as_string` returns T_STRING values as-is — a user
    /// `String#to_s` override is NOT consulted); otherwise dispatch
    /// `to_s` through `do_call` (user overrides honored, e.g.
    /// `"#{5}"` sees a reopened `Integer#to_s`). Replaces the plain
    /// `Op::Call(to_s)` the interpolation compiler used to emit —
    /// which both diverged on String parts and paid a full dispatch
    /// per part. The u16 is the call-site cache id for the dispatch
    /// path (same slot a Call would carry).
    InterpToS(u16),
    /// Assignment-syntax dispatch (`recv.attr = v` / `recv[k] = v`):
    /// identical to `Op::Call` EXCEPT the expression result is the
    /// final positional argument (the RHS), never the method's
    /// return value (CRuby rule — purely syntactic, `send(:attr=)`
    /// keeps the return). The handler snapshots the RHS (stack top)
    /// before dispatch; an inline completion gets its pushed result
    /// replaced, a frame-based user method gets `Frame.swap_return`
    /// (the Class.new mechanism, already a GC root). Emitted by
    /// `Expr::AssignCall`.
    CallAset(SymId, u8, u16),
    LoadSymbol(SymId),
    LoadNil,
    LoadTrue,
    LoadFalse,
    LoadSelf,
    LoadLocal(u16),
    StoreLocal(u16),
    /// Fast path for `name = name + 1`: increment slot in place, push new value.
    /// Falls back to a synthesised `BinOp::Add` if the slot doesn't hold an Int.
    IncLocal(u16),
    /// Same as `IncLocal` but does *not* push the resulting value. Emitted
    /// in statement position where the body discards the value anyway.
    IncLocalNoPush(u16),
    Dup,
    Pop,
    /// Swap the top two values on the operand stack. Used by
    /// multi-write with method-call setters
    /// (`obj.foo, obj.bar = a, b`): we need `[..., recv, val]`
    /// to dispatch `recv.foo=(val)` but the natural eval order
    /// produces `[..., val, recv]`. One Op::Swap fixes it
    /// without needing a temp local.
    Swap,
    /// Coerce the top-of-stack into an Array for parallel
    /// assignment (`a, b = rhs`). An Array stays as-is; a value that
    /// responds to `to_ary` is converted; anything else (including
    /// `nil`) becomes a one-element `[rhs]`. Mirrors CRuby's massign
    /// RHS handling so `a, b = nil` → `[nil, nil]` (not a
    /// `NoMethodError` from `nil[0]`). Emitted by
    /// `compile_multiwrite_arm` right after the RHS so the
    /// subsequent `[]` / `__mw_splat` / `__mw_post` calls always see
    /// an Array.
    MassignSplat,
    LoadIvar(SymId),
    StoreIvar(SymId),
    /// `@@name` read. Resolves the surrounding class at runtime
    /// (frame.self_val is either a Value::Class or
    /// Value::Object; toplevel falls through to `Vm.toplevel_cvars`
    /// — a single fallback table so toplevel `@@foo` warnings
    /// don't trap). Missing names return `Value::Nil` (lenient
    /// default, like ivars).
    LoadCvar(SymId),
    /// `@@name = expr` write. Stores into the surrounding
    /// class's `class_vars` table or the toplevel fallback
    /// when no class is on the stack.
    StoreCvar(SymId),
    /// Fast path for `@name = @name + 1`. Same shape as IncLocal but on
    /// self's ivar table.
    IncIvar(SymId),
    /// Same as `IncIvar` but does *not* push the resulting value.
    IncIvarNoPush(SymId),
    LoadConst(SymId),
    /// Same lookup as `LoadConst` but missing → `Value::Nil`
    /// instead of raising `NameError`. Emitted by the AST
    /// translator ONLY for the `||=` read position (`FOO ||=
    /// default` and the `Foo::Bar ||= default` path form). CRuby
    /// special-cases `||=` so the lazy-init idiom works; every
    /// other op-write (`&&=`, `+=`, ...) uses strict `LoadConst`
    /// and raises NameError on undefined. Not exposed to user
    /// code directly. No ENV intercept (unlike `LoadConst`) —
    /// `ENV ||= ...` isn't an idiomatic shape and the
    /// short-circuit makes the missing intercept invisible in
    /// practice (ENV always resolves via `LoadConst` on every
    /// other read site).
    LoadConstOrNil(SymId),
    /// CRuby-style cref-walk constant resolution for a bare-name
    /// read INSIDE a non-empty class / module scope. The `u32`
    /// is an index into the current Proto's `const_chains`; each
    /// chain entry is the ordered list of qualified SymIds the
    /// runtime should try in turn (innermost-scope first, falling
    /// back outward to the top-level bare name). First hit in
    /// `Vm.classes` then `Vm.constants` wins; running off the end
    /// invokes the same ENV lazy-build intercept that
    /// `LoadConst("ENV")` uses, BUT only when the chain's tail
    /// (the unqualified bare-name candidate, last entry due to
    /// innermost-first ordering) is "ENV". This lets nested
    /// `class Foo; ENV[...]; end` resolve the same toplevel
    /// ENV that bare `ENV` at the top level resolves — without
    /// it, the chain `[Foo::ENV, ENV]` would fail because the
    /// fallback never consulted the intercept (PR #239 /
    /// pass-9.7c layer #20). Any other unresolved name still
    /// raises `NameError`. Top-level reads (empty class_path
    /// at compile time) keep using `LoadConst(SymId)` directly.
    LoadConstChain(u32),
    /// Silent-nil variant of `LoadConstChain` for the `||=` read
    /// position; running off the chain returns `Value::Nil`
    /// instead of raising `NameError`. Mirrors `LoadConstOrNil`'s
    /// role for plain `LoadConst`.
    LoadConstChainOrNil(u32),
    /// Pop top of stack, store as the value of constant `SymId`.
    /// Caller is responsible for emitting `Dup` first when the
    /// expression's value should also remain on the stack (CRuby's
    /// `FOO = 42` evaluates to 42).
    StoreConst(SymId),
    /// `$foo` — push the global's current value onto the stack.
    /// Special globals (`$$`, `$0`) are intercepted in the handler;
    /// plain user globals are looked up in `Vm.globals`; unknown
    /// names fall through to `Value::Nil` (CRuby's lenient
    /// uninitialized-global default).
    LoadGlobal(SymId),
    /// `$foo = expr` — pop top of stack and store as the value of
    /// global `SymId`. Caller emits `Dup` first if the assignment-
    /// as-expression value should also remain on the stack (same
    /// pattern as `Op::StoreConst`).
    StoreGlobal(SymId),
    Jump(i32),
    JumpIfFalse(i32),
    /// Default-arg prologue helper. If positional `slot` was
    /// supplied by the caller (i.e. `slot < frame.n_given_positional`),
    /// jump by `off` to skip the default-eval body. Otherwise
    /// fall through — the subsequent ops compute the default
    /// expression and `StoreLocal(slot)`. One per optional
    /// positional param; emitted at the very top of the method
    /// body by the compiler.
    JumpIfArgGiven(u16, i32),
    /// Kwarg-default prologue helper, analogous to
    /// `JumpIfArgGiven` but for keyword params with computed
    /// (non-literal) defaults. If kwarg index `kw_idx` (0-based,
    /// within the method's kw_params list) was supplied by the
    /// caller — `frame.kw_given_mask & (1 << kw_idx) != 0` —
    /// jump by `off` to skip the default-eval body. Otherwise
    /// fall through to the body that evaluates the default
    /// expression and `StoreLocal(slot)`. One per keyword
    /// param with a non-literal default; emitted immediately
    /// after the positional-default prologue at the top of the
    /// method body. Mask is 64-bit, capping non-literal kwarg
    /// defaults per method at 64 — far beyond any real
    /// signature.
    JumpIfKwArgGiven(u16, i32),
    /// Args: name SymId, argc, per-call-site inline-cache slot id.
    Call(SymId, u8, u16),
    CallNoRecv(SymId, u8, u16),
    /// Variant of `Call` / `CallNoRecv` for call sites that the
    /// compiler determined have a trailing kwargs hash (i.e. the
    /// last arg originated from a `KeywordHashNode`, the `foo(a: 1)`
    /// sugar — distinct from `foo({a: 1})` which is a positional
    /// Hash). `argc` includes the trailing Hash; the dispatcher
    /// pops it into a dedicated kwargs channel before invoking
    /// `primitive_call` / user method dispatch so primitive arms
    /// can read keyword arguments instead of having to inspect
    /// the trailing positional Hash heuristically.
    CallKw(SymId, u8, u16),
    CallKwNoRecv(SymId, u8, u16),
    /// `foo(*args)` — single-splat call. Pops the args Array
    /// (which must be `Value::Array`) and uses its elements as
    /// the positional args. Argc is dynamic. Receiver above
    /// the array on stack for `ApplyCall`; absent for the
    /// `NoRecv` variant. Used by the compiler when call args
    /// contain a SplatNode at the only position.
    ApplyCall(SymId, u16),
    ApplyCallNoRecv(SymId, u16),
    /// Like `ApplyCall` (with-recv `self.name(*args)`) but forces
    /// PRIMITIVE dispatch — sets `force_primitive_dispatch` so `do_call`
    /// skips a subclass's user override and runs the primitive. Emitted
    /// ONLY as the body of a `<primitive-alias-forwarder>` so an
    /// `alias own_keys keys` of a primitive `keys` snapshots the
    /// primitive instead of late-binding to a later `def keys`.
    ApplyCallPrimitive(SymId, u16),
    /// `foo(*args, &block)` — splat + explicit block-arg. Stack
    /// layout (bottom→top): `[recv?, block, array]`. Pops the
    /// args Array and expands its elements as positional args,
    /// then dispatches via the block-aware path so the popped
    /// block value installs as the called method's block. Used
    /// by middleware-chain build loops like
    /// `klass.new(inner_app, *args, &block)`.
    ApplyCallBlock(SymId, u16),
    ApplyCallNoRecvBlock(SymId, u16),
    /// `super(args...)`. Receiver stays `self` (popped from the
    /// current frame, not the operand stack). Method name and
    /// argc are baked in at compile time. Lookup starts at the
    /// SUPERCLASS of `self.class`, so the current method is
    /// skipped — letting overrides delegate "up" the chain.
    /// IC slot isn't used (super resolves via class chain, not
    /// the per-site cache).
    Super(SymId, u8),
    /// `super(*args)` — apply-style super dispatch. Pops one
    /// Array off the stack and uses its elements as the
    /// positional args. Mirrors `Op::ApplyCall`'s shape but
    /// the receiver is implicit (self) and lookup starts at
    /// the defining-class's superclass per CRuby's "module
    /// of definition" rule. Same name_id resolves the method.
    ApplySuper(SymId),
    /// `super(*args, &block)` — splat-super with explicit block.
    /// Stack: `[block, array]`. Pops both, expands the array's
    /// elements as positional args, and runs the same super-
    /// lookup path as `Op::ApplySuper`. The block installs on
    /// the dispatched frame so `def foo(*a, &b); super(*a, &b);
    /// end` forwards both channels through the inheritance chain
    /// (sinatra-contrib/MultiRoute uses this on every HTTP verb).
    ApplySuperBlock(SymId),
    DefMethod(SymId, u32),         // name, proto_idx
    /// `def self.foo` inside a class body — installs `foo` on
    /// the surrounding class's `singleton_methods` table (not
    /// the instance-method `methods` table). Compiled from
    /// `Expr::Def { receiver: Some(SelfExpr), .. }`. Looks up
    /// the target class via `class_stack.last()`; outside a
    /// class body the handler falls back to `toplevel_methods`.
    DefSingletonMethod(SymId, u32),
    /// `def obj.name; ...; end` with a non-`self` receiver, or
    /// `recv.define_singleton_method(:name) { ... }` —
    /// instance-level singleton install. Pops the receiver off
    /// the operand stack; raises `TypeError` if it's not a
    /// `Value::Object` (singleton methods on primitives need
    /// a more elaborate model than this PoC provides);
    /// lazily allocates a per-Object eigenclass; installs a
    /// Method (built from `proto_idx`) into the eigenclass.
    /// Distinct from `DefSingletonMethod` because the target
    /// is an Instance's eigenclass rather than a Class's own
    /// `singleton_methods` table. Bumps `method_gen` like
    /// `Op::DefMethod`.
    DefObjectSingletonMethod(SymId, u32),
    /// `alias_method :new, :old`. Resolves `old` by walking the
    /// surrounding class's ancestor chain (or `toplevel_methods` at
    /// the top level) so inherited methods can be aliased; installs
    /// the same `Rc<Method>` under `new` on the *current* class.
    /// Sharing the Rc keeps the alias and the original semantically
    /// identical — including `defining_class`, so `super` from the
    /// aliased name walks the *original*'s superclass chain. Bumps
    /// `method_gen` so per-call-site IC entries re-resolve. Raises
    /// `NameError` if `old` doesn't exist anywhere on the chain.
    AliasMethod(SymId, SymId),     // new, old
    /// `alias new old` (keyword form) inside a `class << self`
    /// body. Same as `Op::AliasMethod` but resolves `old` along
    /// the surrounding class's SINGLETON-method chain
    /// (`lookup_class_singleton_method`) and installs the same
    /// Rc<Method> under `new` in
    /// `class_stack.last().singleton_methods`. AST only emits
    /// this op when the receiver is literally `self` (no
    /// class_stack push happens for `class << X` body — without
    /// the SelfExpr guard, non-self receivers would silently
    /// alias on the wrong table).
    ///
    /// At toplevel (`class << self` outside any class), AST
    /// still emits this op — `self` is `main`. The runtime
    /// handler then falls back to `toplevel_methods` because
    /// `class_stack.last()` is None. That landing is correct
    /// for the common case: toplevel `def foo` installs in
    /// `toplevel_methods`, so aliasing there keeps both names
    /// in the same table. (Not a strict CRuby match — CRuby
    /// installs on main's eigenclass — but observably equivalent
    /// for the toplevel call shapes that actually appear.)
    AliasSingletonMethod(SymId, SymId), // new, old
    /// Pop a Module/Class value and push it onto
    /// `class_stack.last()`'s `singleton_prepends` chain.
    /// Mirrors the instance `prepend` recogniser in dispatch.rs
    /// but targets the singleton chain. Emitted only by the
    /// AST translation of `class << self; prepend Mod; end`.
    /// CRuby semantics: the prepended module's instance methods
    /// take precedence over the class's own singleton methods.
    /// Lookup story implemented in
    /// `lookup_class_singleton_method` — walks
    /// `singleton_prepends` (transitive, with cycle defensiveness)
    /// before the class's own `singleton_methods` at each
    /// superclass level.
    SingletonChainPrepend,
    /// Push a new `Visibility::Public` entry onto
    /// `class_visibility_stack`. Emitted by the AST translator
    /// at the start of EVERY `class << <expr>` body (receiver-
    /// independent — `class << self`, `class << obj`,
    /// `class << Const` all wrap their body with Push/Pop) so
    /// bare `private` / `public` / `protected` modifiers inside
    /// the singleton body don't leak their visibility mutation
    /// back into the enclosing class body's stack entry. CRuby's
    /// `class << <expr>` constitutes its own body with its own
    /// initial-Public visibility scope; this opcode replicates
    /// that isolation by giving the singleton body its own stack
    /// frame to mutate.
    ///
    /// Push/Pop are emitted in an UNWIND-SAFE shape: the
    /// translator wraps the singleton body in
    /// `Expr::Begin { ensure: [PopClassVisibility] }`, so the
    /// Pop runs on BOTH normal exit AND exception unwind. A
    /// `raise` inside the body (or rescued by an outer
    /// `begin`) still triggers the ensure clause, keeping
    /// `class_visibility_stack` balanced. PR #233 code-
    /// review #1 / #3.
    PushClassVisibilityPublic,
    /// Pop one entry from `class_visibility_stack`. Paired with
    /// `PushClassVisibilityPublic` via the body's ensure
    /// clause (see Push docs for unwind details). Underflow is
    /// a translator-level invariant breakage and triggers
    /// unconditional `assert!` in the handler — fires in both
    /// debug and release builds so an unbalanced Pop surfaces
    /// during CI's `cargo test --release` (PR #233 round 3 #2).
    PopClassVisibility,
    /// `define_method(:name) { |args| ... }`. Pops a `Value::Block`
    /// off the operand stack, wraps its BlockHandle's captured
    /// locals into a Method, and installs it under `name` in the
    /// surrounding class (or toplevel). The Method shares the same
    /// `Rc<RefCell<Vec<Value>>>` as the BlockHandle, so closures
    /// over outer-scope locals stay live. Bumps `method_gen`.
    DefMethodBlock(SymId),         // name (block on stack)
    /// `recv.define_singleton_method(:name) { ... }` — closure-
    /// method install on the receiver's eigenclass. Pops the
    /// block (top of stack) then the receiver. Same closure-
    /// over-captured-locals semantics as `Op::DefMethodBlock`,
    /// but installs into `recv`'s singleton class rather than
    /// the surrounding class_stack target. Raises `TypeError` if
    /// the receiver isn't a `Value::Object` (consistent with
    /// `Op::DefObjectSingletonMethod`'s restriction).
    DefObjectSingletonMethodBlock(SymId),
    /// Args: bare name SymId, proto index, fully-qualified name
    /// SymId. The third arg holds the lexical-path-prefixed name
    /// (`"Foo::Bar"`) used to stamp `Class.name` on first
    /// construction — so `Class#name` / `#to_s` / `#inspect`
    /// report the qualified form CRuby does. `SymId(u32::MAX)`
    /// is the "no path" sentinel: top-level `class Foo`
    /// classes use the bare SymId for their name field
    /// (already-equal to the first arg's resolution).
    DefClass(SymId, u32, SymId),
    /// `module X; body; end`. Same shape as `Op::DefClass` —
    /// builds the surrounding Class shell, pushes its body
    /// frame, runs the body, leaves the Class on the stack —
    /// but flips `Class.is_module = true` on first creation
    /// so dispatch arms can distinguish Module-vs-Class
    /// (e.g. `Module#is_a?(Class)` returns false; `class_of`
    /// reports "Module"). On re-open of the same name, the
    /// existing Class wins regardless of which keyword was
    /// used — Ruby's `module Foo; end` then `class Foo; end`
    /// raises TypeError, but rubyrs leniently keeps the
    /// first-defined kind. Documented divergence.
    DefModule(SymId, u32, SymId),
    /// `class << <expr>; body; end` — REAL eigenclass-body
    /// execution (as opposed to the AST-level desugar that
    /// `tr_singleton_class` applies to the def/attr/alias-only
    /// fast cases). Pops the receiver value pushed immediately
    /// before this op, materializes its eigenclass (the
    /// `singleton_view` shell for a Class/Module — whose
    /// `singleton_target` redirects installs into the real
    /// class's `singleton_methods`; the lazily-allocated
    /// per-instance eigenclass for an Object), then pushes that
    /// eigenclass onto `class_stack` and opens a class body
    /// frame with `self = the eigenclass`. The body therefore
    /// runs with `self` being the metaclass, so `def`, `include`,
    /// `private`/`public`, `attr_*`, and `internal def`-style
    /// runtime indirection all consistently target the metaclass
    /// (= the real class's singleton tables). Emitted by
    /// `tr_singleton_class` only for bodies that the desugar
    /// cannot express faithfully (`include`, nested `module`,
    /// `internal def` / `private def` keyword-wrapped defs). The
    /// def/attr/alias-only fast cases still desugar. `u32` is the
    /// body proto index. The body's frame carries
    /// `is_class_body: true`, so the existing class-body return
    /// arm pops `class_stack` / visibility / module_function and
    /// pushes the eigenclass as the construct's value.
    OpenSingletonClass(u32),
    /// Call a Kernel GLOBAL builtin (`require`/`puts`/...) DIRECTLY
    /// via `builtin_call`, bypassing `do_call` and therefore any user
    /// override of that name. Args come from a popped Array (the
    /// `f(*args)` shape, like `ApplyCallNoRecv(_, u16::MAX)`). Emitted
    /// only by `synth_kernel_forwarder` for an alias of a Kernel
    /// builtin: CRuby's `alias_method :orig_require, :require`
    /// captures the ORIGINAL implementation, so calling the alias must
    /// reach the builtin even after `require` is redefined — and must
    /// NOT re-enter the override (which would recurse, since the
    /// override calls the alias).
    CallBuiltinDirect(SymId),
    // u32 operands: a literal array/hash can legitimately exceed 65535
    // elements — mail's Ragel-generated address parser builds a ~230k
    // element `_indicies` table as one array literal. A u16 count
    // wrapped at 2^16, corrupting the stack.
    NewArray(u32),
    NewHash(u32),
    /// Pops two values (begin, end). u8 nonzero = exclusive (`...`).
    NewRange(u8),
    /// proto_idx, param_start, n_params, rest_slot, kw_rest_slot.
    /// `rest_slot == u16::MAX` is the sentinel for "no rest";
    /// any other value is the local-slot index where `*args`
    /// gathers overflow into a fresh Array at invoke time.
    /// `kw_rest_slot == u16::MAX` likewise means "no `**opts`";
    /// otherwise the slot invoke_block binds the trailing kwargs
    /// Hash (default `{}`) into.
    CreateBlock(u32, u16, u16, u16, u16),
    /// Identical operands to `CreateBlock`, but the resulting Proc is
    /// flagged as a LAMBDA (`Proc#lambda?` → true). Emitted only for a
    /// `->(){}` literal; `lambda { }` flips the bit on its received
    /// block at dispatch time instead.
    CreateLambda(u32, u16, u16, u16, u16),
    CallBlock(SymId, u8, u16),
    CallNoRecvBlock(SymId, u8, u16),
    Yield(u8),
    /// `yield(*arr)` — like `Yield` but the args come from a popped
    /// Array (dynamic argc), the yield analogue of `Op::ApplyCall`.
    /// Stack on entry: `[..., args_array]`.
    ApplyYield,
    BinOp(BinOpKind),
    /// Fast path for `recv <op> <int_literal>` — fuses the preceding
    /// `LoadConstInt` into the BinOp. Saves one op and one stack
    /// round-trip per such expression. Falls back to generic dispatch
    /// when LHS isn't an `Int`.
    BinOpInt(BinOpKind, i64),
    /// Superinstruction for `<local> <op> <local>` — fuses
    /// `LoadLocal(a); LoadLocal(b); BinOp(kind)` into a single op
    /// reading both operands directly from the frame's locals (no
    /// two-op stack round-trip). The args are the LHS and RHS local
    /// slot indices. Semantically identical to the unfused sequence:
    /// the handler mirrors `Op::BinOp`'s Int×Int fast path, bigint /
    /// rational promotions, primitive dispatch, and the fall-to-
    /// `do_call` cold path for user-defined operators. Targets the
    /// `i < n` loop-condition shape and two-local arithmetic.
    BinOpLocalLocal(BinOpKind, u16, u16),
    /// Args: handler-offset, bind-slot, bind-flag, filter-class
    /// SymId. The filter SymId is resolved to a class at push-time
    /// by looking it up in `Vm.classes`. Bare `rescue` (no class
    /// listed) is compiled with the SymId of `StandardError`, so
    /// the lookup always succeeds for any well-formed program; an
    /// unresolved class (e.g. `rescue UndefinedConst`) makes the
    /// handler match nothing — see `unwind_with_exception`.
    PushRescue(i32, u16, u8, SymId),
    /// `rescue *exp` where the splat operand is a LOCAL variable
    /// (minitest's `assert_raises *exp` shape — the class list is
    /// the method's own args array, not a constant). Args:
    /// handler-offset, bind-slot, bind-flag, source-local-slot.
    /// At push time the source slot is read from the frame and its
    /// Array elements become the filter list (`RescueFilter::Any`);
    /// a single Class coerces to a one-element match and anything
    /// else (incl. an unset Nil slot) matches nothing. Re-executed
    /// on every begin entry, so retry re-reads the local — same
    /// re-evaluation timing as CRuby.
    PushRescueSplatLocal(i32, u16, u8, u16),
    PopRescue,
    /// Begin/rescue baseline marker. Pushes the current
    /// `frame.rescues.len()` onto `frame.begin_rescue_depths`
    /// at the start of a `begin / rescue` block — captured
    /// BEFORE the `PushRescue` ops, so retry's truncation
    /// restores the depth to "no rescue handlers from this
    /// begin block". Paired with `Op::ExitBegin` at the end
    /// of the begin/rescue arm. (Code-review #306 round 1
    /// — closes the stale-handler accumulation bug surfaced
    /// by multi-class rescue + retry shapes.)
    EnterBegin,
    /// Begin/rescue baseline marker pop — drops the top of
    /// `frame.begin_rescue_depths`. Emitted at the end of the
    /// normal-success path AND at the end of each rescue
    /// clause body (before its jump-to-end), so a sibling
    /// or outer begin block doesn't accidentally see this
    /// block's baseline. (Code-review #306 round 1.)
    ExitBegin,
    /// Retry path: truncate `frame.rescues` back to the depth
    /// recorded at the top of `frame.begin_rescue_depths` so
    /// any partially-unwound handlers from a multi-class
    /// rescue clause (where one filter matched but its
    /// siblings remained on the stack) get cleaned up before
    /// the retry's `PushRescue` ops re-register fresh
    /// entries. Followed by an `Op::Jump` back to begin_top.
    /// (Code-review #306 round 1.)
    TruncateRescuesToBeginBaseline,
    /// Like PushRescue but for `ensure` clauses. When an exception is
    /// unwinding and hits a PushEnsure handler, the exception value is
    /// pushed onto the operand stack and control jumps to the handler;
    /// the handler runs the ensure body (which must leave the stack
    /// unchanged) and ends with `Op::EndEnsure` to either rethrow the
    /// exception or resume an in-flight `break`/`next` loop transfer
    /// (see `Op::EndEnsure` for the two paths).
    PushEnsure(i32),
    PopEnsure,
    Raise,
    /// Terminator emitted at the tail of every `ensure` handler
    /// body. Two paths:
    ///   - Normal exception-unwind path: an exception value sits
    ///     on top of the operand stack (pushed by the unwinder
    ///     when it jumped to this handler). Pop it and re-raise
    ///     so the unwind continues to the next handler / frame.
    ///   - Loop-transfer path: `vm.pending_loop_transfer` is
    ///     `Some` because `BreakLoop`/`NextLoop` started a
    ///     `break`/`next` walk through this ensure. The stack
    ///     was NOT pushed-to on entry; we resume the transfer
    ///     by walking the remaining rescues, running any further
    ///     `is_ensure` handlers, and eventually landing at the
    ///     loop's target IP.
    ///
    /// Replaces the prior `Op::Raise` the compiler used to emit
    /// at the same position. User-level `raise` keyword still
    /// emits `Op::Raise` and never reaches this op.
    EndEnsure,
    /// Signals the current iteration driver (Array#each, #map, etc.) to
    /// stop and use the value on top of the operand stack as the call's
    /// return value. Almost always emitted as `<val>; Break; Return` so
    /// the block frame also pops.
    Break,
    /// Push the current `rescues.len()` onto the frame's
    /// `loop_rescue_depths` stack — emitted at the start of a `while`
    /// expression so a `break` inside the loop body knows how many
    /// `PushRescue`/`PushEnsure` handlers it needs to discard before
    /// jumping out. Paired with `Op::ExitLoop` on the structured exit
    /// paths (normal cond-false exit AND `break`). Non-local control
    /// transfers can bypass the matching `ExitLoop`:
    ///   - Exception unwind: `unwind_with_exception` truncates
    ///     `loop_rescue_depths` to the matched handler's
    ///     `loop_depth_at_push` snapshot, so any `EnterLoop` entries
    ///     pushed by loops the exception is escaping out of are
    ///     discarded along with the handler entries above them.
    ///   - Non-local `return` (`Op::ReturnMethod`) pops the frame
    ///     entirely, taking `loop_rescue_depths` with it.
    ///
    /// Without those two compensating paths the entry would stay
    /// installed on the frame and a later `BreakLoop` would read it
    /// as the innermost loop.
    EnterLoop,
    /// Pop the most-recent entry off `loop_rescue_depths`. Emitted at
    /// the join point past a `while` expression's body. Not reached
    /// on the exception-unwind path; see `Op::EnterLoop` for how the
    /// entry is reclaimed in that case.
    ExitLoop,
    /// `break` from a `while` loop. Pops dynamic rescue/ensure handlers
    /// down to the depth recorded by the matching `Op::EnterLoop`, then
    /// jumps by `i32` offset (same encoding as `Op::Jump`). The break
    /// value (or `nil`) is already on the operand stack and stays for
    /// the post-loop expression value. Distinct from `Op::Break` (which
    /// signals an iteration driver / block return, NOT a structured
    /// `while`-loop exit).
    BreakLoop(i32),
    /// `next` from a `while` loop — skip the rest of this iteration
    /// and re-evaluate the condition. Same handler-pop logic as
    /// `Op::BreakLoop`; jumps to the per-loop "iter_check" label
    /// (the condition expression's position) so the loop either
    /// continues with the next iteration or falls through to the
    /// natural exit. Distinct from `Op::Return` (the pre-PR fallback
    /// `next` used to emit, which returned from the enclosing
    /// method/block).
    NextLoop(i32),
    Return,
    /// Explicit `return val` — non-local. Unlike `Op::Return`
    /// which pops a single frame, this signals the dispatch
    /// loop to unwind through block frames until it reaches the
    /// enclosing method frame, then pop that too. Implemented
    /// via `Vm.method_return` — the op only sets the signal;
    /// the unwind itself happens in `dispatch` / `dispatch_until`
    /// at the top of their next loop iteration.
    ReturnMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "preamble-cache", derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum BinOpKind { Add, Sub, Mul, Div, Mod, Lt, Le, Gt, Ge, Eq, Ne }

impl BinOpKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            BinOpKind::Add => "+", BinOpKind::Sub => "-", BinOpKind::Mul => "*",
            BinOpKind::Div => "/", BinOpKind::Mod => "%",
            BinOpKind::Lt => "<", BinOpKind::Le => "<=",
            BinOpKind::Gt => ">", BinOpKind::Ge => ">=",
            BinOpKind::Eq => "==", BinOpKind::Ne => "!=",
        }
    }
    pub(crate) fn from_op_name(s: &str) -> Option<Self> {
        Some(match s {
            "+" => BinOpKind::Add, "-" => BinOpKind::Sub, "*" => BinOpKind::Mul,
            "/" => BinOpKind::Div, "%" => BinOpKind::Mod,
            "<" => BinOpKind::Lt, "<=" => BinOpKind::Le,
            ">" => BinOpKind::Gt, ">=" => BinOpKind::Ge,
            "==" => BinOpKind::Eq, "!=" => BinOpKind::Ne,
            _ => return None,
        })
    }
    /// Applies the op against two i64 operands. Returns `Some(v)`
    /// for the in-range result; returns `None` when the caller
    /// must promote to BigInt. With `bignum` on, `None` fires for:
    /// (a) Add/Sub/Mul overflow (via `checked_*`), and
    /// (b) `Div` on `i64::MIN / -1` (result is 2^63, doesn't fit
    /// i64). With `bignum` off, Add/Sub/Mul fall back to
    /// `wrapping_*` and the Div overflow case wraps to `i64::MIN`
    /// per the existing wrapping-on-overflow convention — both
    /// paths always return `Some(...)`. Div/Mod implement CRuby's
    /// floor-division semantics via `floor_div_i64` /
    /// `floor_mod_i64` (sign of remainder matches divisor);
    /// `% -1` is always 0 so Mod can't overflow. Comparison arms
    /// cannot overflow.
    pub(crate) fn apply_int(self, a: i64, b: i64) -> Option<Value> {
        #[cfg(feature = "bignum")]
        let arith = |a: i64, b: i64, op: fn(i64, i64) -> Option<i64>| op(a, b);
        #[cfg(not(feature = "bignum"))]
        let arith = |a: i64, b: i64, op: fn(i64, i64) -> i64| Some(op(a, b));
        #[cfg(feature = "bignum")]
        let (add, sub, mul): (fn(i64, i64) -> Option<i64>, _, _) =
            (i64::checked_add, i64::checked_sub, i64::checked_mul);
        #[cfg(not(feature = "bignum"))]
        let (add, sub, mul): (fn(i64, i64) -> i64, _, _) =
            (i64::wrapping_add, i64::wrapping_sub, i64::wrapping_mul);
        Some(match self {
            BinOpKind::Add => Value::Int(arith(a, b, add)?),
            BinOpKind::Sub => Value::Int(arith(a, b, sub)?),
            BinOpKind::Mul => Value::Int(arith(a, b, mul)?),
            // CRuby uses floor division for Integer#/ and #%: the
            // remainder's sign matches the divisor's sign, so
            // `(-13) / 4 == -4` (Rust's wrapping_div gives -3) and
            // `(-13) % 4 == 3` (Rust's wrapping_rem gives -1).
            // Delegated to the helpers re-exported through `vm`
            // (`crate::vm::floor_div_i64` / `crate::vm::floor_mod_i64`)
            // so the method-call path (`5.send(:/, 2)`) and this
            // BinOp fast path stay in lock-step. Definitions live
            // in vm/numeric.rs, but that module is private — the
            // re-exports in vm.rs are what we can name from here.
            //
            // `i64::MIN / -1` is the one overflow case: the result
            // `2^63` doesn't fit i64. Bignum builds return None
            // here so the caller's `bigint_arith` fallback promotes
            // to BigInt (matching CRuby parity). No-bignum builds
            // wrap to `i64::MIN` per the existing wrapping-on-
            // overflow convention (the same one `+`/`-`/`*` use
            // via `wrapping_*` under no-bignum). `% -1` is
            // always 0 — no overflow.
            #[cfg(feature = "bignum")]
            BinOpKind::Div => {
                if a == i64::MIN && b == -1 { return None; }
                Value::Int(crate::vm::floor_div_i64(a, b))
            }
            #[cfg(not(feature = "bignum"))]
            BinOpKind::Div => Value::Int(crate::vm::floor_div_i64(a, b)),
            BinOpKind::Mod => Value::Int(crate::vm::floor_mod_i64(a, b)),
            BinOpKind::Lt => Value::Bool(a < b),
            BinOpKind::Le => Value::Bool(a <= b),
            BinOpKind::Gt => Value::Bool(a > b),
            BinOpKind::Ge => Value::Bool(a >= b),
            BinOpKind::Eq => Value::Bool(a == b),
            BinOpKind::Ne => Value::Bool(a != b),
        })
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "preamble-cache", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct Proto {
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    /// body's entry prologue (`JumpIfArgGiven` then the default
    /// expression then `StoreLocal`, one triple per optional slot,
    /// emitted by the compiler). Required params always come
    /// before optionals in source order. Defaults can be arbitrary
    /// expressions (`def f(a, b=a+1)`, `def f(level=Logger::INFO)`)
    /// because the prologue runs after positional slots are
    /// bound — there's no compile-time-literal restriction.
    pub(crate) n_required_positional: u16,
    /// M27 A4: required positional params that come AFTER the rest
    /// splat (`def mid(a, *b, c, d)` → 2). Only non-zero when
    /// `rest_param` is `Some` (CRuby grammar requires rest before
    /// any post-required). At call time the binder peels the last
    /// `n_required_post` args off and gives them to the trailing
    /// slots BEFORE the rest binding gathers the middle. Without
    /// this, `def mid(a, *b, c); mid(1,2,3,4,5)` bound a=1,
    /// b=[2,3,4,5], c=nil instead of CRuby's a=1, b=[2,3,4], c=5.
    pub(crate) n_required_post: u16,
    /// `Some(name)` for `def foo(*args)` — the rest-parameter
    /// name. At call time, args past the last positional slot
    /// gather into a fresh Array stored in the local named here.
    /// `None` means no rest param.
    pub(crate) rest_param: Option<String>,
    /// Keyword params live at the tail of `params` — these are
    /// the parallel defaults. Length matches the number of
    /// keyword params. `None` = required keyword (raises
    /// ArgumentError on miss); `Some(v)` = optional with literal
    /// default. NOTE: a kwarg with a non-literal (computed)
    /// default has `None` here AND `true` in
    /// `kw_has_computed_default` — the binder leaves the slot
    /// Nil and the prologue evaluates the default expression.
    /// `None + computed=false` is the only shape that surfaces
    /// the missing-keyword ArgumentError.
    pub(crate) kw_param_defaults: Vec<Option<Value>>,
    /// Parallel to `kw_param_defaults`: `true` at index `i`
    /// means kwarg `i` has a computed (non-literal) default
    /// emitted in the method-body prologue (via
    /// `Op::JumpIfKwArgGiven(i, _)`). Binder uses this to
    /// distinguish "leave nil for prologue" (computed) from
    /// "raise missing-keyword" (required) — see kw bind loop
    /// in `vm/dispatch.rs`. Empty when the method has no
    /// kwargs OR every kwarg default is a literal.
    pub(crate) kw_has_computed_default: Vec<bool>,
    /// `Some(name)` for `def foo(**opts)` — the keyword-rest
    /// parameter name. Leftover keyword args (those whose key
    /// isn't bound by a named entry in `kw_param_defaults`)
    /// collect into a fresh Hash stored in the local named
    /// here. `None` means no kw-rest; unrecognised kwarg keys
    /// raise ArgumentError. Lives at the very end of `params`
    /// (after every kw_param) so the existing kw-binding loop
    /// remains contiguous.
    pub(crate) kw_rest_param: Option<String>,
    /// `Some(name)` for `def foo(&blk)` — the block-as-data
    /// parameter name. At call time, the BlockHandle the caller
    /// passed (the same ObjId held by `frame.block_arg`) is bound
    /// into the local named here as a `Value::Block`; if the
    /// caller passed no block the slot gets `Value::Nil`. Lives
    /// at the very end of `params` (after kw_rest if any) so the
    /// rest/kw layout stays contiguous.
    pub(crate) block_param: Option<String>,
    /// BLOCK protos only: `(name, absolute_slot, required)` per
    /// `|k1:, k2: default|` keyword param, in declaration order.
    /// `invoke_block` binds these by ABSOLUTE slot index (block
    /// locals share the captured frame, so by-position math over
    /// `params` doesn't apply): caller-supplied value, or Nil on
    /// miss — required + missing raises ArgumentError; optional's
    /// default is a body-prologue desugar (`k = d if k.nil?`,
    /// see ast.rs). Empty for every non-block proto AND for
    /// kw-less blocks — the hot invoke_block1/2 paths gate on
    /// `is_empty()`. Kw names are deliberately NOT in `params`,
    /// so the define_method-as-method binder's positional math
    /// is unchanged (kw-blocks installed as methods keep their
    /// pre-existing arity behaviour — documented gap).
    pub(crate) block_kw_params: Vec<(String, u16, bool)>,
    /// BLOCK protos only: the ABSOLUTE local slot of a `|.., &b|`
    /// block parameter. `invoke_block` binds the caller's block
    /// (`proc.call(args, &blk)` → `Vm::pending_block_arg`) or Nil
    /// into it EVERY invocation (the slot sits below
    /// `block_body_local_start`, so a skipped write would leak the
    /// previous invocation's block). `None` for non-block protos
    /// and `&`-less blocks — the invoke_block1/2 fast paths gate
    /// on it. Distinct from `block_param` (the NAME, used by the
    /// define_method-as-method binder's by-position math).
    pub(crate) block_param_slot: Option<u16>,
    pub(crate) n_locals: u16,
    /// Slot → source name for each local variable (length `n_locals`;
    /// `""` for synthetic/unnamed slots). Retained so `Kernel#binding`
    /// can snapshot the live frame's named locals by name, and
    /// `eval(src, binding)` can seed them back as same-slot params so
    /// the eval'd source resolves them. (Empty on protos compiled
    /// before this was populated is impossible — `build` always fills
    /// it.) Discovery: rack's ShowExceptions/ShowStatus ERB templates
    /// reference the calling method's locals via `result(binding)`.
    pub(crate) local_names: Vec<String>,
    /// `true` when the source file (or eval string) carried a
    /// `# frozen_string_literal: true` magic comment. Plain string
    /// literals (`Op::LoadConstStr` / `LoadConstStrBytes`) executed in
    /// this proto are then pushed FROZEN. Set on every proto compiled
    /// from a file with the comment (the parse entries stamp the whole
    /// proto range). `false` by default. Interpolated strings stay
    /// mutable (CRuby semantics). Discovery: rack's spec_builder
    /// frozen.ru rackup asserts `'frozen'.frozen?`.
    pub(crate) frozen_string_literal: bool,
    /// `true` when `code` contains an `Op::CreateBlock` — i.e. running
    /// this proto can capture the frame's locals cell into a
    /// `BlockHandle` (block literal, `proc`/`lambda`/`->`,
    /// `define_method`'s body, …). Method-call sites consult this for
    /// the `Locals::Stack` escape analysis: a method proto with
    /// `creates_block == false` can never leak its locals, so its
    /// slots may live in the contiguous `Vm::locals_arena` instead of
    /// an `Rc<RefCell<Vec>>` cell. Set by `finish_proto` after the
    /// body is emitted; conservatively `true` would only cost
    /// performance, never correctness.
    pub(crate) creates_block: bool,
    pub(crate) code: Vec<Op>,
    /// Parallel to `code`: op_spans[i] is the source span where code[i] was emitted.
    pub(crate) op_spans: Vec<Span>,
    /// Source filename — used by Trap backtrace formatting.
    pub(crate) filename: Rc<str>,
    /// Start index of body-introduced local slots for block
    /// protos — `invoke_block` resets slots
    /// `[block_body_local_start, n_locals)` to `Value::Nil` on
    /// every invocation so a variable first-assigned inside the
    /// block doesn't leak its value across iterations / calls.
    /// CRuby semantics: each `do ... end` / `proc.call` /
    /// `lambda.call` invocation gets fresh block-locals; outer
    /// scope variables stay shared (their slot index is below
    /// this threshold because compile_block snapshots
    /// `parent.n_locals` first). `u16::MAX` is the "no reset"
    /// sentinel — set for every non-block proto (methods, class
    /// bodies, toplevel `<main>`).
    pub(crate) block_body_local_start: u16,
    /// How many of this block proto's positional params are OPTIONAL
    /// (`|a, b = 1|`). Counted in `n_params`/`n_required_positional`
    /// like requireds (they take real slots), but tracked separately so
    /// `Proc#arity` can report `-(required + 1)` when optionals (or a
    /// rest) are present. 0 for methods and option-less blocks. Stamped
    /// post-`build()` by compile_block alongside `block_body_local_start`.
    pub(crate) n_optional_params: u16,
    /// Per-proto pool of binary string literals — bytes from
    /// `\xNN` escapes that aren't valid UTF-8. Indexed by
    /// `Op::LoadConstStrBytes(u32)`. The global interner (which
    /// keys on `Rc<str>`) can't hold non-UTF-8 bytes, so binary
    /// literals get their own per-Proto store; deduplication
    /// within a single Proto isn't attempted (binary literals
    /// are usually small and rare). Valid-UTF-8 literals still
    /// go through the interner via `Op::LoadConstStr(SymId)`.
    pub(crate) byte_literals: Vec<std::rc::Rc<[u8]>>,
    /// Per-call-site cref chains for `Op::LoadConstChain` /
    /// `Op::LoadConstChainOrNil`. Each entry is the ordered list
    /// of qualified SymIds the runtime should try in turn when
    /// resolving a bare constant read inside a non-empty class /
    /// module scope. For scope `[Foo, Bar]` and bare `X` the chain
    /// is `[sym("Foo::Bar::X"), sym("Foo::X"), sym("X")]`. First
    /// hit in `Vm.classes` / `Vm.constants` wins; running off the
    /// chain raises NameError (or returns Nil for the OrNil variant).
    /// Top-level reads (empty class_path at emit time) keep using
    /// `Op::LoadConst(SymId)` directly — no chain needed.
    pub(crate) const_chains: Vec<Vec<crate::intern::SymId>>,
    /// Lexical class/module nesting at the point this proto was
    /// compiled, expressed as qualified-name SymIds in
    /// **innermost-first** order. For a proto compiled inside
    /// `module A; module B; class C; ...; end; end; end` the value
    /// is `[sym("A::B::C"), sym("A::B"), sym("A")]`. Top-level
    /// protos and `<main>` get an empty vec.
    ///
    /// Read by `Module.nesting` reflection: at call time we resolve
    /// each SymId through `Vm.classes` and return the resulting
    /// Array. Class bodies, method bodies, and blocks all inherit
    /// the surrounding scope through the compiler's `class_path`,
    /// so `Module.nesting` inside a block defined in a method body
    /// inside a class body still reports the full chain.
    pub(crate) lexical_scope: Vec<crate::intern::SymId>,
}
