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
    pub(crate) fn run(&mut self, entry: usize) -> Result<Value, Trap> {
        let proto = &self.protos[entry];
        let n_locals = proto.n_locals as usize;
        self.frames.push(Frame {
            proto_idx: entry,
            ip: 0,
            locals: Rc::new(RefCell::new(vec_nil(n_locals))),
            self_val: Value::Nil,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None, is_block: false, n_given_positional: 0, rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![], pending_yield: false, begin_rescue_depths: vec![],
            block_writeback: None,
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
    #[inline]
    pub(crate) fn check_frames(&self) -> Result<(), Trap> {
        if let Some(max) = self.max_frames
            && self.frames.len() >= max {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("stack level too deep ({} frames, max {})", self.frames.len(), max),
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
        // factor (heap.rs's `live * 2 max 1024`) is what determines
        // sweep frequency; bumping it cuts sweep count linearly.
        // The env knob below stays in for ongoing perf-regression
        // investigations (mirrors `RUBYRS_IC_STATS` shape).
        if std::env::var_os("RUBYRS_GC_DISABLE").is_some() {
            // Bump next_gc out of reach so we don't even re-enter
            // the gather/walk machinery on the next allocation.
            self.heap.next_gc = usize::MAX;
            return;
        }
        // Gather roots: stack + every frame's locals + self_val + swap_return
        // + pinned (native-code accumulators). class_stack holds Rc<Class>
        // which isn't GC-managed, so we don't need to walk it.
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + self.pinned.len() + 64);
        for v in &self.stack { roots.push(v.clone()); }
        for v in &self.pinned { roots.push(v.clone()); }
        // In-flight break/next transfer: the break value lives only
        // in `pending_loop_transfer` between `begin_loop_transfer`
        // and the final landing. The ensure body runs in between
        // and can trigger GC at allocation sites; without rooting
        // here a heap-allocated break value (Array/Hash/String/
        // Object) gets swept and the eventual stack.push in
        // `continue_loop_transfer` would re-publish a dangling
        // handle — silent heap corruption (ICE on the next op
        // that consults the slot's type). Reproduced under
        // STRESS_GC=1.
        if let Some(super::LoopTransfer {
            kind: super::LoopTransferKind::Break { value }, ..
        }) = &self.pending_loop_transfer {
            roots.push(value.clone());
        }
        // ENV hash, once initialised, is reachable from script
        // code via the `ENV` constant — pin it so the cache
        // doesn't get swept between LoadConst loads.
        if let Some(id) = self.env_hash { roots.push(Value::Hash(id)); }
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
        }
        for f in &self.frames {
            roots.push(f.self_val.clone());
            for v in f.locals.borrow().iter() { roots.push(v.clone()); }
            if let Some(v) = &f.swap_return { roots.push(v.clone()); }
            if let Some(id) = f.block_arg {
                // Block lives in the GC heap now (P2-13). Pushing
                // the Value::Block root is enough — the mark phase
                // walks the BlockHandle's `captured` and `self_val`
                // when it reaches the slot.
                roots.push(Value::Block(id));
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
                    for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
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
                    for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
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
                        for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
                    }
                }
                for m in shell.singleton_methods.borrow().values() {
                    if let Some(cl) = &m.closure {
                        for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
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
        for m in self.toplevel_methods.values() {
            if let Some(cl) = &m.closure {
                for v in cl.captured.borrow().iter() { roots.push(v.clone()); }
            }
        }
        let pending_frees = self.heap.collect(&roots);
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
        let arr_id = vm.heap.alloc(crate::heap::HeapObj::Array(Vec::new()));
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
}
