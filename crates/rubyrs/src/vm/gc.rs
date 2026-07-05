//! Resource-cap enforcement + GC trigger + the Vm runtime
//! entry point. Mirrors what CRuby splits between `gc.c`
//! (allocation/cap interaction), `thread.c` (deadline/fuel),
//! and `vm.c` (the rb_vm_exec entry).
//!
//! Contents:
//!   - `Vm::run` — push the entry frame and call dispatch.
//!   - `Vm::check_fuel` / `check_alloc` / `check_frames` — the
//!     three resource caps (P1-D), checked on the hot paths in
//!     `dispatch_until`, `maybe_gc`, and `do_call`.
//!   - `Vm::trap` — build a `Trap` with the current frame stack
//!     as backtrace.
//!   - `Vm::maybe_gc` — heap-pressure / stress-GC trigger.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{RubyError, Span, Trap, TrapFrame};
use crate::value::Value;

/// Discriminator for `check_path_in_allowlist` — which trap class
/// to raise on a scope violation. Filesystem ops want `IOError`
/// (script-side `rescue IOError` catches FS failures), load ops
/// (require/require_relative/cext_require) want `LoadError`
/// (`rescue LoadError` catches "feature unavailable").
// `#[allow(dead_code)]` on `Load` — the variant exists for the
// `require`/`require_relative`/`cext_require` callers gated behind
// `cfg(feature = "stdlib")` (require family) / `cfg(feature =
// "cext")` (cext_require). Non-default-features builds compile the
// enum but never construct `Load`. Variant kept distinct (not
// merged into `Io`) so the LoadError mapping stays explicit when
// the require family is built in.
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum PathTrapKind {
    Io,
    Load,
}

use super::{vec_nil, Frame, Vm};

impl Vm {
    /// The top-level `main` object — `self` at a script's top level
    /// (and in a required file / bare `eval`). CRuby makes this a
    /// singleton `Object`; rubyrs materialises it LAZILY once `Object`
    /// exists. The preamble runs before `Object` is defined, so it
    /// keeps `self = nil` (returns `Value::Nil` here); every later
    /// top-level frame shares the one main, so `self.extend Module`
    /// accumulates across evals. Rooted by the GC mark phase via
    /// `main_obj`, so it survives the frame-clear between evals.
    pub(crate) fn main_object(&mut self) -> Value {
        if let Some(id) = self.main_obj {
            return Value::Object(id);
        }
        let object_sym = self.interner.intern("Object");
        let Some(cls) = self.classes.get(&object_sym).cloned() else {
            return Value::Nil;
        };
        let id = self
            .heap
            .alloc(crate::heap::HeapObj::Instance(crate::value::Instance::pristine(cls)));
        self.main_obj = Some(id);
        Value::Object(id)
    }

    /// True when `v` is the top-level `main` object. The no-receiver
    /// dispatch historically treated `Value::Nil` as "top-level main
    /// self" (toplevel_methods take precedence over Kernel there);
    /// now that `main` is a real Object, the same gates must also
    /// recognise it. Preamble code (before `main` exists) still runs
    /// with `self = nil`, so those gates check BOTH.
    #[inline]
    pub(crate) fn is_main_self(&self, v: &Value) -> bool {
        matches!(v, Value::Object(id) if self.main_obj == Some(*id))
    }

    pub(crate) fn run(&mut self, entry: usize) -> Result<Value, Trap> {
        let proto = &self.protos[entry];
        let n_locals = proto.n_locals as usize;
        let main_self = self.main_object();
        self.frames.push(Frame {
            proto_idx: entry,
            ip: 0,
            locals: crate::vm::Locals::Shared(Rc::new(RefCell::new(vec_nil(n_locals)))),
            self_val: main_self,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None, lexical_cvar_class: None, #[cfg(feature = "regex")] saved_last_match: None, is_block: false, is_lambda: false, n_given_positional: 0, kw_given_mask: 0, aux: None, pending_yield: false,
            block_writeback: None,
            dm_share: false,
            own_start: 0,
            outer_cell_start: 0,
            outer_cell: None,
            outer_rest: None,
            captured_yield_block: None,
        });
        self.dispatch()?;
        Ok(self.stack.pop().unwrap_or(Value::Nil))
    }

