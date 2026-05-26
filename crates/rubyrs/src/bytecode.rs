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
    #[cfg(feature = "regex")]
    LoadRegex(SymId),
    /// Pop a Value::Str, compile it as a Regex pattern, push
    /// Value::Regex. Emitted by `Expr::InterpolatedRegex` after
    /// the same `to_s + +` build sequence used by InterpolatedStr.
    /// Pattern reuse hits the same `regex_cache` keyed by SymId
    /// of the assembled pattern. Compile errors surface as
    /// SyntaxError traps at runtime (same shape as `LoadRegex`,
    /// since the pattern is unknown until interpolation runs).
    #[cfg(feature = "regex")]
    CompileRegex,
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
    LoadIvar(SymId),
    StoreIvar(SymId),
    /// Fast path for `@name = @name + 1`. Same shape as IncLocal but on
    /// self's ivar table.
    IncIvar(SymId),
    /// Same as `IncIvar` but does *not* push the resulting value.
    IncIvarNoPush(SymId),
    LoadConst(SymId),
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
    /// Args: name SymId, argc, per-call-site inline-cache slot id.
    Call(SymId, u8, u16),
    CallNoRecv(SymId, u8, u16),
    /// `foo(*args)` — single-splat call. Pops the args Array
    /// (which must be `Value::Array`) and uses its elements as
    /// the positional args. Argc is dynamic. Receiver above
    /// the array on stack for `ApplyCall`; absent for the
    /// `NoRecv` variant. Used by the compiler when call args
    /// contain a SplatNode at the only position.
    ApplyCall(SymId, u16),
    ApplyCallNoRecv(SymId, u16),
    /// `super(args...)`. Receiver stays `self` (popped from the
    /// current frame, not the operand stack). Method name and
    /// argc are baked in at compile time. Lookup starts at the
    /// SUPERCLASS of `self.class`, so the current method is
    /// skipped — letting overrides delegate "up" the chain.
    /// IC slot isn't used (super resolves via class chain, not
    /// the per-site cache).
    Super(SymId, u8),
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
    NewArray(u16),
    NewHash(u16),
    /// Pops two values (begin, end). u8 nonzero = exclusive (`...`).
    NewRange(u8),
    /// proto_idx, param_start, n_params, rest_slot.
    /// `rest_slot == u16::MAX` is the sentinel for "no rest";
    /// any other value is the local-slot index where `*args`
    /// gathers overflow into a fresh Array at invoke time.
    CreateBlock(u32, u16, u16, u16),
    CallBlock(SymId, u8, u16),
    CallNoRecvBlock(SymId, u8, u16),
    Yield(u8),
    BinOp(BinOpKind),
    /// Fast path for `recv <op> <int_literal>` — fuses the preceding
    /// `LoadConstInt` into the BinOp. Saves one op and one stack
    /// round-trip per such expression. Falls back to generic dispatch
    /// when LHS isn't an `Int`.
    BinOpInt(BinOpKind, i64),
    /// Args: handler-offset, bind-slot, bind-flag, filter-class
    /// SymId. The filter SymId is resolved to a class at push-time
    /// by looking it up in `Vm.classes`. Bare `rescue` (no class
    /// listed) is compiled with the SymId of `StandardError`, so
    /// the lookup always succeeds for any well-formed program; an
    /// unresolved class (e.g. `rescue UndefinedConst`) makes the
    /// handler match nothing — see `unwind_with_exception`.
    PushRescue(i32, u16, u8, SymId),
    PopRescue,
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
    pub(crate) fn apply_int(self, a: i64, b: i64) -> Value {
        match self {
            BinOpKind::Add => Value::Int(a.wrapping_add(b)),
            BinOpKind::Sub => Value::Int(a.wrapping_sub(b)),
            BinOpKind::Mul => Value::Int(a.wrapping_mul(b)),
            BinOpKind::Div => Value::Int(a.wrapping_div(b)),
            BinOpKind::Mod => Value::Int(a.wrapping_rem(b)),
            BinOpKind::Lt => Value::Bool(a < b),
            BinOpKind::Le => Value::Bool(a <= b),
            BinOpKind::Gt => Value::Bool(a > b),
            BinOpKind::Ge => Value::Bool(a >= b),
            BinOpKind::Eq => Value::Bool(a == b),
            BinOpKind::Ne => Value::Bool(a != b),
        }
    }
}

#[derive(Debug)]
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
    /// `Some(name)` for `def foo(*args)` — the rest-parameter
    /// name. At call time, args past the last positional slot
    /// gather into a fresh Array stored in the local named here.
    /// `None` means no rest param.
    pub(crate) rest_param: Option<String>,
    /// Keyword params live at the tail of `params` — these are
    /// the parallel defaults. Length matches the number of
    /// keyword params. `None` = required keyword (raises
    /// ArgumentError on miss); `Some(v)` = optional with literal
    /// default.
    pub(crate) kw_param_defaults: Vec<Option<Value>>,
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
    pub(crate) n_locals: u16,
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
    /// Per-proto pool of binary string literals — bytes from
    /// `\xNN` escapes that aren't valid UTF-8. Indexed by
    /// `Op::LoadConstStrBytes(u32)`. The global interner (which
    /// keys on `Rc<str>`) can't hold non-UTF-8 bytes, so binary
    /// literals get their own per-Proto store; deduplication
    /// within a single Proto isn't attempted (binary literals
    /// are usually small and rare). Valid-UTF-8 literals still
    /// go through the interner via `Op::LoadConstStr(SymId)`.
    pub(crate) byte_literals: Vec<std::rc::Rc<[u8]>>,
}