    /// Decrement fuel; on exhaustion return a `ResourceExhausted` trap.
    #[inline]
    pub(crate) fn check_fuel(&mut self) -> Result<(), Trap> {
        if let Some(f) = self.fuel {
            if f == 0 {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: "out of fuel".to_string(),
                }));
            }
            self.fuel = Some(f - 1);
        }
        // Wall-clock deadline: piggyback on `check_fuel` since both
        // fire on every op. `Instant::now()` is a syscall on most
        // platforms, so we only call it every 1024 ops; this keeps
        // the no-deadline case to a single conditional + an i32
        // increment per op. The op_counter is intentionally `u32`
        // (wraps freely) — we never read its absolute value.
        self.op_counter = self.op_counter.wrapping_add(1);
        if self.op_counter & 1023 == 0
            && let Some(at) = self.deadline_at
                && std::time::Instant::now() >= at {
                    return Err(self.trap(RubyError::ResourceExhausted {
                        msg: "wall-clock deadline exceeded".to_string(),
                    }));
                }
        Ok(())
    }

    /// Check the heap can accept another object. Call after `maybe_gc`
    /// (so the limit applies to *live* objects, not transient garbage).
    #[inline]
    pub(crate) fn check_alloc(&self) -> Result<(), Trap> {
        if let Some(max) = self.heap.max_live
            && self.heap.live_count >= max {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("heap exhausted: {} live objects (max {})", self.heap.live_count, max),
                }));
            }
        Ok(())
    }

    /// Check the frame stack can accept another frame.
    ///
    /// Two layers, in priority order:
    ///
    /// 1. **Always-on CRuby-parity cap** (`DEFAULT_MAX_CALL_DEPTH`).
    ///    Trips with `SystemStackError` (catchable via `rescue
    ///    SystemStackError` or `rescue Exception` — same placement
    ///    as CRuby, outside the StandardError subtree). Without
    ///    this, infinite recursion (e.g. the alias_method-into-
    ///    feedback-loop shape sinatra-contrib/WebDAV's `safe?`
    ///    redefine creates on double-`register`) allocates frames
    ///    unboundedly until the OS OOM-kills the host — observed
    ///    at >90 GB of resident memory before the kill in one
    ///    Ghostty session. Catching it as `SystemStackError` instead
    ///    matches CRuby's contract that any recursion that goes
    ///    too deep raises a normal, rescue-able Ruby exception.
    ///
    /// 2. **Always-on Rust-stack-safety cap**
    ///    (`DEFAULT_MAX_DISPATCH_DEPTH`). Re-entrant Rust calls into
    ///    `dispatch_until` (driven by `then` / `tap` / `yield_self` /
    ///    `yield` / native iter drivers) consume Rust stack linearly
    ///    in Ruby recursion depth. The Ruby-frame cap above doesn't
    ///    protect against this: a script like
    ///    `def f(x); x.then { |y| f(y) }; end; f(1)` blows the host's
    ///    Rust stack at ~5–6k recursion levels — well below the
    ///    10k Ruby-frame cap — and aborts the process. Mirror the
    ///    Ruby-frame cap shape: trip with `SystemStackError`,
    ///    catchable, with the same "stack level too deep" message.
    ///
    /// 3. **Embedder-configurable cap** (`max_frames`, default
    ///    `None`). Trips with `ResourceExhausted` — intentionally
    ///    `< Exception` not `< StandardError` so untrusted scripts
    ///    cannot swallow their own fuel/heap/frame trap with a
    ///    bare `rescue`. Embedders set this lower than
    ///    `DEFAULT_MAX_CALL_DEPTH` when sandboxing untrusted code.
    #[inline]
    pub(crate) fn check_frames(&self) -> Result<(), Trap> {
        // CRuby's default is ~10000 frames before SystemStackError;
        // we mirror it exactly. Embedders that want a different
        // ceiling for trusted code can lift this constant; for
        // untrusted code they should configure `max_frames` to a
        // smaller value (which trips the ResourceExhausted branch
        // below before this one fires).
        const DEFAULT_MAX_CALL_DEPTH: usize = 10_000;
        if self.frames.len() >= DEFAULT_MAX_CALL_DEPTH {
            return Err(self.trap(RubyError::SystemStackError {
                msg: "stack level too deep".to_string(),
            }));
        }
        // Each re-entrant `dispatch_until` push (`then`, `tap`,
        // `yield_self`, `yield`, `Proc#call`, native iter drivers)
        // costs ~10 KB of Rust stack at release optimisation.
        // Debug + llvm-cov instrumented builds inflate the
        // per-level Rust stack 2-3× (no inlining + the cov
        // counters add a few KB per frame), so the same script
        // that costs 1.5 MB of stack at release blows past 3-5
        // MB under debug+cov.
        //
        // Empirical bisection (cargo's 2 MB test-thread stack):
        //   - release: ~250 pushes overflows; cap at 150 holds
        //   - debug+cov (CI Coverage job): ~80 pushes overflows
        //
        // Use cfg(debug_assertions) to pick a tighter cap for
        // debug builds. The release cap of 150 stays generous
        // on 8 MB main-thread setups (still 75%+ headroom).
        // 150 nested block-recursion levels is far beyond normal
        // app code — this trap is a safety net for runaway
        // recursion, not a working-program limit. Trips before
        // the 10k Ruby-frame cap on block-recursion shapes that
        // the frame cap can't catch. Embedders that need tighter
        // (sandboxed) bounds can configure `max_dispatch_depth`
        // (which trips first, with ResourceExhausted instead of
        // SystemStackError).
        // Debug floor raised 5 → 8 (2026-07): the coop scheduler's
        // inline-park fallback (preamble/thread.rb __coop_wait_inline —
        // a green thread parking beneath a native iterator frame drives
        // the scheduler in place instead of Fiber-yielding) legitimately
        // sits at ~6 levels for `[..].each { sleep 0 }` inside a green
        // thread: main + fiber resume + iter step_block + the sleep
        // builtin-arm's override re-entry + the poll machinery. 8 keeps
        // a 10× margin under the ~80-push debug+cov overflow point
        // (and the Coverage job additionally runs RUST_MIN_STACK=16M).
        #[cfg(debug_assertions)]
        const DEFAULT_MAX_DISPATCH_DEPTH: usize = 8;
        #[cfg(not(debug_assertions))]
        const DEFAULT_MAX_DISPATCH_DEPTH: usize = 150;
        if self.dispatch_until_depths.len() >= DEFAULT_MAX_DISPATCH_DEPTH {
            return Err(self.trap(RubyError::SystemStackError {
                msg: "stack level too deep".to_string(),
            }));
        }
        if let Some(max) = self.max_frames
            && self.frames.len() >= max {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("stack level too deep ({} frames, max {})", self.frames.len(), max),
                }));
            }
        if let Some(max) = self.max_dispatch_depth
            && self.dispatch_until_depths.len() >= max
        {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!(
                    "dispatch recursion too deep ({} levels, max {})",
                    self.dispatch_until_depths.len(),
                    max,
                ),
            }));
        }
        // P1e.2 (ADR 0023 v2): when inside a Fiber, enforce
        // the per-Fiber frame cap too. Without this a Fiber
        // could deepen its frame stack to OOM while staying
        // under `max_frames` (which counts the resumer's
        // frames — stashed during resume) and `max_live`
        // (Frames aren't separate heap objects).
        #[cfg(feature = "_fiber")]
        if self.current_fiber_id.is_some()
            && let Some(max) = self.max_fiber_frame_depth
            && self.frames.len() >= max
        {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!(
                    "fiber stack level too deep ({} frames, fiber max {})",
                    self.frames.len(),
                    max,
                ),
            }));
        }
        Ok(())
    }

    /// Gate every script-callable filesystem-touching site
    /// (excluding `require`-class loaders — see
    /// `check_load_allowed` for those). Two-layer check:
    ///
    /// 1. **Capability** (`Config::allow_filesystem_io`). If
    ///    `false`, traps unconditionally with `IOError` — sandbox
    ///    is shut.
    /// 2. **Scope** (`Config::allowed_paths`). When the cap is
    ///    `Some(prefixes)` AND the caller passes
    ///    `path: Some(target)`, the target is lexically resolved
    ///    (joined with cwd if relative, `..`/`.` collapsed) and
    ///    must start with one of `prefixes`; otherwise traps
    ///    with `IOError`. `path: None` skips the scope check —
    ///    used by call sites that gate before path resolution
    ///    is available.
    ///
    /// `op` is the user-visible operation name for the trap
    /// message ("File.read", "File.size", ...).
    pub(crate) fn check_filesystem_io_allowed(
        &self,
        op: &str,
        path: Option<&std::path::Path>,
    ) -> Result<(), Trap> {
        if !self.allow_filesystem_io {
            return Err(self.trap(RubyError::IOError {
                msg: format!(
                    "{op} blocked: filesystem I/O disabled by Config::allow_filesystem_io"
                ),
            }));
        }
        if let Some(p) = path {
            self.check_path_in_allowlist(op, p, PathTrapKind::Io)?;
        }
        Ok(())
    }

    /// Gate `require` / `require_relative` / `cext_require`.
    /// Same two-layer check as `check_filesystem_io_allowed`,
    /// but the scope-violation trap class is `LoadError` so
    /// `rescue LoadError` catches it like CRuby's
    /// require-failure exception.
    #[allow(dead_code)]
    pub(crate) fn check_load_allowed(
        &self,
        op: &str,
        path: Option<&std::path::Path>,
    ) -> Result<(), Trap> {
        if !self.allow_filesystem_io {
            return Err(self.trap(RubyError::LoadError {
                msg: format!(
                    "{op} blocked: filesystem I/O disabled by Config::allow_filesystem_io"
                ),
            }));
        }
        if let Some(p) = path {
            self.check_path_in_allowlist(op, p, PathTrapKind::Load)?;
        }
        Ok(())
    }

    /// Internal: check a target path against `vm.allowed_paths`.
    /// No-op when `allowed_paths: None` (no narrowing configured).
    /// When `Some(prefixes)`, lexically resolves the target
    /// (joining with cwd if relative, collapsing `..`/`.`) and
    /// requires `starts_with` against at least one prefix.
    ///
    /// Path matching is prefix-only on lexical form. Symlinks
    /// that point outside the allowed prefixes are NOT defended
    /// at this layer — the contract is documented in
    /// `Config::allowed_paths`.
    fn check_path_in_allowlist(
        &self,
        op: &str,
        path: &std::path::Path,
        kind: PathTrapKind,
    ) -> Result<(), Trap> {
        let Some(prefixes) = self.allowed_paths.as_ref() else {
            return Ok(());
        };
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative path — join with cwd. The host already
            // enabled `allow_filesystem_io: true`, so the cwd
            // syscall is consistent with that capability. If
            // cwd lookup fails (sandboxed container, no perms),
            // fall back to the bare relative path — the
            // `starts_with` will then fail against any absolute
            // prefix, trapping safely.
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(path),
                Err(_) => path.to_path_buf(),
            }
        };
        let resolved = crate::lexically_resolve_path(&joined);
        if prefixes.iter().any(|prefix| resolved.starts_with(prefix)) {
            return Ok(());
        }
        // Trap message embeds the ORIGINAL script-supplied input
        // (`path`), not the cwd-joined `resolved`. If a script
        // catches the IOError/LoadError, it learns only what it
        // already typed — not the host process's cwd, which would
        // otherwise leak via `e.message` to script code that has
        // no other way to read cwd (`Dir.pwd` isn't implemented,
        // chdir doesn't exist).
        let msg = format!(
            "{op} blocked: path {:?} outside Config::allowed_paths",
            path,
        );
        Err(self.trap(match kind {
            PathTrapKind::Io => RubyError::IOError { msg },
            PathTrapKind::Load => RubyError::LoadError { msg },
        }))
    }

    /// Coerce a Float argument to i64 with CRuby's
    /// `each_slice(2.5) → 2` truncation semantics. NaN and ±Inf
    /// raise RangeError with CRuby's exact wording (note: the
    /// short label "Inf" / "-Inf" / "NaN" — NOT
    /// `float_domain_label`'s "Infinity" / "NaN" used elsewhere).
    /// Finite-out-of-range floats (e.g. `1e30`) silently
    /// saturate via the `as i64` cast; CRuby raises there too
    /// with `"float <%g> out of range of integer"` but exact
    /// %g-style formatting isn't worth the parity cost for a
    /// pathological input.
    pub(crate) fn float_to_int_arg(&self, f: f64) -> Result<i64, Trap> {
        if f.is_nan() {
            return Err(self.trap(RubyError::RangeError {
                msg: "float NaN out of range of integer".to_string(),
            }));
        }
        if f.is_infinite() {
            let label = if f > 0.0 { "Inf" } else { "-Inf" };
            return Err(self.trap(RubyError::RangeError {
                msg: format!("float {label} out of range of integer"),
            }));
        }
        Ok(f as i64)
    }

    /// Variant of `arity_error_arg1_int` for methods that take
    /// 0 or 1 Integer argument (`Array#first`, `#last`, `#pop`,
    /// `#shift`). Same TypeError shape for non-Int 1-arg, but
    /// the wrong-arity wording is "expected 0..1" (CRuby uses
    /// the range form for these). Place AFTER the `[]` and
    /// `[Value::Int(n)]` (and Float/BigInt) arms so this only
    /// fires for genuinely-wrong shapes.
    pub(crate) fn arity_error_arg0_or_1_int(&self, _name: &str, args: &[Value]) -> Trap {
        if args.len() > 1 {
            return self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len()
                ),
            });
        }
        // args.len() == 1, but the value wasn't matched by
        // Int/Float/BigInt arms — coerce-into-Integer TypeError.
        match args.first() {
            Some(Value::Nil) => self.trap(RubyError::TypeError {
                msg: "no implicit conversion from nil to integer".to_string(),
            }),
            Some(other) => {
                let name = match other {
                    Value::Block(_) | Value::CurriedProc(_) => "Proc",
                    Value::BoundMethod(_) => "Method",
                    _ => super::numeric::type_name_for_coerce(other),
                };
                self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {name} into Integer"),
                })
            }
            // Unreachable: callers route through this helper
            // only when the `[]` arm has already matched empty
            // args (and the surrounding `[Value::Int(_)]` /
            // `[Value::Float(_)]` / `[Value::BigInt(_)]` arms
            // for the 1-arg path). If a future refactor swaps
            // the arm order, we'd land here with a non-erroring
            // 0-arg call — the `"wrong number of arguments"`
            // wording would be actively misleading (0..1 accepts
            // 0). Use an explicit internal-error trap so the
            // misroute is obvious during debugging.
            None => {
                debug_assert!(
                    false,
                    "arity_error_arg0_or_1_int reached with empty args; the `[]` arm should have matched first"
                );
                self.trap(RubyError::RuntimeError {
                    msg: "internal: arity_error_arg0_or_1_int reached with 0 args".to_string(),
                })
            }
        }
    }

    /// Build a Trap matching CRuby's wrong-arity-and-type
    /// surface for "method takes exactly one Integer argument"
    /// dispatch arms (`each_slice(n)` / `each_cons(n)`):
    ///   - args.len() != 1     → ArgumentError "wrong number of arguments (given N, expected 1)"
    ///   - args[0] is Nil      → TypeError "no implicit conversion from nil to integer"
    ///   - args[0] is non-Int  → TypeError "no implicit conversion of X into Integer"
    ///
    /// Used as `return Err(self.arity_error_arg1_int(name, args))`
    /// in catch-all arms placed after the matching `[Value::Int(n)]`
    /// arm so the success path is unchanged.
    pub(crate) fn arity_error_arg1_int(&self, _name: &str, args: &[Value]) -> Trap {
        if args.len() != 1 {
            return self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 1)",
                    args.len()
                ),
            });
        }
        match &args[0] {
            // Nil uses CRuby's distinct "from nil to integer"
            // wording rather than "of nil into Integer".
            Value::Nil => self.trap(RubyError::TypeError {
                msg: "no implicit conversion from nil to integer".to_string(),
            }),
            other => {
                // CRuby's coercion error uses class names for
                // closures and bound methods (`Proc`, `Method`)
                // but the lowercase `true`/`false` tokens for
                // Bool. `type_name_for_coerce` gives us the
                // Bool wording but falls back to `"Object"`
                // for Block / CurriedProc / BoundMethod —
                // override those inline so Proc args render
                // as `Proc` (CRuby parity).
                let name = match other {
                    Value::Block(_) | Value::CurriedProc(_) => "Proc",
                    Value::BoundMethod(_) => "Method",
                    _ => super::numeric::type_name_for_coerce(other),
                };
                self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {name} into Integer"),
                })
            },
        }
    }

    /// Build a Trap with a backtrace snapshot taken at the current frame stack.
    pub(crate) fn trap(&self, err: RubyError) -> Trap {
        let mut bt = Vec::with_capacity(self.frames.len());
        for f in self.frames.iter().rev() {
            let proto = &self.protos[f.proto_idx];
            let op_ip = if f.ip == 0 { 0 } else { f.ip - 1 };
            let span = proto.op_spans.get(op_ip).copied().unwrap_or(Span::ZERO);
            bt.push(TrapFrame {
                filename: proto.filename.clone(),
                method: Rc::from(proto.name.as_str()),
                span,
            });
        }
        Trap { err, backtrace: bt }
    }

    pub(crate) fn maybe_gc(&mut self) {
        if !self.stress_gc && !self.heap.should_gc() { return; }
        // `RUBYRS_GC_DISABLE=1` short-circuits the actual sweep — for
        // perf experiments only (heap grows unbounded until the
        // process exits). Acts as a profiling probe: if a workload's
        // wall time drops sharply with GC disabled, the workload is
        // GC-bound and worth tuning; if it stays flat, GC isn't the
        // bottleneck and optimisation effort goes elsewhere.
        //
        // Validated on the json_bench round_trip workload (5000
        // parse+generate iters discarding ~150 short-lived objects
        // each): baseline 44 µs/iter, with disable 32 µs/iter — so
        // ~27 % of round_trip wall is GC. The next_gc growth
        // factor (heap.rs's `live * 2 max 4096`; see the threshold
        // comment there for the growth-factor history) determines
        // sweep frequency; bumping it cuts sweep count linearly.
        // The env knob below stays in for ongoing perf-regression
        // investigations (mirrors `RUBYRS_IC_STATS` shape).
        if std::env::var_os("RUBYRS_GC_DISABLE").is_some() {
            // Bump next_gc out of reach so we don't even re-enter
            // the gather/walk machinery on the next allocation.
            self.heap.next_gc = usize::MAX;
            return;
        }
        self.gc_now();
    }

    /// The collection itself — root gather + `Heap::collect` —
    /// WITHOUT `maybe_gc`'s threshold / stress / disable gating.
    /// Two callers: `maybe_gc` (the normal allocation-pressure
    /// path) and `Runtime`'s post-preamble-snapshot capture, which
    /// forces one FULL collection (via `Heap::schedule_major`) so
    /// the captured baseline contains no still-uncollected
    /// preamble garbage. Without that, a post-snapshot sweep frees
    /// pre-snapshot slots, user allocs recycle them BELOW the
    /// high-water mark, and `reset()`'s truncate-based rewind
    /// can't reach them (the zombie-Array dangle the nightly-fuzz
    /// reset loop surfaced).
    pub(crate) fn gc_now(&mut self) {
        // Collection start time — BEFORE the root gather, which walks
        // every class table below and dominates a loaded program's
        // per-collection fixed cost. `Heap::collect`'s adaptive-floor
        // controller sizes the next trigger window from this.
        let t0 = std::time::Instant::now();
        // Gather roots: stack + every frame's locals + self_val + swap_return
        // + pinned (native-code accumulators). class_stack holds Rc<Class>
        // which isn't GC-managed, so we don't need to walk it.
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + self.pinned.len() + 64);
        // Per-cycle dedup of locals-CELL content scans. Frames,
        // capture-routing chains and `define_method` closures share
        // cells heavily (rubocop-ast installs ~40 `*_type?` closures
        // over ONE class-body cell); without dedup each sharer
        // re-pushes the whole cell's contents every cycle — the
        // gather cost blows up O(sharers × cell-size). Ptr-keyed
        // dedup is sound within a single gather: no Ruby code runs
        // mid-GC, so cell contents can't change between visits.
        let mut seen_cells: crate::intern::FxHashSet<usize> = crate::intern::FxHashSet::default();
        #[inline]
        fn push_cell(
            roots: &mut Vec<Value>,
            seen: &mut crate::intern::FxHashSet<usize>,
            cell: &Rc<RefCell<Vec<Value>>>,
        ) {
            if seen.insert(Rc::as_ptr(cell) as usize) {
                for v in cell.borrow().iter() {
                    roots.push(v.clone());
                }
            }
        }
        for v in &self.stack { roots.push(v.clone()); }
        for v in &self.pinned { roots.push(v.clone()); }
        // In-flight break/next transfers: a break value lives only
        // in `pending_loop_transfers` between `begin_loop_transfer`
        // and the final landing. The ensure bodies run in between
        // and can trigger GC at allocation sites; without rooting
        // here a heap-allocated break value (Array/Hash/String/
        // Object) gets swept and the eventual stack.push in
        // `continue_loop_transfer` would re-publish a dangling
        // handle — silent heap corruption (ICE on the next op
        // that consults the slot's type). Reproduced under
        // STRESS_GC=1.
        for t in &self.pending_loop_transfers {
            if let super::LoopTransferKind::Break { value } = &t.kind {
                roots.push(value.clone());
            }
        }
        // Same lifetime for a non-local return / block-break value:
        // it lives only in `pending_method_breaks` while the walk
        // runs the intervening ensure bodies (which are arbitrary
        // user code and can allocate). `def m; return [1,2]; ensure;
        // Array.new(9); end` under STRESS_GC swept the return value
        // before this rooting existed.
        for mb in &self.pending_method_breaks {
            roots.push(mb.value.clone());
        }
        // ENV hash, once initialised, is reachable from script
        // code via the `ENV` constant — pin it so the cache
        // doesn't get swept between LoadConst loads.
        if let Some(id) = self.env_hash { roots.push(Value::Hash(id)); }
        // The pooled JIT Hash-accumulate scratch (ADR 0034) survives between calls.
        #[cfg(feature = "jit-native")]
        if let Some(id) = self.jit_hash_scratch { roots.push(Value::Hash(id)); }
        // `at_exit { }` / `END { }` handlers are GC-heap Block objects
        // held only by their ObjId in `at_exit_handlers` until the
        // runtime drains them at program exit. Without rooting, a GC
        // between registration and exit sweeps the Block bodies and
        // their ObjIds get reused by later allocations — every handler
        // then aliases the last-allocated block (observed under
        // STRESS_GC: five `at_exit` blocks all ran the fifth one's body).
        for id in &self.at_exit_handlers { roots.push(Value::Block(*id)); }
        // Marshal round-trip registry — dumped objects must survive
        // until (a possible) Marshal.load names them again.
        for v in &self.marshal_registry { roots.push(v.clone()); }
        // Top-level constants (`FOO = expr`) are reachable from any
        // future LoadConst — root every value so Array/Hash/Object
        // constants don't get swept between assignment and read.
        for v in self.constants.values() { roots.push(v.clone()); }
        // Global variables (`$foo = []`) hold arbitrary Values
        // (including heap-backed Array/Hash/String/Object). Without
        // rooting, any global pointing at a heap object can be swept
        // between assignment and read.
        for v in self.globals.values() { roots.push(v.clone()); }
        // Class-level instance variables (`@foo` on a Class
        // value, e.g. `module Tilt; @default = ...; end`).
        // Without rooting, a class ivar holding a heap-backed
        // Array/Hash/String/Object could be swept between
        // assignment and read from inside a class method.
        for cls in self.classes.values() {
            for v in cls.ivars.borrow().values() { roots.push(v.clone()); }
            // Root the class AS A VALUE too: the mark-phase
            // `visit_value(Value::Class)` arm walks tables the
            // direct ivar push can't see — in particular the
            // SUPERCLASS chain, where an anonymous generated
            // parent (`class MimePart < Struct.new(...)`) keeps
            // its own ivar table (`@__struct_attrs`) that no
            // other root reaches (rack multipart UAF).
            roots.push(Value::Class(cls.clone()));
        }
        // Anonymous classes named only by assignment (`S = Struct.new`),
        // stashed in a `$global`, or held inside a container (an
        // Array/Hash constant, an ivar, a local) are NOT in
        // `self.classes`, so the loop above misses their class-level
        // ivars. They no longer need a hand-rolled root here: rooting the
        // constant/global/container Value is enough because
        // `visit_value`'s `Value::Class` arm descends into class ivars
        // when the mark phase reaches the class through ANY rooted Value
        // (replacing the earlier constant/global-only special case;
        // found via /code-review — the special case missed sibling
        // container paths and the Struct `@__struct_attrs` UAF).
        // The whole live arena prefix is a root — covers every
        // `Locals::Stack` frame's slots in one pass, INCLUDING values
        // parked there mid call-setup before their frame is pushed.
        for v in &self.locals_arena { roots.push(v.clone()); }
        // The top-level `main` object persists across evals (so
        // `self.extend` accumulates) even when the frame stack — which
        // would otherwise root it via `self_val` — is cleared between
        // evals. Root it directly.
        if let Some(id) = self.main_obj {
            roots.push(Value::Object(id));
        }
        for f in &self.frames {
            roots.push(f.self_val.clone());
            // Stack frames' slots were covered by the arena walk above.
            if let Some(rc) = f.locals.as_shared() {
                push_cell(&mut roots, &mut seen_cells, rc);
            }
            // Capture-routing cells: an ORIGINAL binding cell (e.g. a
            // popped scope's locals kept alive only by this escaped
            // closure's routing) may be reachable through nothing else.
            // Routed reads/writes dereference these, so their contents
            // must survive collection.
            if let Some(cell) = &f.outer_cell {
                push_cell(&mut roots, &mut seen_cells, cell);
            }
            if let Some(chain) = &f.outer_rest {
                for (cell, _) in chain.iter() {
                    push_cell(&mut roots, &mut seen_cells, cell);
                }
            }
            if let Some(v) = &f.swap_return { roots.push(v.clone()); }
            if let Some(id) = f.block_arg {
                // Block lives in the GC heap now (P2-13). Pushing
                // the Value::Block root is enough — the mark phase
                // walks the BlockHandle's `captured` and `self_val`
                // when it reaches the slot.
                roots.push(Value::Block(id));
            }
            // A block frame running an escaped closure forwards
            // `yield` to a block whose defining method has returned;
            // root it while the frame executes (the heap Block mark
            // also walks it, but a directly-called closure result may
            // not be otherwise held).
            if let Some(id) = f.captured_yield_block {
                roots.push(Value::Block(id));
            }
            // `BeginBaseline.saved_dollar_bang` snapshots the
            // dynamically-scoped `$!` at Op::EnterBegin; once an
            // inner `raise` REPLACES the global, the snapshot can be
            // the ONLY reference to the previous exception object —
            // Op::ExitBegin then restores a swept ObjId. Repro
            // (STRESS_GC): nested begin/rescue with an unbound outer
            // rescue + alloc churn inside the inner rescue →
            // `$!.message` ICEs with "class_of called on non-Object
            // slot" (begin_dollar_bang_snapshot_gc fixture).
            if let Some(aux) = &f.aux {
                for b in &aux.begin_rescue_depths {
                    roots.push(b.saved_dollar_bang.clone());
                }
            }
        }
        // During a fiber resume, the SUSPENDED main program's state
        // (its frames, operand stack, pinned set) lives on
        // `fiber_stash_stack` (FiberStashGuard::install) — without
        // walking it, any GC inside a fiber body sweeps every heap
        // object reachable only from the suspended side (the
        // `fiber_current_is_nil...` class_of ICE). Mirrors the
        // heap-side `HeapObj::Fiber` mark arm's snapshot walk.
        // `proc.call(.., &blk)`'s one-shot block channel — set
        // before invoke_block's frame push, so allocs in that
        // window (rest-array, kwrest) must not sweep the block.
        if let Some(bid) = self.pending_block_arg {
            roots.push(crate::value::Value::Block(bid));
        }
        // String per-instance eigenclasses: their methods capture
        // closures over GC objects, and nothing else roots the
        // side-table's classes.
        for (_, sc) in self.str_singletons.values() {
            roots.push(crate::value::Value::Class(sc.clone()));
        }
        // Instance variables set on String values — the side-table is
        // the only thing rooting these values.
        for (_, ivars) in self.str_ivars.values() {
            for v in ivars.values() {
                roots.push(v.clone());
            }
        }
        // Array/Proc per-instance eigenclasses: root both the
        // eigenclass (its methods capture closures) AND the keyed
        // object itself — the side-table holds the Value precisely so
        // the id can't be swept and reused under the stale key.
        for (obj, sc) in self.heap_singletons.values() {
            roots.push(obj.clone());
            roots.push(crate::value::Value::Class(sc.clone()));
        }
        // Kernel#binding local snapshots: the captured Values are
        // reachable only from here until the Binding is eval'd.
        for snap in self.binding_locals.values() {
            for (_, v) in snap {
                roots.push(v.clone());
            }
        }
        if let Some(v) = &self.last_uncaught_exception {
            roots.push(v.clone());
        }
        #[cfg(feature = "_fiber")]
        for snap in &self.fiber_stash_stack {
            // The suspended side's Stack-frame slots live in ITS
            // swapped-out arena.
            for v in &snap.locals_arena { roots.push(v.clone()); }
            for f in &snap.frames {
                roots.push(f.self_val.clone());
                if let Some(rc) = f.locals.as_shared() {
                    push_cell(&mut roots, &mut seen_cells, rc);
                }
                // Capture-routing cells — same reasoning as the live-
                // frame walk above (a suspended fiber's block frame
                // may hold the only path to an original binding cell).
                if let Some(cell) = &f.outer_cell {
                    push_cell(&mut roots, &mut seen_cells, cell);
                }
                if let Some(chain) = &f.outer_rest {
                    for (cell, _) in chain.iter() {
                        push_cell(&mut roots, &mut seen_cells, cell);
                    }
                }
                if let Some(v) = &f.swap_return { roots.push(v.clone()); }
                if let Some(id) = f.block_arg { roots.push(Value::Block(id)); }
                if let Some(id) = f.captured_yield_block { roots.push(Value::Block(id)); }
                if let Some(aux) = &f.aux {
                    for b in &aux.begin_rescue_depths {
                        roots.push(b.saved_dollar_bang.clone());
                    }
                }
            }
            for v in &snap.stack { roots.push(v.clone()); }
            for v in &snap.pinned { roots.push(v.clone()); }
            if let Some(v) = &snap.method_return { roots.push(v.clone()); }
            for t in &snap.pending_loop_transfers {
                if let crate::vm::LoopTransferKind::Break { value } = &t.kind {
                    roots.push(value.clone());
                }
            }
            for mb in &snap.pending_method_breaks {
                roots.push(mb.value.clone());
            }
        }
        // `define_method`-installed methods carry captured-locals
        // Rcs that aren't reachable from any Frame once the lexical
        // scope has popped. Walk every class's method table (plus
        // the toplevel table) and root the captured slots so heap
        // values held only via a closure survive GC.
        //
        // Cost is O(total installed methods). For programs that
        // never use `define_method`, the inner `if let Some` short-
        // circuits — this is a single field-check per Method. ADR-
        // worthy optimisation if we ever care: track a counter of
        // closure-methods on the Vm and skip this entirely when 0.
        for cls in self.classes.values() {
            for m in cls.methods.borrow().values() {
                if let Some(cl) = &m.closure {
                    cl.each_capture_cell(|cell| push_cell(&mut roots, &mut seen_cells, cell));
                    if let Some(b) = cl.captured_yield_block { roots.push(Value::Block(b)); }
                }
            }
            // `singleton_class.class_eval { define_method(...) }`
            // installs closure methods into `singleton_methods`
            // via the eigenclass-shell redirect (PR #253). Without
            // walking this table, captures reachable only through
            // a singleton method's `MethodClosure` would be swept
            // under STRESS_GC. (Code-review #253 round 5.)
            for m in cls.singleton_methods.borrow().values() {
                if let Some(cl) = &m.closure {
                    cl.each_capture_cell(|cell| push_cell(&mut roots, &mut seen_cells, cell));
                    if let Some(b) = cl.captured_yield_block { roots.push(Value::Block(b)); }
                }
            }
            // The eigenclass shell itself isn't in `self.classes`
            // (only the real class is); walk the cached shell's
            // own tables too so any method installed on the
            // shell's own singleton-methods (a meta-meta case
            // from `def self.foo` inside
            // `singleton_class.class_eval`) keeps its captures
            // and class-vars alive.
            if let Some(shell) = cls.singleton_view.borrow().as_ref() {
                for m in shell.methods.borrow().values() {
                    if let Some(cl) = &m.closure {
                        cl.each_capture_cell(|cell| push_cell(&mut roots, &mut seen_cells, cell));
                    if let Some(b) = cl.captured_yield_block { roots.push(Value::Block(b)); }
                    }
                }
                for m in shell.singleton_methods.borrow().values() {
                    if let Some(cl) = &m.closure {
                        cl.each_capture_cell(|cell| push_cell(&mut roots, &mut seen_cells, cell));
                    if let Some(b) = cl.captured_yield_block { roots.push(Value::Block(b)); }
                    }
                }
                for v in shell.ivars.borrow().values() { roots.push(v.clone()); }
                for v in shell.class_vars.borrow().values() { roots.push(v.clone()); }
            }
            // Class variables (`@@foo`) hold arbitrary Values
            // (Array/Hash/Object); without rooting them, a
            // `@@items = []; ...; @@items << x` pattern under
            // STRESS_GC=1 sweeps the array between the write
            // and the next iteration's read.
            for v in cls.class_vars.borrow().values() {
                roots.push(v.clone());
            }
            // CLASS-LEVEL instance variables (`@foo` inside
            // `class << self` / `def self.x`) hold heap Values
            // too — minitest's Runnable keeps its registry as
            // `@runnables = []` and Spec::DSL keeps `@children`,
            // both class ivars holding Arrays of live state. The
            // `visit_value(Value::Class)` arm walks ivars when a
            // class is reached AS A VALUE, but this registry walk
            // iterates `self.classes` directly and never routed
            // the Rc through that arm — so a class ivar holding
            // the ONLY reference to a heap value was swept under
            // STRESS_GC (minitest's at_exit then read the freed
            // Array: ICE use-after-free).
            for v in cls.ivars.borrow().values() {
                roots.push(v.clone());
            }
        }
        // Toplevel `@@foo` fallback (no class on hand).
        for v in self.toplevel_cvars.values() {
            roots.push(v.clone());
        }
        // `$LOAD_PATH` Array (lazily allocated; `None` until
        // first read). The Array's String elements are
        // Rc-backed; the mark walk through Value::Array picks
        // them up via the normal Array-children visit.
        if let Some(id) = self.load_path {
            roots.push(Value::Array(id));
        }
        // `$LOADED_FEATURES` Array (lazily allocated; twin of
        // `load_path`). String elements visited via the Array walk.
        if let Some(id) = self.loaded_features_list {
            roots.push(Value::Array(id));
        }
        for m in self.toplevel_methods.values() {
            if let Some(cl) = &m.closure {
                cl.each_capture_cell(|cell| push_cell(&mut roots, &mut seen_cells, cell));
                if let Some(b) = cl.captured_yield_block { roots.push(Value::Block(b)); }
            }
        }
        let pending_frees = self.heap.collect(&roots, t0);
        // Prune `binding_locals` entries whose Binding object was just
        // swept. The snapshot Values were roots (above), so they kept
        // the binding's *contents* alive — but the binding Instance
        // itself isn't rooted by this table, so it can die here. Drop
        // the stale entry NOW, before the freed slot is recycled by a
        // later `alloc` (which would alias the snapshot onto an
        // unrelated new object).
        if !self.binding_locals.is_empty() {
            let heap = &self.heap;
            self.binding_locals
                .retain(|&id, _| heap.is_live(crate::value::ObjId(id as u32)));
        }
        // Run TypedData dfree callbacks AFTER `collect` has
        // returned and the &mut Heap borrow is released (review #2
        // on PR #19). Conservative shape — even though
        // well-behaved cexts shouldn't re-enter the VM from dfree,
        // this avoids the aliasing footgun if one ever does.
        for (f, p) in pending_frees {
            // SAFETY: `f` and `p` originate from a TypedData slot
            // we just swept (the slot was unreachable from any GC
            // root, so the cext can't observe `p` again). The
            // cext's contract for `dfree` is to release ownership
            // of `p` — exactly what we want here.
            unsafe { f(p); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Proto;
    use crate::intern::Interner;

    fn mk_vm() -> Vm {
        Vm::new(Vec::<Proto>::new(), Interner::new())
    }

    #[test]
    fn check_fuel_passes_when_unlimited() {
        let mut vm = mk_vm();
        // Default fuel is None — unlimited.
        assert!(vm.check_fuel().is_ok());
        // op_counter increments even on the unlimited path.
        let before = vm.op_counter;
        assert!(vm.check_fuel().is_ok());
        assert_eq!(vm.op_counter, before.wrapping_add(1));
    }

    #[test]
    fn check_fuel_decrements_then_traps_at_zero() {
        let mut vm = mk_vm();
        vm.fuel = Some(2);
        assert!(vm.check_fuel().is_ok());
        assert_eq!(vm.fuel, Some(1));
        assert!(vm.check_fuel().is_ok());
        assert_eq!(vm.fuel, Some(0));
        let trap = vm.check_fuel().expect_err("third check_fuel should trap");
        assert!(matches!(trap.err, RubyError::ResourceExhausted { .. }));
        assert_eq!(trap.err.message(), "out of fuel");
    }

    #[test]
    fn check_alloc_passes_under_cap() {
        let mut vm = mk_vm();
        vm.heap.max_live = Some(10);
        assert!(vm.check_alloc().is_ok());
    }

    #[test]
    fn check_alloc_traps_at_cap() {
        let mut vm = mk_vm();
        vm.heap.max_live = Some(0);
        let trap = vm.check_alloc().expect_err("0-live cap should trap");
        assert!(matches!(trap.err, RubyError::ResourceExhausted { .. }));
        assert!(trap.err.message().contains("heap exhausted"));
    }

    #[test]
    fn check_alloc_unlimited_passes() {
        let vm = mk_vm();
        // Default max_live = None.
        assert!(vm.check_alloc().is_ok());
    }

    #[test]
    fn check_frames_passes_under_cap() {
        let mut vm = mk_vm();
        vm.max_frames = Some(10);
        assert!(vm.check_frames().is_ok());
    }

    #[test]
    fn check_frames_traps_at_cap() {
        let mut vm = mk_vm();
        vm.max_frames = Some(0);
        let trap = vm.check_frames().expect_err("0-frame cap should trap");
        assert!(matches!(trap.err, RubyError::ResourceExhausted { .. }));
        assert!(trap.err.message().contains("stack level too deep"));
    }

    #[test]
    fn check_frames_max_dispatch_depth_traps_when_exceeded() {
        let mut vm = mk_vm();
        // 0-cap traps on the first push (dispatch_until_depths is
        // empty until a dispatch_until is pushed; len() == 0 >= 0).
        vm.max_dispatch_depth = Some(0);
        let trap = vm
            .check_frames()
            .expect_err("0-dispatch cap should trap");
        assert!(matches!(trap.err, RubyError::ResourceExhausted { .. }));
        assert!(trap.err.message().contains("dispatch recursion too deep"));
    }

    #[test]
    fn check_frames_max_dispatch_depth_unlimited_passes() {
        let mut vm = mk_vm();
        // None (the default) leaves only the always-on 500 cap.
        // With an empty dispatch_until_depths stack, the always-on
        // SystemStackError check at 500 won't trip either.
        vm.max_dispatch_depth = None;
        assert!(vm.check_frames().is_ok());
    }

    #[test]
    fn trap_with_empty_frames_has_empty_backtrace() {
        let vm = mk_vm();
        let t = vm.trap(RubyError::RuntimeError { msg: "boom".into() });
        assert!(t.backtrace.is_empty());
        assert!(matches!(t.err, RubyError::RuntimeError { .. }));
    }

    #[test]
    fn maybe_gc_is_noop_when_not_due_and_not_stressed() {
        let mut vm = mk_vm();
        let before_live = vm.heap.live_count;
        vm.maybe_gc();
        assert_eq!(vm.heap.live_count, before_live);
    }

    #[test]
    fn maybe_gc_keeps_values_reachable_via_globals() {
        // Regression: `$g = []` followed by GC must not sweep the
        // array. Globals must be in the root set.
        let mut vm = mk_vm();
        vm.stress_gc = true;
        let arr_id = vm.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into()));
        let name_id = vm.interner.intern("$g");
        vm.globals.insert(name_id, Value::Array(arr_id));
        let before = vm.heap.live_count;
        vm.maybe_gc();
        assert_eq!(vm.heap.live_count, before, "global-rooted array was swept");
        assert!(vm.heap.array(arr_id).is_empty());
    }

    #[test]
    fn maybe_gc_runs_under_stress_with_no_roots() {
        let mut vm = mk_vm();
        vm.stress_gc = true;
        let before = vm.heap.live_count;
        vm.maybe_gc();
        // live_count can only stay or decrease — and with no
        // allocations and no roots, it stays at 0.
        assert!(vm.heap.live_count <= before);
    }

    #[test]
    fn maybe_gc_prunes_dead_binding_locals_and_roots_live_snapshots() {
        // `binding_locals` (Kernel#binding's local snapshots) must
        // (a) keep its captured Values alive across a sweep, and
        // (b) drop entries for Binding objects that have themselves
        // been swept — before the freed slot is recycled.
        let mut vm = mk_vm();
        vm.stress_gc = true;
        // A live Binding-shaped object, rooted via a global so the
        // sweep keeps it; its binding_locals entry must be RETAINED.
        let live = vm.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into()));
        let g = vm.interner.intern("$keep");
        vm.globals.insert(g, Value::Array(live));
        // A snapshot Value reachable ONLY through binding_locals — the
        // GC root loop must keep it alive.
        let snap_val = vm.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into()));
        vm.binding_locals
            .insert(live.0 as usize, vec![("x".to_string(), Value::Array(snap_val))]);
        // A dead Binding: never rooted, so the sweep frees its slot;
        // its binding_locals entry must be PRUNED.
        let dead = vm.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into()));
        let dead_key = dead.0 as usize;
        vm.binding_locals.insert(dead_key, vec![("y".to_string(), Value::Nil)]);

        vm.maybe_gc();

        assert!(
            vm.binding_locals.contains_key(&(live.0 as usize)),
            "live binding's locals were pruned"
        );
        assert!(
            !vm.binding_locals.contains_key(&dead_key),
            "dead binding's locals were not pruned"
        );
        assert!(
            vm.heap.is_live(snap_val),
            "snapshot value was swept despite the binding_locals root"
        );
    }
}
