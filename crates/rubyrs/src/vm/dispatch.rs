//! Method dispatch and call setup. Mirrors the call-handling
//! machinery CRuby keeps in `vm_eval.c` / `vm_insnhelper.c` —
//! finding the target method on a receiver, pushing a frame,
//! threading args/block through, and routing to host fns or
//! to interpreter bodies.
//!
//! Contents:
//!   - `do_call` / `do_call_block` — the Op::Call entry points
//!     called from the opcode loop.
//!   - `invoke_method` / `invoke_method_with_block` — frame
//!     setup once the target Method has been resolved.
//!   - `invoke_block` — re-enter a captured block.
//!   - `cext_invoke_method` — bridge for C-ext re-entering the
//!     Ruby side via `rb_funcallv`.
//!   - `try_method_missing` — fallback dispatch path when the
//!     name lookup fails.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::intern::SymId;
use crate::value::{Class, Instance, Method, ObjId, Value, Visibility};

#[cfg(any(all(feature = "cext", not(target_os = "wasi")), feature = "_http_server"))]
use super::with_vm_ptr_set;
use super::{
    primitive_call, value_cmp_v_heap, vec_nil, visibility_from_name, Frame, HostFnSlot, PinGuard, Vm,
};
use crate::HostCtx;

/// Outcome of [`Vm::try_dispatch_send_bypass`].
///
/// `Handled(r)` means the helper has already done the work
/// (parsed target sym from `args[0]`, set
/// `bypass_visibility_once`, pushed args/recv onto the stack,
/// and recursed into `do_call`); the caller should propagate
/// `r` immediately.
///
/// `NotHandled { args, recv_opt }` means this isn't a `send`
/// call, or it's a `send` with a user-defined override on the
/// surrounding self / explicit receiver (CRuby's reserved-name
/// rule applies only to `__send__`, never `send`); the helper
/// has moved `args` and `recv_opt` back out so the caller can
/// continue dispatch with them intact.
enum SendBypass {
    Handled(Result<(), Trap>),
    NotHandled {
        args: Vec<Value>,
        recv_opt: Option<Value>,
    },
}

/// Outcome of [`Vm::try_dispatch_callable_intrinsics`].
///
/// `Handled` means the helper dispatched (Block.call /
/// `method(:name)` capture / BoundMethod-or-UnboundMethod-or-
/// CurriedProc arm); the caller `do_call` should
/// `return Ok(())` immediately. Any trap raised by an inner
/// arm bubbles through the helper's outer `Result<_, Trap>`.
///
/// `NotHandled { args, recv }` returns the inputs intact so
/// the caller continues with the rest of dispatch.
enum CallableOutcome {
    Handled,
    NotHandled {
        args: Vec<Value>,
        recv: Value,
    },
}

/// Outcome of [`Vm::try_dispatch_class_intrinsics`].
///
/// Same shape as [`CallableOutcome`] — `Handled` means the
/// helper fired one of the class-receiver arms (`Hash[]` /
/// `cls.new` / `cls.allocate` / `cls.include` / etc.) and
/// pushed the result; caller returns `Ok(())` immediately.
/// `NotHandled { args, recv }` returns inputs intact so the
/// caller continues with the rest of dispatch.
enum ClassOutcome {
    Handled,
    NotHandled {
        args: Vec<Value>,
        recv: Value,
    },
}

impl Vm {
    /// `String#encoding` intercept — pushes the preamble's
    /// `Encoding::UTF_8` instance and returns true if the call
    /// matches the shape; returns false otherwise so the caller
    /// falls through to its usual primitive dispatch.
    ///
    /// Used by BOTH `do_call` and `do_call_block`. The Encoding
    /// object lives in the joined-name constants table seeded by
    /// the preamble; materialising it requires `&mut self`, which
    /// the stateless `primitive::string_call` free function can't
    /// supply.
    ///
    /// ICE if the constant is missing — only reachable when the
    /// preamble didn't load (e.g. a misconfigured test harness),
    /// and silently returning Nil leaves downstream callers
    /// (`enc.dummy?` etc.) with a NoMethodError far from the root
    /// cause. Panic surfaces the actual bootstrap failure.
    pub(crate) fn try_push_string_encoding(
        &mut self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> bool {
        if !matches!(recv, Value::Str(_)) || name != "encoding" || !args.is_empty() {
            return false;
        }
        let key = self.interner.intern("Encoding::UTF_8");
        let v = self.constants.get(&key).cloned()
            .expect("ICE: Encoding::UTF_8 not in constants table — preamble didn't load");
        self.stack.push(v);
        true
    }

    /// Re-entrant dispatch entry for C extensions calling back into
    /// Ruby via `rb_funcall*`. Invokes `recv.method(args)` through
    /// the normal `do_call` path, leaving the result on the stack
    /// where the caller can pop it.
    ///
    /// Setup mirrors what the compiler emits for a Ruby-side
    /// `recv.method(args)`: push the receiver, then each argument,
    /// then call `do_call` with `no_recv = false`. After `do_call`
    /// the result sits on top of the operand stack — pop and return.
    ///
    /// `cache_id = u16::MAX` is a sentinel that
    /// `lookup_method_cached` treats as "no cache slot": the
    /// `idx < call_caches.len()` guard naturally fails (the table
    /// is bounded by the number of compiled `Op::Call` instructions
    /// — nowhere near 65535 in any realistic program), so both the
    /// read and writeback paths short-circuit. Without this sentinel
    /// a hard-coded `cache_id = 0` would poison whichever compiled
    /// call site got slot 0 — that site would silently dispatch
    /// whatever class/method the C ext last invoked. Future work:
    /// allocate a per-`(recv-class, method)` cache for cext calls
    /// if profiling shows the uncached path matters.
    #[cfg(all(feature = "cext", not(target_os = "wasi")))]
    pub(crate) fn cext_invoke_method(
        &mut self,
        recv: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, Trap> {
        let name_id = self.interner.intern(method);
        let argc = args.len();
        self.stack.push(recv);
        for a in args {
            self.stack.push(a);
        }
        self.do_call(
            name_id,
            argc,
            /* no_recv = */ false,
            /* cache_id = */ u16::MAX,
        )?;
        Ok(self
            .stack
            .pop()
            .expect("ICE: cext_invoke_method: do_call produced no result"))
    }



    /// Look up `method_missing` on `recv`'s class chain. If found,
    /// prepend the missed `name_id` as a Symbol arg and invoke it
    /// (pushing a frame); returns `Ok(true)` so the caller can
    /// `return Ok(())` instead of raising. Returns `Ok(false)` when
    /// the receiver doesn't carry a `method_missing` (or isn't a
    /// `Value::Object`) — caller proceeds to raise NoMethodError.
    ///
    /// Scope of this PoC: only Object receivers (user instances).
    /// Primitive receivers (Int, Str, …) skip the lookup — adding
    /// per-primitive class chains is a follow-up.
    pub(crate) fn try_method_missing(
        &mut self,
        recv: &Value,
        name_id: SymId,
        args: Vec<Value>,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        let cls = match recv {
            Value::Object(id) => self.heap.class_of(*id),
            _ => return Ok(false),
        };
        let mm_id = self.interner.intern("method_missing");
        let m = match self.lookup_method_uncached(&cls, mm_id) {
            Some(m) => m,
            None => return Ok(false),
        };
        let mut new_args = Vec::with_capacity(args.len() + 1);
        new_args.push(Value::Sym(name_id));
        new_args.extend(args);
        self.invoke_method_with_block(m, recv.clone(), new_args, block)?;
        Ok(true)
    }



    /// Invoke a registered host fn (either v1 or v2 slot).
    ///
    /// V1 stashes `*mut Vm` via `with_vm_ptr_set` so a cext-style
    /// re-entrant `rb_funcall` can find the running VM (ADR 0013) —
    /// but only on builds where that re-entry channel actually
    /// exists, i.e. `all(feature = "cext", not(target_os = "wasi"))`.
    /// With `--no-default-features` (or on wasi) `with_vm_ptr_set`
    /// itself lives inside the cfg'd-off `mod cext`, so the V1 arm
    /// just calls `host(args)` directly; see the in-fn comment for
    /// the migration site if a non-cext V1 host ever needs TLS-Vm
    /// access. V1 closures hold no Rust borrow of `self` during the
    /// call, so the raw-ptr reborrow inside cext is the only access
    /// path and aliasing is well-defined.
    ///
    /// V2 deliberately does NOT call `with_vm_ptr_set`. The V2
    /// closure holds a `HostCtx` that borrows `&self.heap` for the
    /// duration of the call; if we *also* re-aimed CURRENT_VM_PTR at
    /// `self` and the closure reborrowed it as `&mut Vm`, that
    /// reborrow would alias the live `&self.heap` borrow — any heap
    /// mutation during the inner call could realloc the backing
    /// `Vec<HeapObj>` and dangle slices returned by
    /// `ctx.resolve_array` / `resolve_hash`.
    ///
    /// Note that `CURRENT_VM_PTR` may already be non-null on entry
    /// (an outer v1/cext frame set it), so the V2 arm is NOT
    /// asserting "TLS is null." The actual boundary is: the TLS is
    /// `pub(crate)`, so an external v2 closure has no language-level
    /// path to read it — the unsafe re-entry channel is unreachable
    /// to user code in the V2 slot. Skipping the overwrite here is
    /// the closing brick: even an internal future v2 helper would
    /// have to explicitly opt into touching the TLS, which is the
    /// point at which the soundness review is expected.
    ///
    /// cext bridges register as V1, so nothing legitimate needs the
    /// ptr from the V2 arm.
    fn invoke_host_fn(&mut self, slot: HostFnSlot, args: &[Value]) -> Result<Value, Trap> {
        match slot {
            HostFnSlot::V1(host) => {
                // V1 contract under the `cext` feature gate:
                // `with_vm_ptr_set` parks the Vm pointer in TLS so a
                // cext-bridge V1 host can re-enter the VM through
                // rb_funcallv. With cext off there is no rb_funcall
                // path to need it and `with_vm_ptr_set` itself lives
                // inside `mod cext`, so we just call the host body
                // directly. Today every legitimate V1 caller IS a
                // cext bridge (see the V1/V2 doc above), so the
                // contract change is invisible at runtime; if a
                // future non-cext V1 host needs TLS-Vm access, this
                // is the site to move `with_vm_ptr_set` out of
                // `mod cext` and lift the cfg gate.
                // Set the TLS Vm pointer for re-entrant V1
                // host fns:
                //   - cext bridge: rb_funcallv callback
                //     dispatches through CURRENT_VM_PTR
                //   - _http_server battery: per-request Ruby
                //     block invocation reads CURRENT_VM_PTR
                //     to access &mut Vm for step_block
                // Either feature enables the machinery; both
                // share the same TLS slot defined in
                // super::vm_ptr.
                #[cfg(any(
                    all(feature = "cext", not(target_os = "wasi")),
                    feature = "_http_server",
                    feature = "_fiber",
                ))]
                {
                    let vm_ptr: *mut Vm = self;
                    with_vm_ptr_set(vm_ptr, || host(args))
                }
                #[cfg(not(any(
                    all(feature = "cext", not(target_os = "wasi")),
                    feature = "_http_server",
                    feature = "_fiber",
                )))]
                { host(args) }
            }
            HostFnSlot::V2(host) => {
                let ctx = HostCtx::new(&self.heap, &self.interner);
                host(&ctx, args)
            }
        }
    }

    /// Resolve a `Symbol` / `String` arg into a SymId for the ivar
    /// name, validating it against an **ASCII-only subset** of
    /// CRuby's ivar-name grammar: `@[A-Za-z_][A-Za-z0-9_]*`.
    /// CRuby accepts some non-ASCII identifier characters too;
    /// rubyrs takes the conservative ASCII subset because no
    /// caller in the surfaced surface needs Unicode ivar names —
    /// see `is_valid_ivar_name` for the precise grammar. Rejects:
    ///   - bare `@` (no body)
    ///   - `@@x` (class var — two `@`)
    ///   - `@1` (digit start after `@`)
    ///   - `@foo?` / `@foo=` / `@foo!` (suffixes that work for
    ///     methods but not for ivars)
    ///
    /// String path enforces `Config::max_symbols` so untrusted code
    /// can't grow the interner unbounded via
    /// `instance_variable_{get,set}("@x#{i}", ...)` in a loop.
    /// Non-Symbol-non-String args raise TypeError matching the
    /// shape `parse_send_target` uses for `send` / `__send__`.
    fn resolve_ivar_name_arg(&mut self, arg: &Value) -> Result<SymId, Trap> {
        match arg {
            Value::Sym(id) => {
                let resolved = self.interner.resolve(*id);
                if is_valid_ivar_name(resolved) {
                    return Ok(*id);
                }
                // Happy path returns above with no allocation. Only
                // the error path materialises the message; build the
                // String here so the borrow of `resolved` is dropped
                // before the `&mut self` call to `trap`.
                let msg = format!(
                    "'{}' is not allowed as an instance variable name",
                    resolved,
                );
                Err(self.trap(RubyError::NameError { msg }))
            }
            Value::Str(s) => {
                let raw = s.to_string_lossy();
                if !is_valid_ivar_name(&raw) {
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("'{}' is not allowed as an instance variable name", raw),
                    }));
                }
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&raw) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Ok(self.interner.intern(&raw))
            }
            other => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
        }
    }

    /// Parse the first arg of a `send` / `__send__` call as the
    /// target method name. Symbol passes through; String is
    /// interned (CRuby's transparent `to_sym` on the name arg).
    /// Anything else returns the CRuby-shape TypeError
    /// (`<inspect> is not a symbol nor a string`); zero args
    /// returns the CRuby-shape ArgumentError. Shared by all four
    /// send-recogniser sites (`do_call` / `do_call_block`, each
    /// with their no_recv and recv arms) so the validation +
    /// error formatting can't drift between paths.
    fn parse_send_target(&mut self, args: &[Value]) -> Result<SymId, Trap> {
        if args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1+)".into(),
            }));
        }
        match &args[0] {
            Value::Sym(s) => Ok(*s),
            Value::Str(s) => {
                // Same `Config::max_symbols` cap as `String#to_sym`
                // (vm/string.rs:971) — without this, untrusted code
                // could grow the interner unbounded by calling
                // `send("dyn_#{i}")` in a loop. Existing symbols
                // always re-resolve; only fresh names count.
                let name = s.to_string_lossy();
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&name) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Ok(self.interner.intern(&name))
            }
            other => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
        }
    }

    /// Primitive-receiver fast-path for the handful of zero-arg
    /// methods (`String#length` / `#size` / `#to_s`, `Integer#to_s`
    /// / `#inspect`) that profiling showed dominate fizzbuzz-shape
    /// loops. Returns true after pushing the result; false if the
    /// receiver / name / arity don't match and `do_call` should
    /// continue through normal dispatch.
    ///
    /// Currently safe to call after `take_bypass_visibility()`
    /// because every arm matches a primitive Value (no visibility
    /// model). Adding an arm for a receiver with a user-Class
    /// method table requires threading the bypass flag through —
    /// see the comment at the call site in `do_call`.
    fn try_fast_primitive(&mut self, name_id: SymId, argc: usize, no_recv: bool) -> bool {
        if no_recv || argc != 0 {
            return false;
        }
        let v = {
            let recv = self
                .stack
                .last()
                .expect("ICE: stack underflow before do_call receiver");
            match recv {
                Value::Str(a) if name_id == self.sym_length || name_id == self.sym_size => {
                    Value::Int(a.char_count() as i64)
                }
                Value::Str(a) if name_id == self.sym_to_s => Value::Str(a.clone()),
                Value::Int(n) if name_id == self.sym_to_s || name_id == self.sym_inspect => {
                    crate::vm::numeric::integer_to_s_value(*n)
                }
                _ => return false,
            }
        };
        self.stack.pop();
        self.stack.push(v);
        true
    }

    /// `no_recv` builtin-or-host fast path. Tries the host-side
    /// builtin table first (`builtin_call` covers `puts` / `p` /
    /// `sprintf` / `require` / ...), then the
    /// `register_fn`-installed host fns. Returns `Ok(true)` if
    /// one of those handled the call (caller should `return
    /// Ok(())` immediately), or `Ok(false)` if neither matched
    /// and `do_call` should fall through to the next arm.
    ///
    /// Extracted from `do_call`'s no_recv preamble per #192
    /// commit 1/5 (the #152 research's first recommendation,
    /// scoped narrower than the research's initial estimate
    /// because the broader 362-431 range turned out to be
    /// interleaved with `try_fast_primitive` and the stack drain;
    /// see #192's commit message for why).
    ///
    /// `suppress_call_result_push` handling stays inside the
    /// helper: `require_relative` (and any future builtin that
    /// could see its caller unwound to an outer `rescue` mid-call)
    /// sets the flag to signal "don't push my return value — the
    /// stack is now the rescue handler's, not yours". Helper
    /// checks + clears the flag (one-shot) just like the inline
    /// code did.
    fn try_dispatch_no_recv_builtin_or_host(
        &mut self,
        name: &str,
        name_id: SymId,
        args: &[Value],
    ) -> Result<bool, Trap> {
        if let Some(res) = self.builtin_call(name, args) {
            let v = res?;
            if self.suppress_call_result_push {
                self.suppress_call_result_push = false;
            } else {
                self.stack.push(v);
            }
            return Ok(true);
        }
        if let Some(host) = self.host_fns.get(&name_id).cloned() {
            let v = self.invoke_host_fn(host, args)?;
            self.stack.push(v);
            return Ok(true);
        }
        Ok(false)
    }

    /// Result of consulting the `send` / `__send__` bypass
    /// recogniser. `Handled` means the helper has already done
    /// all the work (parsed target sym, set
    /// `bypass_visibility_once`, pushed args/recv, recursed
    /// into `do_call`) and the caller should `return` the
    /// contained `Result` immediately. `NotHandled` means the
    /// call isn't a `send` form, or it's a `send` with a user-
    /// defined override on the surrounding self/recv (reserved-
    /// name rule applies only to `__send__`); the helper has
    /// moved `args` and `recv_opt` *back out* so the caller can
    /// continue dispatch.
    ///
    /// See `try_dispatch_send_bypass` for the full doc; #192
    /// commit 2/5.
    fn try_dispatch_send_bypass(
        &mut self,
        name: &str,
        name_id: SymId,
        cache_id: u16,
        args: Vec<Value>,
        recv_opt: Option<Value>,
    ) -> SendBypass {
        // Early out for non-send names — the common case.
        if !matches!(name, "send" | "__send__") {
            return SendBypass::NotHandled { args, recv_opt };
        }
        // Subject for the user-override check:
        //   - With-recv form: the explicit receiver.
        //   - No-recv form: the surrounding frame's `self_val`
        //     (because `bare_send(:x)` is implicit-self).
        let frame_self_storage;
        let subject: &Value = match &recv_opt {
            Some(r) => r,
            None => {
                frame_self_storage = self.frames.last()
                    .expect("ICE: do_call(no_recv) with empty frames")
                    .self_val
                    .clone();
                &frame_self_storage
            }
        };
        // User override only blocks `send` (the reserved-name
        // rule applies only to `__send__`). Same lookup shape
        // as the originals at the two inlined sites.
        let user_override = name == "send" && match subject {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_cached(&cls, name_id, cache_id).is_some()
            }
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
            _ => false,
        };
        if user_override {
            return SendBypass::NotHandled { args, recv_opt };
        }
        // Bypass path. Parse target sym from args[0]; on failure
        // surface the trap through Handled so the caller's `?`
        // sees it.
        let target_sym = match self.parse_send_target(&args) {
            Ok(t) => t,
            Err(e) => return SendBypass::Handled(Err(e)),
        };
        let new_argc = args.len() - 1;
        // Set bypass_visibility BEFORE recursing so the inner
        // do_call's `take_bypass_visibility()` sees it. Note:
        // recursing through the same `do_call` entry preserves
        // the existing setter-then-recurse pattern; the helper
        // does NOT call do_call while still holding any borrow.
        self.bypass_visibility_once = true;
        let no_recv_for_recursion = recv_opt.is_none();
        if let Some(recv) = recv_opt {
            self.stack.push(recv);
        }
        for a in args.into_iter().skip(1) {
            self.stack.push(a);
        }
        SendBypass::Handled(self.do_call(target_sym, new_argc, no_recv_for_recursion, u16::MAX))
    }

    /// Callable intrinsics — dispatch to the `Method` / `Block` /
    /// `BoundMethod` / `UnboundMethod` / `CurriedProc` family.
    ///
    /// Returns [`CallableOutcome::Handled`] if one of the arms
    /// fired (the helper has already pushed any result to the
    /// stack, or has recursed into `do_call` and bubbled its
    /// result via `?`); the caller `do_call` should `return Ok(())`
    /// immediately. Returns [`CallableOutcome::NotHandled { args,
    /// recv }`] if no arm matched; the caller continues with the
    /// rest of dispatch using the returned `args` + `recv`.
    ///
    /// Extracted from `do_call` per the #152 research deliverable;
    /// see #192 commit 3/5 for the migration rationale.
    fn try_dispatch_callable_intrinsics(
        &mut self,
        name: &str,
        _name_id: SymId,
        args: Vec<Value>,
        recv: Value,
    ) -> Result<CallableOutcome, Trap> {
        if let Value::Block(bid) = &recv
            && matches!(name, "call" | "[]" | "()" | "yield") {
                // CRuby exposes block invocation under four names:
                // `.call(args)`, `.()` (already lowered to `call`
                // by parsers but kept here defensively), `[args]`
                // bracket form, and `.yield(args)` (mostly a
                // documentation alias). All four route the same
                // way: invoke the block, drive until its frame
                // returns, leave the result on the stack.
                let pre_frames = self.frames.len();
                self.invoke_block(*bid, args)?;
                self.dispatch_until(pre_frames)?;
                return Ok(CallableOutcome::Handled);
            }
        // `Proc#arity` — CRuby-shape arity for the block. Block
        // params in rubyrs Tier-1 are only required + rest (no
        // optionals, no keyword params — `compile_block` accepts
        // only `BlockParam::{Single, Destructure, Rest}`), so
        // the formula is:
        //   has_rest → -(n_required + 1)
        //   else     →  n_required
        // The Proto's `rest_param` field is NOT populated for
        // blocks (rest_slot lives on the BlockHandle directly);
        // can't share the `proto_arity` helper used by
        // `Method#arity` / `UnboundMethod#arity` without
        // walking the BlockHandle here. Sinatra's `compile!`
        // (sinatra/base.rb:1810) reads `block.arity` to size
        // the route block's positional bindings. (TRY_RUNS
        // layer #24.)
        if matches!(&recv, Value::Block(_) | Value::CurriedProc(_))
            && name == "arity" && !args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
            }));
        }
        if let Value::Block(bid) = &recv
            && name == "arity" && args.is_empty() {
            let (n_required, has_rest) = {
                let bh = self.heap.block(*bid);
                (bh.n_params as i64, bh.rest_slot.is_some())
            };
            let arity = if has_rest { -(n_required + 1) } else { n_required };
            self.stack.push(Value::Int(arity));
            return Ok(CallableOutcome::Handled);
        }
        // `CurriedProc#arity` — CRuby returns -1 for any curried
        // proc/lambda regardless of remaining required slots
        // (the curried wrapper accepts a variable number of args
        // per `.call` site as the partial application grows).
        // Without this arm, `proc { |a| }.curry.arity` falls
        // through to NoMethodError even though `Proc#arity`
        // works — inconsistent now that the Block arm exists.
        // (Copilot review #263 round 3.)
        if let Value::CurriedProc(_) = &recv
            && name == "arity" && args.is_empty() {
            self.stack.push(Value::Int(-1));
            return Ok(CallableOutcome::Handled);
        }
        // `Object#method(:name)` — capture (recv, name_id) into a
        // BoundMethod heap object. Returned Value can be `.call`'d
        // (handled in the next arm) or stored. Args must be a
        // single Symbol; CRuby also accepts String but we keep
        // the subset narrow for now.
        //
        // GC rooting: `recv` here came from the operand-stack pop
        // at the top of `do_call` and lives only in this Rust
        // local. The `maybe_gc` below would otherwise sweep its
        // heap slot (e.g. a fresh `Squared.new.method(:call)`
        // where the Squared instance has no other root), then the
        // alloc'd BoundMethod would store a stale ObjId. Repro:
        // `proc_curry_compose.rb` under STRESS_GC=1 — the
        // BoundMethod survives but its `recv` points at a Dead
        // slot, panicking later in `class_of`.
        if name == "method" && args.len() == 1
            && let Value::Sym(bound_name_id) = &args[0] {
                // Snapshot the resolved Method at capture time so
                // `bm.call` survives a subsequent `remove_method`
                // (CRuby parity, matches the `instance_method` arm).
                //
                // Use the DISPATCH class (`heap.class_of`) for
                // Object receivers — that's the class chain that
                // a regular `recv.foo` would walk, and it
                // honours singleton methods (`def obj.foo; ...`).
                // `Vm::class_of` reports the *real* class for
                // script-visible `obj.class`, which skips the
                // eigenclass; using that here would snapshot the
                // real-class body and silently invoke it instead
                // of the singleton override.
                let snapshot = match &recv {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_uncached(&cls, *bound_name_id)
                    }
                    _ => match self.class_of(&recv) {
                        Value::Class(cls) => self.lookup_method_uncached(&cls, *bound_name_id),
                        _ => None,
                    },
                };
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(recv.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::BoundMethod {
                    recv: recv.clone(),
                    name_id: *bound_name_id,
                    method: snapshot,
                });
                g.vm.stack.push(Value::BoundMethod(id));
                return Ok(CallableOutcome::Handled);
            }
        // `bm.call(args)` / `bm.()` / `bm[args]` — dispatch the
        // captured method on the captured receiver. We re-enter
        // `do_call` recursively with the bound recv pushed below
        // the args, the captured name interned, and the original
        // argc.
        // `bm.unbind` — strip the receiver, keep (class_of(recv),
        // name_id). The captured class is the receiver's class at
        // unbind time; CRuby technically captures the *owner* (the
        // class that defined the method), but for our subset
        // `class_of` is the closest approximation and roundtrips
        // through `bind` correctly for the common shapes.
        if let Value::BoundMethod(bid) = &recv && name == "unbind" && args.is_empty() {
            // Inherit the snapshot the BoundMethod was carrying;
            // if it has none (legacy values constructed before
            // the snapshot field, or `method` capture sites that
            // synthesise a transient BM), look up live from the
            // receiver's class. The resulting UnboundMethod
            // survives a subsequent `remove_method` on either
            // side of the round-trip.
            let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id, method } => (recv.clone(), *name_id, method.clone()),
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            // Use the DISPATCH class (heap.class_of) for Object
            // receivers so the captured class reflects any
            // singleton class on `recv`. Otherwise the
            // UnboundMethod would carry the REAL class plus a
            // singleton-method snapshot — `um.bind(other)` would
            // pass the is_a fence (other is_a real_class) and
            // silently invoke the singleton body on an unrelated
            // instance. With the dispatch class, the captured
            // class IS the singleton class, and is_a on a
            // different instance correctly fails (singleton
            // classes only contain the original instance via
            // class_is_a).
            let cls = match &bm_recv {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&bm_recv) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: "cannot unbind method on a value without a class".into(),
                    })),
                },
            };
            let snapshot = bm_method.or_else(|| self.lookup_method_uncached(&cls, bm_name_id));
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::UnboundMethod {
                class: cls,
                name_id: bm_name_id,
                method: snapshot,
            });
            self.stack.push(Value::UnboundMethod(id));
            return Ok(CallableOutcome::Handled);
        }
        // `ubm.bind(obj)` — reconstitute a BoundMethod, checking
        // that `obj` is_a? the captured class. Raises TypeError on
        // mismatch, matching CRuby.
        if let Value::UnboundMethod(uid) = &recv && name == "bind" && args.len() == 1 {
            let (cap_class, cap_name_id, cap_method) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id, method } => {
                    (class.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args;
            let target = args.swap_remove(0);
            // Use dispatch class (heap.class_of) for Object
            // targets — matches the eigenclass-aware capture in
            // unbind. Otherwise binding a singleton-method
            // UnboundMethod back to its ORIGINAL instance would
            // fail the is_a fence (target's real class doesn't
            // walk through the singleton class).
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            // Kernel is the universally-bindable sentinel — CRuby
            // models it as a Module included in Object, so every
            // value is_a Kernel. Modules in general also accept
            // any receiver: CRuby's
            // `Module#instance_method(:foo).bind(obj)` succeeds
            // regardless of whether obj's class includes the
            // module (verified against 3.4). Class.instance_method
            // stays strict — `obj.is_a?(cls)` required. Same
            // fence as the `bind_call` arm below.
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            // GC rooting: `target` came from `args.swap_remove(0)`,
            // which itself was drained from the operand stack at the
            // top of `do_call`. It now lives only in this Rust local
            // — not in `self.stack`, not in any frame's locals. The
            // `maybe_gc` below would otherwise sweep its heap slot
            // (Greeter.new in `kernel_instance_method.rb` under
            // STRESS_GC=1), and the BoundMethod's `recv` would point
            // at a Dead slot. Same fix shape as `Object#method` and
            // `invoke_block` rest-slot in commit 86db73d.
            let mut g = crate::vm::PinGuard::new(self);
            g.pin(target.clone());
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            // Propagate the snapshot from the UnboundMethod —
            // a later `bm.call` after a `remove_method` on the
            // captured class still invokes the original body.
            let id = g.vm.heap.alloc(HeapObj::BoundMethod {
                recv: target,
                name_id: cap_name_id,
                method: cap_method,
            });
            g.vm.stack.push(Value::BoundMethod(id));
            return Ok(CallableOutcome::Handled);
        }
        // `ubm.bind_call(recv, *args)` — CRuby 2.7+ fused
        // bind-then-call: identical to `ubm.bind(recv).call(*args)`
        // but without allocating a transient BoundMethod heap
        // object. Re-uses the same is_a check (with the Kernel
        // sentinel) and dispatches the captured method with
        // `recv` pushed below the args.
        //
        // Motivating consumer: tilt-2.7.0
        // `lib/tilt/template.rb:496` calls
        // `method.bind_call(scope, **locals, &block)` per render —
        // the fast path that replaces older `bind(scope).call(...)`
        // shapes. Without this arm tilt falls through to
        // NoMethodError on every render.
        //
        // Arity: at least 1 arg (the receiver); extra args + block
        // are forwarded to the captured method.
        // `Method#bind_call(other, *args)` — mirror of
        // `UnboundMethod#bind_call`, but starts from a bound
        // Method (which carries a receiver). Equivalent to
        // `m.unbind.bind(other).call(*args)` but doesn't
        // allocate intermediate UnboundMethod / Method
        // wrappers. The is-a fence + snapshot-preferred
        // dispatch are identical to the UnboundMethod arm
        // below — see the longer comment block there for the
        // singleton-class / Module-mixin / Kernel edge cases.
        if let Value::BoundMethod(bid) = &recv && name == "bind_call" && !args.is_empty() {
            let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id, method } => (recv.clone(), *name_id, method.clone()),
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            // Capture class from the original receiver — same
            // dispatch-class shape `unbind` uses, so singleton
            // methods round-trip correctly.
            let cap_class = match &bm_recv {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&bm_recv) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: "cannot bind_call on a Method whose receiver has no class".into(),
                    })),
                },
            };
            let mut args = args;
            let target = args.remove(0);
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind_call argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind_call argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            let m = match bm_method.or_else(|| self.lookup_method_uncached(&cap_class, bm_name_id)) {
                Some(m) => m,
                None => {
                    let mname = self.interner.resolve(bm_name_id).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cap_class.name),
                    }));
                }
            };
            self.invoke_method(m, target, args)?;
            return Ok(CallableOutcome::Handled);
        }
        if let Value::BoundMethod(_) = &recv && name == "bind_call" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..)".into(),
            }));
        }
        if let Value::UnboundMethod(uid) = &recv && name == "bind_call" && !args.is_empty() {
            let (cap_class, cap_name_id, cap_method) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id, method } => {
                    (class.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args;
            let target = args.remove(0);
            // Dispatch class for Object targets — mirrors the
            // eigenclass-aware capture in unbind so a
            // singleton-method UnboundMethod can bind_call back
            // to its original receiver.
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind_call argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            // Skip the is-a fence when:
            // (a) captured class is Kernel — every value is_a
            //     Kernel in CRuby; we don't model the Kernel
            //     Module-mixin and use this sentinel to match.
            // (b) captured class is any Module — CRuby's
            //     `Module#instance_method(:foo).bind_call(obj)`
            //     accepts ANY obj, not just instances of classes
            //     that include the module. Verified against 3.4:
            //     `module M; def foo; end; end;
            //      M.instance_method(:foo).bind_call(Object.new)`
            //     succeeds and runs `foo`. Note the captured
            //     method is invoked directly via `invoke_method`
            //     on the resolved Method (snapshot-preferred,
            //     `cap_class`-chain fallback) — no name-based
            //     lookup on the receiver's class chain happens,
            //     so the receiver doesn't need to have `foo`
            //     defined on its class.
            //     `Class.instance_method(:foo).bind_call(obj)`
            //     stays strict — `obj.is_a?(cls)` required.
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind_call argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            // Prefer the snapshot taken at capture time — tilt's
            // pattern of capture→remove→bind_call would otherwise
            // miss the now-removed entry. Fall back to live chain
            // lookup when no snapshot exists (e.g. UnboundMethod
            // values created from `unbind` paths that pre-date
            // the snapshot field).
            let m = match cap_method.or_else(|| self.lookup_method_uncached(&cap_class, cap_name_id)) {
                Some(m) => m,
                None => {
                    let mname = self.interner.resolve(cap_name_id).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cap_class.name),
                    }));
                }
            };
            self.invoke_method(m, target, args)?;
            return Ok(CallableOutcome::Handled);
        }
        if let Value::UnboundMethod(_) = &recv && name == "bind_call" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..)".into(),
            }));
        }
        // `m.to_proc` — explicit conversion to a Proc. Equivalent
        // to the implicit `&m` coercion: routes through the same
        // `coerce_callable_to_block` forwarder so calling the
        // resulting Proc splats its args back into `bm.call(...)`.
        if let Value::BoundMethod(bid) = &recv
            && name == "to_proc" && args.is_empty() {
                let bm_id = *bid;
                let id = self.coerce_callable_to_block(Value::BoundMethod(bm_id))?;
                self.stack.push(Value::Block(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m.curry` / `m.curry(n)` — host-side partial application.
        // Returns a CurriedProc that gathers args across successive
        // `.call` invocations until `target_arity` is reached, then
        // invokes the underlying with the full arg list. `class_of`
        // reports CurriedProc as `Proc`, matching CRuby.
        if matches!(&recv, Value::BoundMethod(_) | Value::Block(_))
            && name == "curry" && args.len() <= 1 {
                let target_arity: u16 = if let Some(Value::Int(n)) = args.first() {
                    if *n < 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("negative arity for curry ({})", n),
                        }));
                    }
                    if *n > u16::MAX as i64 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("curry arity out of range ({})", n),
                        }));
                    }
                    *n as u16
                } else if let Value::BoundMethod(bid) = &recv {
                    let (bm_recv, m_name_id) = {
                        let (r, n) = self.heap.bound_method(*bid);
                        (r.clone(), n)
                    };
                    let class = match self.class_of(&bm_recv) {
                        Value::Class(c) => c,
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: "Method receiver has no resolvable class".into(),
                        })),
                    };
                    match self.lookup_method_uncached(&class, m_name_id) {
                        Some(m) => self.protos[m.proto_idx].n_required_positional,
                        None => return Err(self.trap(RubyError::ArgumentError {
                            msg: "cannot curry a method with unknown arity (builtin)".into(),
                        })),
                    }
                } else if let Value::Block(bid) = &recv {
                    // Proc#curry — derive arity from the underlying
                    // proto's required-positional count. Rest / kw
                    // are not supported as auto-arity for curry; user
                    // can still pass an explicit arity hint above.
                    let bh = self.heap.block(*bid);
                    let proto = &self.protos[bh.proto_idx];
                    if bh.rest_slot.is_some() && proto.n_required_positional == 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "cannot curry a proc with only rest params (pass explicit arity)".into(),
                        }));
                    }
                    proto.n_required_positional
                } else {
                    unreachable!()
                };
                // Pin `recv` (the underlying BoundMethod / Proc):
                // it was popped from the operand stack by do_call, so
                // it has no GC root by the time maybe_gc fires. Same
                // root-hole shape as the BoundMethod-coerce-to-Block
                // fix in PR #45 (5874798 / 50867c5).
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(recv.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::CurriedProc {
                    underlying: recv.clone(),
                    gathered: Vec::new(),
                    target_arity,
                });
                g.vm.stack.push(Value::CurriedProc(id));
                return Ok(CallableOutcome::Handled);
            }
        // `cp.call(args)` — append to gathered; invoke if arity hit,
        // else return a new CurriedProc carrying the appended state.
        if let Value::CurriedProc(cid) = &recv
            && matches!(name, "call" | "[]" | "()") {
                let (underlying, gathered, arity) = {
                    let (u, g, a) = self.heap.curried_proc(*cid);
                    (u.clone(), g.clone(), a)
                };
                let mut combined = gathered;
                combined.extend(args);
                if combined.len() >= arity as usize {
                    let argc = combined.len();
                    self.stack.push(underlying);
                    for a in combined { self.stack.push(a); }
                    let call_sym = self.interner.intern("call");
                    self.do_call(call_sym, argc, false, u16::MAX)?;
                    return Ok(CallableOutcome::Handled);
                }
                // Same pin-the-underlying pattern as the curry-on-Method
                // branch above. `combined` may also contain heap-typed
                // arg values that are only held in this Rust-local Vec;
                // pinning the underlying alone is enough because the
                // mark phase walks CurriedProc's contents only after
                // alloc — but the new alloc's reading the SAME Vec, so
                // we need both pinned across the maybe_gc call.
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(underlying.clone());
                for v in &combined { g.pin(v.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::CurriedProc {
                    underlying,
                    gathered: combined,
                    target_arity: arity,
                });
                g.vm.stack.push(Value::CurriedProc(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m >> other` / `m << other` — function composition.
        // `(m >> g).(x) == g.(m.(x))`; `(m << g).(x) == m.(g.(x))`.
        // Both sides must be callable — BoundMethod or Block. The
        // result is a Block (Proc) that splats `*args` through the
        // chain in the right order.
        if matches!(&recv, Value::BoundMethod(_) | Value::Block(_))
            && matches!(name, ">>" | "<<") && args.len() == 1 {
                let mut args = args;
                let other = args.swap_remove(0);
                if !matches!(&other, Value::BoundMethod(_) | Value::Block(_)) {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "compose argument must be a Method or Proc (got {})",
                            other.type_name(),
                        ),
                    }));
                }
                let (outer, inner) = if name == ">>" {
                    (other, recv)
                } else {
                    (recv, other)
                };
                let id = self.coerce_compose_to_block(outer, inner)?;
                self.stack.push(Value::Block(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m.hash` — Integer hash derived from receiver identity
        // (ObjId / value / Rc-ptr address) + name_id. Two
        // BoundMethods compared equal under `Method#==` must
        // collide; that's the only invariant CRuby promises. The
        // mix below is wrapping_add + wrapping_mul to be cheap
        // and avoid raising.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "hash" && args.is_empty() {
                let h: i64 = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n) = self.heap.bound_method(*bid);
                        let recv_h = method_recv_hash(r);
                        recv_h.wrapping_mul(0x9E3779B1).wrapping_add(n.0 as i64)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n) = self.heap.unbound_method(*uid);
                        let cls_h = std::rc::Rc::as_ptr(&cls) as i64;
                        cls_h.wrapping_mul(0x9E3779B1).wrapping_add(n.0 as i64)
                    }
                    _ => unreachable!(),
                };
                self.stack.push(Value::Int(h));
                return Ok(CallableOutcome::Handled);
            }
        // `m.source_location` — three shapes:
        //   - User-defined methods: `[filename, lineno]` derived
        //     from the proto's first op_span via the Vm-side
        //     `sources` mirror; falls back to lineno 0 if the
        //     source text isn't available (rare — synthesised
        //     protos for forwarders / preamble eval).
        //   - Synth builtins with `source_label = Some(label)`
        //     (Kernel reflection records): `[label, line]` where
        //     label is the static "<internal:kernel>" string and
        //     line is the meta's placeholder.
        //   - Synth builtins with `source_label = None`
        //     (BasicObject reflection records): `nil`. CRuby
        //     reports nil for these C-defined methods even though
        //     the Kernel set returns a label — we mirror.
        //   - Methods with no snapshot (none-of-the-above
        //     fallback): `nil`.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "source_location" && args.is_empty() {
                // Prefer the snapshot Method so introspection
                // survives a subsequent `remove_method` between
                // capture and the source_location query.
                let (class, m_name_id, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let snap = snap.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => { self.stack.push(Value::Nil); return Ok(CallableOutcome::Handled); }
                        };
                        (cls, n, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                let m = match snapshot.or_else(|| self.lookup_method_uncached(&class, m_name_id)) {
                    Some(m) => m,
                    None => { self.stack.push(Value::Nil); return Ok(CallableOutcome::Handled); }
                };
                // Builtin Methods carry their own source_location
                // label (e.g. `"<internal:kernel>"`) rather than a
                // real proto's filename. The proto_idx on a builtin
                // is a placeholder; reading `self.protos[0].filename`
                // would surface an unrelated file.
                if let Some(meta) = &m.builtin {
                    // `None` source_label → nil. CRuby's behavior
                    // for some C-defined methods (e.g.
                    // BasicObject's __id__).
                    let Some(label) = meta.source_label else {
                        self.stack.push(Value::Nil);
                        return Ok(CallableOutcome::Handled);
                    };
                    let filename_str = Value::new_str(label.to_string());
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::Array(vec![filename_str, Value::Int(meta.source_line)]));
                    self.stack.push(Value::Array(id));
                    return Ok(CallableOutcome::Handled);
                }
                let proto = &self.protos[m.proto_idx];
                let filename = proto.filename.clone();
                let first_offset = proto.op_spans.first().map(|s| s.byte_offset).unwrap_or(0);
                let line: u32 = self.sources.get(&*filename)
                    .map(|src| crate::error::line_col(src, first_offset).0)
                    .unwrap_or(0);
                let filename_str = Value::new_str(filename.to_string());
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(vec![filename_str, Value::Int(line as i64)]));
                self.stack.push(Value::Array(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m.owner` — the class that defined the resolved Method
        // (CRuby's `Method#owner` / `UnboundMethod#owner`). Walks
        // the ancestor chain to find where the method actually
        // lives; falls back to the captured class for builtins
        // (whose primitive_call backing has no Method record).
        //
        // `m.receiver` — the captured recv on a BoundMethod.
        // UnboundMethod#receiver raises NoMethodError, matching
        // CRuby (it has no receiver to give).
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && matches!(name, "owner" | "receiver") && args.is_empty() {
                if name == "receiver" {
                    return match &recv {
                        Value::BoundMethod(bid) => {
                            let (r, _) = self.heap.bound_method(*bid);
                            let r = r.clone();
                            self.stack.push(r);
                            Ok(CallableOutcome::Handled)
                        }
                        Value::UnboundMethod(_) => Err(self.trap(RubyError::NoMethodError {
                            kind: crate::error::NoMethodErrorKind::Missing,
                            method: "receiver".into(),
                            recv_type: std::borrow::Cow::Borrowed("UnboundMethod"),
                        })),
                        _ => unreachable!(),
                    };
                }
                // owner: resolve Method through snapshot (or live
                // lookup as fallback) and prefer its
                // `defining_class.upgrade()` over the captured
                // class.
                let (cap_class, m_name_id, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let snap = snap.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, n, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                let owner = match snapshot.or_else(|| self.lookup_method_uncached(&cap_class, m_name_id)) {
                    Some(m) => m.defining_class.as_ref()
                        .and_then(|w| w.upgrade())
                        .unwrap_or_else(|| cap_class.clone()),
                    None => cap_class.clone(),
                };
                self.stack.push(Value::Class(owner));
                return Ok(CallableOutcome::Handled);
            }
        // `m.arity` / `m.parameters` — Method introspection. Walks
        // the captured class chain to find the user-defined Method;
        // if absent (builtin / primitive_call backed), returns
        // CRuby's "fully varadic" signature: arity = -1,
        // parameters = `[[:rest]]`. Same shape for BoundMethod and
        // UnboundMethod.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && matches!(name, "arity" | "parameters") && args.is_empty() {
                let (class, m_name_id, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (bm_recv, nid, snap) = {
                            let (r, n, snap) = self.heap.bound_method_full(*bid);
                            (r.clone(), n, snap.clone())
                        };
                        let cls = match self.class_of(&bm_recv) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, nid, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                // Prefer the snapshot Method — survives a later
                // remove_method that strips the live entry.
                let m_opt = snapshot.or_else(|| self.lookup_method_uncached(&class, m_name_id));
                let (arity, params_info) = match m_opt {
                    // Builtin Methods (synthesised on Kernel etc.)
                    // carry their introspection metadata directly —
                    // their `proto_idx` is a placeholder. Read from
                    // `builtin` before falling back to the
                    // proto-derived path.
                    Some(ref m) if m.builtin.is_some() => {
                        let meta = m.builtin.as_ref().unwrap();
                        (meta.arity, meta.parameters.clone())
                    }
                    Some(m) => {
                        let proto = &self.protos[m.proto_idx];
                        // Shared `proto_arity` helper carries the
                        // CRuby formula (required-kw bumping,
                        // block-param exclusion, etc.). NOTE:
                        // `Proc#arity` does NOT share this helper
                        // — blocks store rest info on
                        // `BlockHandle`, not on the Proto, so the
                        // block intrinsic arm above computes
                        // arity from the handle directly.
                        let arity = self.proto_arity(m.proto_idx);
                        // Other counts still needed for the
                        // `parameters` build below.
                        let n_req_pos = proto.n_required_positional as usize;
                        let rest_count = proto.rest_param.is_some() as usize;
                        let kw_count = proto.kw_param_defaults.len();
                        let kw_rest_count = proto.kw_rest_param.is_some() as usize;
                        let block_count = proto.block_param.is_some() as usize;
                        let positional_total = proto.params.len()
                            .saturating_sub(rest_count + kw_count + kw_rest_count + block_count);
                        let mut params: Vec<(&'static str, Option<String>)> = Vec::new();
                        for i in 0..n_req_pos {
                            params.push(("req", Some(proto.params[i].clone())));
                        }
                        for i in n_req_pos..positional_total {
                            params.push(("opt", Some(proto.params[i].clone())));
                        }
                        if let Some(rname) = &proto.rest_param {
                            let n = if rname.is_empty() { None } else { Some(rname.clone()) };
                            params.push(("rest", n));
                        }
                        let kw_name_start = positional_total + rest_count;
                        for (i, default) in proto.kw_param_defaults.iter().enumerate() {
                            let kind = if default.is_none() { "keyreq" } else { "key" };
                            params.push((kind, Some(proto.params[kw_name_start + i].clone())));
                        }
                        if let Some(krname) = &proto.kw_rest_param {
                            let n = if krname == "__kw_rest_anon" { None } else { Some(krname.clone()) };
                            params.push(("keyrest", n));
                        }
                        if let Some(bname) = &proto.block_param {
                            // For anonymous `def foo(&)` the sentinel
                            // `"&"` round-trips here as the Symbol
                            // `:&` — matches CRuby exactly, which
                            // also surfaces the anonymous block as
                            // `[[:block, :&]]` (the literal `&` is a
                            // legal Symbol payload, just an unusual
                            // one). No anonymization needed: passing
                            // the sentinel through gives byte-for-
                            // byte parity. NOT analogous to the
                            // `__kw_rest_anon` case above, which
                            // CRuby DOES report as nameless.
                            params.push(("block", Some(bname.clone())));
                        }
                        (arity, params)
                    }
                    None => (-1i64, vec![("rest", None)]),
                };
                if name == "arity" {
                    self.stack.push(Value::Int(arity));
                    return Ok(CallableOutcome::Handled);
                }
                // Build [[kind_sym, name_sym?], ...] array. Anonymous
                // rest / kw_rest yields a single-element pair, matching
                // CRuby's `[[:rest]]` / `[[:keyrest]]`.
                //
                // PinGuard across the whole loop so the inner-pair
                // ObjIds in `outer` survive every maybe_gc — without
                // this, under STRESS_GC each iteration's pair slot
                // gets swept (no GC root: `outer` is a Rust-local
                // Vec), the next alloc reuses it, and the final
                // `heap.alloc(HeapObj::Array(outer))` can land on the
                // same recycled slot — yielding a self-referencing
                // Array whose `.inspect` recurses to stack overflow.
                let mut g = crate::vm::PinGuard::new(self);
                let mut outer: Vec<Value> = Vec::with_capacity(params_info.len());
                for (kind, name_opt) in params_info {
                    let kind_sym = g.vm.interner.intern(kind);
                    let mut pair = vec![Value::Sym(kind_sym)];
                    if let Some(n) = name_opt {
                        let nsym = g.vm.interner.intern(&n);
                        pair.push(Value::Sym(nsym));
                    }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pid = g.vm.heap.alloc(HeapObj::Array(pair));
                    g.pin(Value::Array(pid));
                    outer.push(Value::Array(pid));
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let aid = g.vm.heap.alloc(HeapObj::Array(outer));
                g.vm.stack.push(Value::Array(aid));
                return Ok(CallableOutcome::Handled);
            }
        if let Value::BoundMethod(bid) = &recv
            && matches!(name, "call" | "[]" | "()") {
                let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                    HeapObj::BoundMethod { recv, name_id, method } => {
                        (recv.clone(), *name_id, method.clone())
                    }
                    _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
                };
                // Snapshot fast path: invoke the captured Method
                // directly so a `remove_method` on the captured
                // class between capture and call doesn't break
                // `bm.call` (CRuby parity, matches the bind_call
                // path).
                if let Some(m) = bm_method {
                    self.invoke_method(m, bm_recv, args)?;
                    return Ok(CallableOutcome::Handled);
                }
                let argc = args.len();
                self.stack.push(bm_recv);
                for a in args {
                    self.stack.push(a);
                }
                self.do_call(
                    bm_name_id, argc,
                    /* no_recv = */ false,
                    /* cache_id = */ u16::MAX,
                )?;
                return Ok(CallableOutcome::Handled);
            }
        // No arm matched; return args + recv intact for the caller
        // to continue dispatch.
        Ok(CallableOutcome::NotHandled { args, recv })
    }
    
    /// Class-receiver intrinsics — `cls.[]` (Hash[]) / `cls.new` /
    /// `cls.allocate` / `cls.include` / `cls.prepend` / `cls.extend`
    /// / `cls.private` / `cls.public` / `cls.protected` /
    /// `cls.name` / `cls.superclass` / `cls.method_defined?`.
    ///
    /// Returns [`ClassOutcome::Handled`] if one of the arms
    /// fired; caller `return`s `Ok(())`. Returns
    /// [`ClassOutcome::NotHandled { args, recv }`] if no arm
    /// matched; caller continues with the rest of dispatch.
    ///
    /// Extracted from `do_call` per the #152 research's
    /// Candidate E recommendation, #192 commit 4/5. The
    /// `Class.new` arm integrates with `cext_alloc_func` +
    /// `with_vm_ptr_set` (R1 from the research). Existing
    /// code pre-clones `cls.name` to a String before entering
    /// the cext closure, so no `cls`-borrow conflict surfaces
    /// from the extraction; kept as-is.
    ///
    /// `_name_id` / `_cache_id` are unused today (arms match
    /// on `name: &str`); kept in the signature for forward
    /// compat with future arms that may need them.
    /// Enforce private/protected access rules for an Object
    /// receiver dispatch (explicit-receiver path).
    ///
    /// Private: cannot be invoked with an explicit receiver,
    /// except the modern (CRuby 3.x) `self.foo` form where
    /// `self == recv` by ObjId.
    ///
    /// Protected: caller's `self` class must be an instance of
    /// (or descendant of) the method's *defining* class — CRuby's
    /// rule, not the receiver's class.
    ///
    /// `bypass_visibility` is the `send` / `__send__` one-shot
    /// override consumed by `do_call` before this call.
    fn check_method_visibility(
        &self,
        m: &Method,
        recv: &Value,
        name: &str,
        bypass_visibility: bool,
    ) -> Result<(), Trap> {
        let vis = m.visibility.get();
        let self_recv = matches!(
            (recv, self.frames.last().map(|f| &f.self_val)),
            (Value::Object(rid), Some(Value::Object(sid))) if rid == sid
        );
        if vis == Visibility::Private && !bypass_visibility && !self_recv {
            return Err(self.trap(RubyError::NoMethodError {
                kind: crate::error::NoMethodErrorKind::Private,
                method: name.to_string(),
                recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&recv)),
            }));
        }
        if vis == Visibility::Protected && !bypass_visibility {
            let caller_self = self
                .frames
                .last()
                .map(|f| f.self_val.clone())
                .unwrap_or(Value::Nil);
            let caller_cls = match &caller_self {
                Value::Object(id) => Some(self.heap.class_of(*id)),
                _ => None,
            };
            let defining = m.defining_class.as_ref().and_then(|w| w.upgrade());
            let allowed = match (&caller_cls, &defining) {
                (Some(c), Some(d)) => super::class_is_a(c, d),
                _ => false,
            };
            if !allowed {
                return Err(self.trap(RubyError::NoMethodError {
                    kind: crate::error::NoMethodErrorKind::Protected,
                    method: name.to_string(),
                    recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&recv)),
                }));
            }
        }
        Ok(())
    }

    /// CRuby-shape receiver description for NoMethodError-style
    /// messages. Object instances render as
    /// `"an instance of <ClassName>"` (matches CRuby 3.3+); all
    /// other Value variants fall back to `Value::type_name()`.
    /// Used by the private/protected visibility error sites so
    /// scripts asserting on the message text see the same words
    /// as CRuby. (TRY_RUNS pass-10 layer #5.)
    pub(crate) fn recv_desc_for_error(&self, recv: &Value) -> String {
        match recv {
            Value::Object(id) => {
                // `real_class_of` skips the eigenclass shell.
                // `class_of` would return the singleton class
                // when one has been installed (e.g. via
                // `def obj.foo`), rendering the error as
                // "an instance of #<Class:#<Inner>>" — never
                // what a script wants to see. (Copilot review
                // #291 round 1.)
                //
                // Known gap: CRuby switches *format* when a
                // singleton is installed — it inspects the
                // receiver with its memory address
                // ("for #<Inner:0x000…>") instead of using
                // "an instance of …". That would require us to
                // mirror `Object#inspect` here, including the
                // memory-address suffix. Tier-1 ships the
                // simpler "an instance of <real class>" form;
                // a script that asserts on the inspect-form
                // wording for singleton-bearing receivers
                // sees a known divergence we accept until a
                // real consumer needs it.
                // `try_real_class_of` is the fallible variant
                // so a corrupt `Value::Object(id)` reaching
                // here doesn't panic the host on the failure
                // path — falls back to the generic type tag.
                // (Code-review #291 round 2.)
                match self.heap.try_real_class_of(*id) {
                    Some(cls) => format!("an instance of {}", cls.name),
                    None => recv.type_name().to_string(),
                }
            }
            other => other.type_name().to_string(),
        }
    }

    /// Class-receiver introspection arms — the second Class
    /// cluster deferred from #192 commit 4. Matches when
    /// `recv` is `Value::Class` AND `name` is one of the
    /// `ancestors` / `include?` / `superclass` /
    /// `singleton_class` / `instance_methods` family /
    /// `constants` / `method_defined?` / `undef_method` /
    /// `instance_method` arms. Returns `Ok(true)` when
    /// handled, `Ok(false)` when the receiver isn't a Class
    /// or no arm matched (caller falls through to the
    /// remaining do_call dispatch).
    ///
    /// No cext integration (unlike commit 4's first Class
    /// cluster) — pure runtime introspection. Free of the
    /// R1 borrow-conflict risk that motivated that helper's
    /// pre-cloning discipline.
    fn try_dispatch_class_introspection(
        &mut self,
        name: &str,
        args: &[Value],
        recv: &Value,
    ) -> Result<bool, Trap> {
        let Value::Class(cls_ref) = recv else { return Ok(false); };
        let cls = cls_ref.clone();
        match (name, args) {
            ("ancestors", []) => {
                let chain: Vec<Value> = super::flatten_ancestors(&cls)
                    .into_iter()
                    .map(Value::Class)
                    .collect();
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(chain));
                self.stack.push(Value::Array(id));
                Ok(true)
            }
            ("include?", [Value::Class(m)]) => {
                if !m.is_module {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "wrong argument type Class (expected Module)".to_string(),
                    }));
                }
                let included = super::class_is_a(&cls, m);
                self.stack.push(Value::Bool(included));
                Ok(true)
            }
            ("include?", [other]) => {
                Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "wrong argument type {} (expected Module)",
                        other.type_name(),
                    ),
                }))
            }
            ("superclass", []) => {
                // CRuby: `Module#superclass` raises NoMethodError
                // because modules don't have a superclass chain
                // (Class < Module but Module has no parent slot).
                // BasicObject has no parent and returns nil. User
                // classes return their parent.
                if cls.is_module {
                    // Probe for a user-defined singleton override
                    // first — `def M.superclass; ...; end` (or
                    // `M.singleton_class.prepend(...)`) lets user
                    // code shadow the default raise. Falling through
                    // here lets the normal dispatch chain in
                    // try_dispatch_callable_intrinsics' caller
                    // resolve and invoke the override.
                    let sup_id = self.interner.intern("superclass");
                    if self.lookup_class_singleton_method(&cls, sup_id).is_some() {
                        return Ok(false);
                    }
                    // No override: raise NoMethodError. CRuby
                    // formats this as
                    // "undefined method 'superclass' for module M",
                    // i.e. lowercase "module" + the actual name.
                    // Carry the dynamic name through `recv_type`'s
                    // owned-Cow form so we match CRuby exactly.
                    // Anonymous modules (`Module.new`) have an
                    // empty `cls.name`; CRuby renders these as
                    // `#<Module:0x...>` in the error. We don't
                    // model the object-id placeholder, so use a
                    // stable `"#<Module>"` instead of letting the
                    // message end with a trailing space.
                    let label = if cls.name.is_empty() {
                        "#<Module>".to_string()
                    } else {
                        cls.name.clone()
                    };
                    return Err(self.trap(RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::Missing,
                        method: "superclass".to_string(),
                        recv_type: std::borrow::Cow::Owned(format!("module {}", label)),
                    }));
                }
                let v = match cls.superclass.borrow().clone() {
                    Some(p) => Value::Class(p),
                    None => Value::Nil,
                };
                self.stack.push(v);
                Ok(true)
            }
            // `Class#<` / `<=` / `>` / `>=` — subclass relation. CRuby:
            //   A <  B → true if A is a STRICT descendant of B
            //                 (B appears in A's ancestor chain, A != B)
            //   A <= B → A == B OR A < B
            //   A >  B → B <  A
            //   A >= B → B <= A
            // Unrelated classes return nil (not false!). Wrong-type
            // arg (not a Class/Module) → TypeError. Used by Class#<
            // family in user code; also reachable through tilt
            // fixtures that assert `Subclass < Parent`.
            // Wrong-arity guard — without it, `A.send(:<)` or
            // `A.send(:<, B, C)` would fall through this exact-
            // one-arg arm and surface as NoMethodError. CRuby
            // raises ArgumentError instead.
            ("<" | "<=" | ">" | ">=", args_) if args_.len() != 1 => {
                Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1)", args_.len()),
                }))
            }
            ("<" | "<=" | ">" | ">=", [arg]) => {
                let Value::Class(other) = arg else {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "compared with non class/module".to_string(),
                    }));
                };
                let same = std::rc::Rc::ptr_eq(&cls, other);
                let self_is_desc = !same && super::class_is_a(&cls, other);
                let other_is_desc = !same && super::class_is_a(other, &cls);
                let result = match name {
                    "<"  => if self_is_desc { Value::Bool(true) }
                            else if same || other_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    "<=" => if same || self_is_desc { Value::Bool(true) }
                            else if other_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    ">"  => if other_is_desc { Value::Bool(true) }
                            else if same || self_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    ">=" => if same || other_is_desc { Value::Bool(true) }
                            else if self_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    _ => unreachable!(),
                };
                self.stack.push(result);
                Ok(true)
            }
            // Lazy eigenclass-shell. The shell carries
            // `singleton_target = Some(Weak(cls))`, which the 3
            // method-install paths consult to redirect installs
            // into `cls.singleton_methods` instead of the shell's
            // own `methods` table. Subsequent calls reuse the
            // cached shell so `A.singleton_class.equal?(A.singleton_class)`
            // holds. Layer #23 of TRY_RUNS pass series.
            //
            // KNOWN GAP — introspection on the shell (e.g.
            // `A.singleton_class.instance_methods(false)`,
            // `A.singleton_class.include?(Mod)`,
            // `A.singleton_class.include(Mod)`) operates on the
            // shell's OWN empty tables; redirected installs are
            // visible only via the real class's
            // singleton-method dispatch chain. Sinatra and the
            // mainstream `singleton_class.class_eval` idiom
            // don't probe the shell reflectively, so this is
            // documented as a Tier-1 divergence rather than
            // fixed by mirroring writes into the shell's
            // tables. (Code-review #253 round 1 #4 / #7 —
            // partial decline.)
            ("singleton_class", []) => {
                let view = {
                    let mut slot = cls.singleton_view.borrow_mut();
                    if let Some(existing) = slot.as_ref() {
                        existing.clone()
                    } else {
                        // Point the shell's superclass at the real
                        // class's own superclass so
                        // `A.singleton_class.ancestors.include?(Object)`
                        // and `A.singleton_class.superclass`
                        // both behave reasonably for code that
                        // walks the metaclass chain — matches the
                        // pre-PR Tier-1 stub's effective behavior
                        // (the stub returned the receiver itself,
                        // so `.superclass` was the real class's
                        // superclass). NOT CRuby's exact metaclass
                        // tower (`#<Class:A> < #<Class:Object> <
                        // … < Class`), but a close-enough Tier-1
                        // approximation that doesn't regress the
                        // common idiom. (Code-review #253 round 9
                        // #2.)
                        let shell_superclass = cls.superclass.borrow().clone();
                        let v = std::rc::Rc::new(crate::value::Class {
                            name: format!("#<Class:{}>", cls.name),
                            is_module: false,
                            ivars: std::cell::RefCell::new(HashMap::new()),
                            methods: std::cell::RefCell::new(HashMap::new()),
                            singleton_methods: std::cell::RefCell::new(HashMap::new()),
                            superclass: std::cell::RefCell::new(shell_superclass),
                            includes: std::cell::RefCell::new(Vec::new()),
                            prepends: std::cell::RefCell::new(Vec::new()),
                            singleton_prepends: std::cell::RefCell::new(Vec::new()),
                            singleton_view: std::cell::RefCell::new(None),
                            singleton_target: std::cell::RefCell::new(Some(std::rc::Rc::downgrade(&cls))),
                            class_vars: std::cell::RefCell::new(HashMap::new()),
                            #[cfg(feature = "cext")]
                            cext_alloc_func: std::cell::Cell::new(None),
                        });
                        *slot = Some(v.clone());
                        v
                    }
                };
                self.stack.push(Value::Class(view));
                Ok(true)
            }
            ("instance_methods", args_)
            | ("public_instance_methods", args_)
            | ("private_instance_methods", args_)
            | ("protected_instance_methods", args_)
                if args_.is_empty()
                    || matches!(args_, [Value::Bool(_)]) =>
            {
                use crate::value::Visibility;
                let inherited = !matches!(args_, [Value::Bool(false)]);
                let allow: fn(Visibility) -> bool = match name {
                    "instance_methods" => |v| matches!(v, Visibility::Public | Visibility::Protected),
                    "public_instance_methods" => |v| v == Visibility::Public,
                    "private_instance_methods" => |v| v == Visibility::Private,
                    "protected_instance_methods" => |v| v == Visibility::Protected,
                    _ => unreachable!(),
                };
                let mut sids: Vec<crate::intern::SymId> = Vec::new();
                if inherited {
                    let mut visited: Vec<*const crate::value::Class> = Vec::new();
                    fn walk(
                        c: &std::rc::Rc<crate::value::Class>,
                        allow: fn(Visibility) -> bool,
                        out: &mut Vec<crate::intern::SymId>,
                        visited: &mut Vec<*const crate::value::Class>,
                    ) {
                        let ptr = std::rc::Rc::as_ptr(c);
                        if visited.contains(&ptr) { return; }
                        visited.push(ptr);
                        for (k, m) in c.methods.borrow().iter() {
                            if allow(m.visibility.get()) && !out.contains(k) {
                                out.push(*k);
                            }
                        }
                        for inc in c.includes.borrow().iter() {
                            walk(inc, allow, out, visited);
                        }
                        if let Some(sup) = c.superclass.borrow().clone() {
                            walk(&sup, allow, out, visited);
                        }
                    }
                    walk(&cls, allow, &mut sids, &mut visited);
                } else {
                    for (k, m) in cls.methods.borrow().iter() {
                        if allow(m.visibility.get()) {
                            sids.push(*k);
                        }
                    }
                }
                sids.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
                let elems: Vec<Value> = sids.into_iter().map(Value::Sym).collect();
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(elems));
                self.stack.push(Value::Array(id));
                Ok(true)
            }
            ("constants", args_) if args_.is_empty()
                || matches!(args_, [Value::Bool(_)]) =>
            {
                let mut names: Vec<String> = Vec::new();
                let collect = |prefix: &str, names: &mut Vec<String>| {
                    for k in self.constants.keys() {
                        let s = self.interner.resolve(*k).to_string();
                        if let Some(short) = s.strip_prefix(prefix)
                            && !short.contains("::")
                            && !names.contains(&short.to_string()) {
                            names.push(short.to_string());
                        }
                    }
                };
                let own_prefix = format!("{}::", cls.name);
                collect(&own_prefix, &mut names);
                for inc in cls.includes.borrow().iter() {
                    let inc_prefix = format!("{}::", inc.name);
                    collect(&inc_prefix, &mut names);
                }
                names.sort();
                let elems: Vec<Value> = names.into_iter()
                    .map(|n| Value::Sym(self.interner.intern(&n)))
                    .collect();
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(elems));
                self.stack.push(Value::Array(id));
                Ok(true)
            }
            ("method_defined?", [Value::Sym(sid)])
            | ("method_defined?", [Value::Sym(sid), _]) => {
                let answer = class_method_defined(self, &cls, *sid);
                self.stack.push(Value::Bool(answer));
                Ok(true)
            }
            ("method_defined?", [Value::Str(s)])
            | ("method_defined?", [Value::Str(s), _]) => {
                let sid = self.interner.intern(&s.to_string_lossy());
                let answer = class_method_defined(self, &cls, sid);
                self.stack.push(Value::Bool(answer));
                Ok(true)
            }
            ("undef_method", _) => {
                // Tier 1 no-op. See docs/SUBSET.md.
                self.stack.push(Value::Class(cls));
                Ok(true)
            }
            // `Module#remove_method(name, ...)` — removes the
            // method(s) from THIS class's own methods table. Does
            // NOT walk the superclass chain (that's `undef_method`'s
            // job in CRuby; we route undef as a no-op pending real
            // semantics).
            //
            // Motivating consumer: tilt-2.7.0
            // `lib/tilt/template.rb:490` calls
            // `TOPOBJECT.class_eval { remove_method(method_name) }`
            // after each `evaluate` to wipe the synthesised
            // `__tilt_<id>` entry. With this arm tilt's cleanup
            // path runs to completion.
            //
            // Variadic: CRuby accepts any number of args
            // (`remove_method(:a, :b, :c)`); 0 args is a no-op
            // returning self.
            //
            // CRuby raises NameError on a method not defined on
            // this class, INCLUDING for primitives — verified
            // against CRuby 3.4 that `String.remove_method(:foo)`
            // raises. This diverges from the permissive stance
            // at `instance_method` / `method_defined?` (which DO
            // skip the user-class fence for primitives because
            // probing is benign). `remove_method` is a mutation,
            // not a probe; matching CRuby's strict shape here
            // avoids quiet divergence on a surface that's
            // unlikely to be exercised as a feature-detect.
            ("remove_method", args) if !args.is_empty() => {
                // Iterative: process args left-to-right, removing
                // each in turn. If a later arg is missing (or a
                // TypeError fires), earlier removals stay — CRuby
                // is partial-mutation on this surface (verified
                // against 3.4: `A.remove_method(:x, :nope)`
                // removes `:x` BEFORE raising NameError on
                // `:nope`). Track whether anything was removed
                // so we can bump `method_gen` on the error path
                // too — without that, inline caches would keep
                // returning the stale lookup for the removed
                // method.
                //
                // Per-arg arg-to-SymId resolution: Symbol uses sid
                // directly (no resolve/intern roundtrip + no
                // `max_symbols` check — Symbols are already
                // interned). String goes through `with_str_lossy`
                // so the cap check + intern run on a borrowed
                // &str (zero-alloc on the valid-UTF-8 hot path).
                // Mirrors the established pattern at the
                // `instance_method` String arm.
                //
                // Strict-on-primitive parity: primitives are NOT
                // exempt from the missing-method NameError
                // (unlike `instance_method` / `method_defined?`,
                // which keep their permissive stance because
                // probes are benign feature-detects;
                // `remove_method` is a mutation).
                //
                // `any_removed` lets each error-return path bump
                // `method_gen` so a half-completed variadic call
                // doesn't leave inline caches stale on the
                // already-removed methods.
                let mut any_removed = false;
                for arg in args {
                    let sid: SymId = match arg {
                        Value::Sym(sid) => *sid,
                        Value::Str(s) => match s.with_str_lossy(|raw| -> Result<SymId, Trap> {
                            if let Some(max) = self.max_symbols
                                && !self.interner.contains(raw)
                                && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                            Ok(self.interner.intern(raw))
                        }) {
                            Ok(sid) => sid,
                            Err(trap) => {
                                if any_removed {
                                    self.method_gen = self.method_gen.wrapping_add(1);
                                }
                                return Err(trap);
                            }
                        },
                        other => {
                            let inspected = other.to_inspect(&self.heap, &self.interner);
                            if any_removed {
                                self.method_gen = self.method_gen.wrapping_add(1);
                            }
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!("{} is not a symbol nor a string", inspected),
                            }));
                        }
                    };
                    // Single `remove()` call: HashMap::remove
                    // returns Option so we get presence-check +
                    // mutation in one hash lookup + one
                    // `borrow_mut()`.
                    if cls.methods.borrow_mut().remove(&sid).is_none() {
                        if any_removed {
                            self.method_gen = self.method_gen.wrapping_add(1);
                        }
                        // Resolve name only on the rare missing
                        // path. Free for the common case.
                        let name_for_msg = self.interner.resolve(sid).to_string();
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("method '{}' not defined in {}", name_for_msg, cls.name),
                        }));
                    }
                    any_removed = true;
                }
                // Bump `method_gen` once even for variadic calls —
                // inline caches see a single coarse generation
                // bump rather than per-method invalidation.
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Class(cls));
                Ok(true)
            }
            ("remove_method", _) => {
                // 0-arg form: no-op, return receiver (CRuby parity).
                self.stack.push(Value::Class(cls));
                Ok(true)
            }
            // Arity guard FIRST so wrong-count calls surface as
            // ArgumentError (CRuby check order: arity → type).
            // 0 args / 2+ args both raise here.
            ("instance_method", args) if args.len() != 1 => {
                Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len()
                    ),
                }))
            }
            // 1 arg of a type other than Symbol or String: CRuby
            // raises TypeError "<inspect> is not a symbol nor a
            // string" (the literal wording from
            // rb_mod_instance_method).
            ("instance_method", [other]) if !matches!(other, Value::Sym(_) | Value::Str(_)) => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
            ("instance_method", [Value::Sym(sid)]) => {
                // Snapshot the Method here so the UnboundMethod
                // survives a subsequent `remove_method` between
                // capture and bind/bind_call. Tilt's
                // `compile_template_method` does exactly that —
                // captures, then removes from the class table,
                // then bind_call's the captured handle.
                //
                // Kernel builtin synth check: when the receiver is
                // Kernel and the name matches a registered
                // builtin (`:class`, `:nil?`, `:is_a?`, ...),
                // synthesise a Method carrying reflection metadata
                // (arity/parameters/source_location). Kept off
                // Kernel.methods deliberately so regular dispatch
                // doesn't re-find it; the registry lives only for
                // this introspection surface.
                // User-defined methods on the class table win —
                // reopening Kernel/BasicObject to shadow `class` /
                // `equal?` / etc. should surface that method
                // through reflection, not the synth metadata.
                // Registry is the fallback when the live table
                // misses, and the ancestor-chain walk lets
                // inherited reflection (`User.instance_method(:class)`
                // → Kernel synth via Object→Kernel include chain)
                // work the same as the direct case.
                let snapshot = self.lookup_method_uncached(&cls, *sid)
                    .or_else(|| self.builtin_method_via_ancestor_chain(&cls, *sid));
                if snapshot.is_none() && !is_primitive_class_name(&cls.name) {
                    let mname = self.interner.resolve(*sid).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cls.name),
                    }));
                }
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::UnboundMethod {
                    class: cls.clone(),
                    name_id: *sid,
                    method: snapshot,
                });
                self.stack.push(Value::UnboundMethod(id));
                Ok(true)
            }
            // `instance_method` accepts a String too (CRuby
            // `to_sym`'s it). Tilt-2.7.0 lib/tilt/template.rb:489
            // calls `TOPOBJECT.instance_method(method_name)`
            // where `method_name` is a String synthesised via
            // `"__tilt_#{...}"` interpolation. `with_str_lossy` is
            // Cow-backed: zero-alloc on the valid-UTF-8 hot path.
            // Cap check + intern + lookup all happen inside the
            // closure; the NameError path `format!`s the borrowed
            // `raw` directly. Same parity stance as the
            // `method_defined?` arm above.
            ("instance_method", [Value::Str(s)]) => {
                s.with_str_lossy(|raw| {
                    if let Some(max) = self.max_symbols
                        && !self.interner.contains(raw)
                        && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                    let sid = self.interner.intern(raw);
                    // Same registry consultation as the Symbol-form
                    // arm above — live table first, then ancestor-
                    // chain walk so inherited reflection works.
                    let snapshot = self.lookup_method_uncached(&cls, sid)
                        .or_else(|| self.builtin_method_via_ancestor_chain(&cls, sid));
                    if snapshot.is_none() && !is_primitive_class_name(&cls.name) {
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method '{}' for class '{}'", raw, cls.name),
                        }));
                    }
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::UnboundMethod {
                        class: cls.clone(),
                        name_id: sid,
                        method: snapshot,
                    });
                    self.stack.push(Value::UnboundMethod(id));
                    Ok(true)
                })
            }
            _ => Ok(false),
        }
    }

    fn try_dispatch_class_intrinsics(
        &mut self,
        name: &str,
        name_id: SymId,
        _cache_id: u16,
        args: Vec<Value>,
        recv: Value,
    ) -> Result<ClassOutcome, Trap> {
        // Local SymId for "new" — used by the `cls.new`
        // override arm. Originally derived in the surrounding
        // `do_call` body above the extracted cluster; computed
        // inside the helper now so the cluster is self-
        // contained.
        let new_id = self.interner.intern("new");
    // Singleton-class-shell fence: `A.singleton_class.new` raises
    // TypeError in CRuby ("can't create instance of singleton
    // class"). Without this fence the shell falls into the
    // default `Class.new` allocator at line 2294 and silently
    // allocates a `Value::Object` whose class is the shell —
    // producing an orphan instance whose every method call
    // raises NoMethodError because the shell's method table is
    // empty. Defensive code that `rescue TypeError`s to detect
    // singleton-class misuse would skip; the orphan only
    // surfaces as the confusing downstream NoMethodError.
    // (Code-review #253 round 9 #1.)
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.singleton_target.borrow().is_some()
    {
        return Err(self.trap(RubyError::TypeError {
            msg: "can't create instance of singleton class".into(),
        }));
    }
    // `Hash[...]` class-method constructor. CRuby has three
    // call shapes:
    //   - `Hash[]`               → empty Hash
    //   - `Hash[k1, v1, k2, v2]` → flat-pair form (even arity)
    //   - `Hash[[[k, v], ...]]`  → 1 Array of 2-element pairs
    //   - `Hash[{k => v, ...}]`  → 1 Hash (copy semantics)
    // The flat-pair form is the most common; older gems prefer
    // it over `pairs.to_h`. Without this intercept, `Hash[]`
    // would NoMethodError on Class (no `[]` defined on
    // Value::Class).
    //
    // Odd-arity (k without matching v) is ArgumentError in
    // CRuby; mirror that.
    if name == "[]"
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Hash"
    {
        // GC rooting: `args` came from `self.stack.drain(...)`
        // and is a Rust-local Vec with no GC root, so any heap-
        // shaped element (Array / Hash for the `Hash[[[k,v],...]]`
        // and `Hash[{…}]` shapes) gets swept if `maybe_gc` runs
        // before we finish reading their pairs. Pin every arg
        // across the entire alloc + pair-extract window. Repro
        // pre-fix: `Hash[[[:x, 10], [:y, 20]]]` under STRESS_GC=1
        // tripped `ICE: use-after-free` on the inner-pair walk.
        let mut g = PinGuard::new(self);
        for a in &args { g.pin(a.clone()); }
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let pairs: Vec<(Value, Value)> = if args.len() == 1 {
            match &args[0] {
                Value::Array(aid) => {
                    // `Hash[[[k, v], ...]]`. Each element must be
                    // a 2-element Array; anything else is
                    // ArgumentError in CRuby (`invalid number of
                    // elements (X for 2)`), but we follow the
                    // common shape — non-pair elements are dropped
                    // with TypeError. Stay strict only on the
                    // outer Array shape.
                    let outer = g.vm.heap.array(*aid).clone();
                    let mut out = Vec::with_capacity(outer.len());
                    for elem in outer {
                        if let Value::Array(pair_id) = elem {
                            let pair = g.vm.heap.array(pair_id);
                            if pair.len() == 2 {
                                out.push((pair[0].clone(), pair[1].clone()));
                            } else {
                                return Err(g.vm.trap(RubyError::ArgumentError {
                                    msg: format!("invalid number of elements ({} for 2)", pair.len()),
                                }));
                            }
                        } else {
                            return Err(g.vm.trap(RubyError::TypeError {
                                msg: format!("wrong element type {} (expected array)", elem.type_name()),
                            }));
                        }
                    }
                    out
                }
                Value::Hash(hid) => g.vm.heap.hash(*hid).clone(),
                _ => return Err(g.vm.trap(RubyError::ArgumentError {
                    msg: "odd number of arguments for Hash".into(),
                })),
            }
        } else if args.len().is_multiple_of(2) {
            args.chunks(2).map(|c| (c[0].clone(), c[1].clone())).collect()
        } else {
            return Err(g.vm.trap(RubyError::ArgumentError {
                msg: "odd number of arguments for Hash".into(),
            }));
        };
        let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
        g.vm.stack.push(Value::Hash(hid));
        return Ok(ClassOutcome::Handled);
    }
    // User-defined `def self.new` takes precedence over the
    // built-in allocator AND over the Hash.new / String.new /
    // other built-in class-level intercepts below. CRuby's
    // `Class#new` is a normal Ruby method (allocate +
    // initialize), and reopening any class — built-in or
    // user — to override `self.new` should win. Without this
    // check ahead of the Hash / String special-cases, e.g.
    // `class Hash; def self.new; ...; end; end; Hash.new`
    // silently bypassed the override and returned an empty
    // `{}` from the hardcoded Hash path.
    //
    // The block-form path (`do_call_block`) generally routes
    // user `self.new` overrides through its general
    // Value::Class singleton-method dispatch arm, so most
    // classes don't need a mirrored check there. The one
    // exception is `do_call_block`'s `Hash.new { block }`
    // intercept, which fires before that generic arm — it
    // carries the same singleton pre-check pattern as this
    // one for parity.
    //
    // Documented gap: `def self.new ... super ... end` still
    // hits the allocator via super only if Class's builtin
    // `new` is reachable through super_lookup — which it
    // isn't today. Override-without-super covers the tilt
    // entry-point (and the common DSL builder pattern); the
    // super-into-allocator case is a separable follow-up.
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && let Some(m) = self.lookup_class_singleton_method(cls, new_id) {
        self.invoke_method(m, recv.clone(), args)?;
        return Ok(ClassOutcome::Handled);
    }
    // `String.new` / `String.new(s)` — Tier 1 primitive
    // constructor. Without this intercept the generic
    // `Class.new` allocator below would build a
    // `Value::Object` (Instance with `class = String`), and
    // every String primitive method (`length`, `<<`,
    // `bytesize`, …) would `NoMethodError` because they
    // pattern-match on `Value::Str`, not `Value::Object`.
    //
    // CRuby supports `String.new(s, encoding: …, capacity: …)`;
    // the encoding model is Tier 3 (ADR 0017), so we cover
    // only the positional `s` argument here. Anything else
    // raises ArgumentError.
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "String"
    {
        match args.as_slice() {
            [] => {
                self.stack.push(Value::new_str(""));
                return Ok(ClassOutcome::Handled);
            }
            [Value::Str(s)] => {
                // Fresh, mutable copy — CRuby's `String.new(s)`
                // returns an unfrozen clone even if `s` was
                // frozen.
                let copy = s.to_string_lossy();
                self.stack.push(Value::new_str(copy));
                return Ok(ClassOutcome::Handled);
            }
            [other] => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        other.type_name(),
                    ),
                }));
            }
            _ => {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0..1)",
                        args.len(),
                    ),
                }));
            }
        }
    }
    // `Module.new` (no block) — returns a fresh anonymous
    // Module. Empty name is the sentinel for "anonymous"
    // that `Module#name` consults to return `nil`; `to_s` /
    // `inspect` render `"#<Module>"` instead. The block-form
    // `Module.new { |m| ... }` evaluates the block as the
    // module body and lives in `do_call_block` — same shape
    // as the existing `Hash.new` / `class_eval` intercepts.
    //
    // Documented divergence (NOT addressed here): CRuby
    // assigns the module's name on first constant write
    // (`M = Module.new` → `M.name == "M"`). rubyrs leaves
    // the name empty until a future StoreConst hook lands;
    // most real-world uses (`include` an anonymous helper)
    // don't depend on the name-promote behaviour.
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Module"
    {
        if !args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len(),
                ),
            }));
        }
        let m = std::rc::Rc::new(Class {
            name: String::new(),
            is_module: true,
            ivars: std::cell::RefCell::new(HashMap::new()),
            methods: std::cell::RefCell::new(HashMap::new()),
            singleton_methods: std::cell::RefCell::new(HashMap::new()),
            superclass: std::cell::RefCell::new(None),
            includes: std::cell::RefCell::new(Vec::new()),
            prepends: std::cell::RefCell::new(Vec::new()),
            singleton_prepends: std::cell::RefCell::new(Vec::new()),
            singleton_view: std::cell::RefCell::new(None),
            singleton_target: std::cell::RefCell::new(None),
            class_vars: std::cell::RefCell::new(HashMap::new()),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        });
        self.stack.push(Value::Class(m));
        return Ok(ClassOutcome::Handled);
    }
    // `Module#define_method` no-block path. The block-form
    // intrinsic lives in `do_call_block`; this arm handles the
    // no-block shapes that CRuby validates here, ordered to
    // match CRuby's actual validation sequence (arity first,
    // then missing-block). The 2-arg Proc/UnboundMethod form
    // (`define_method(:foo, proc { … })`) is NOT yet supported
    // in rubyrs Tier-1 — it falls through to standard dispatch
    // and surfaces as NoMethodError so a caller that hits the
    // unsupported shape gets a clear "not implemented" signal.
    // A future PR landing the 2-arg form should swap that
    // fall-through for the install arm.
    // (PR #245 Copilot round 2 #2 + round 4 #1 + round 5 #1.)
    if name == "define_method"
        && let Value::Class(cls) = &recv
    {
        // Same precedence rule as the block-form arm — user
        // override wins regardless of arity (let the override
        // own its own validation).
        if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
            let recv_val = Value::Class(cls.clone());
            self.invoke_method(m, recv_val, args)?;
            return Ok(ClassOutcome::Handled);
        }
        // CRuby validates arity before the missing-block check:
        //   0 args      → ArgumentError "wrong number of arguments
        //                 (given 0, expected 1..2)"
        //   1 arg, none → ArgumentError "tried to create Proc
        //                 object without a block"
        //   2 args      → Proc/UnboundMethod install form, NOT yet
        //                 supported in rubyrs Tier-1; raise an
        //                 ArgumentError that names the actual
        //                 cause. (code-review #245 round 7 #3 —
        //                 previously fell through to NoMethodError,
        //                 which misleadingly claimed the method
        //                 was undefined when dispatch actually
        //                 reached this arm. NotImplementedError
        //                 would be more semantically accurate but
        //                 RubyError lacks a registered variant for
        //                 it, and Uncaught is by design not
        //                 catchable by `rescue` — ArgumentError
        //                 with an explicit "not yet supported"
        //                 message is the best catchable shape.)
        //   3+ args     → ArgumentError "wrong number of arguments
        //                 (given N, expected 1..2)"
        match args.len() {
            0 => return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..2)".into(),
            })),
            1 => return Err(self.trap(RubyError::ArgumentError {
                msg: "tried to create Proc object without a block".into(),
            })),
            2 => return Err(self.trap(RubyError::ArgumentError {
                msg: "the 2-arg Proc/UnboundMethod form of `Module#define_method` is not yet supported by rubyrs Tier-1".into(),
            })),
            n => return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1..2)", n),
            })),
        }
    }
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Hash"
    {
        // `Hash.new` without a block. CRuby shapes:
        //   - 0 args: empty Hash, no default
        //   - 1 arg:  empty Hash with scalar default; missing-
        //             key lookup returns this value as-is (not
        //             cached into the Hash).
        //   - 2+ args: ArgumentError
        // The block-form (`Hash.new { |h, k| ... }`) routes
        // through `do_call_block` and has its own intercept
        // (which raises ArgumentError when a scalar default is
        // also given — CRuby refuses both at once).
        if args.len() > 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0..1)", args.len()),
            }));
        }
        let default = args.first().cloned();
        // Pin the default across maybe_gc — if it's a heap
        // value (Array / Hash / String), it could be a
        // temporary on its way to becoming the default and
        // would otherwise be unrooted between args.first() and
        // hash_set_default_value below.
        let mut g = PinGuard::new(self);
        if let Some(v) = &default { g.pin(v.clone()); }
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
        if default.is_some() {
            g.vm.heap.hash_set_default_value(hid, default);
        }
        g.vm.stack.push(Value::Hash(hid));
        return Ok(ClassOutcome::Handled);
    }
    // `Class#allocate` user-singleton override — CRuby allows
    // `def self.allocate` to replace the built-in allocator (used
    // by Marshal / dup / ORM hydration hooks). Mirrors the
    // `def self.new` pre-check at line 1053. Must fire BEFORE the
    // builtin allocate arm below or the user override is silently
    // shadowed; do_call_block has the same precedence (its
    // generic singleton check at ~4601 runs before its allocate
    // arm). PR #181 follow-up: code-review caught the asymmetry.
    if name == "allocate"
        && let Value::Class(cls) = &recv
        && let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
        self.invoke_method(m, recv.clone(), args)?;
        return Ok(ClassOutcome::Handled);
    }
    // `Class#allocate` — bare-instance allocator without calling
    // `initialize`. Used by frameworks for unmarshalling / dup /
    // clone / ORM hydration, and by the TRY_RUNS pass-7 probe's
    // `ERB.new` stub (layer #4). Sits before the `new` arm so the
    // class-receiver path is uniform.
    //
    // Semantics:
    //   - User classes (`Value::Class` not in the primitive
    //     whitelist): allocate a fresh `HeapObj::Instance` with
    //     the class pointer set, empty ivars, no singleton class.
    //     No `initialize` call.
    //   - Primitive class shells fall into two groups:
    //       * "Truly disallowed" in CRuby — Integer / Float /
    //         Symbol / Regexp / Proc / Method / UnboundMethod /
    //         TrueClass / FalseClass / NilClass / Kernel. CRuby
    //         raises TypeError; rubyrs matches byte-for-byte.
    //       * "Allowed in CRuby" — String / Array / Hash / Range.
    //         CRuby produces a bare instance of the builtin
    //         (empty string / array / hash / Range struct); rubyrs
    //         currently raises TypeError because the heap model
    //         unboxes those values and we don't yet route through
    //         a TypedData allocator. Documented as a KNOWN GAP
    //         below; the comment used to claim CRuby parity here
    //         which was wrong (PR #181 review round 4 Copilot
    //         comment #2).
    //     Either way: zero Instance slot to populate, so the
    //     bare-allocator path can't run for any primitive shell.
    //   - Zero args; any positional arg raises ArgumentError
    //     with the standard "wrong number of arguments" shape.
    //
    // KNOWN GAP: `cext_alloc_func` (set by
    // `rb_define_alloc_func`) is currently NOT routed through
    // this arm. The `new` arm below DOES route through it (so a
    // cext `Foo.new` produces a TypedData-wrapped Object), but
    // `Foo.allocate` here falls back to the default bare
    // Instance. For a cext whose initialize-after-allocate
    // relies on the alloc_func having wrapped its C struct, the
    // separation of allocate-vs-new becomes visible. No caller
    // surfaced today (pass-7 probe layer #4 only needs the
    // bare Instance path). Routed via a follow-up if a cext
    // surfaces the need; tracked as a comment so a future
    // reader doesn't think the bare-allocate is an oversight.
    // String-compare on the already-resolved `name` instead of
    // interning "allocate" each call (PR #181 review round 3
    // Copilot comment #1). Avoids both the per-call hash lookup
    // on a hot dispatch path and the latent edge case where
    // unconditional `intern()` could grow the symbol table
    // outside the existing `Config::max_symbols` accounting
    // points.
    if name == "allocate"
        && let Value::Class(cls) = &recv {
        if !args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
            }));
        }
        // Eigenclass-shell fence — CRuby:
        // `A.singleton_class.allocate` raises TypeError ("can't
        // create instance of singleton class"). Without this the
        // shell falls into the bare-instance allocator below and
        // produces an orphan. (Code-review #253 round 9 #1.)
        if cls.singleton_target.borrow().is_some() {
            return Err(self.trap(RubyError::TypeError {
                msg: "can't create instance of singleton class".into(),
            }));
        }
        // Module / Class shells are NOT user classes — a real
        // CRuby raises NoMethodError ("undefined method
        // 'allocate' for ...Module/Class...") on Module-flavored
        // receivers; we approximate with the same TypeError
        // surface as the primitive shells so the call site sees
        // a clean failure instead of a bogus bare-Instance whose
        // `class` says Module but which can't behave like one
        // (PR #181 review #1 — Copilot flagged this gap).
        // KNOWN GAP: `Class.allocate` itself in CRuby DOES
        // succeed (returns a new anonymous Class). We block it
        // here for safety until a proper Class/Module allocator
        // lands; the only caller surfaced today (ERB stub) wants
        // an Instance, not a Class.
        if cls.is_module
            || cls.name == "Module"
            || cls.name == "Class"
            || is_primitive_class_name(&cls.name)
        {
            // Anonymous Module / Class shells have an empty
            // `cls.name`; without a fallback the message would
            // read "allocator undefined for " (trailing space,
            // no class hint). Pick "Module" vs "Class" by the
            // `is_module` flag so the surface is actionable
            // (PR #181 review round 3 Copilot comment #2).
            let display = if cls.name.is_empty() {
                if cls.is_module { "Module" } else { "Class" }
            } else {
                &cls.name
            };
            return Err(self.trap(RubyError::TypeError {
                msg: format!("allocator undefined for {}", display),
            }));
        }
        let obj = self.alloc_default_instance(cls)?;
        self.stack.push(obj);
        return Ok(ClassOutcome::Handled);
    }
    if name_id == new_id
        && let Value::Class(cls) = &recv {
            // L3-F: cext-registered allocator path. When the class
            // came from rb_define_class_under AND the cext called
            // rb_define_alloc_func on it, route the allocation
            // through that callback (typically wraps a malloc'd C
            // struct in TypedData) instead of producing a bare
            // Instance. Without this, every TypedData_Get_Struct in
            // the cext's instance methods fails because `self` is a
            // plain Instance, not a TypedData slot.
            // Outer PinGuard covers BOTH the allocator call and
            // the subsequent initialize. cext_dispatch can trigger
            // maybe_gc (TypedData wrap, result translation,
            // nested rb_funcall); args + obj live only as Rust
            // locals here and would be swept otherwise (PR #50
            // review #1 + #3 — same shape as the Integer#times
            // PinGuard fix in L3-D).
            let mut g = PinGuard::new(self);
            for a in &args { g.pin(a.clone()); }
            // Default Instance allocator — used by every branch of
            // the cext-selection cascade below that doesn't go
            // through `rb_define_alloc_func`. Delegates to
            // `Vm::alloc_default_instance` so this path and the
            // `Class#allocate` arm above can't drift on
            // GC/rooting/allocation behavior (PR #181 review #2).
            let alloc_instance = |g: &mut PinGuard, cls: &Rc<Class>| -> Result<Value, Trap> {
                g.vm.alloc_default_instance(cls)
            };
            // Allocator selection. With `cext`, the class may carry
            // an `rb_define_alloc_func`-registered allocator that
            // must run instead of the default Instance allocation.
            // Without `cext`, there is no path that could set such
            // a function, so we collapse to the default allocator
            // unconditionally. Splitting the whole expression by
            // cfg (instead of the previous `Option<()>` sentinel
            // trick) keeps both arms well-typed and removes a
            // brittle `unreachable!()` site that any future
            // refactor inside the cfg arm could turn into a real
            // panic.
            #[cfg(feature = "cext")]
            let obj = if let Some(alloc_func) = cls.cext_alloc_func.get() {
                #[cfg(not(target_os = "wasi"))]
                {
                    // arity=0 (self-only) is the alloc_func ABI:
                    // VALUE allocate(VALUE klass). CURRENT_VM_PTR
                    // must be set so the cext can rb_funcall back
                    // and rb_data_typed_object_wrap can locate
                    // the Vm to allocate on its heap.
                    let class_name = cls.name.clone();
                    let qualified = format!("{}::allocate", class_name);
                    let vm_ptr: *mut Vm = g.vm;
                    let raw = super::cext::with_vm_ptr_set(vm_ptr, || {
                        super::cext::cext_dispatch(
                            &qualified,
                            alloc_func,
                            0,
                            &[],
                            super::cext::CextSelfHandle::Class(&class_name),
                        )
                    })?;
                    // PR #50 review #2: validate that the cext
                    // honored the rb_define_alloc_func contract.
                    // CRuby's allocator must return an Object
                    // (typically TypedData_Wrap_Struct'd); if a
                    // buggy cext returns Nil / a Class / an Int
                    // and we silently proceed, `initialize` is
                    // called on something that's not an instance,
                    // and instance-method dispatch later fails
                    // in a way that's hard to trace back to the
                    // allocator. Trap immediately with TypeError.
                    match &raw {
                        Value::Object(_) => raw,
                        other => {
                            let msg = format!(
                                "allocator function for {} must return an Object, got {}",
                                class_name,
                                other.type_name()
                            );
                            return Err(g.vm.trap(RubyError::TypeError { msg }));
                        }
                    }
                }
                #[cfg(target_os = "wasi")]
                {
                    // wasi: cext path is stubbed; fall back to
                    // plain Instance allocation. The `alloc_func`
                    // from the if-let binding is unused on this
                    // target (no cext_dispatch to forward it to);
                    // marker reference keeps -D warnings happy.
                    let _ = alloc_func;
                    alloc_instance(&mut g, cls)?
                }
            } else {
                alloc_instance(&mut g, cls)?
            };
            #[cfg(not(feature = "cext"))]
            let obj = {
                // No cext_alloc_func field exists in this build;
                // the class always allocates a plain Instance.
                alloc_instance(&mut g, cls)?
            };
            // Pin the freshly-allocated obj across initialize so
            // a maybe_gc inside the (cext-defined or Ruby-defined)
            // initialize doesn't sweep it.
            g.pin(obj.clone());
            let init_id = g.vm.interner.intern("initialize");
            let ruby_init = g.vm.lookup_method_uncached(cls, init_id);
            if let Some(m) = ruby_init {
                // Ruby-defined initialize takes precedence.
                // Drop the guard before invoke_method (which
                // needs &mut self uncontested); the pinned
                // entries survive only the alloc step — by this
                // point obj/args are already on Rust locals that
                // invoke_method propagates.
                drop(g);
                self.invoke_method(m, obj.clone(), args)?;
                self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
            } else {
                // L3-F + L3-H: cext-defined initialize (registered
                // via rb_define_method) lives in
                // cext_instance_methods. Dispatch through the
                // existing instance-method path if present — this
                // picks up arity validation and rb_raise handling
                // for free. Both fixed arity 0..=5 AND variadic
                // arity -1 are now dispatchable (L3-H setjmp shim
                // supports case -1); the filter below mirrors
                // cext_dispatch's accepted-arities rule.
                #[cfg(all(feature = "cext", not(target_os = "wasi")))]
                {
                    // PR #60 review #10: don't silently skip
                    // initialize on arity mismatch — that
                    // diverges from Ruby semantics
                    // (`Klass.new` must raise ArgumentError if
                    // the args don't fit initialize). Only
                    // filter on whether the arity is
                    // dispatchable by the setjmp shim at all
                    // ({-1} ∪ 0..=5); cext_dispatch then
                    // validates argc against arity for fixed
                    // cases and raises ArgumentError on a
                    // mismatch.
                    let cext_init_reg = g.vm.cext_instance_methods
                        .get(cls.name.as_str())
                        .and_then(|t| t.get(&init_id).cloned())
                        .filter(|reg| reg.arity == -1 || (0..=5).contains(&reg.arity));
                    if let Some(reg) = cext_init_reg {
                        let qualified = reg.qualified_name.clone();
                        let func = reg.func;
                        let arity = reg.arity;
                        let obj_clone = obj.clone();
                        let args_ref = args.clone();
                        let vm_ptr: *mut Vm = g.vm;
                        super::cext::with_vm_ptr_set(vm_ptr, || {
                            super::cext::cext_dispatch(
                                &qualified, func, arity, &args_ref,
                                super::cext::CextSelfHandle::Object(obj_clone),
                            )
                        })?;
                    }
                }
                drop(g);
                self.stack.push(obj);
            }
            return Ok(ClassOutcome::Handled);
        }
        // No class-arm matched; return args + recv intact.
        Ok(ClassOutcome::NotHandled { args, recv })
    }

        /// `Op::CallKw*` entry — the compiler emits this for call
        /// sites whose trailing arg came from `KeywordHashNode`
        /// (`foo(k: v)` sugar). Peek at the trailing Hash on the
        /// stack; if the call targets a primitive that consumes
        /// the kwarg (currently only `Integer#round(half:)` /
        /// `Float#round(half:)`), dispatch the kwarg-aware path
        /// directly. Otherwise fall through to `do_call`, which
        /// continues to treat the trailing Hash as a positional
        /// arg — preserves today's behaviour for user-defined
        /// methods (whose `invoke_method` already pops the Hash
        /// when the proto declares kw_params) and for primitives
        /// that genuinely take a positional Hash.
        pub(crate) fn do_call_kw(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
            // Only `round` is kwarg-aware today, AND only for
            // Int/Float receivers with a supported arg shape.
            // Every other shape — user-defined `C#round(half:)`,
            // 2+ positional args, non-Integer precision, BigInt
            // receiver — must fall back to `do_call` so the
            // existing primitive arms (arity ArgumentError, TypeError
            // for non-Integer precision) AND user-method dispatch
            // still fire. The trailing Hash travels as positional in
            // that path, identical to pre-CallKw behaviour.
            // SymId compare instead of resolving + cloning the
            // name on every CallKw dispatch — the `interner.intern`
            // is amortised across the run (same id returned for the
            // canonical "round" string), so a single == lookup
            // beats a per-call heap allocation. Same pattern below
            // for the `:half` key probe.
            let round_id = self.interner.intern("round");
            if name_id != round_id {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            // Peek receiver + trailing arg WITHOUT disturbing the
            // stack — the fallback `do_call` needs the stack intact.
            if argc == 0 {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            let stack_len = self.stack.len();
            let trailing = self.stack[stack_len - 1].clone();
            let Value::Hash(hash_id) = trailing else {
                return self.do_call(name_id, argc, no_recv, cache_id);
            };
            // Receiver position: if `no_recv` it's the frame self;
            // else it's stack[stack_len - argc - 1].
            let recv_peek = if no_recv {
                self.frames.last().expect("ICE: do_call_kw no frames").self_val.clone()
            } else {
                if stack_len < argc + 1 {
                    return self.do_call(name_id, argc, no_recv, cache_id);
                }
                self.stack[stack_len - argc - 1].clone()
            };
            if !matches!(recv_peek, Value::Int(_) | Value::Float(_)) {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            // Positional arg shape — only `[]` (no precision) and
            // `[Int]` (single Integer precision) are supported by
            // the kwarg helpers. Anything else (arity > 1,
            // non-Integer precision, BigInt precision) is left to
            // the regular round arm in numeric.rs which has the
            // existing ArgumentError / TypeError / BigInt guards.
            let positional_argc = argc - 1; // exclude the kwargs Hash
            if positional_argc > 1 {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            if positional_argc == 1 {
                let precision = &self.stack[stack_len - 2];
                if !matches!(precision, Value::Int(_)) {
                    return self.do_call(name_id, argc, no_recv, cache_id);
                }
            }
            // Resolve the :half kwarg. CRuby raises
            // `ArgumentError: unknown keyword: :foo` for unknown
            // keys, `ArgumentError: invalid rounding mode: foo`
            // for unknown values.
            let half_sym = self.interner.intern("half");
            let pairs: Vec<(Value, Value)> = self.heap.hash(hash_id).clone();
            let mut mode = crate::vm::numeric::HalfMode::Up;
            for (k, v) in &pairs {
                match k {
                    Value::Sym(s) if *s == half_sym => {
                        // Mode resolution without per-dispatch allocation:
                        // Symbol values match against the canonical SymId
                        // (pre-interned once before the loop); String
                        // values use `with_str_lossy` so the comparison
                        // runs against borrowed `&str` instead of an
                        // owned `String`. Non-Sym/Str values surface a
                        // CRuby-shape "invalid rounding mode: <inspect>"
                        // — using `to_inspect` instead of the class name
                        // mirrors `Float#round` / `Numeric#round`'s
                        // shape (e.g. `0` / `nil` / `1.5` instead of
                        // `Integer` / `nil` / `Float`).
                        let up_id = self.interner.intern("up");
                        let down_id = self.interner.intern("down");
                        let even_id = self.interner.intern("even");
                        let resolved: Option<crate::vm::numeric::HalfMode> = match v {
                            Value::Sym(vsym) => {
                                if *vsym == up_id { Some(crate::vm::numeric::HalfMode::Up) }
                                else if *vsym == down_id { Some(crate::vm::numeric::HalfMode::Down) }
                                else if *vsym == even_id { Some(crate::vm::numeric::HalfMode::Even) }
                                else { None }
                            }
                            Value::Str(s) => s.with_str_lossy(|t| match t {
                                "up" => Some(crate::vm::numeric::HalfMode::Up),
                                "down" => Some(crate::vm::numeric::HalfMode::Down),
                                "even" => Some(crate::vm::numeric::HalfMode::Even),
                                _ => None,
                            }),
                            _ => {
                                let inspected = v.to_inspect(&self.heap, &self.interner);
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: format!("invalid rounding mode: {}", inspected),
                                }));
                            }
                        };
                        mode = match resolved {
                            Some(m) => m,
                            None => {
                                // For unknown Symbol/String values
                                // CRuby reports the bare name
                                // (`invalid rounding mode: weird`);
                                // for non-Sym/Str values the inspect
                                // shape carries more information
                                // (handled in the outer match arm).
                                let label: String = match v {
                                    Value::Sym(vsym) => self.interner.resolve(*vsym).to_string(),
                                    Value::Str(s) => s.to_string_lossy(),
                                    _ => unreachable!("non-Sym/Str handled by outer arm"),
                                };
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: format!("invalid rounding mode: {}", label),
                                }));
                            }
                        };
                    }
                    Value::Sym(s) => {
                        let key = self.interner.resolve(*s).to_string();
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("unknown keyword: :{}", key),
                        }));
                    }
                    _ => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "non-symbol key in keyword arguments".to_string(),
                        }));
                    }
                }
            }
            // Stack consume — receiver + positional + kwargs Hash.
            // Guards above guarantee shape is one of:
            //   - (Int|Float, [])
            //   - (Int|Float, [Int])
            let _kwargs_hash = self.stack.pop().expect("ICE: kwargs hash");
            let pos_args: Vec<Value> = {
                let split = self.stack.len() - positional_argc;
                self.stack.drain(split..).collect()
            };
            let recv = if no_recv {
                self.frames.last().expect("ICE: do_call_kw no frames").self_val.clone()
            } else {
                self.stack.pop().expect("ICE: do_call_kw recv")
            };
            // i128 overflow → BigInt promotion under bignum, or a
            // RangeError without it (matches CRuby's behaviour for
            // overflow into a number that doesn't fit native int).
            let promote_overflow = |this: &mut Vm, overflow: i128| -> Result<Value, Trap> {
                #[cfg(feature = "bignum")]
                {
                    let b = num_bigint::BigInt::from(overflow);
                    this.bigint_to_value(b)
                }
                #[cfg(not(feature = "bignum"))]
                {
                    let _ = overflow;
                    Err(this.trap(RubyError::RangeError {
                        msg: "rounded result out of i64 range".to_string(),
                    }))
                }
            };
            let result = match (&recv, pos_args.as_slice()) {
                (Value::Int(a), []) => {
                    match crate::vm::numeric::int_round_with_half(*a, 0, mode) {
                        Ok(v) => v,
                        Err(over) => promote_overflow(self, over)?,
                    }
                }
                (Value::Int(a), [Value::Int(n)]) => {
                    match crate::vm::numeric::int_round_with_half(*a, *n, mode) {
                        Ok(v) => v,
                        Err(over) => promote_overflow(self, over)?,
                    }
                }
                (Value::Float(a), []) => {
                    crate::vm::numeric::float_round_with_half(*a, 0, mode)
                        .map_err(|e| self.trap(e))?
                }
                (Value::Float(a), [Value::Int(n)]) => {
                    crate::vm::numeric::float_round_with_half(*a, *n, mode)
                        .map_err(|e| self.trap(e))?
                }
                _ => unreachable!("guards above limit recv+args to Int/Float × [] | [Int]"),
            };
            self.stack.push(result);
            Ok(())
        }
        pub(crate) fn do_call(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        // Consume `bypass_visibility_once` at the dispatch
        // boundary, before any arm runs. A naive consume-at-the-
        // vis-check would leak the flag whenever the dispatch
        // bottoms out without entering the Value::Object arm
        // (e.g. `send(:nonexistent)` on a primitive receiver
        // raises NoMethodError before the Object arm is reached
        // — the flag would survive and silently bypass the next
        // call's vis check).
        let bypass_visibility = self.take_bypass_visibility();
        if no_recv {
            let self_val = self
                .frames
                .last()
                .expect("ICE: do_call(no_recv) with empty frames")
                .self_val
                .clone();
            if matches!(self_val, Value::Nil)
                && !self.host_fns.contains_key(&name_id)
                && let Some(m) = self.lookup_toplevel_method_cache_hit(cache_id)
                && self.try_invoke_fixed_method_from_stack(m, self_val, argc, None)?
            {
                return Ok(());
            }
        }
        // Primitive-receiver fast-path. Runs after
        // `take_bypass_visibility()` above; the helper's doc
        // comment spells out why that's currently safe and what
        // changes if a non-primitive arm is ever added.
        if self.try_fast_primitive(name_id, argc, no_recv) {
            return Ok(());
        }
        let name = self.interner.resolve(name_id).clone();
        if no_recv {
            let self_val = self.frames.last()
                .expect("ICE: do_call(no_recv) with empty frames")
                .self_val.clone();
            let can_try_toplevel_fast_path = matches!(self_val, Value::Nil)
                && !self.host_fns.contains_key(&name_id)
                && !Self::is_builtin_name(&name)
                && !matches!(&*name, "send" | "__send__" | "method" | "__dir__");
            if can_try_toplevel_fast_path
                && let Some(m) = self.lookup_toplevel_method_cached(name_id, cache_id)
                && self.try_invoke_fixed_method_from_stack(m, self_val, argc, None)?
            {
                return Ok(());
            }
        }
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before do_call receiver"))
        };

        if no_recv && self.try_dispatch_no_recv_builtin_or_host(&name, name_id, &args)? {
            return Ok(());
        }
        if no_recv {
            // Bare `send(:foo)` / `__send__(:foo)` — CRuby treats
            // these as `self.send(:foo)`. Resolve target and re-aim
            // through `do_call` with `no_recv = true` so the call
            // routes through the same implicit-self lookup path the
            // bare-call arm uses below. User `def send` on the
            // surrounding self wins for `send` (reserved-name rule
            // applies only to `__send__`); when the lookup finds a
            // user override, skip the recogniser so the normal
            // implicit-self arm below invokes it.
            //
            // The visibility-bypass flag is irrelevant here — the
            // no_recv arm doesn't enforce private/protected (calls
            // with implicit-self are always allowed) — but we still
            // set it for parity with the receiver-form arm, so any
            // helper that later inspects the flag sees a consistent
            // shape.
            // send/__send__ bypass recogniser — unified helper
            // (#192 commit 2/5). NotHandled returns args back so
            // the dispatcher can continue below.
            let args = match self.try_dispatch_send_bypass(&name, name_id, cache_id, args, None) {
                SendBypass::Handled(r) => return r,
                SendBypass::NotHandled { args, .. } => args,
            };
            // Bare `method(:foo)` — implicit-self capture. Same
            // shape as `obj.method(:foo)` (the receiver-form arm
            // below) but the receiver is the surrounding frame's
            // `self_val`. Lets `arr.map(&method(:foo))` work from
            // inside an instance method body without writing
            // `&self.method(:foo)`.
            let self_val = self.frames.last().expect("ICE: do_call with empty frames").self_val.clone();
            if &*name == "method" && args.len() == 1
                && let Value::Sym(bound_name_id) = &args[0] {
                    // Snapshot the Method at capture time so the
                    // BoundMethod survives a later remove_method.
                    // Use `heap.class_of` for Object self so a
                    // singleton method on `self` is captured
                    // (matches dispatch); `Vm::class_of` would
                    // skip the eigenclass and snapshot the real
                    // class's body instead.
                    let snapshot = match &self_val {
                        Value::Object(id) => {
                            let cls = self.heap.class_of(*id);
                            self.lookup_method_uncached(&cls, *bound_name_id)
                        }
                        _ => match self.class_of(&self_val) {
                            Value::Class(cls) => self.lookup_method_uncached(&cls, *bound_name_id),
                            _ => None,
                        },
                    };
                    self.maybe_gc(); // allow: gc-rooting — BoundMethod holds `recv: self_val.clone()` (cloned from `frames.last().self_val`, which stays rooted via `self.frames` for the whole alloc window) and a primitive `SymId`; no unrooted slot at risk.
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::BoundMethod {
                        recv: self_val.clone(),
                        name_id: *bound_name_id,
                        method: snapshot,
                    });
                    self.stack.push(Value::BoundMethod(id));
                    return Ok(());
                }
            if let Value::Object(id) = &self_val {
                let cls = self.heap.class_of(*id);
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                    self.invoke_method(m, self_val.clone(), args)?;
                    return Ok(());
                }
            }
            // `self` is a Class — inside a class singleton method
            // body (`def self.foo; bar; end` or `class << self; def
            // foo; bar; end; end`), a bare call to `bar` should
            // resolve against THIS class's own `singleton_methods`
            // table AND its superclass chain (so `Sub.foo` defined
            // via `def self.foo` is reachable from inside `Sub`'s
            // class methods even when foo lives on `Super`).
            // Without this arm the lookup fell through to
            // toplevel_methods only and produced
            // "undefined method ... for Class" — even though
            // `bar` was sitting right there on `self`.
            //
            // Uses the same `lookup_class_singleton_method` helper
            // as the explicit `cls.foo` dispatch (vm/dispatch.rs
            // ~660), so `self.bar` and bare `bar` resolve
            // identically.
            if let Value::Class(c) = &self_val
                && let Some(m) = self.lookup_class_singleton_method(c, name_id) {
                self.invoke_method(m, self_val.clone(), args)?;
                return Ok(());
            }
            // Bare calls on Class instances inside `class Foo
            // ... end` bodies and `def self.X` singleton methods.
            // Each whitelisted name has a receiver-form arm
            // further down `do_call` (Class.new allocator,
            // Class#name, Class#method_defined?, Class#
            // instance_method, ...). Without this bridge the
            // bare-call branch would fall through to
            // `toplevel_methods` and raise NoMethodError, even
            // though `self.foo` works fine. Vendored msgpack-
            // ruby surfaced two of these:
            //   - `def self.from_msgpack_ext(...); new(...); end`
            //     in timestamp.rb (bare `new`)
            //   - `class Symbol; if method_defined?(:name); ...`
            //     in symbol.rb (bare `method_defined?` inside an
            //     `if`/`else` at class-body top level)
            // Sinatra surfaced more (TRY_RUNS pass 8 layer #8):
            //   - `class Bar < Foo; superclass.class_eval { ... }`
            //     (bare `superclass` inside class body)
            // Push self_val + the original args back onto the
            // stack and re-enter `do_call` with `no_recv=false`
            // so the receiver-form dispatch takes over. Re-entry
            // walks all the explicit-receiver arms in order —
            // for `allocate` this means the dedicated arm with
            // its Module/primitive fences and user-singleton
            // override fires WITH all fences intact (PR #196
            // Copilot review #1 caught that a previous version
            // of this comment claimed `allocate` was omitted
            // "to preserve fences", but the bridge re-entry
            // routes through the dedicated arm, so including
            // it both fixes bare `allocate` AND keeps the
            // fences).
            //
            // Whitelist contract: this set is exactly lookup.rs's
            // `Value::Class(_)` primitive-method respond_to set
            // (see the `Value::Class(cls) =>` arm of
            // `Vm::responds_to`, around the `"allocate"` gate).
            // Keep both in lockstep — `respond_to?(:foo)` true
            // should mean a bare call to `foo` from inside a
            // class body resolves identically to `self.foo`.
            // `allocate` has the same Module fence as respond_to
            // (applied below); the rest of the names apply to
            // all `Value::Class` receivers.
            //
            // `class_eval` / `module_eval` added so a bare call
            // inside a class body (`class C; class_eval(...); end`)
            // reaches the receiver-form dispatch instead of falling
            // through to NoMethodError, mirroring how
            // `self.class_eval(...)` and `respond_to?(:class_eval)`
            // already work.
            //
            // (A future refactor could lift this list to a
            // shared `pub(crate) const &[&str]` consumed by
            // both sites — out of scope for this PR but tracked
            // as a follow-up by Copilot review #1.)
            if let Value::Class(cls) = &self_val {
                let in_set = matches!(&*name,
                    "new" | "name" | "to_s" | "inspect"
                    | "method_defined?" | "instance_method" | "undef_method" | "remove_method"
                    | "superclass" | "ancestors" | "include?"
                    | "instance_methods" | "public_instance_methods"
                    | "private_instance_methods" | "protected_instance_methods"
                    | "constants"
                    | "autoload" | "autoload?" | "const_defined?" | "const_get" | "private_constant" | "public_constant"
                    | "deprecate_constant"
                    | "singleton_class"
                    | "class_eval" | "module_eval"
                    // `define_method` joins the bridge so bare
                    // `define_method(:foo)` inside a class body
                    // (no_recv, NO block) is forwarded to the
                    // Value::Class(cls) recv form, where
                    // `try_dispatch_class_intrinsics` raises the
                    // CRuby-shape `ArgumentError ("tried to create
                    // Proc object without a block")`. The block
                    // form (`define_method(:foo) { … }`) has its
                    // own no_recv handling in `do_call_block` and
                    // does NOT need this bridge. Keeps the
                    // do_call bridge whitelist in lockstep with
                    // lookup.rs's respond_to whitelist (PR #245
                    // Copilot round 2 #1).
                    | "define_method"
                );
                // `allocate` gets the same Module fence as
                // lookup.rs's respond_to gate so bare `allocate`
                // inside a `module Foo; ... end` body falls
                // through to NoMethodError instead of bridging
                // into the dedicated arm and raising TypeError.
                // True lockstep with respond_to: if
                // `m.respond_to?(:allocate)` is false (Modules,
                // the global `Module` shell), bare `allocate`
                // shouldn't dispatch. PR #196 Copilot round 2 #1.
                let allocate_allowed =
                    &*name == "allocate"
                        && !cls.is_module
                        && cls.name != "Module";
                if in_set || allocate_allowed {
                    let argc = args.len();
                    self.stack.push(self_val.clone());
                    for a in args { self.stack.push(a); }
                    // `cache_id = u16::MAX` (sentinel: skip cache
                    // write) — re-entry from a bare-call site
                    // into a receiver-form lookup; the cache
                    // slot was minted for the bare shape and
                    // mustn't be populated with a receiver-form
                    // entry that a future bare retry could
                    // consult. Same pattern as send / send_with_
                    // block re-entries (lines ~464 / ~924, plus
                    // the lib.rs sentinel comment at ~77).
                    return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
                }
            }
            // `__dir__` — returns the directory of the source
            // file the call lexically appears in. CRuby's
            // contract is "canonicalized absolute path" — it
            // calls `File.realpath(__FILE__)` first, so
            // symlinks resolve and `..` segments collapse.
            // We canonicalize the proto's stored filename via
            // `fs::canonicalize` (follows symlinks, fails if
            // the path doesn't exist) and then take its
            // parent; on canonicalize failure (typically when
            // `__dir__` runs from an `eval`'d inline string
            // whose "filename" is a synthetic label like
            // `<inline>`) we fall back to the lexical
            // `Path::parent` of the raw filename. Lets
            // vendored Ruby helpers do
            // `$LOAD_PATH.unshift __dir__` and match what
            // CRuby resolves through symlinked gem-vendor
            // trees.
            if &*name == "__dir__" && args.is_empty() {
                use std::path::Path;
                let fname = self.frames.last()
                    .map(|f| self.protos[f.proto_idx].filename.to_string())
                    .unwrap_or_default();
                // When `allow_filesystem_io` is true, canonicalize
                // for the symlink-resolved parent (matches CRuby);
                // otherwise (sandbox on), skip the canonicalize
                // syscall and return the lexical parent directly —
                // the same shape the existing `Err(_) =>` fallback
                // already produces when canonicalize fails.
                // Empty-parent guard: `Path::new("test.rb").parent()`
                // returns `Some("")` (not None), so a bare unwrap_or
                // wouldn't collapse the empty case to ".". Filter the
                // empty string out alongside None — both mean
                // "no enclosing directory in the lexical path",
                // which `__dir__` reports as ".".
                let lexical_parent = |fname: &str| -> String {
                    Path::new(fname).parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| ".".to_string())
                };
                // Mirrors File.expand_path's mode selection: only
                // touch the host FS (canonicalize → symlink-resolved
                // parent) in the wide-open shape (sandbox on AND no
                // allowlist). Under `allowed_paths: Some(_)`,
                // canonicalize would resolve symlinks anywhere on
                // the host — same info-leak shape File.expand_path
                // closed. Fall back to lexical parent in every
                // other case, matching the Err(_) arm above.
                let wide_open = self.allow_filesystem_io && self.allowed_paths.is_none();
                let dir = if wide_open {
                    match std::fs::canonicalize(&fname) {
                        Ok(real) => real.parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| ".".to_string()),
                        Err(_) => lexical_parent(&fname),
                    }
                } else {
                    lexical_parent(&fname)
                };
                self.stack.push(Value::new_str(dir));
                return Ok(());
            }
            // Mirror the fast-path guard above (`can_try_toplevel_fast_path`
            // around line 345): the toplevel cache slot key
            // (`TOPLEVEL_METHOD_CACHE_KEY`) doesn't carry the
            // name, so the cache-hit fast path
            // (`lookup_toplevel_method_cache_hit`) can't tell a
            // user `def sprintf` from the builtin. Skipping the
            // populator here for builtin names keeps the cache
            // slot empty for those call sites, so the fast path
            // can't return a shadowing user method on a future
            // hit. This is the load-bearing version of the
            // `debug_assert!` inside `lookup_toplevel_method_cached`
            // (which only fires in debug builds).
            if !Self::is_builtin_name(&name)
                && let Some(m) = self.lookup_toplevel_method_cached(name_id, cache_id)
            {
                self.invoke_method(m, self_val, args)?;
                return Ok(());
            }
            // `include Mod` / `extend Mod` / `prepend Mod` inside
            // a class body — `self` is the class, name resolves
            // with no receiver. Pushes the source module onto the
            // target's `includes` or `prepends` chain (split by
            // method name; see the dispatch order comment on
            // `lookup_method_uncached`). Methods aren't copied —
            // `lookup_method_uncached` walks the chain at dispatch
            // time. Bumps `method_gen` so any monomorphic inline
            // cache entry that thought the class lacked the
            // included/prepended methods invalidates.
            // `private_constant :Foo, :Bar` / `public_constant ...` —
            // visibility hints for module constants. CRuby uses them
            // to prevent external `Tilt::EMPTY_HASH` access; rubyrs
            // doesn't enforce constant visibility yet (separate gap),
            // so the call is a no-op that returns the class. Returning
            // self matches CRuby's chainable form. Required for tilt
            // load (tilt.rb:11/14, tilt/mapping.rb:77/411 all use this).
            //
            // `autoload :Const, "path"` — CRuby's lazy-load hook: the
            // constant materialises when first referenced. rubyrs
            // doesn't model lazy loads (the embeddable host registers
            // template engines eagerly), so this is a no-op returning
            // nil (CRuby's actual return value for autoload). The
            // constant simply won't exist until someone explicitly
            // requires the target file. tilt's `register_lazy` calls
            // autoload internally; the documented gap is that
            // `Tilt['erb']` won't find the engine without a separate
            // eager `require 'tilt/erb'`.
            //
            // Arity matches CRuby: exactly 2 args. Wrong arity still
            // raises ArgumentError so caller bugs don't get hidden by
            // the stub fast-path.
            if &*name == "autoload"
                && let Value::Class(_) = &self_val {
                if args.len() != 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
                    }));
                }
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // `autoload?(:Const [, inherit])` — CRuby returns the
            // file path string if `:Const` is set for autoload on
            // this module, else nil. Since `autoload` is itself a
            // no-op stub (rubyrs doesn't model lazy loading), the
            // registry is always empty and `autoload?` always
            // returns nil. tilt's `mapping.rb:362` calls
            // `scope.autoload?(n)` inside `constant_defined?` —
            // expects nil so the second `const_defined?` check
            // proceeds. (TRY_RUNS pass-10 layer #1.)
            if &*name == "autoload?"
                && let Value::Class(_) = &self_val {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // `Mod.const_defined?(:Const [, inherit])` — looks up
            // the qualified name in `self.classes` (Class/Module
            // table) AND `self.constants` (other Value constants).
            // tilt's `mapping.rb:361-365` walks `Tilt::Backend` etc.
            // via `scope.const_defined?(n)`. The `inherit` arg is
            // accepted for arity parity but Tier-1 doesn't model
            // ancestor const lookup — `Foo::Bar` only resolves on
            // Foo itself, not its includes/superclass chain.
            // (TRY_RUNS pass-10 layer #2.)
            if &*name == "const_defined?"
                && let Value::Class(cls) = &self_val {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                // CRuby splits the path on `::` for String args
                // but treats Symbol args as bare names
                // (`:"Foo::Bar"` raises wrong-name).
                // `resolve_const_path` centralises validation,
                // intern-cap gating, and per-segment walk.
                // (Copilot review #277 round 4 #3.)
                let (const_name, split) = match &args[0] {
                    Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                    Value::Str(s) => (s.to_string_lossy(), true),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                    })),
                };
                let cls_clone = cls.clone();
                let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
                match outcome {
                    ConstPathOutcome::Found(_) => self.stack.push(Value::Bool(true)),
                    ConstPathOutcome::Missing { .. } => self.stack.push(Value::Bool(false)),
                    ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                        msg: format!("wrong constant name {}", name),
                    })),
                    ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                        msg: format!("{} does not refer to class/module", full_path),
                    })),
                }
                return Ok(());
            }
            // `Mod.const_get(:Const [, inherit])` — paired with
            // const_defined?. Returns the actual Class/Value
            // constant if defined; raises NameError otherwise.
            // tilt's `constant_defined?` walk calls `scope.const_get(n)`
            // after the `const_defined?` check passes.
            if &*name == "const_get"
                && let Value::Class(cls) = &self_val {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                let (const_name, split) = match &args[0] {
                    Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                    Value::Str(s) => (s.to_string_lossy(), true),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                    })),
                };
                let cls_clone = cls.clone();
                let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
                match outcome {
                    ConstPathOutcome::Found(v) => { self.stack.push(v); return Ok(()); }
                    ConstPathOutcome::Missing { missing_qualified } => return Err(self.trap(RubyError::NameError {
                        msg: format!("uninitialized constant {}", missing_qualified),
                    })),
                    ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                        msg: format!("wrong constant name {}", name),
                    })),
                    ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                        msg: format!("{} does not refer to class/module", full_path),
                    })),
                }
            }
            // `private_constant` / `public_constant` /
            // `deprecate_constant` accept any number of symbol args
            // (CRuby; including zero, which is a no-op). We don't
            // enforce that args are Symbols since the stub ignores
            // them anyway; the documented gap is that wrong arg
            // types silently no-op here instead of TypeError.
            // `deprecate_constant` would emit a deprecation warning
            // in CRuby when the constant is read; rubyrs doesn't
            // model deprecation warnings, so the read path returns
            // the value silently (visibility unaffected).
            // Motivating use: MRI `lib/erb.rb:264`
            // (`deprecate_constant :Revision`).
            if matches!(&*name, "private_constant" | "public_constant" | "deprecate_constant")
                && let Value::Class(_) = &self_val {
                self.stack.push(self_val);
                return Ok(());
            }
            if matches!(&*name, "include" | "extend" | "prepend") && !args.is_empty()
                && let Value::Class(target) = &self_val {
                    let is_prepend = &*name == "prepend";
                    for a in &args {
                        let src = match a {
                            Value::Class(c) => c.clone(),
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "wrong argument type {} (expected Module)",
                                    a.type_name(),
                                ),
                            })),
                        };
                        // CRuby last-{included,prepended}-wins:
                        // push to the front so it's checked first
                        // by the lookup walk (which goes head-to-
                        // tail). `prepend` and `include` route into
                        // separate chains — `lookup_method_uncached`
                        // walks prepends BEFORE the class's own
                        // methods, and includes AFTER.
                        //
                        // Idempotency check is full ancestor-chain,
                        // not just the direct vec — CRuby treats
                        // `include M` / `prepend M` as a no-op if
                        // `M` is anywhere in ancestors (transitive
                        // includes/prepends too). Without
                        // `class_is_a`, `include ContainsM` then
                        // `include M` would move `M` ahead of
                        // `ContainsM` and reorder lookup.
                        if !super::class_is_a(target, &src) {
                            let mut chain = if is_prepend {
                                target.prepends.borrow_mut()
                            } else {
                                target.includes.borrow_mut()
                            };
                            chain.insert(0, src);
                        }
                    }
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.stack.push(self_val.clone());
                    return Ok(());
                }
            // `private` / `protected` / `public` inside a class
            // body. With no args, switch the current visibility
            // mode for any subsequent `def`s. With Symbol or
            // String args, retroactively flip the visibility of
            // the listed methods on the current class. Outside a
            // class body these are no-ops returning nil — same
            // shape as CRuby's Module#private at the toplevel.
            if let Some(vis) = visibility_from_name(&name) {
                if let Value::Class(cls) = &self_val {
                    if args.is_empty() {
                        if let Some(top) = self.class_visibility_stack.last_mut() {
                            *top = vis;
                        }
                    } else {
                        let methods = cls.methods.borrow();
                        for a in &args {
                            let key: Option<SymId> = match a {
                                Value::Sym(s) => Some(*s),
                                Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                                _ => None,
                            };
                            if let Some(mid) = key
                                && let Some(m) = methods.get(&mid) {
                                    m.visibility.set(vis);
                                }
                        }
                    }
                    self.stack.push(Value::Nil);
                    return Ok(());
                }
                // Toplevel `private` / `protected` / `public` —
                // CRuby treats these as visibility modifiers on
                // Object's singleton class. We don't model
                // toplevel methods as Object instance methods, so
                // the call has no observable effect; accept it as
                // a no-op rather than NoMethodError to keep
                // common preamble patterns (`private; def helper;`
                // at the toplevel) parseable.
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // `respond_to?(:foo)` / `respond_to?(:foo, true)` with no
            // explicit receiver — implicit-self dispatch against the
            // current frame's `self_val`. Mirrors the recv-bearing
            // arm below (~line 2239); included here because the
            // no-recv path runs FIRST and would NoMethodError before
            // reaching that arm. Required by tilt.rb:143's
            // `respond_to?(:deprecate_constant, true)` feature
            // detection inside `class Tilt` body where self is a
            // Class.
            if &*name == "respond_to?" {
                // Arity: CRuby raises ArgumentError on 0 args or 3+,
                // before reaching method_missing / NoMethodError. The
                // no-recv path runs FIRST so this guard is what users
                // see for bare `respond_to?` calls inside a method
                // body or class body.
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                // Reopened-primitive user override: `class String;
                // def respond_to?; ...; end; end` installs a method
                // on the primitive's preamble class. Value::Object
                // self routes through `lookup_method_cached` at
                // line ~320; Value::Class through
                // `lookup_class_singleton_method` at ~343. Primitives
                // (Str / Int / Sym / Array / Hash / ...) had no
                // equivalent user-method lookup before the stub
                // fired, so a user override on the primitive was
                // silently shadowed. Resolve the primitive's class
                // via `class_of` and check its method table; if a
                // user `respond_to?` exists, invoke it instead of
                // the stub.
                //
                // Documented narrower gap: this only fixes
                // `respond_to?` specifically. Other bare calls in
                // reopened-primitive method bodies (e.g.
                // `class String; def trigger; custom_helper; end;
                // end`) still surface NoMethodError because the
                // no-recv path doesn't generally consult the
                // primitive's class. Tracked as a separate broader
                // gap in SUBSET.md.
                if !matches!(&self_val, Value::Object(_) | Value::Class(_))
                    && let Value::Class(cls) = self.class_of(&self_val)
                    && let Some(m) = self.lookup_method_uncached(&cls, name_id)
                {
                    self.invoke_method(m, self_val.clone(), args)?;
                    return Ok(());
                }
                // Type: CRuby raises `TypeError: X is not a symbol nor
                // a string` when arg[0] isn't a Symbol or String.
                // Without this guard the call would silently fall
                // through to method_missing / NoMethodError, which
                // misreports the failure as "method missing" instead
                // of "wrong arg type" and confuses debugging.
                let lookup_name: SymId = match &args[0] {
                    Value::Sym(id) => *id,
                    Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} is not a symbol nor a string",
                            other.to_inspect(&self.heap, &self.interner),
                        ),
                    })),
                };
                let yes = self.responds_to(&self_val, lookup_name);
                self.stack.push(Value::Bool(yes));
                return Ok(());
            }
            // method_missing fallback (PoC #2). For Object self, look
            // up the class chain — if found, hand it the missed name
            // as a Symbol arg. Primitives skip this and raise directly.
            if self.try_method_missing(&self_val, name_id, args, None)? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                kind: crate::error::NoMethodErrorKind::Missing,
                method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
            }));
        }

        let recv = recv.expect("ICE: receiver missing");

        // `cls.class_eval(source_string [, file, line])` — runtime
        // parse + compile + run of a Ruby source string. Tier 1
        // divergence (documented in docs/SUBSET.md): does NOT
        // switch to the receiver class's class-body context, so
        // `Foo.class_eval("def bar; end")` lands `bar` at top
        // level. Tilt's tilt-2.7.0 `eval_compiled_method` self-
        // wraps its source in a nested block-form
        // `Tilt::TOPOBJECT.class_eval do def ... end end`, so
        // the inner block-form (intercepted in `do_call_block`)
        // does the actual class context switching.
        // No-arg, no-block `C.class_eval` / `C.module_eval` would
        // otherwise fall through to NoMethodError, but
        // respond_to?(:class_eval) reports true. CRuby raises
        // ArgumentError "wrong number of arguments (given 0,
        // expected 1..3)" for the no-arg string-form call;
        // (block-only form is handled in do_call_block).
        if (&*name == "class_eval" || &*name == "module_eval")
            && let Value::Class(cls) = &recv
            && args.is_empty()
            && self.lookup_class_singleton_method(cls, name_id).is_none()
        {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..3)".into(),
            }));
        }
        if (&*name == "class_eval" || &*name == "module_eval")
            && let Value::Class(cls) = &recv
            && !args.is_empty()
            // Defer to user-defined `def self.class_eval(s)` /
            // `def self.module_eval(s)` if present — same
            // ordering as the singleton-method lookup at
            // dispatch.rs:1597. Without this check, a class
            // overriding its own `class_eval` would have the
            // override silently bypassed.
            && self.lookup_class_singleton_method(cls, name_id).is_none()
        {
            // Arity guard FIRST so too-many-arg calls surface as
            // ArgumentError, matching CRuby's check order (arity
            // → type). Without this, `C.class_eval(123, "f", 1,
            // :extra)` would report a misleading TypeError on
            // args[0] even though the call is out of the 1..3
            // signature.
            if args.len() > 3 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1..3)",
                        args.len()
                    ),
                }));
            }
            // Validate args[0] (source) type after arity. Non-
            // String falls through here (no user override + no
            // block path matched) and should surface as TypeError,
            // NOT NoMethodError. `respond_to?(:class_eval)`
            // returns true, so the dispatch reaching this point
            // means the method exists — bad arg type is a
            // TypeError.
            if !matches!(args[0], Value::Str(_)) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        args[0].type_name()
                    ),
                }));
            }
            // Validate args[1] (filename) type when present:
            // CRuby raises TypeError for non-String. Falling back
            // to the default label would silently ignore the
            // caller's mistake.
            if let Some(a1) = args.get(1)
                && !matches!(a1, Value::Str(_)) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        a1.type_name()
                    ),
                }));
            }
            // Validate args[2] (line) when present: CRuby raises
            // TypeError for non-Integer-coercible values. Accept
            // Int and Float (Float has `to_int`); reject other
            // types even though we ultimately ignore the line
            // offset — silent acceptance would mask caller bugs.
            if let Some(a2) = args.get(2)
                && !matches!(a2, Value::Int(_) | Value::Float(_)) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        a2.type_name()
                    ),
                }));
            }
            let src = if let Value::Str(s) = &args[0] { s.to_string_lossy() } else { unreachable!() };
            // Track whether the filename is our synthetic default
            // or caller-supplied. Only the synthetic case opts
            // into the source-table collision-suffix dedupe; an
            // explicit user filename should stay verbatim across
            // repeated calls so `__FILE__` is stable.
            let (filename, synthetic) = match args.get(1) {
                Some(Value::Str(f)) => (f.to_string_lossy(), false),
                _ => ("(class_eval)".to_string(), true),
            };
            let v = self.eval_string(&src, &filename, synthetic)?;
            if self.suppress_call_result_push {
                self.suppress_call_result_push = false;
            } else {
                self.stack.push(v);
            }
            return Ok(());
        }

        // `Object#send(:name, args...)` / `__send__(:name, args...)`
        // — dynamic dispatch. Resolve the first arg as the target
        // method name and re-enter `do_call` with `recv` pushed
        // back, the remaining args on the stack, and the resolved
        // SymId in name_id. The whole normal lookup path then
        // handles the rest (primitives, singleton methods, host
        // fns, method_missing, etc.) — `send` is just a name
        // re-aim, not a separate dispatch table.
        //
        // The method-name arg accepts both Symbol and String
        // (CRuby's transparent `to_sym`). Same precedent as
        // `Object#method` but broader because shared specs and
        // tilt-style libraries commonly pass `send("foo")`.
        // Block-form (`send(:name) { ... }`) lives in
        // `do_call_block`; this arm covers the block-less call.
        //
        // cache_id passed as `u16::MAX` because the re-entered call
        // resolves a runtime-dynamic name — caching it at the
        // original `send` call site's slot would poison whatever
        // method the bytecode actually compiled for that slot.
        //
        // **CRuby parity — user-defined `def send`**: only
        // `__send__` is reserved. A user `def send` on the
        // receiver's class wins over the built-in re-aim when the
        // call is named `send`. We check that first and fall
        // through to the regular `Value::Object` arm if found.
        //
        // **CRuby parity — visibility bypass**: `send` and
        // `__send__` may invoke private/protected methods. Set
        // `bypass_visibility_once` to suppress the visibility
        // check during the re-entered call. The flag is consumed
        // (single-shot) at the top of the next `do_call` /
        // `do_call_block` into a local — *not* at the visibility
        // check site — so a dispatch that bottoms out before the
        // Object arm (e.g. `send(:nonexistent)` raising
        // NoMethodError on a primitive) can't leak the bypass
        // into the next unrelated call.
        // send/__send__ bypass recogniser — unified helper
        // (#192 commit 2/5). NotHandled returns recv + args
        // back so the dispatcher can continue below.
        let (recv, args) = match self.try_dispatch_send_bypass(&name, name_id, cache_id, args, Some(recv)) {
            SendBypass::Handled(r) => return r,
            SendBypass::NotHandled { args, recv_opt } => (recv_opt.expect("with-recv path"), args),
        };

        // Int#+/-/* operator method-call BigInt-aware intercept.
        // Op::BinOp's hot path inlines `apply_int.unwrap_or →
        // bigint_arith`, but the method-call form (`a.+(b)` /
        // `a.send(:+, b)`) goes through primitive_call which uses
        // plain i64 ops that wrap on overflow. Route Int×Int
        // operator names through `apply_int_promote` here so
        // `a.send(:+, big_literal)` matches Op::BinOp's
        // overflow-promotion behaviour exactly. With bignum off
        // apply_int_promote falls back to wrapping so the
        // pre-PR behaviour is preserved.
        #[cfg(feature = "bignum")]
        if args.len() == 1
            && matches!(&recv, Value::Int(_))
            && matches!(&args[0], Value::Int(_))
            && let Some(kind) = crate::bytecode::BinOpKind::from_op_name(&name)
            && matches!(kind,
                crate::bytecode::BinOpKind::Add
                | crate::bytecode::BinOpKind::Sub
                | crate::bytecode::BinOpKind::Mul
            )
        {
            let (Value::Int(x), Value::Int(y)) = (&recv, &args[0]) else { unreachable!() };
            let v = self.apply_int_promote(kind, *x, *y)?;
            self.stack.push(v);
            return Ok(());
        }

        if self.try_push_string_encoding(&recv, &name, &args) {
            return Ok(());
        }
        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes)
            .map_err(|e| self.trap(e))? {
            self.stack.push(v);
            return Ok(());
        }
        if let Some(v) = self.sym_primitive(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // BigInt method dispatch — `primitive_call` and friends
        // are stateless and can't read the heap, so the BigInt
        // surface is hooked here where `&mut self` is available.
        // Covers `to_s` / `inspect` (heap read) AND the operator
        // method-call shape (`big.+(1)`, `big.send(:==, x)`),
        // routed through `try_bigint_binop` so method-call form
        // matches the `Op::BinOp` semantics exactly. Without this
        // route, `big.send(:==, other)` would fall through to
        // `ruby_eq`'s Object-identity arm and miss canonical-value
        // equality.
        #[cfg(feature = "bignum")]
        if let Some(v) = self.bigint_primitive(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }

        // `Hash.new` interception. The preamble defines a stub
        // `class Hash; end` (lib.rs) that has no connection to the
        // primitive `Value::Hash` storage — without this short-
        // circuit, `Hash.new` falls through to the generic
        // `Class.new` allocator below and returns a bare
        // `Value::Object`, which then NoMethodErrors on every
        // collection-style call (`.[]`, `.keys`, `.each`, ...).
        //
        // Three call shapes (CRuby semantics):
        //   - `Hash.new`           → empty Hash, no default
        //   - `Hash.new(default)`  → empty Hash, scalar default
        //     (NOT yet modelled — falls through to no-default; the
        //     scalar arg is silently ignored as a documented gap)
        //   - `Hash.new { |h, k| block }` → empty Hash with default-
        //     block stored alongside; `Hash#[]` invokes it on
        //     missing keys with `(self, key)`.
        //
        // Tilt's `@lazy_map = Hash.new { |h, k| h[k] = [] }` (the
        // motivating case) is the block form. Without default-
        // block support the whole tilt-load chain stalls on the
        // first `@lazy_map[ext]` access.
        // Class-receiver intrinsics — Hash[] / new / allocate /
        // include / prepend / extend / private / public / protected
        // / name / superclass / method_defined?. Extracted into
        // try_dispatch_class_intrinsics (#192 commit 4/5).
        let (args, recv) = match self.try_dispatch_class_intrinsics(&name, name_id, cache_id, args, recv)? {
            ClassOutcome::Handled => return Ok(()),
            ClassOutcome::NotHandled { args, recv } => (args, recv),
        };

        // Primitive-receiver fallback to the user-Class method
        // table. CRuby's dispatch walks every value's class chain
        // uniformly; rubyrs's primitive arms above handle the
        // built-in methods, but `class Symbol; alias_method
        // :to_msgpack_ext, :name; end` installs a forwarder in
        // `self.classes[Symbol].methods` that's only reachable
        // through the user-Class table. Look up the primitive's
        // class name via `class_of` and try `lookup_method_cached`
        // on it. Skip Object (its own arm below handles that) and
        // Class (Class.new etc. handled by the earlier arm).
        //
        // No synth-bypass flag is needed: the Kernel reflection
        // builtins live in a separate `Vm.kernel_builtin_metas`
        // registry, NOT on `Kernel.methods`, so chain-walking
        // here doesn't re-find them. See `install_kernel_builtins`
        // (vm/lookup.rs) for the rationale.
        if !matches!(&recv, Value::Object(_) | Value::Class(_))
            && let Value::Class(cls) = self.class_of(&recv)
            && let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
            self.invoke_method(m, recv.clone(), args)?;
            return Ok(());
        }
        if let Value::Object(id) = &recv {
            let cls = self.heap.class_of(*id);
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                self.check_method_visibility(&m, &recv, &name, bypass_visibility)?;
                self.invoke_method(m, recv.clone(), args)?;
                return Ok(());
            }
            // L3-C: cext-registered instance method
            // (`rb_define_method`). Looked up AFTER script-defined
            // methods so a Ruby-side override wins for
            // concrete-class methods.
            //
            // **Known limitation** (review #1 on PR #27): the
            // current shape walks the script-method ancestor chain
            // via lookup_method_cached, THEN checks cext methods
            // only on the receiver's own class. So a Ruby method
            // on a superclass shadows a cext method on the
            // subclass, and a cext method on a superclass is
            // invisible to subclass instances. A complete fix
            // would interleave cext lookup INSIDE the per-class
            // walk in lookup_method_cached — out of L3-C wedge
            // scope. Real-world impact is small: the common pattern
            // is `class Foo; end` + `rb_define_method(Foo, ...)`
            // on the same class, which works correctly.
            #[cfg(all(feature = "cext", not(target_os = "wasi")))]
            {
                if let Some(table) = self.cext_instance_methods.get(cls.name.as_str())
                    && let Some(reg) = table.get(&name_id).cloned() {
                        // Pin recv + args across the cext call
                        // (review #4 on PR #27). cext_dispatch may
                        // run maybe_gc during arg translation /
                        // TypedData wrapping / result translation;
                        // recv was popped from vm.stack before we
                        // got here, so without pinning a STRESS_GC
                        // sweep can reclaim it mid-call →
                        // use-after-free in the cext body. Same
                        // shape as the L1.5 P0-A pattern.
                        //
                        // RAII guard holding only a `*mut Vec<Value>`
                        // (not `&mut Vm`) so it doesn't conflict with
                        // the `vm_ptr: *mut Vm` we hand to
                        // `with_vm_ptr_set` — PinGuard's `&mut Vm`
                        // would alias under Stacked Borrows when
                        // cext_dispatch's rb_funcall reentrance
                        // re-derefs the raw pointer (same gotcha L3-A
                        // review #15 / PR #6 hit). The narrower
                        // pointer is sound because it borrows only
                        // the field, not the whole Vm.
                        //
                        // Truncate runs on Drop, so a panic from
                        // `with_vm_ptr_set` / `cext_dispatch` (or
                        // the trailing `?`) doesn't leak pinned
                        // entries — fixes review #11 on PR #27,
                        // where the prior manual push/truncate
                        // skipped truncate on the unwind path.
                        struct PinTruncateGuard {
                            pinned: *mut Vec<Value>,
                            saved_depth: usize,
                        }
                        impl Drop for PinTruncateGuard {
                            fn drop(&mut self) {
                                // SAFETY: `pinned` was taken from
                                // `&mut self.pinned` in the
                                // enclosing scope; the guard is
                                // dropped before that borrow could
                                // be used elsewhere, and no other
                                // Rust code mutates `pinned` while
                                // the cext call is on the stack.
                                unsafe { (*self.pinned).truncate(self.saved_depth); }
                            }
                        }
                        let saved_pin_depth = self.pinned.len();
                        self.pinned.push(recv.clone());
                        for a in &args { self.pinned.push(a.clone()); }
                        let _pin_guard = PinTruncateGuard {
                            pinned: &raw mut self.pinned,
                            saved_depth: saved_pin_depth,
                        };
                        let vm_ptr: *mut Vm = self;
                        let recv_clone = recv.clone();
                        let v = with_vm_ptr_set(vm_ptr, || {
                            crate::vm::cext::cext_dispatch(
                                &reg.qualified_name,
                                reg.func,
                                reg.arity,
                                &args,
                                crate::vm::cext::CextSelfHandle::Object(recv_clone),
                            )
                        })?;
                        // Explicit drop here is documentation, not
                        // necessity — `_pin_guard` drops at scope
                        // end either way.
                        drop(_pin_guard);
                        self.stack.push(v);
                        return Ok(());
                    }
            }
        }
        // C-ext singleton dispatch: `BCrypt::Engine.__bc_crypt(args)`
        // arrives here with recv = Value::Class(c). Look up the
        // method in the per-class cext table populated by
        // `Vm::cext_require` (rb_define_singleton_method).
        // File class-method shims. CRuby exposes File.read / .write
        // / .exist? / .open / .basename as class methods; we don't
        // have a `def self.foo` syntax yet, so the dispatch is a
        // hand-rolled intercept on the File class. I/O paths
        // surface OS errors as a generic RuntimeError so scripts
        // can `rescue` them.
        if let Value::Class(cls) = &recv {
            // User-Ruby `def self.foo` singletons: walk the
            // per-class `singleton_methods` table chain via the
            // shared helper. CRuby's metaclass model has the
            // singleton class of `Dog < Animal` inherit from the
            // singleton class of `Animal`, so `Dog.kingdom` finds
            // `Animal`'s `def self.kingdom`. The same helper is
            // used by the bare-call path (no_recv when self is a
            // Class) so `self.bar` and bare `bar` stay in sync.
            let user_singleton = self.lookup_class_singleton_method(cls, name_id);
            if let Some(m) = user_singleton {
                let target_self = recv.clone();
                return self.invoke_method(m, target_self, args);
            }
            if cls.name.as_str() == "File"
                && let Some(v) = self.file_class_dispatch(&name, &args)? {
                    self.stack.push(v);
                    return Ok(());
                }
            // `Module.nesting` — CRuby reflection returning the
            // lexical scope chain at the call site, innermost-first.
            // Resolves through the current frame's proto's
            // `lexical_scope` (built at compile time from
            // `b.class_path`). Each SymId is looked up in
            // `self.classes`; missing entries are skipped (a top-
            // level `module` whose body hasn't run yet at the call
            // site can't appear here in practice — class_path is
            // set ONLY when we're already inside the body, so the
            // class table already has the entry by the time
            // `Module.nesting` runs).
            if cls.name.as_str() == "Module" && &*name == "nesting" && args.is_empty() {
                let frame = self.frames.last().expect("ICE: Module.nesting no frame");
                let lex = self.protos[frame.proto_idx].lexical_scope.clone();
                let mut items: Vec<Value> = Vec::with_capacity(lex.len());
                for sym in lex {
                    if let Some(c) = self.classes.get(&sym).cloned() {
                        items.push(Value::Class(c));
                    }
                }
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(items));
                self.stack.push(Value::Array(id));
                return Ok(());
            }
            #[cfg(feature = "cext")]
            if let Some(table) = self.cext_class_methods.get(cls.name.as_str())
                && let Some(host) = table.get(&name_id).cloned() {
                    // Stash Vm pointer for the singleton-method's
                    // C body — same rationale as the top-level
                    // host_fns dispatch above.
                    #[cfg(not(target_os = "wasi"))]
                    let v = {
                        let vm_ptr: *mut Vm = self;
                        with_vm_ptr_set(vm_ptr, || host(&args))?
                    };
                    #[cfg(target_os = "wasi")]
                    let v = host(&args)?;
                    self.stack.push(v);
                    return Ok(());
                }
        }
        // `include Mod` — without real Modules in the subset, we
        // approximate by copying the source class's method table
        // into the target class. Only fills methods the target
        // doesn't already define, so user overrides win (matching
        // CRuby's ancestor-chain semantics where own methods
        // shadow included ones). Defines `include` ad-hoc on
        // Class receivers; the call is a no-op for any other
        // receiver and falls through to NoMethodError.
        // `proc.call(args)` / `lambda.call(args)` — invoke the
        // block synchronously and push its result. Sub-frame
        // runs until it returns; same dispatch shape as iterator
        // drivers' invoke_block + dispatch_until pattern, but
        // accessible from script code (rather than only from
        // builtin iterators).
        // Callable intrinsics — Block.call / method capture /
        // BoundMethod / UnboundMethod / CurriedProc family.
        // Extracted into try_dispatch_callable_intrinsics
        // (#192 commit 3/5). NotHandled returns args + recv back.
        let (args, recv) = match self.try_dispatch_callable_intrinsics(&name, name_id, args, recv)? {
            CallableOutcome::Handled => return Ok(()),
            CallableOutcome::NotHandled { args, recv } => (args, recv),
        };
        // Explicit-receiver no-op stubs — `Foo.private_constant :X`,
        // `Foo.public_constant :X`, `Foo.deprecate_constant :X`,
        // `Foo.autoload :X, "path"`. Counterparts to the no-recv
        // arm above. See that arm for the rationale (visibility /
        // lazy-load / deprecation hooks rubyrs doesn't model yet).
        // Tilt's `Tilt.autoload class_name, file` inside
        // `register_lazy` is the canonical caller.
        if &*name == "autoload"
            && let Value::Class(_) = &recv {
            if args.len() != 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
                }));
            }
            self.stack.push(Value::Nil);
            return Ok(());
        }
        // `Foo.autoload?(:Bar)` — explicit-receiver parallel of
        // the no_recv arm above. Returns nil since the registry
        // is always empty (autoload is a no-op stub).
        if &*name == "autoload?"
            && let Value::Class(_) = &recv {
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            self.stack.push(Value::Nil);
            return Ok(());
        }
        // `Foo.const_defined?(:Bar)` — explicit-receiver parallel.
        // tilt's actual call site is
        // `scope.const_defined?(n)` where scope is reached via the
        // `inject(Object)` walk in `constant_defined?`.
        if &*name == "const_defined?"
            && let Value::Class(cls) = &recv {
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            // String args walked via `::`; Symbol args treated as
            // bare names — see `resolve_const_path` doc.
            // (Copilot review #277 round 4 #3.)
            let (const_name, split) = match &args[0] {
                Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                Value::Str(s) => (s.to_string_lossy(), true),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                })),
            };
            let cls_clone = cls.clone();
            let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
            match outcome {
                ConstPathOutcome::Found(_) => self.stack.push(Value::Bool(true)),
                ConstPathOutcome::Missing { .. } => self.stack.push(Value::Bool(false)),
                ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                    msg: format!("wrong constant name {}", name),
                })),
                ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                    msg: format!("{} does not refer to class/module", full_path),
                })),
            }
            return Ok(());
        }
        if &*name == "const_get"
            && let Value::Class(cls) = &recv {
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            let (const_name, split) = match &args[0] {
                Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                Value::Str(s) => (s.to_string_lossy(), true),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                })),
            };
            let cls_clone = cls.clone();
            let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
            match outcome {
                ConstPathOutcome::Found(v) => { self.stack.push(v); return Ok(()); }
                ConstPathOutcome::Missing { missing_qualified } => return Err(self.trap(RubyError::NameError {
                    msg: format!("uninitialized constant {}", missing_qualified),
                })),
                ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                    msg: format!("wrong constant name {}", name),
                })),
                ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                    msg: format!("{} does not refer to class/module", full_path),
                })),
            }
        }
        if matches!(&*name, "private_constant" | "public_constant" | "deprecate_constant")
            && let Value::Class(_) = &recv {
            self.stack.push(recv);
            return Ok(());
        }
        if let Value::Class(target) = &recv
            && matches!(&*name, "include" | "extend" | "prepend") && !args.is_empty() {
                // Explicit-receiver form: `MyClass.include(Mod)` /
                // `.prepend(Mod)`. Same chain-push semantics as the
                // no-receiver form above — see that comment for the
                // rationale and the prepend-vs-include split.
                let is_prepend = &*name == "prepend";
                for a in &args {
                    let src = match a {
                        Value::Class(c) => c.clone(),
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Module)",
                                a.type_name(),
                            ),
                        })),
                    };
                    // Full ancestor-chain idempotency, same as the
                    // no-receiver arm — see that comment for the
                    // reorder hazard a shallow vec-check creates.
                    if !super::class_is_a(target, &src) {
                        let mut chain = if is_prepend {
                            target.prepends.borrow_mut()
                        } else {
                            target.includes.borrow_mut()
                        };
                        chain.insert(0, src);
                    }
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(recv.clone());
                return Ok(());
            }
        // Universal class predicates: `is_a?` / `kind_of?` walk
        // the ancestor chain (own class + includes + superclass);
        // `instance_of?` is exact-class only. CRuby exposes both
        // on `Object`, so they apply to every receiver — for
        // primitives (Int / Str / Sym / ...) we resolve their
        // class via `class_of`.
        if matches!(&*name, "is_a?" | "kind_of?" | "instance_of?") && args.len() == 1
            && let Value::Class(target) = &args[0] {
                let recv_class_v = self.class_of(&recv);
                let recv_class = if let Value::Class(c) = recv_class_v { c } else {
                    self.stack.push(Value::Bool(false));
                    return Ok(());
                };
                let result = if &*name == "instance_of?" {
                    Rc::ptr_eq(&recv_class, target)
                } else {
                    super::class_is_a(&recv_class, target)
                };
                self.stack.push(Value::Bool(result));
                return Ok(());
            }
        // Class-receiver introspection cluster (the second
        // Class cluster from #192 commit 4 — deferred to its
        // own helper). Returns true when an arm matched and
        // pushed a result; otherwise falls through to the
        // remaining dispatch.
        if self.try_dispatch_class_introspection(&name, &args, &recv)? {
            return Ok(());
        }
        if let Some(v) = self.collection_call(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // `obj.methods` — Array of Symbols of every method the
        // receiver can dispatch. For user instances walks the
        // class chain (own + includes + superclass). For a Class
        // receiver, walks the class-method chain — each level's
        // `singleton_prepends` (recursing through each module's
        // own prepends/includes) and `singleton_methods` — up
        // the superclass chain. Other shapes return an empty
        // Array (the subset doesn't expose Kernel-level methods
        // individually). De-dups by SymId, sorted by interner
        // string order for determinism.
        if &*name == "methods" && args.is_empty() {
            let mut names: Vec<crate::intern::SymId> = Vec::new();
            if let Value::Object(id) = &recv {
                let cls = self.heap.class_of(*id);
                let mut visited: Vec<*const crate::value::Class> = Vec::new();
                fn walk(
                    c: &std::rc::Rc<crate::value::Class>,
                    out: &mut Vec<crate::intern::SymId>,
                    visited: &mut Vec<*const crate::value::Class>,
                ) {
                    let ptr = std::rc::Rc::as_ptr(c);
                    if visited.contains(&ptr) { return; }
                    visited.push(ptr);
                    for k in c.methods.borrow().keys() {
                        if !out.contains(k) { out.push(*k); }
                    }
                    for inc in c.includes.borrow().iter() {
                        walk(inc, out, visited);
                    }
                    if let Some(sup) = c.superclass.borrow().clone() {
                        walk(&sup, out, visited);
                    }
                }
                walk(&cls, &mut names, &mut visited);
                names.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
            } else if let Value::Class(cls) = &recv {
                // Walk a prepended module's transitive includes /
                // prepends — same shape as `walk_module` in
                // lookup.rs, but collects method names rather
                // than searching for one.
                fn walk_mod(
                    m: &std::rc::Rc<crate::value::Class>,
                    out: &mut Vec<crate::intern::SymId>,
                    visited: &mut Vec<*const crate::value::Class>,
                ) {
                    let ptr = std::rc::Rc::as_ptr(m);
                    if visited.contains(&ptr) { return; }
                    visited.push(ptr);
                    for pre in m.prepends.borrow().iter() {
                        walk_mod(pre, out, visited);
                    }
                    for k in m.methods.borrow().keys() {
                        if !out.contains(k) { out.push(*k); }
                    }
                    for inc in m.includes.borrow().iter() {
                        walk_mod(inc, out, visited);
                    }
                }
                let mut sc_visited: Vec<*const crate::value::Class> = Vec::new();
                let mut mod_visited: Vec<*const crate::value::Class> = Vec::new();
                let mut current = cls.clone();
                loop {
                    let ptr = std::rc::Rc::as_ptr(&current);
                    if sc_visited.contains(&ptr) { break; }
                    sc_visited.push(ptr);
                    for pre in current.singleton_prepends.borrow().iter() {
                        walk_mod(pre, &mut names, &mut mod_visited);
                    }
                    for k in current.singleton_methods.borrow().keys() {
                        if !names.contains(k) { names.push(*k); }
                    }
                    let parent = current.superclass.borrow().clone();
                    match parent {
                        Some(p) => current = p,
                        None => break,
                    }
                }
                names.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
            }
            let elems: Vec<Value> = names.into_iter().map(Value::Sym).collect();
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `obj.instance_variables` — Array of Symbols (with `@`
        // prefix). Reads ivars from `Value::Object` (Instance) and
        // `Value::Class` receivers (cls.ivars), staying consistent
        // with `instance_variable_get` / `_set` which also support
        // both shapes. Other receivers (primitives, Array/Hash/etc.
        // that don't carry ivars in rubyrs's heap model) get an
        // empty Array.
        if &*name == "instance_variables" && args.is_empty() {
            let mut names: Vec<Value> = Vec::new();
            let ivar_ids: Vec<crate::intern::SymId> = match &recv {
                Value::Object(id) => {
                    if let crate::heap::HeapObj::Instance(inst) = self.heap.get(*id) {
                        inst.ivars.keys().copied().collect()
                    } else {
                        Vec::new()
                    }
                }
                Value::Class(cls) => cls.ivars.borrow().keys().copied().collect(),
                _ => Vec::new(),
            };
            if !ivar_ids.is_empty() {
                let mut decorated: Vec<(String, crate::intern::SymId)> = ivar_ids.into_iter()
                    .map(|s| {
                        let raw = self.interner.resolve(s).to_string();
                        // Internal interner key includes the `@`
                        // prefix already (matches how parser interns
                        // ivar names). If not, prepend.
                        let key = if raw.starts_with('@') { raw } else { format!("@{}", raw) };
                        (key, s)
                    })
                    .collect();
                decorated.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, _) in decorated {
                    let sid = self.interner.intern(&key);
                    names.push(Value::Sym(sid));
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(names));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `obj.instance_variable_get(name)` / `instance_variable_set(name, value)`
        // — pure ivar read/write by name. Surfaced as a blocker
        // for sinatra/indifferent_hash.rb's `Gem::Version#<=>` shape
        // (TRY_RUNS pass 7 layer #2) and load-bearing for any
        // introspection-heavy gem.
        //
        // Name validation: CRuby ivar names must match
        // `@[A-Za-z_][A-Za-z0-9_]*` — single `@` followed by an
        // identifier char (letter or underscore), then zero or more
        // identifier-or-digit chars. Rejects `@@x` (class var),
        // `@1` (digit start), `@foo?` (predicate suffix), bare `@`.
        // String intern path enforces `Config::max_symbols`.
        //
        // Heap shape: read/write reaches the ivar table on
        // `Value::Object` (Instance) and `Value::Class` (Class)
        // receivers — same storage that `Op::LoadIvar` /
        // `Op::StoreIvar` in vm/step.rs:552/562 read and write.
        // The set path is MORE defensive than `Op::StoreIvar`:
        // that op still calls `heap.instance_mut(*oid)` which
        // panics with the same "ICE: heap slot is not an
        // Instance" assertion this fix avoids; if `Op::StoreIvar`
        // is ever reached for a non-Instance Object slot it will
        // still ICE (a separate hardening concern, not covered
        // by this PR). The `_ =>` arm below catches every
        // non-Object/non-Class receiver — Int/Str/Float/Sym/
        // Nil/Bool/Array/Hash/Range/Proc/etc. — and raises
        // FrozenError. For mutable shapes like Array/Hash that
        // CRuby DOES allow ivars on, supporting that surface
        // would require ivar slots on those HeapObj variants;
        // explicit out-of-scope until a caller surfaces it.
        if &*name == "instance_variable_get" && args.len() == 1 {
            let ivar_id = self.resolve_ivar_name_arg(&args[0])?;
            let v = match &recv {
                Value::Object(oid) => match self.heap.get(*oid) {
                    crate::heap::HeapObj::Instance(inst) => {
                        inst.ivars.get(&ivar_id).cloned().unwrap_or(Value::Nil)
                    }
                    _ => Value::Nil,
                },
                Value::Class(cls) => {
                    cls.ivars.borrow().get(&ivar_id).cloned().unwrap_or(Value::Nil)
                }
                _ => Value::Nil,
            };
            self.stack.push(v);
            return Ok(());
        }
        if &*name == "instance_variable_set" && args.len() == 2 {
            let ivar_id = self.resolve_ivar_name_arg(&args[0])?;
            let value = args[1].clone();
            match &recv {
                Value::Object(oid) => match self.heap.get_mut(*oid) {
                    crate::heap::HeapObj::Instance(inst) => {
                        inst.ivars.insert(ivar_id, value.clone());
                        self.stack.push(value);
                        return Ok(());
                    }
                    // TypedData (and any future non-Instance Object
                    // heap variant) genuinely accepts ivars in CRuby
                    // — the limitation is rubyrs-specific (no ivar
                    // table on `TypedDataObj`). RubyError doesn't
                    // model `NotImplementedError` yet, so RuntimeError
                    // is the closest fit; keep the message terse and
                    // explicit about the rubyrs-side limitation so a
                    // gem hitting this knows it's not a CRuby
                    // semantic difference.
                    _ => return Err(self.trap(RubyError::RuntimeError {
                        msg: "instance_variable_set on TypedData receivers is not yet supported in rubyrs".to_string(),
                    })),
                },
                Value::Class(cls) => {
                    cls.ivars.borrow_mut().insert(ivar_id, value.clone());
                    self.stack.push(value);
                    return Ok(());
                }
                _ => {
                    let cls = crate::vm::numeric::class_name_for_error(&recv);
                    let inspected = recv.to_inspect(&self.heap, &self.interner);
                    return Err(self.trap(RubyError::FrozenError {
                        msg: format!("can't modify frozen {}: {}", cls, inspected),
                    }));
                }
            }
        }
        // `obj.instance_variable_defined?(name)` — true iff the
        // named ivar has been set (even to nil). Mirrors the
        // get/set storage shape: reads the same Instance.ivars
        // map for Value::Object and Class.ivars for Value::Class.
        // Other receivers carry no ivar table, so the answer is
        // always false. The name argument goes through the same
        // `resolve_ivar_name_arg` validator as get/set, so an
        // invalid identifier (e.g. `:foo` without `@`) raises
        // NameError before the lookup runs — matching CRuby.
        if &*name == "instance_variable_defined?" && args.len() == 1 {
            let ivar_id = self.resolve_ivar_name_arg(&args[0])?;
            let defined = match &recv {
                Value::Object(oid) => match self.heap.get(*oid) {
                    crate::heap::HeapObj::Instance(inst) => {
                        inst.ivars.contains_key(&ivar_id)
                    }
                    _ => false,
                },
                Value::Class(cls) => cls.ivars.borrow().contains_key(&ivar_id),
                _ => false,
            };
            self.stack.push(Value::Bool(defined));
            return Ok(());
        }
        // Wrong-arity arms for the ivar-introspection family —
        // match CRuby's ArgumentError surface. Without these,
        // `obj.instance_variables(1)`, `obj.instance_variable_get()`,
        // or `obj.instance_variable_set(:@x)` would fall through to
        // NoMethodError, which is wrong (CRuby reports arity, not
        // unknown method). `instance_variables` takes zero args;
        // `_get` / `_defined?` take one; `_set` takes two.
        if &*name == "instance_variables" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
            }));
        }
        if &*name == "instance_variable_get" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
            }));
        }
        if &*name == "instance_variable_set" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
            }));
        }
        if &*name == "instance_variable_defined?" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
            }));
        }
        // `Integer#digits([base])` for Int receivers — LSB-first
        // digit Array, i64 fast path (no BigInt arithmetic for
        // small inputs). Default base 10; base must be >= 2.
        // Error semantics match `Vm::try_integer_digits` (the
        // BigInt-receiver path under `feature = "bignum"`) so
        // both profiles agree on the surface user code sees:
        //   - Arity > 1 → ArgumentError "wrong number of arguments
        //     (given N, expected 0..1)" matching CRuby. Under
        //     bignum the equivalent guard in `bigint_primitive`
        //     fires first; this arm catches the no-bignum profile.
        //   - Non-Integer base → TypeError matching CRuby text.
        //   - Negative base → ArgumentError "negative radix".
        //   - 0/1 base → ArgumentError "invalid radix N".
        //   - Negative receiver → ArgumentError "out of domain"
        //     (CRuby uses Math::DomainError; substituted because
        //     Math::DomainError isn't modelled in this subset —
        //     same convention as other numeric-out-of-domain
        //     arms elsewhere in `Vm::do_call`).
        // CRuby precedence: negative receiver raises
        // Math::DomainError BEFORE any arity / base check. Mirror
        // the order with the substitute ArgumentError, so user
        // code's `rescue ArgumentError` catches the negative-recv
        // path regardless of the other args' validity. Under
        // bignum the equivalent check in `bigint_primitive` fires
        // before this dispatcher runs, but keep this guard for
        // the no-bignum profile and as defense-in-depth.
        if let Value::Int(n) = &recv && &*name == "digits" && *n < 0 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "out of domain".to_string(),
            }));
        }
        // `Integer#divmod(b)` — returns [q, r] Array where
        // q = floor(a/b), r = a - b*q (CRuby floor semantics).
        // Lives in dispatch.rs because the Array result needs
        // heap-alloc + maybe_gc + check_alloc. Sits alongside
        // `digits` for the same reason. Sibling BigInt dispatch
        // is handled in bigint_primitive (which routes its own
        // BigInt-receiver path through here for the alloc).
        let recv_is_integer = {
            #[cfg(feature = "bignum")]
            { matches!(&recv, Value::Int(_) | Value::BigInt(_)) }
            #[cfg(not(feature = "bignum"))]
            { matches!(&recv, Value::Int(_)) }
        };
        if recv_is_integer && &*name == "divmod" {
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            // Compute q, r as Values. Float arg → both q, r Float.
            // Zero divisor (Int or Float) → ZeroDivisionError.
            // NaN divisor → FloatDomainError.
            // Non-Numeric → TypeError.
            let arg = &args[0];
            let (q, r) = match arg {
                Value::Int(b) => {
                    if *b == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    match &recv {
                        Value::Int(a) => {
                            // Compute via the floor helpers. Under
                            // bignum, i64::MIN/-1 needs BigInt
                            // promotion (sibling to apply_int's
                            // None-on-overflow path); route through
                            // bigint_arith for that case.
                            #[cfg(feature = "bignum")]
                            if *a == i64::MIN && *b == -1 {
                                // recv is Int(i64::MIN), no heap id
                                // to pin on the recv side — but q is
                                // a freshly-promoted BigInt whose
                                // only root is this local across the
                                // Mod call's `bigint_to_value` →
                                // `maybe_gc` window.
                                let mut g = PinGuard::new(self);
                                let q = g.vm.bigint_arith(
                                    crate::bytecode::BinOpKind::Div, &recv, arg,
                                ).expect("ICE: bigint_arith None for i64::MIN/-1")?;
                                g.pin(q.clone());
                                let r = g.vm.bigint_arith(
                                    crate::bytecode::BinOpKind::Mod, &recv, arg,
                                ).expect("ICE: bigint_arith None for i64::MIN/-1")?;
                                (q, r)
                            } else {
                                (
                                    Value::Int(crate::vm::floor_div_i64(*a, *b)),
                                    Value::Int(crate::vm::floor_mod_i64(*a, *b)),
                                )
                            }
                            #[cfg(not(feature = "bignum"))]
                            (
                                Value::Int(crate::vm::floor_div_i64(*a, *b)),
                                Value::Int(crate::vm::floor_mod_i64(*a, *b)),
                            )
                        }
                        #[cfg(feature = "bignum")]
                        Value::BigInt(_) => {
                            // BigInt × Int — promotes through bigint_arith.
                            // Pin recv AND q across BOTH calls — both
                            // route through `bigint_to_value` →
                            // `maybe_gc`, which would otherwise sweep
                            // recv (drained from the stack) before
                            // its bigint heap slot is read, AND sweep
                            // q before r lands.
                            let mut g = PinGuard::new(self);
                            g.pin(recv.clone());
                            let q = g.vm.bigint_arith(
                                crate::bytecode::BinOpKind::Div, &recv, arg,
                            ).expect("ICE: bigint_arith None for BigInt divmod")?;
                            g.pin(q.clone());
                            let r = g.vm.bigint_arith(
                                crate::bytecode::BinOpKind::Mod, &recv, arg,
                            ).expect("ICE: bigint_arith None for BigInt divmod")?;
                            (q, r)
                        }
                        _ => unreachable!("recv is Int or BigInt by outer guard"),
                    }
                }
                Value::Float(b) => {
                    if b.is_nan() {
                        // CRuby raises `FloatDomainError: NaN`.
                        // FloatDomainError < RangeError < StandardError,
                        // so `rescue FloatDomainError`, `rescue RangeError`,
                        // and a bare `rescue` all catch this (verified
                        // in tests/embed/numeric.rs's
                        // `float_domain_error_class_and_rescue_chain`).
                        return Err(self.trap(RubyError::FloatDomainError {
                            msg: "NaN".to_string(),
                        }));
                    }
                    if *b == 0.0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let a_f = match &recv {
                        Value::Int(n) => *n as f64,
                        #[cfg(feature = "bignum")]
                        Value::BigInt(id) => {
                            use num_traits::ToPrimitive;
                            self.heap.bigint(*id).to_f64().unwrap_or(f64::NAN)
                        }
                        _ => unreachable!("recv is Int or BigInt"),
                    };
                    let q_f = (a_f / *b).floor();
                    let r_f = crate::vm::numeric::floor_mod_f64(a_f, *b);
                    // CRuby: q is Integer-valued Float for Int.divmod(Float)? No —
                    // for `13.divmod(4.0)` CRuby returns `[3, 1.0]` (Int q, Float r).
                    let q_int = if q_f.is_finite() && q_f >= (i64::MIN as f64) && q_f < (i64::MAX as f64) {
                        Value::Int(q_f as i64)
                    } else {
                        // q overflows i64 → keep as Float (CRuby would
                        // promote to BigInt; approximate by Float for
                        // now matching the fdiv precision tier).
                        Value::Float(q_f)
                    };
                    (q_int, Value::Float(r_f))
                }
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => {
                    // BigInt arg arm — pin recv + arg + q across the
                    // bigint_arith calls (each routes through
                    // bigint_to_value → maybe_gc).
                    let mut g = PinGuard::new(self);
                    g.pin(recv.clone());
                    g.pin(arg.clone());
                    let q = g.vm.bigint_arith(
                        crate::bytecode::BinOpKind::Div, &recv, arg,
                    ).expect("ICE: bigint_arith None for BigInt divmod")?;
                    g.pin(q.clone());
                    let r = g.vm.bigint_arith(
                        crate::bytecode::BinOpKind::Mod, &recv, arg,
                    ).expect("ICE: bigint_arith None for BigInt divmod")?;
                    (q, r)
                }
                _ => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} can't be coerced into Integer",
                            crate::vm::numeric::type_name_for_coerce(arg),
                        ),
                    }));
                }
            };
            // GC root hole (sibling to the coerce fix in PR #289):
            // for BigInt divmod, `q` and `r` are freshly-allocated
            // BigInt ObjIds returned by `bigint_arith` — their only
            // live root at this point is the Rust local. Without the
            // PinGuard, `maybe_gc()` runs with both ObjIds
            // unreachable and sweeps them before the result Array is
            // allocated, leaving the Array with dangling slots.
            // Pin both Values across maybe_gc + heap.alloc; Drop
            // restores normal GC reachability via the freshly-pushed
            // `Value::Array(id)` on the stack.
            let arr_id = {
                let mut g = PinGuard::new(self);
                g.pin(q.clone());
                g.pin(r.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(vec![q, r]))
            };
            self.stack.push(Value::Array(arr_id));
            return Ok(());
        }
        // `Numeric#coerce(other)` — the Tier-2 Numeric protocol
        // entry point. Returns a 2-element Array `[other_promoted,
        // self_promoted]` so arithmetic operators on heterogeneous
        // numeric pairs can route through a uniform "promote then
        // operate on same-type" path. Implemented for Integer
        // (Int + BigInt) and Float receivers; Phase C (Rational /
        // Complex) will extend this surface.
        //
        // CRuby parity:
        //   - Int.coerce(Integer)  → [Integer, Integer]
        //   - Int.coerce(Float)    → [Float,   Float]
        //   - Float.coerce(Numeric)→ [Float,   Float]
        //   - any.coerce(non-Numeric) → TypeError
        //     "<other> can't be coerced into <recv_class>"
        let recv_is_numeric = matches!(&recv, Value::Int(_) | Value::Float(_))
            || {
                #[cfg(feature = "bignum")]
                { matches!(&recv, Value::BigInt(_)) }
                #[cfg(not(feature = "bignum"))]
                { false }
            };
        if recv_is_numeric && &*name == "coerce" {
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            let arg = &args[0];
            let recv_class: &str = match &recv {
                Value::Int(_) => "Integer",
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => "Integer",
                Value::Float(_) => "Float",
                _ => unreachable!("guarded by recv_is_numeric"),
            };
            // Pair: [coerced_other, coerced_self]. Float dominates
            // — any pair containing a Float collapses both sides
            // to Float. Otherwise both stay Integer (Int and
            // BigInt are the same Ruby class; pass through
            // unchanged).
            let (other_v, self_v) = match (&recv, arg) {
                (Value::Float(_), Value::Float(_)) => (arg.clone(), recv.clone()),
                (Value::Float(s), Value::Int(o)) => {
                    (Value::Float(*o as f64), Value::Float(*s))
                }
                (Value::Int(s), Value::Float(_)) => {
                    (arg.clone(), Value::Float(*s as f64))
                }
                #[cfg(feature = "bignum")]
                (Value::Float(s), Value::BigInt(id)) => {
                    let o_f = crate::vm::bignum::bigint_to_f64_sign_preserving(self.heap.bigint(*id));
                    (Value::Float(o_f), Value::Float(*s))
                }
                #[cfg(feature = "bignum")]
                (Value::BigInt(id), Value::Float(_)) => {
                    let s_f = crate::vm::bignum::bigint_to_f64_sign_preserving(self.heap.bigint(*id));
                    (arg.clone(), Value::Float(s_f))
                }
                (Value::Int(_), Value::Int(_)) => (arg.clone(), recv.clone()),
                #[cfg(feature = "bignum")]
                (Value::Int(_), Value::BigInt(_))
                | (Value::BigInt(_), Value::Int(_))
                | (Value::BigInt(_), Value::BigInt(_)) => (arg.clone(), recv.clone()),
                _ => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} can't be coerced into {}",
                            crate::vm::numeric::type_name_for_coerce(arg),
                            recv_class,
                        ),
                    }));
                }
            };
            // GC root hole: both `other_v` and `self_v` may carry
            // pass-through BigInt ObjIds whose only live root at this
            // point is the Rust local (recv / args were drained from
            // the stack on the way in). Without the PinGuard,
            // `maybe_gc()` runs with those ObjIds unreachable and
            // sweeps the BigInt — leaving the result Array with a
            // dangling slot. Pin both Values across the alloc; drop
            // restores normal GC reachability via the freshly-pushed
            // `Value::Array(id)` on the stack.
            let arr_id = {
                let mut g = PinGuard::new(self);
                g.pin(other_v.clone());
                g.pin(self_v.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(vec![other_v, self_v]))
            };
            self.stack.push(Value::Array(arr_id));
            return Ok(());
        }
        if let Value::Int(_) = &recv && &*name == "digits" && args.len() > 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len(),
                ),
            }));
        }
        if let Value::Int(n) = &recv && &*name == "digits" && args.len() <= 1 {
            let base: i64 = match args.first() {
                None => 10,
                Some(Value::Int(b)) => *b,
                // BigInt base under bignum: `n` is i64-sized and
                // any BigInt that survived `bigint_to_value`'s
                // demote-on-fit is necessarily > i64::MAX in
                // magnitude. So `|n| < base` always holds and the
                // result is a single-element array (n or 0 after
                // the negative-recv check). Validate the base
                // sign here — negative BigInt is "negative radix"
                // matching the i64 path's text.
                #[cfg(feature = "bignum")]
                Some(Value::BigInt(id)) => {
                    if self.heap.bigint(*id).sign() == num_bigint::Sign::Minus {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "negative radix".to_string(),
                        }));
                    }
                    if *n < 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "out of domain".to_string(),
                        }));
                    }
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::Array(vec![Value::Int(*n)]));
                    self.stack.push(Value::Array(id));
                    return Ok(());
                }
                Some(other) => return Err(self.trap(RubyError::TypeError {
                    // Share the same class-name helper as the
                    // BigInt-receiver path in `Vm::try_integer_digits`
                    // so cross-profile error text agrees ("nil",
                    // "true", "false" vs `Value::type_name`'s
                    // "NilClass", "Boolean").
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        crate::vm::numeric::type_name_for_coerce(other),
                    ),
                })),
            };
            if base < 0 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "negative radix".to_string(),
                }));
            }
            if base < 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("invalid radix {}", base),
                }));
            }
            if *n < 0 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "out of domain".to_string(),
                }));
            }
            let mut elems: Vec<Value> = Vec::new();
            let mut m = *n;
            if m == 0 {
                elems.push(Value::Int(0));
            } else {
                while m > 0 {
                    elems.push(Value::Int(m % base));
                    m /= base;
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `Object#equal?` — identity comparison. For heap-managed
        // receivers, same `ObjId`; for inline values, same content.
        // CRuby never overrides this on subclasses, so we always
        // intercept (above class-lookup would be redundant work).
        if &*name == "equal?" && args.len() == 1 {
            let same = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) => a == b,
                (Value::Array(a), Value::Array(b)) => a == b,
                (Value::Hash(a), Value::Hash(b)) => a == b,
                (Value::Range(a), Value::Range(b)) => a == b,
                (Value::Block(a), Value::Block(b)) => a == b,
                (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
                // String is now Rc-shared and identity-bearing
                // (frozen flag, aliasing). `equal?` should reflect
                // Rc-pointer identity, not content equality.
                (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b),
                // BigInt is heap-allocated; `equal?` is ObjId
                // identity, matching CRuby (where two separately-
                // allocated Bignums with the same magnitude are
                // distinct objects). Without this arm BigInt fell
                // through to the value-equality default, so
                // `(2**64).equal?(2**64)` (two distinct allocs)
                // wrongly returned true.
                #[cfg(feature = "bignum")]
                (Value::BigInt(a), Value::BigInt(b)) => a == b,
                // Other heap-allocated variants — `equal?` is
                // ObjId / Rc-pointer identity. Pre-fix these fell
                // through to ruby_eq, which has no arms for them
                // and returned false even for self-comparison
                // (`m = obj.method(:foo); m.equal?(m)` was false).
                // Mirrors the BigInt/Array/Hash arms above.
                (Value::BoundMethod(a), Value::BoundMethod(b)) => a == b,
                (Value::UnboundMethod(a), Value::UnboundMethod(b)) => a == b,
                (Value::CurriedProc(a), Value::CurriedProc(b)) => a == b,
                #[cfg(feature = "regex")]
                (Value::Regex(a), Value::Regex(b)) => Rc::ptr_eq(a, b),
                // Immediates (Int, Float, Sym, Bool, Nil) — fall
                // back on ruby_eq (value equality).
                _ => recv.ruby_eq(&args[0], &self.heap),
            };
            self.stack.push(Value::Bool(same));
            return Ok(());
        }
        // Universal `Object#eql?` fallback. Per-type type-strict
        // numeric overrides (`Integer#eql?`, `Float#eql?`,
        // `BigInt#eql?`) live in `primitive_call` arms above and
        // would have fired before reaching here. By the time
        // control gets here no per-type arm matched, so delegate
        // to `ruby_eq`:
        //  - String / Array / Hash / Range: value equality
        //    (matches CRuby's Array#eql? / Hash#eql? overrides
        //    that compare elementwise). Minor divergence at the
        //    nested-numeric leaf where CRuby's element-wise eql?
        //    distinguishes `[5].eql?([5.0])` from `[5] == [5.0]`;
        //    we use the `==`-flavoured ruby_eq for elements, so
        //    both come out true. Acceptable for now — the common
        //    cases (same-shape containers, same-string lookups)
        //    all match CRuby.
        //  - Object / BoundMethod / UnboundMethod / CurriedProc /
        //    Block / BigInt: ObjId identity via ruby_eq's
        //    per-variant arms (matches CRuby's Kernel#eql?
        //    default, which is identity for user objects).
        //  - Class: Rc::ptr_eq via ruby_eq.
        //  - Sym / Bool / Nil: identity == value equality for
        //    immediates.
        // Universal `respond_to?(:eql?)` already returns true via
        // the universal whitelist.
        if &*name == "eql?" {
            // Arity guard fires regardless of receiver — CRuby
            // raises ArgumentError before doing any per-type
            // dispatch. Primitive_call's per-type arms above only
            // match exact 1-arg shape, so we know arity must
            // mismatch if control reaches this `eql?` block with
            // != 1 arg.
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            let same = recv.ruby_eq(&args[0], &self.heap);
            self.stack.push(Value::Bool(same));
            return Ok(());
        }
        // Universal `hash` arity guard — fires only after
        // per-type arms in primitive_call have rejected the
        // wrong-arity call. The per-type arms (Int/Float/BigInt
        // /String) only match the exact 0-arg shape, so arity
        // mismatch reaches here. We don't dispatch hash itself
        // universally (not every receiver supports it), but we
        // DO raise ArgumentError for receivers that do —
        // identified by `responds_to(:hash)`. Without the
        // `responds_to` check, this would also fire on
        // `obj.hash(:x)` where obj doesn't support hash at all
        // (CRuby: NoMethodError for the missing method, not
        // ArgumentError for arity). Use the existing whitelist
        // to make the distinction.
        if &*name == "hash" && !args.is_empty() {
            let name_id = self.interner.intern("hash");
            if self.responds_to(&recv, name_id) {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len(),
                    ),
                }));
            }
            // Falls through to NoMethodError below.
        }
        // `Method#==` / `UnboundMethod#==` — intercept before the
        // universal `==` fallback (which has no arm for these and
        // would return `false`).
        //
        // BoundMethod: same name_id AND receiver identity. Heap-
        // backed recvs compare by ObjId / Rc-pointer; primitives
        // (Int / Sym / Bool / ...) compare by value. This matches
        // CRuby, where `s1.method(:length) == s2.method(:length)`
        // is `false` for distinct String instances but `true` for
        // the same Integer literal.
        //
        // UnboundMethod: lookup both classes' Method records via
        // `lookup_method_uncached` (walks ancestor chain) and
        // compare by Rc-pointer. Two UnboundMethods that resolve
        // to the same underlying definition — e.g., a parent's
        // method inherited by a subclass — are equal, matching
        // CRuby's `C.instance_method(:foo) == D.instance_method(:foo)`.
        if args.len() == 1 && &*name == "=="
            && matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_)) {
                let other = &args[0];
                let result = match (&recv, other) {
                    (Value::BoundMethod(a), Value::BoundMethod(b)) => {
                        let (ra, na) = self.heap.bound_method(*a);
                        let ra = ra.clone();
                        let (rb, nb) = self.heap.bound_method(*b);
                        let rb = rb.clone();
                        na == nb && method_recv_identity(&ra, &rb)
                    }
                    (Value::UnboundMethod(a), Value::UnboundMethod(b)) => {
                        let (ca, na) = self.heap.unbound_method(*a);
                        let (cb, nb) = self.heap.unbound_method(*b);
                        let ma = self.lookup_method_uncached(&ca, na);
                        let mb = self.lookup_method_uncached(&cb, nb);
                        match (ma, mb) {
                            (Some(x), Some(y)) => Rc::ptr_eq(&x, &y),
                            _ => na == nb && Rc::ptr_eq(&ca, &cb),
                        }
                    }
                    _ => false,
                };
                self.stack.push(Value::Bool(result));
                return Ok(());
            }
        // `Object#==` / `Object#!=` cross-type fallback. The
        // per-type primitive arms (`String == String`,
        // `Sym == Sym`, `Class == Class`, etc.) all fired earlier
        // in this dispatch. Anything that reaches here is a
        // cross-type comparison (`"x" == nil`, `nil == :foo`,
        // `[] == ""`) — those return `false` in CRuby, not
        // NoMethodError. Same-type comparisons that we don't
        // have per-type arms for (e.g. `Array == Array`) get
        // value-equality via `ruby_eq`. Universal fallback —
        // never raises — so it must go before NoMethodError.
        if args.len() == 1 && (&*name == "==" || &*name == "!=") {
            let eq = recv.ruby_eq(&args[0], &self.heap);
            let result = if &*name == "==" { eq } else { !eq };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `===` case-equality. Used by `case/when` desugaring.
        // Per-type semantics:
        //   Range#=== → include? (numeric containment)
        //   Class#=== → instance-of (walks ancestor chain)
        //   everything else → `==` value equality
        // User classes can override `===` via class-method
        // lookup, which fires above this fallback (no shadowing
        // needed since the universal check is the last resort).
        if &*name == "===" && args.len() == 1 {
            let arg = &args[0];
            let result = match &recv {
                Value::Range(rid) => {
                    // Generic numeric containment: coerce both
                    // bounds and the arg to Float so Int/Float
                    // mixes (5 in 1..10, 5.0 in 0..10, 5 in 0.0..10.0)
                    // all work. Strings / Symbols compare
                    // lexicographically — handled below.
                    let r = self.heap.range(*rid);
                    #[cfg(feature = "bignum")]
                    let to_f64 = |v: &Value| -> Option<f64> {
                        match v {
                            Value::Int(n) => Some(*n as f64),
                            Value::Float(f) => Some(*f),
                            // BigInt-to-f64 via the decimal-string
                            // round-trip — adequate for the
                            // include?/cover? containment check
                            // (Float comparison is already lossy),
                            // and avoids importing a `ToPrimitive`
                            // trait for one use. Without this arm a
                            // BigInt-bounded range fails the to_f64
                            // pass and falls into the lex fallback,
                            // which also lacked BigInt support.
                            Value::BigInt(id) => self.heap.bigint(*id).to_string().parse::<f64>().ok(),
                            _ => None,
                        }
                    };
                    #[cfg(not(feature = "bignum"))]
                    let to_f64 = |v: &Value| -> Option<f64> {
                        match v {
                            Value::Int(n) => Some(*n as f64),
                            Value::Float(f) => Some(*f),
                            _ => None,
                        }
                    };
                    let excl = r.exclusive;
                    
                    match (to_f64(&r.begin), to_f64(&r.end), to_f64(arg)) {
                        (Some(b), Some(e), Some(v)) => {
                            if excl { v >= b && v < e }
                            else { v >= b && v <= e }
                        }
                        _ => {
                            // Non-numeric: fall back to lexicographic
                            // compare using value_cmp_v if both bounds
                            // and the arg are the same comparable type.
                            let b = &r.begin; let e = &r.end;
                            let ge_lo = value_cmp_v_heap(arg, b, &self.interner, &self.heap)
                                .map(|o| o != std::cmp::Ordering::Less)
                                .unwrap_or(false);
                            let cmp_hi = value_cmp_v_heap(arg, e, &self.interner, &self.heap);
                            let le_hi = match cmp_hi {
                                Some(o) => if excl { o == std::cmp::Ordering::Less }
                                           else { o != std::cmp::Ordering::Greater },
                                None => false,
                            };
                            ge_lo && le_hi
                        }
                    }
                }
                Value::Class(target) => {
                    // Walk the argument's class chain looking for
                    // an Rc-identical match with `target`. For
                    // built-in receivers, look up the stub class
                    // by interned type name.
                    let start: Option<Rc<Class>> = match arg {
                        Value::Object(id) => Some(self.heap.class_of(*id)),
                        _ => {
                            let class_val = self.class_of(arg);
                            if let Value::Class(c) = class_val { Some(c) } else { None }
                        }
                    };
                    let mut cur = start;
                    let mut hit = false;
                    while let Some(cls) = cur {
                        if Rc::ptr_eq(&cls, target) { hit = true; break; }
                        cur = cls.superclass.borrow().clone();
                    }
                    hit
                }
                #[cfg(feature = "regex")]
                Value::Regex(re) => match arg {
                    // CRuby: `Regexp#===` (used by `case/when`) sets
                    // `$~`/`$1`.. on hit and clears them on miss,
                    // just like `=~`/`String#match`. Switch from
                    // `is_match` to `captures` so the side-channel
                    // sees the same view through every entry point.
                    // Keep `with_str_lossy` for the miss path's
                    // zero-alloc happy case (a String whose bytes
                    // are already valid UTF-8 borrows through the
                    // closure without allocating). Only materialize
                    // an owned `input` String inside the Some arm.
                    Value::Str(s) => s.with_str_lossy(|input| match re.captures(input) {
                        Some(caps) => {
                            let m0 = caps.get(0).unwrap();
                            let (m_start, m_end) = (m0.start(), m0.end());
                            let whole = m0.as_str().to_string();
                            let last_caps: Vec<Option<String>> = (1..caps.len())
                                .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                                .collect();
                            self.last_match = Some(crate::vm::LastMatch {
                                whole,
                                caps: last_caps,
                                input: input.to_string(),
                                m_start,
                                m_end,
                            });
                            true
                        }
                        None => {
                            self.last_match = None;
                            false
                        }
                    }),
                    _ => false,
                },
                _ => recv.ruby_eq(arg, &self.heap),
            };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `=~` — Regex/String matching. Returns the byte offset of
        // the first match, or nil. On a hit, populate `last_match`
        // (with captures) so `$~` and `$1`..`$N` (any positive
        // index — multi-digit forms like `$10` work too) see the
        // same match; on a miss, clear it (CRuby parity — a failed
        // `=~` wipes the prior match's globals).
        if &*name == "=~" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                #[cfg(feature = "regex")]
                (Value::Regex(re), Value::Str(s)) | (Value::Str(s), Value::Regex(re)) => {
                    let bound = s.to_string_lossy();
                    match re.captures(&bound) {
                        Some(caps) => {
                            let m0 = caps.get(0).unwrap();
                            let (m_start, m_end) = (m0.start(), m0.end());
                            let whole = m0.as_str().to_string();
                            let last_caps: Vec<Option<String>> = (1..caps.len())
                                .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                                .collect();
                            drop(caps);
                            self.last_match = Some(crate::vm::LastMatch {
                                whole,
                                caps: last_caps,
                                input: bound,
                                m_start,
                                m_end,
                            });
                            Value::Int(m_start as i64)
                        }
                        None => {
                            self.last_match = None;
                            Value::Nil
                        }
                    }
                }
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#<=>` fallback for `Value::Object` receivers. The
        // per-type primitive_call arms above handle every built-in
        // lhs (Int / Float / Str / Bool / Nil — Sym lives in
        // sym_primitive). When we reach here on `<=>`, the only
        // remaining lhs shape is `Value::Object` whose class
        // didn't define `<=>`. CRuby's default `Object#<=>`
        // returns `0` if the two values are identical (in our
        // model: same `ObjId`) and `nil` otherwise. User-defined
        // `<=>` on a class already fired via class-method-lookup
        // earlier, so we don't shadow.
        if &*name == "<=>" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) if a == b => Value::Int(0),
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#class` — universal, no args. Returns the Class
        // associated with the receiver. For built-in types it's
        // the stub class registered by the preamble; for user
        // instances it's the instance's stored class.
        if &*name == "class" && args.is_empty() {
            let c = self.class_of(&recv);
            self.stack.push(c);
            return Ok(());
        }
        // `Object#object_id` / `BasicObject#__id__` — universal,
        // no args. Delegates to `object_id_for` (defined at the
        // bottom of this file). The encoding contract:
        //   - CRuby-exact for nil/true/false/Int (4 / 20 / 0 /
        //     `n*2+1`).
        //   - High-bit type discriminators for everything else
        //     (bit 62 = heap, 61 = Sym, 60 = Float). These bit
        //     positions are unreachable by `n*2+1` for any
        //     practical integer literal (`|n| < 2^58`), so
        //     cross-type collisions are eliminated by
        //     construction.
        //   - 4-bit type subtag at bits 58..61 distinguishes
        //     heap variants (Object vs Array vs Hash etc.),
        //     leaving a 58-bit payload that fits both u32
        //     ObjId and 48-bit virtual pointers natively.
        if (&*name == "object_id" || &*name == "__id__") && args.is_empty() {
            let id = object_id_for(&recv);
            self.stack.push(Value::Int(id));
            return Ok(());
        }
        // Arity guard for the Object-extras family. All four
        // take zero arguments; CRuby raises ArgumentError on
        // extra args regardless of whether a block is present,
        // so check before the per-method arms to keep the
        // error type consistent. Without this guard
        // `42.tap(1)` falls through to NoMethodError, hiding
        // the real mistake.
        if matches!(&*name, "itself" | "tap" | "then" | "yield_self") && !args.is_empty() {
            return Err(self.trap(crate::error::RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len()
                ),
            }));
        }
        // `Object#itself` — universal, no args. Returns the
        // receiver unchanged. Common with `group_by(&:itself)`
        // and other Symbol#to_proc idioms. CRuby ignores any
        // attached block (`obj.itself { ... }` still returns
        // obj); see the block-form fast path in
        // `collection_call_block` (vm/iter.rs) for that case.
        if &*name == "itself" && args.is_empty() {
            self.stack.push(recv);
            return Ok(());
        }
        // `Object#dup` / `Object#clone` — universal shallow
        // copy. Primitive arms in vm/string.rs / vm/array.rs /
        // vm/hash.rs intercept their own receivers earlier in
        // dispatch; this arm catches everything else.
        //
        // Immediates (Int/Float/Sym/Bool/Nil) return self —
        // CRuby's `5.dup`, `nil.dup`, `:foo.dup` all return the
        // receiver unchanged since Ruby 2.4. Plain `Value::Object`
        // gets a fresh Instance with the same class and a
        // shallow-cloned ivar table; the singleton class is NOT
        // copied (CRuby's `dup` discards singleton methods, and
        // `clone` properly copies them — we don't model the
        // copy yet so both arms drop singletons. Documented
        // divergence — Tier-2 follow-up alongside the
        // `clone(freeze:)` kwarg).
        //
        // Arity: zero positional args for `dup`; `clone`
        // accepts a `freeze:` kwarg in CRuby that we don't
        // route yet — extra args fall to the wrong-arity arm
        // below.
        if matches!(&*name, "dup" | "clone") && args.is_empty() {
            let copied = match &recv {
                Value::Int(_)
                | Value::Float(_)
                | Value::Sym(_)
                | Value::Bool(_)
                | Value::Nil => recv.clone(),
                // CRuby treats Integer as immediate-like for
                // dup/clone regardless of Fixnum/Bignum
                // representation — `(10**100).dup.equal?(...)`
                // returns true. We don't have to allocate a
                // fresh heap slot for Bignum; returning the
                // same Value is identity-preserving and matches
                // user expectations.
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => recv.clone(),
                Value::Object(oid) => {
                    let (cls, ivars) = match self.heap.get(*oid) {
                        crate::heap::HeapObj::Instance(inst) => {
                            (inst.class.clone(), inst.ivars.clone())
                        }
                        // TypedData (cext-allocated) carries no
                        // ivar table on the rubyrs side; punt to
                        // the fallback below until a caller
                        // surfaces a need.
                        _ => {
                            return Err(self.trap(RubyError::NoMethodError {
                                kind: crate::error::NoMethodErrorKind::Missing,
                                method: format!("undefined method '{}' called", &*name),
                                recv_type: std::borrow::Cow::Owned(
                                    crate::vm::numeric::class_name_for_error(&recv).to_string(),
                                ),
                            }));
                        }
                    };
                    self.maybe_gc();
                    self.check_alloc()?;
                    let new_id = self.heap.alloc(HeapObj::Instance(crate::value::Instance {
                        class: cls,
                        ivars,
                        singleton_class: None,
                    }));
                    Value::Object(new_id)
                }
                // Range/Block/Method/Regex/BigInt/etc.: no
                // shallow-copy support yet. Surface a clear
                // NoMethodError rather than silently returning
                // self — a future commit can add per-variant
                // copy logic as use cases land.
                _ => {
                    return Err(self.trap(RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::Missing,
                        method: format!("undefined method '{}' called", &*name),
                        recv_type: std::borrow::Cow::Owned(
                            crate::vm::numeric::class_name_for_error(&recv).to_string(),
                        ),
                    }));
                }
            };
            self.stack.push(copied);
            return Ok(());
        }
        if matches!(&*name, "dup" | "clone") {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len()
                ),
            }));
        }
        // `Object#tap` / `#then` / `#yield_self` without a
        // block — the block-taking forms are handled by
        // `collection_call_block` (vm/iter.rs). Reaching this
        // arm means no block was passed; CRuby raises
        // LocalJumpError for `tap`, while `then`/`yield_self`
        // would normally return an Enumerator. rubyrs has no
        // Enumerator type yet, so for now both raise
        // LocalJumpError uniformly — documented divergence,
        // less surprising than silent NoMethodError.
        if args.is_empty() && matches!(&*name, "tap" | "then" | "yield_self") {
            return Err(self.trap(crate::error::RubyError::LocalJumpError {
                msg: format!("no block given (yield)"),
            }));
        }
        // `Object#frozen?` — universal, no args.
        // CRuby treats all immediates (Integer, Float, Symbol,
        // true, false, nil) as always-frozen. Str/Array/Hash/Regex
        // have their own primitive arms earlier in dispatch and
        // never reach here. For plain user-class instances we
        // return false (we don't model a freeze bit on
        // Value::Object yet).
        if &*name == "frozen?" && args.is_empty() {
            let frozen = matches!(
                &recv,
                Value::Int(_)
                    | Value::Float(_)
                    | Value::Sym(_)
                    | Value::Bool(_)
                    | Value::Nil
            );
            self.stack.push(Value::Bool(frozen));
            return Ok(());
        }
        // `Object#to_s` / `Object#inspect` — universal default.
        // For plain Object instances, CRuby renders as
        // `"#<ClassName:0xADDR>"`. We can't expose real addresses
        // (sandbox), so use the object_id hex form. Primitive
        // arms for Str/Int/Sym/Array/Hash run earlier in dispatch
        // and shadow this, and `Value::Class` is handled by
        // `primitive_call` (vm/primitive.rs). Any receiver type
        // without a specialized `to_s`/`inspect` handler falls
        // through here — that includes plain `Object` instances
        // but also BoundMethod / UnboundMethod / CurriedProc /
        // future heap variants we add without a custom default.
        if (&*name == "to_s" || &*name == "inspect") && args.is_empty() {
            // Range has no primitive to_s/inspect arm of its own.
            // Without this short-circuit the universal
            // `#<Range:0xHEX>` form below would silently win for
            // Range and diverge from CRuby. `to_display` /
            // `to_inspect` in heap.rs already render Range with
            // the correct endpoint-quoting and endless/beginless
            // handling — funnel through them so the Array#inspect
            // path (which also calls `to_inspect`) stays
            // consistent.
            if matches!(&recv, Value::Range(_) | Value::Rational(_)) {
                // Range and Rational both render via
                // `to_display`/`to_inspect` — Rational#to_s is
                // `"num/den"`, #inspect is `"(num/den)"`. Without
                // this short-circuit the universal `#<Class:0xHEX>`
                // fallback wins and rendering diverges from CRuby.
                let rendered = if &*name == "inspect" {
                    recv.to_inspect(&self.heap, &self.interner)
                } else {
                    recv.to_display(&self.heap, &self.interner)
                };
                self.stack.push(Value::new_str(rendered));
                return Ok(());
            }
            // BoundMethod / UnboundMethod: render
            //   `#<Method: RecvClass#name(params)>`
            //   `#<Method: RecvClass(DefiningClass)#name(params)>`
            //   `#<UnboundMethod: DefiningClass#name(params)>`
            // mirroring CRuby's form. The source-location suffix
            // (`path:line`) CRuby tacks on is omitted — we don't
            // track per-method definition location yet. Without
            // this short-circuit the universal `#<Method:0xHEX>`
            // fallback wins, losing the receiver/owner class and
            // method name that defensive logging idioms rely on.
            if let Value::BoundMethod(bid) = &recv {
                let (recv_v, name_id, params, defining_rc) = {
                    let (rv, nid, snap) = self.heap.bound_method_full(*bid);
                    let params = snap
                        .as_ref()
                        .map(|m| format_method_params(&self.protos[m.proto_idx]))
                        .unwrap_or_default();
                    let defining_rc = snap
                        .as_ref()
                        .and_then(|m| m.defining_class.as_ref())
                        .and_then(|w| w.upgrade());
                    (rv.clone(), nid, params, defining_rc)
                };
                let method_name = self.interner.resolve(name_id).to_string();
                // Singleton methods (`def obj.foo`): defining
                // class IS the receiver's eigenclass shell. CRuby
                // renders these as `#<RecvClass:0xHEX>.foo(...)`
                // with a `.` separator instead of `#`. Detect by
                // ptr-eq: `class_of(obj_id)` returns the eigenclass
                // when one is installed, so if it matches the
                // method's defining_class we're looking at a
                // singleton method.
                // Singleton iff: receiver has an eigenclass
                // installed AND defining_class IS that
                // eigenclass. The first conjunct distinguishes
                // singleton methods from regular methods —
                // without it, every method on a singleton-less
                // object would also satisfy
                // `class_of == defining_class`.
                let is_singleton = match (&recv_v, &defining_rc) {
                    (Value::Object(id), Some(def)) => {
                        let cls = self.heap.class_of(*id);
                        let real = self.heap.real_class_of(*id);
                        !std::rc::Rc::ptr_eq(&cls, &real)
                            && std::rc::Rc::ptr_eq(&cls, def)
                    }
                    _ => false,
                };
                let s = if is_singleton {
                    // `#<Method: #<A:0xHEX>.foo(params)>` — receiver
                    // rendered as its real class (skip the
                    // eigenclass) plus a stable hex identity.
                    let real_class = match &recv_v {
                        Value::Object(id) => self.heap.real_class_of(*id).name.clone(),
                        _ => "Object".to_string(),
                    };
                    let oid = object_id_for(&recv_v);
                    format!(
                        "#<Method: #<{}:0x{:016x}>.{}({})>",
                        real_class, oid, method_name, params
                    )
                } else {
                    let recv_class = match self.class_of(&recv_v) {
                        Value::Class(c) => c.name.clone(),
                        _ => "Object".to_string(),
                    };
                    let defining_name = defining_rc.map(|c| c.name.clone());
                    let class_part = match defining_name {
                        Some(d) if d != recv_class => format!("{}({})", recv_class, d),
                        _ => recv_class,
                    };
                    format!("#<Method: {}#{}({})>", class_part, method_name, params)
                };
                self.stack.push(Value::new_str(s));
                return Ok(());
            }
            if let Value::UnboundMethod(uid) = &recv {
                let (class_name, name_id, params) = {
                    let (cls, nid, snap) = self.heap.unbound_method_full(*uid);
                    let params = snap
                        .as_ref()
                        .map(|m| format_method_params(&self.protos[m.proto_idx]))
                        .unwrap_or_default();
                    // CRuby prints the class where the method was
                    // *defined*, not the class it was captured on:
                    // `B.instance_method(:foo).inspect` shows
                    // `A#foo` when foo is inherited from A. Fall
                    // back to the captured class when the snap is
                    // absent or the Weak ref has been collected.
                    let defining = snap
                        .as_ref()
                        .and_then(|m| m.defining_class.as_ref())
                        .and_then(|w| w.upgrade())
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| cls.name.clone());
                    (defining, nid, params)
                };
                let method_name = self.interner.resolve(name_id).to_string();
                let s = format!("#<UnboundMethod: {}#{}({})>", class_name, method_name, params);
                self.stack.push(Value::new_str(s));
                return Ok(());
            }
            let cls_name = match self.class_of(&recv) {
                Value::Class(c) => c.name.clone(),
                _ => "Object".to_string(),
            };
            let oid = object_id_for(&recv);
            let s = format!("#<{}:0x{:016x}>", cls_name, oid);
            self.stack.push(Value::new_str(s));
            return Ok(());
        }
        // Phase C.1 Rational readers / conversions. Lives here in
        // dispatch.rs (not primitive_call) because the stateless
        // primitive layer can't read the heap-stored RationalRepr.
        // Arithmetic + comparison whitelist expansion lands in
        // Phase C.2.
        if let Value::Rational(id) = &recv {
            let r = *self.heap.rational(*id);
            match (&*name, args.len()) {
                ("numerator", 0) => {
                    self.stack.push(Value::Int(r.num));
                    return Ok(());
                }
                ("denominator", 0) => {
                    self.stack.push(Value::Int(r.den));
                    return Ok(());
                }
                ("to_r", 0) => {
                    self.stack.push(recv.clone());
                    return Ok(());
                }
                ("to_i", 0) => {
                    // CRuby `to_i` / `to_int` for Rational truncates
                    // toward zero (NOT floor). `(7/2r).to_i == 3`,
                    // `(-7/2r).to_i == -3`.
                    self.stack.push(Value::Int(r.num / r.den));
                    return Ok(());
                }
                ("to_f", 0) => {
                    self.stack.push(Value::Float(r.num as f64 / r.den as f64));
                    return Ok(());
                }
                // Arity guards for the readers — they take no args.
                ("numerator" | "denominator" | "to_r" | "to_i" | "to_f", _) => {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0)",
                            args.len(),
                        ),
                    }));
                }
                _ => {}
            }
        }
        // `Object#hash` — universal, no args. Returns an integer
        // hash. For value types (Int/Str/Sym/Bool/Nil), hash by
        // content so `{1 => :a}[1] == :a` works. For heap objects
        // where equality is identity, hash by object_id.
        if &*name == "hash" && args.is_empty() {
            // Single source of truth — `object_hash` handles all
            // per-variant salt and recursive container hashing
            // (Array order-sensitive, Hash order-insensitive)
            // with cycle detection. See its doc for the type-tag
            // table.
            let v = object_hash(&recv, &self.heap);
            self.stack.push(Value::Int(v));
            return Ok(());
        }
        // `Object#respond_to?(name)` — pure feature detection, no
        // invocation. Goes last so user classes that override
        // `respond_to?` (we don't support that yet, but conceptually)
        // would shadow this. Accepts either a `Symbol` or a `String`
        // argument; anything else falls through to NoMethodError.
        // `respond_to?(:foo)` or `respond_to?(:foo, include_private)`.
        // CRuby's second arg toggles whether private methods count;
        // we don't enforce method visibility precisely in the lookup
        // path used here, so the bool is effectively ignored — the
        // check passes through to `responds_to` which already walks
        // the method table without filtering by visibility. Accepting
        // the 2-arg form lets feature-detection patterns like
        // `respond_to?(:deprecate_constant, true)` work without
        // tripping NoMethodError.
        if &*name == "respond_to?" {
            // Arity check matches the no-recv path: CRuby raises
            // ArgumentError on 0 args or 3+. Keeps the explicit-
            // receiver shape (`obj.respond_to?()`) from misreporting
            // as method_missing / NoMethodError.
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            // Type check matches the no-recv arm — CRuby raises
            // `TypeError: X is not a symbol nor a string` for any
            // other arg[0] type, before reaching method_missing.
            let lookup_name: SymId = match &args[0] {
                Value::Sym(id) => *id,
                Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "{} is not a symbol nor a string",
                        other.to_inspect(&self.heap, &self.interner),
                    ),
                })),
            };
            let yes = self.responds_to(&recv, lookup_name);
            self.stack.push(Value::Bool(yes));
            return Ok(());
        }
        if self.try_method_missing(&recv, name_id, args.clone(), None)? {
            return Ok(());
        }
        // Kernel module-function fallback: CRuby's `Kernel#Array`,
        // `Kernel#Integer`, `Kernel#Float`, `Kernel#String`,
        // `Kernel#sprintf`, `Kernel#format` are private instance
        // methods on Kernel (included in Object). With an
        // explicit receiver CRuby raises NoMethodError-private,
        // which lets `method_missing` intercept; only if NO
        // `method_missing` is defined does the call actually
        // surface as NoMethodError. We model the latter half
        // here: when normal lookup AND method_missing miss, route
        // to `builtin_call`. This sits AFTER `try_method_missing`
        // so a user `method_missing` wins (matches CRuby), and
        // before NoMethodError so sinatra's
        // `codes.flat_map(&method(:Array))` shape (sinatra/base.rb
        // :1404) — `method(:Array)` captures, `.call` re-dispatches
        // through here with no user method_missing — succeeds.
        // (TRY_RUNS layer #25.)
        //
        // `eval` is intentionally NOT in this set: with-recv
        // `obj.eval(...)` would silently discard the receiver
        // (Kernel#eval ignores it), which is surprise-driven.
        // CRuby raises NoMethodError-private here. The
        // `method(:eval).call(src)` route still works via the
        // no_recv `builtin_call` at the top of do_call.
        // (code-review #267 #3.)
        if matches!(name.as_ref(),
            "Array" | "Integer" | "Float" | "String"
            | "sprintf" | "format"
        ) && let Some(res) = self.builtin_call(name.as_ref(), &args) {
            let v = res?;
            // Mirror the flag handling in the no_recv builtin
            // path (line 452-459): clears
            // `suppress_call_result_push` if set; unconditionally
            // pushing would corrupt the rescue handler's stack
            // (Copilot review #267 round 1).
            if self.suppress_call_result_push {
                self.suppress_call_result_push = false;
            } else {
                self.stack.push(v);
            }
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            kind: crate::error::NoMethodErrorKind::Missing,
            method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&recv)),
        }))
    }



    pub(crate) fn invoke_method(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>) -> Result<(), Trap> {
        self.invoke_method_with_block(m, self_val, args, None)
    }

    fn try_invoke_fixed_method_from_stack(
        &mut self,
        m: Rc<Method>,
        self_val: Value,
        argc: usize,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        if m.closure.is_some() {
            return Ok(false);
        }
        let fixed = match m.fixed_arity {
            Some(fixed) if fixed.required as usize == argc => fixed,
            _ => return Ok(false),
        };
        self.check_frames()?;
        let n_locals = fixed.n_locals as usize;
        let locals = if n_locals == argc {
            match argc {
                0 => Vec::new(),
                1 => vec![
                    self.stack
                        .pop()
                        .expect("ICE: fixed method fast path arg underflow"),
                ],
                _ => {
                    let split = self.stack.len() - argc;
                    self.stack.drain(split..).collect()
                }
            }
        } else {
            let mut locals = vec_nil(n_locals);
            for slot in (0..argc).rev() {
                locals[slot] = self
                    .stack
                    .pop()
                    .expect("ICE: fixed method fast path arg underflow");
            }
            locals
        };
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals: Rc::new(RefCell::new(locals)),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: block,
            defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
            is_block: false,
            n_given_positional: fixed.required,
            rescues: vec![],
            loop_rescue_depths: vec![],
            loop_stack_depths: vec![],
        });
        Ok(true)
    }



    pub(crate) fn invoke_method_with_block(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>, block: Option<ObjId>) -> Result<(), Trap> {
        // Builtin-method short-circuit: synthesised Methods on
        // Kernel (and any future host class with similar
        // reflection records) carry a `builtin: Some(...)` payload
        // that supplies introspection metadata. Their `proto_idx`
        // is a placeholder (`0`) and must not be executed as
        // bytecode — re-enter `do_call`/`do_call_block` with the
        // primitive's real name so the inline arm handles dispatch
        // (`obj.class`, `obj.is_a?(X)`, ...).
        if let Some(meta) = &m.builtin {
            // Synth Method dispatch routes back through `do_call`
            // with the primitive's real name. The synth lives only
            // in `Vm.kernel_builtin_metas` (not on Kernel.methods),
            // so the chain-walking sites below won't re-find it
            // and we don't need a skip flag — `obj.class`'s normal
            // inline arm fires naturally.
            let name_id = meta.name_id;
            let argc = args.len();
            self.stack.push(self_val);
            if let Some(bid) = block {
                self.stack.push(Value::Block(bid));
                for a in args { self.stack.push(a); }
                return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
            } else {
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
            }
        }
        // `define_method`-installed methods carry a captured Rc and
        // diverge from the normal fresh-locals path: their frame
        // *shares* `captured` with the lexical scope that created
        // the block. Writes to outer-scope locals from inside the
        // method body propagate back, matching CRuby semantics.
        if let Some(cl) = &m.closure {
            let given = args.len();
            let n_params = cl.n_params as usize;
            if given != n_params {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected {})", given, n_params),
                }));
            }
            self.check_frames()?;
            let proto_idx = m.proto_idx;
            let proto_n_locals = self.protos[proto_idx].n_locals as usize;
            let param_start = cl.param_start as usize;
            // Block params live *after* the captured frame's n_locals
            // (block locals layout inherits the parent — see ADR 0004).
            // Resize the shared Vec if a previous invocation hasn't
            // already grown it.
            {
                let mut caps = cl.captured.borrow_mut();
                let need = param_start.max(proto_n_locals);
                if caps.len() < need {
                    caps.resize(need, Value::Nil);
                }
                for (i, a) in args.into_iter().enumerate() {
                    caps[param_start + i] = a;
                }
            }
            self.frames.push(Frame {
                proto_idx,
                ip: 0,
                locals: cl.captured.clone(),
                self_val,
                base_sp: self.stack.len(),
                is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()), is_block: false,
                // `define_method` enforces exact arity (no
                // defaults), so all params are "given".
                n_given_positional: given as u16,
                rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
            });
            return Ok(());
        }
        if let Some(fixed) = m.fixed_arity
            && args.len() == fixed.required as usize
        {
            self.check_frames()?;
            let mut locals = args;
            locals.resize(fixed.n_locals as usize, Value::Nil);
            self.frames.push(Frame {
                proto_idx: m.proto_idx,
                ip: 0,
                locals: Rc::new(RefCell::new(locals)),
                self_val,
                base_sp: self.stack.len(),
                is_class_body: false,
                swap_return: None,
                block_arg: block,
                defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
                is_block: false,
                n_given_positional: fixed.required,
                rescues: vec![],
                loop_rescue_depths: vec![],
                loop_stack_depths: vec![],
            });
            return Ok(());
        }
        // Default-argument support (literal defaults only): a Proto
        // carries a `defaults` vec parallel to `params`. `None`
        // entries are required; `Some(v)` entries can be omitted by
        // the caller and the slot is filled from the literal at
        // invocation time. Required params always come before
        // optionals in source order, so the legal arg-count range
        // is `[required, params.len()]`.
        //
        // Rest-param (`*args`) — m.params holds the positional
        // names; the rest-name (if any) is in proto.rest_param.
        // Excess args past `params.len()` collect into an Array
        // bound to the rest slot. With a rest param there's no
        // upper bound on the arg count.
        let proto = &self.protos[m.proto_idx];
        let has_rest = proto.rest_param.is_some();
        let has_kw_rest = proto.kw_rest_param.is_some();
        let has_block_param = proto.block_param.is_some();
        let kw_count = proto.kw_param_defaults.len();
        // Layout of `m.params` tail:
        //   [...positional..., rest?, ...kw_params..., kw_rest?, block_param?]
        let positional_max = m.params.len()
            - (if has_rest { 1 } else { 0 })
            - kw_count
            - (if has_kw_rest { 1 } else { 0 })
            - (if has_block_param { 1 } else { 0 });
        let required = proto.n_required_positional as usize;
        // Pop trailing Hash arg (if present and we expect kw
        // params) — those entries become keyword bindings, not
        // positional args.
        let mut args = args;
        let kw_hash: Option<Vec<(Value, Value)>> = if kw_count > 0 || has_kw_rest {
            if let Some(Value::Hash(hid)) = args.last().cloned() {
                args.pop();
                Some(self.heap.hash(hid).clone())
            } else {
                None
            }
        } else {
            None
        };
        let given = args.len();
        let arity_ok = if has_rest {
            given >= required
        } else {
            given >= required && given <= positional_max
        };
        if !arity_ok {
            let expected = if has_rest {
                format!("{}+", required)
            } else if required == positional_max {
                format!("{}", required)
            } else {
                format!("{}..{}", required, positional_max)
            };
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected {})", given, expected),
            }));
        }
        self.check_frames()?;
        let n_locals = proto.n_locals as usize;
        // Snapshot proto-derived data needed during arg binding,
        // dropping the immutable borrow on self.protos so the
        // subsequent maybe_gc / heap.alloc calls (for the rest
        // Array) can take &mut self.
        let kw_defaults_snapshot: Vec<Option<Value>> = proto.kw_param_defaults.clone();
        // Optional positional slots that the caller omitted stay
        // `Nil` here; the method body's entry prologue runs
        // `Op::JumpIfArgGiven(slot, skip)` + default-expr +
        // `Op::StoreLocal(slot)` per optional, evaluating any
        // expression (literal, prior param, constant lookup, full
        // method call). `frame.n_given_positional = positional_take`
        // is what the prologue consults to tell "caller-supplied"
        // from "left for default-eval".
        let mut locals = vec_nil(n_locals);
        // Bind up to positional_max args into positional slots; any
        // overflow flows into the rest slot as a fresh Array.
        let positional_take = given.min(positional_max);
        let mut args_iter = args.into_iter();
        for slot in locals.iter_mut().take(positional_take) {
            *slot = args_iter.next().unwrap();
        }
        if has_rest {
            // Remaining args (possibly empty) → fresh Array in the
            // rest slot.
            //
            // GC root hole guard: at this point everything we need
            // to survive `maybe_gc` lives only as Rust locals —
            // not in `self.stack`, `self.frames`, or `self.pinned`.
            // That covers:
            //   - `locals` — the not-yet-installed frame locals
            //     (already populated with positional + default args)
            //   - `rest_vec` — trailing args destined for the rest slot
            //   - `self_val` — the receiver. For inline-allocated
            //     receivers like `Ghost.new.poof`, the Object isn't
            //     bound to any caller local, so this window is the
            //     only thing keeping it alive
            //   - `block` (when Some) — heap-resident `BlockHandle`
            //     not yet attached to the new frame
            //   - `kw_hash` keys+values (when present) — the Hash
            //     contents were cloned out earlier; the per-pair
            //     Values may be heap-y and need to survive until
            //     the kw_count > 0 branch below reads them.
            //
            // Master commit 01b28ed shipped a narrower version of
            // this guard (pinning only `self_val` + `rest_vec`).
            // This widens it to `locals` / `block` / `kw_hash` and
            // adds the `check_alloc?` the original cut was missing
            // — a host configured with `max_heap_objects` would
            // otherwise see the rest-Array silently slip past the
            // cap, since `heap.alloc` itself doesn't enforce it.
            // The PinGuard's Drop pops on the early-return path of
            // `check_alloc?` too, so adding the check is safe.
            let rest_vec: Vec<Value> = args_iter.collect();
            let rest_slot = positional_max;
            let arr_id = {
                let mut g = PinGuard::new(self);
                for v in &locals { g.pin(v.clone()); }
                for v in &rest_vec { g.pin(v.clone()); }
                g.pin(self_val.clone());
                if let Some(id) = block { g.pin(Value::Block(id)); }
                if let Some(kw) = &kw_hash {
                    for (k, v) in kw {
                        g.pin(k.clone());
                        g.pin(v.clone());
                    }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(rest_vec))
            };
            locals[rest_slot] = Value::Array(arr_id);
        }
        // Bind keyword params. kw names live at the tail of
        // m.params; for each, look up the corresponding key in
        // the kw_hash (Symbol-keyed). Missing required keyword
        // → ArgumentError. Missing optional → use literal default.
        let kw_start = positional_max + if has_rest { 1 } else { 0 };
        if kw_count > 0 {
            for (i, (default, kw_name)) in kw_defaults_snapshot.iter()
                .zip(m.params[kw_start..kw_start + kw_count].iter())
                .enumerate()
            {
                let key_sym = self.interner.intern(kw_name);
                let key_val = Value::Sym(key_sym);
                let found = kw_hash.as_ref().and_then(|h| {
                    h.iter().find(|(k, _)| k.ruby_eql(&key_val, &self.heap))
                        .map(|(_, v)| v.clone())
                });
                match (found, default) {
                    (Some(v), _) => locals[kw_start + i] = v,
                    (None, Some(d)) => locals[kw_start + i] = d.clone(),
                    (None, None) => return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("missing keyword: :{}", kw_name),
                    })),
                }
            }
        }
        // **kw_rest binding. Take the kw_hash entries whose keys
        // weren't claimed by a named kw_param above and collect
        // them into a fresh Hash bound to the kw_rest slot. With
        // no kw_hash at all (caller passed no kwargs), the slot
        // still gets a fresh empty Hash so `**opts` reliably yields
        // a Hash to user code. The known-names set is built from
        // the same kw_param name slice we just zipped over.
        if has_kw_rest {
            let kw_rest_slot = kw_start + kw_count;
            let known_keys: Vec<Value> = m.params[kw_start..kw_start + kw_count]
                .iter()
                .map(|nm| Value::Sym(self.interner.intern(nm)))
                .collect();
            let leftover: Vec<(Value, Value)> = match &kw_hash {
                Some(h) => h.iter()
                    .filter(|(k, _)| !known_keys.iter().any(|kk| kk.ruby_eql(k, &self.heap)))
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            // Same GC root-hole pattern as the rest-arg path above
            // (and the master Array#zip / Hash#sort_by chain fixed
            // in earlier PRs): `locals` / `self_val` / `block` /
            // `kw_hash` / `leftover` are Rust locals, NOT on
            // vm.stack / pinned, so the explicit `maybe_gc()` here
            // sweeps any heap-backed values they reference. Pin
            // everything participating in the new Hash alloc + the
            // already-bound locals through the alloc point.
            //
            // Master shipped the kw_rest code without this guard
            // (commits 680dbef "Module include chain + is_a?" /
            // ed0b872 "nested block destructure"); STRESS_GC tests
            // `anon_kwrest` and `kwrest_args` were the canary.
            let hid = {
                let mut g = PinGuard::new(self);
                for v in &locals { g.pin(v.clone()); }
                g.pin(self_val.clone());
                if let Some(id) = block { g.pin(Value::Block(id)); }
                if let Some(kw) = &kw_hash {
                    for (k, v) in kw {
                        g.pin(k.clone());
                        g.pin(v.clone());
                    }
                }
                for (k, v) in &leftover {
                    g.pin(k.clone());
                    g.pin(v.clone());
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(leftover)))
            };
            locals[kw_rest_slot] = Value::Hash(hid);
        }
        // `&blk` named block param: bind the caller's block (if any)
        // into the trailing block_param slot as `Value::Block(id)`,
        // or `Value::Nil` if no block was passed. The slot lives at
        // the very end of `params` after kw_rest (see Proto.block_param
        // / compile_proto for layout).
        if has_block_param {
            let block_slot = positional_max
                + if has_rest { 1 } else { 0 }
                + kw_count
                + if has_kw_rest { 1 } else { 0 };
            locals[block_slot] = match block {
                Some(id) => Value::Block(id),
                None => Value::Nil,
            };
        }
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals: Rc::new(RefCell::new(locals)),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()), is_block: false,
            // Drives the body's default-arg prologue. Slots
            // `[0, positional_take)` came from the caller; slots
            // `[positional_take, positional_max)` are left Nil
            // here and the prologue's `Op::JumpIfArgGiven` skips
            // the default-eval for the former, executes it for
            // the latter.
            n_given_positional: positional_take as u16,
            rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
        });
        Ok(())
    }



    /// `obj.instance_eval { |o| ... }` / `cls.class_eval { |c| ... }`
    /// — invoke the block with `self` swapped to `new_self`.
    ///
    /// When `as_class_body` is true (the `class_eval` case),
    /// we also push `cls` onto `class_stack` + a fresh
    /// `Public` visibility entry, and mark the new frame
    /// `is_class_body: true`. That re-uses the existing
    /// class-body machinery so `def name; …; end` inside the
    /// block lands on the receiver class's method table — the
    /// dominant DSL use of `class_eval`. The cost: per the
    /// existing class-body Return semantics
    /// (`vm/step.rs::Op::Return`), the frame returns the class
    /// itself rather than the block's last expression. CRuby
    /// returns the block value; we'll need a non-`is_class_body`
    /// path to match exactly when a real use-case appears (see
    /// SUBSET.md). For `instance_eval` (`as_class_body=false`)
    /// the frame is a normal block, so the block's last
    /// expression is the return value — that part matches CRuby.
    ///
    /// `instance_eval { def name; ...; end }` defines a
    /// *singleton* method on the receiver in CRuby. rubyrs
    /// doesn't model singleton classes yet; `def` inside an
    /// `instance_eval` block lands on `toplevel_methods` (the
    /// same documented divergence as `attr_*` / `alias_method` /
    /// `define_method` outside a class body — see SUBSET.md's
    /// PoC caveat list). Real uses of `instance_eval` in our
    /// niche (configuration DSLs) typically read state rather
    /// than define methods, so this is acceptable for now.
    pub(crate) fn invoke_block_with_self(
        &mut self,
        block_id: ObjId,
        new_self: Value,
        as_class_body: bool,
        args: Vec<Value>,
    ) -> Result<(), Trap> {
        self.check_frames()?;
        let (proto_idx, captured, param_start, n_params) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
        };
        // Bind args into the block's param slots, same auto-splat
        // shape as `invoke_block`. For instance_eval/class_eval
        // the conventional arg is a single value (self), so the
        // single-Array auto-splat case is unlikely to trigger,
        // but we keep the rule identical to avoid surprising
        // future callers.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            for (i, a) in args.into_iter().enumerate() {
                if i < n_params as usize {
                    locals[param_start as usize + i] = a;
                }
            }
        }
        if as_class_body {
            // class_eval: re-use the class-body machinery so
            // `def` inside the block goes onto cls's method
            // table. Mirrors what `Op::DefClass` does at the
            // top of a `class X ... end` body. The return-path
            // handlers in vm/step.rs pop both stacks when this
            // frame returns, keyed off `is_class_body: true`.
            if let Value::Class(cls) = &new_self {
                self.class_stack.push(cls.clone());
                self.class_visibility_stack.push(crate::value::Visibility::Public);
            } else {
                // Caller checked Type before getting here, so
                // this is a programmer-error path. ICE rather
                // than silent-corruption: the class_stack pop
                // on frame return would underflow.
                panic!("ICE: invoke_block_with_self as_class_body=true requires Value::Class new_self");
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: captured,
            self_val: new_self,
            base_sp: self.stack.len(),
            is_class_body: as_class_body,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            // class_eval's frame is BOTH `is_block: true` and
            // `is_class_body: true`. That dual role matters for
            // non-local `return`: per the unwind loop in
            // `vm/step.rs` (Op::ReturnMethod's branch), a
            // `return` inside the block walks back through
            // is_block frames to find the enclosing method.
            // With `is_block: false` the class_eval frame would
            // be the target itself — `return` would return *from
            // class_eval* rather than the enclosing method,
            // diverging from CRuby. The matching unwind change
            // (pop class_stack/visibility_stack when walking
            // past a `is_block && is_class_body` frame) lives
            // in `vm/step.rs`.
            is_block: true,
            n_given_positional: 0,
            rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
        });
        Ok(())
    }

    /// Wrap a callable Value (BoundMethod, CurriedProc, ...) into
    /// a fresh `Value::Block` so it can be passed wherever a
    /// block is expected. Lazily compiles a single shared
    /// forwarder proto on first call; subsequent calls reuse the
    /// same proto index. The synthesised BlockHandle stashes the
    /// callable in `captured[0]` and uses the proto's rest slot
    /// to splat the caller's args into a `.call(...)` on it.
    /// Caller must pass a value whose `.call` dispatch is
    /// already wired up (currently BoundMethod and CurriedProc).
    pub(crate) fn coerce_callable_to_block(&mut self, callable: Value)
        -> Result<crate::value::ObjId, Trap>
    {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        use crate::heap::HeapObj;
        use std::cell::RefCell;

        // Lazy proto build. Locals layout:
        //   slot 0: the callable (captured)
        //   slot 1: args Array (rest slot, filled by invoke_block)
        let proto_idx = if let Some(idx) = self.callable_forwarder_proto {
            idx
        } else {
            let call_id = self.interner.intern("call");
            let proto = Proto {
                name: "<callable-forwarder>".to_string(),
                params: Vec::new(),
                n_required_positional: 0,
                rest_param: None,
                kw_param_defaults: Vec::new(),
                kw_rest_param: None,
                block_param: None,
                n_locals: 2,
                code: vec![
                    Op::LoadLocal(0),
                    Op::LoadLocal(1),
                    Op::ApplyCall(call_id, u16::MAX),
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 4],
                filename: "<synthetic>".into(),
                // Synthetic forwarder protos have no body-
                // introduced locals; every slot they touch is
                // either filled at invoke time (block params /
                // rest) or written by the proto's own emitted
                // ops. `u16::MAX` skips the per-invocation reset.
                block_body_local_start: u16::MAX,
                byte_literals: Vec::new(),
                const_chains: Vec::new(),
                lexical_scope: Vec::new(),
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            self.callable_forwarder_proto = Some(idx);
            idx
        };

        // captured[0] = the callable; captured[1] left to
        // invoke_block to populate with the rest Array.
        //
        // Pin the callable across maybe_gc — the Rc<RefCell<Vec>>
        // we just built is a Rust-local with no GC root yet (the
        // Block that would own it isn't alloc'd until after the
        // maybe_gc). Without the pin, STRESS_GC sweeps the
        // callable's slot between Vec construction and Block alloc;
        // the new Block alloc reuses the freed slot, and the
        // captured ObjId silently points at the Block itself —
        // invoke_block then panics when `.call` dispatches.
        let captured = Rc::new(RefCell::new(vec![callable.clone(), Value::Nil]));
        let mut g = crate::vm::PinGuard::new(self);
        g.pin(callable);
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let id = g.vm.heap.alloc(HeapObj::Block(crate::value::BlockHandle {
            proto_idx,
            captured,
            self_val: Value::Nil,
            param_start: 0,
            n_params: 0,
            rest_slot: Some(1),
        }));
        Ok(id)
    }

    /// Build a `Value::Block` that, when called with `*args`,
    /// invokes `outer.call(inner.(*args))`. Used by Method#`>>` /
    /// `<<` to express function composition. Both sides must be
    /// callable (BoundMethod or Block); validated by the caller.
    /// Proto is lazy-built and shared across all composition sites.
    pub(crate) fn coerce_compose_to_block(
        &mut self,
        outer: Value,
        inner: Value,
    ) -> Result<crate::value::ObjId, Trap> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        use crate::heap::HeapObj;
        use std::cell::RefCell;

        // Locals layout:
        //   slot 0: outer callable (runs second)
        //   slot 1: inner callable (runs first)
        //   slot 2: args Array (filled via rest_slot)
        let proto_idx = if let Some(idx) = self.method_compose_forwarder_proto {
            idx
        } else {
            let call_id = self.interner.intern("call");
            let proto = Proto {
                name: "<method-compose-forwarder>".to_string(),
                params: Vec::new(),
                n_required_positional: 0,
                rest_param: None,
                kw_param_defaults: Vec::new(),
                kw_rest_param: None,
                block_param: None,
                n_locals: 3,
                code: vec![
                    Op::LoadLocal(0),                   // [outer]
                    Op::LoadLocal(1),                   // [outer, inner]
                    Op::LoadLocal(2),                   // [outer, inner, args]
                    Op::ApplyCall(call_id, u16::MAX),   // [outer, inner_result]
                    Op::Call(call_id, 1, u16::MAX),     // [outer_result]
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 6],
                filename: "<synthetic>".into(),
                // Synthetic forwarder protos have no body-
                // introduced locals; every slot they touch is
                // either filled at invoke time (block params /
                // rest) or written by the proto's own emitted
                // ops. `u16::MAX` skips the per-invocation reset.
                block_body_local_start: u16::MAX,
                byte_literals: Vec::new(),
                const_chains: Vec::new(),
                lexical_scope: Vec::new(),
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            self.method_compose_forwarder_proto = Some(idx);
            idx
        };
        let captured = Rc::new(RefCell::new(vec![outer.clone(), inner.clone(), Value::Nil]));
        let mut g = crate::vm::PinGuard::new(self);
        g.pin(outer);
        g.pin(inner);
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let id = g.vm.heap.alloc(HeapObj::Block(crate::value::BlockHandle {
            proto_idx,
            captured,
            self_val: Value::Nil,
            param_start: 0,
            n_params: 0,
            rest_slot: Some(2),
        }));
        Ok(id)
    }

    pub(crate) fn invoke_block(&mut self, block_id: ObjId, args: Vec<Value>) -> Result<(), Trap> {
        self.check_frames()?;
        // Snapshot what we need out of the block's heap slot before
        // taking any `&mut self` action. BlockHandle.captured is a
        // shared `Rc<RefCell<Vec<Value>>>` — cheap to clone.
        let (proto_idx, captured, self_val, param_start, n_params, rest_slot) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.self_val.clone(),
             bh.param_start, bh.n_params, bh.rest_slot)
        };
        // CRuby auto-splat: when a block declared with >1 parameter
        // is called with a single Array argument, the Array's
        // elements are spread into the parameter slots. The most
        // common ergonomic surfaces this enables:
        //   arr_of_pairs.each { |a, b| ... }       # arr = [[1,2], [3,4]]
        //   hash.each_with_index { |(k, v), i| }   # pair + index
        //   hash.to_a.sort_by { |k, v| v }         # pair after Hash#to_a
        // Hash#each / #map already yield two args directly, so this
        // path doesn't change their behaviour. Single-param blocks
        // also unaffected — they bind the whole Array.
        //
        // Auto-splat doesn't apply to rest-param blocks — `|*args|`
        // wants to capture the whole arg list, including a single
        // Array as-is.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 && rest_slot.is_none() {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        // Build the rest Array (if any) BEFORE taking the locals
        // borrow — heap.alloc needs &mut self.heap, which conflicts
        // with the captured.borrow_mut() below.
        //
        // GC rooting: at this point the caller has popped the
        // Value::Block(block_id) off the operand stack (see
        // `do_call_block` and the Block.call arm in `do_call`),
        // so the only live reference to the block + its captured
        // Vec is this fn's `block_id` parameter — *not* a GC root.
        // Without pinning, the maybe_gc below would sweep the
        // Block's heap slot and (transitively) every captured
        // BoundMethod/Block held inside `captured`. The new alloc
        // could reuse the freed slot, and the forwarder would
        // dispatch through a dangling ObjId. Reproduced under
        // STRESS_GC=1 by `proc_curry_compose.rb`'s `(succ >> m).(4)`
        // — composing a Block with a BoundMethod produces a
        // compose-forwarder Block with `rest_slot = Some(2)`, so
        // this branch fires; the Squared instance held inside
        // `m`'s BoundMethod gets swept between pop and the
        // recursive `m.call`, panicking later at heap.rs's
        // `class_of called on non-Object slot`.
        let rest_array_val = if let Some(slot) = rest_slot {
            let rest_args: Vec<Value> = args.iter().skip(n_params as usize).cloned().collect();
            // Truncate args to the leading required slots — the
            // overflow now lives in rest_args.
            let mut g = crate::vm::PinGuard::new(self);
            g.pin(Value::Block(block_id));
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::Array(rest_args));
            Some((slot, Value::Array(id)))
        } else {
            None
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        let body_local_start = proto.block_body_local_start;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            // Reset body-introduced block-local slots before
            // rebinding params. CRuby's "block-locals are fresh
            // each invocation" semantics: a variable
            // first-assigned inside the block body (e.g.
            // `y = 100 if cond`, `n ||= 0`, plain `tmp = expr`)
            // sees `nil` at the top of every call, even when an
            // earlier invocation assigned it. Outer-scope
            // variables (slot index < parent.n_locals at compile
            // time) and the block's own params keep their
            // values across invocations because their slot
            // indices sit below `body_local_start`.
            //
            // `block_body_local_start == u16::MAX` is the
            // sentinel for "not a block-shaped proto" — set by
            // `ProtoBuilder::build` and by the cext synthetic
            // forwarders. The branch is also a no-op when the
            // block body assigned no new locals (start equals
            // n_locals).
            if (body_local_start as usize) < needed {
                for slot in body_local_start as usize..needed {
                    locals[slot] = Value::Nil;
                }
            }
            // Place args into the block's required param slots.
            // CRuby's arity-mismatch semantics: too few args →
            // leftover slots bind to Nil. Overflow past n_params
            // either flows into the rest slot (handled below) or
            // is silently dropped (block-arity-permissive default).
            let mut it = args.into_iter();
            for i in 0..n_params as usize {
                locals[param_start as usize + i] = it.next().unwrap_or(Value::Nil);
            }
            if let Some((slot, val)) = rest_array_val {
                locals[slot as usize] = val;
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: captured,
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None,
            is_block: true, n_given_positional: 0, rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
        });
        Ok(())
    }



    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        // Consume `bypass_visibility_once` at the dispatch boundary
        // — same reasoning as `do_call`. `do_call_block` itself
        // has no visibility-check site today (block-form
        // private/protected enforcement is a pre-existing gap), so
        // the consumed value is mostly there to prevent leaking
        // past the block-form `send`/`__send__` re-aim into the
        // next unrelated call. The `&nil` arm below re-installs
        // it before delegating to `do_call`, which DOES enforce
        // visibility — so `send(:priv, &nil)` still bypasses.
        let bypass_visibility = self.take_bypass_visibility();
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let block_val = self.stack.pop().expect("ICE: stack underflow before block");
        let block = match block_val {
            Value::Block(id) => id,
            // `&method_object` forwarding (K8): coerce the
            // BoundMethod into a Block via `to_proc` semantics.
            // Synthesises a vararg-lambda whose captured locals
            // hold the BoundMethod; when invoked, it does
            // `m.call(*args)`. See `coerce_callable_to_block`.
            Value::BoundMethod(bm_id) => self.coerce_callable_to_block(Value::BoundMethod(bm_id))?,
            // `&curried_proc` — a curried proc is still a Proc in
            // CRuby, so `&` on it forwards as a block. Same shape
            // as the BoundMethod arm: the synthesised forwarder
            // does `cp.call(*args)`, and `CurriedProc#call`
            // (dispatch.rs:1159) handles arity-completion / partial
            // application from there.
            Value::CurriedProc(cp_id) => self.coerce_callable_to_block(Value::CurriedProc(cp_id))?,
            // `foo(&nil)` in CRuby is equivalent to `foo` without
            // a block. Common shape: `def render(&block);
            // evaluate(&block); end` invoked without a block ⇒
            // `block` is Nil, the `&block` forwarding becomes
            // `evaluate(&nil)`. Restore args to the stack and
            // delegate to the no-block dispatch path.
            Value::Nil => {
                // Re-install the visibility-bypass flag we consumed
                // at entry. `send(:priv_method, &nil)` should still
                // bypass visibility — without this, `do_call` would
                // raise NoMethodError on a private method because
                // its own bypass slot is now `false`.
                self.bypass_visibility_once = bypass_visibility;
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            // Anything else (Int / Str / ...) is a real type error
            // — CRuby raises `TypeError: wrong argument type X
            // (expected Proc)`, where X is the class name (e.g.
            // "Integer", "TrueClass", or a user class), NOT
            // `type_name()`'s short tag ("Boolean", etc.). Use
            // `class_of` so the message matches CRuby for booleans
            // and user instances.
            other => {
                let class_name = match self.class_of(&other) {
                    Value::Class(c) => c.name.clone(),
                    _ => other.type_name().to_string(),
                };
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "wrong argument type {} (expected Proc)",
                        class_name,
                    ),
                }));
            }
        };
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before block receiver"))
        };

        // Bare `instance_exec { ... }` inside an instance method —
        // `recv` is None, so the receiver-form arm below won't see
        // it. Dispatch on `self` from the current frame, mirroring
        // `self.instance_exec(&block)`. Same override-precedence
        // probe as the receiver-form arm so a user-defined
        // `instance_exec` still wins.
        if no_recv && &*name == "instance_exec" {
            let self_val = self.frames.last().expect("ICE: do_call_block no frame").self_val.clone();
            let user_override = match &self_val {
                Value::Object(id) => {
                    let cls = self.heap.class_of(*id);
                    self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                }
                Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                _ => match self.class_of(&self_val) {
                    Value::Class(cls) => self.lookup_method_cached(&cls, name_id, cache_id).is_some(),
                    _ => false,
                },
            };
            if !user_override {
                self.invoke_block_with_self(block, self_val, /*as_class_body=*/false, args)?;
                return Ok(());
            }
            // User override exists — re-shape stack as receiver form
            // (`recv, block, args...`) and re-enter so the normal
            // dispatch finds and invokes the user method.
            let argc = args.len();
            self.stack.push(self_val);
            self.stack.push(Value::Block(block));
            for a in args { self.stack.push(a); }
            return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
        }

        // `bm.call(args, &block)` — the block-form counterpart to
        // the no-block BoundMethod#call arm in `do_call` (line
        // ~1969). Without this, calling a stored `Method` with a
        // block (`@scan_line.call(@src, &block)` — ERB's
        // lib/erb/compiler.rb:147 pattern) raises NoMethodError
        // because the fallthrough never sees Method as a valid
        // receiver. Re-shape the stack as
        // `bm_recv, block, args...` (the order do_call_block
        // expects — see the push sequence below) and recursively
        // dispatch through `do_call_block` so the underlying
        // method receives the block argument.
        if let Some(Value::BoundMethod(bid)) = &recv
            && matches!(&*name, "call" | "[]" | "()") {
            let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id, method } => {
                    (recv.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            // Snapshot fast path — invoke directly with the
            // attached block, matching the no-block BoundMethod#call
            // arm's parity with capture-then-remove-then-call.
            if let Some(m) = bm_method {
                self.invoke_method_with_block(m, bm_recv, args, Some(block))?;
                return Ok(());
            }
            // do_call_block entry expects stack layout
            // `recv, block, args...` (drain last `argc` for args,
            // then pop block, then pop recv). Push in that order.
            let argc_new = args.len();
            self.stack.push(bm_recv);
            self.stack.push(Value::Block(block));
            for a in args { self.stack.push(a); }
            return self.do_call_block(bm_name_id, argc_new, false, u16::MAX);
        }
        // `ubm.bind_call(recv, *args, &block)` — block-form parallel
        // of the no-block bind_call arm in `try_dispatch_callable_intrinsics`
        // (line ~690). That arm runs via `do_call`'s pre-block
        // dispatch path and never sees a block argument; tilt's
        // `method.bind_call(scope, **locals, &block)` (template.rb:
        // ~392) passes one, which lands here. Without this arm
        // the call raises NoMethodError even though the blockless
        // shape succeeds.
        if let Some(Value::UnboundMethod(uid)) = &recv && &*name == "bind_call" {
            if args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "wrong number of arguments (given 0, expected 1..)".into(),
                }));
            }
            let (cap_class, cap_name_id, cap_method) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id, method } => {
                    (class.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args;
            let target = args.remove(0);
            // Dispatch class for Object targets — mirrors the
            // eigenclass-aware capture in unbind so a
            // singleton-method UnboundMethod can bind_call back
            // to its original receiver.
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind_call argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            // Same is_a fence as the no-block path: Kernel sentinel
            // and any Module captured class are exempt; Class
            // capture is strict.
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind_call argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            let m = match cap_method.or_else(|| self.lookup_method_uncached(&cap_class, cap_name_id)) {
                Some(m) => m,
                None => {
                    let mname = self.interner.resolve(cap_name_id).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cap_class.name),
                    }));
                }
            };
            self.invoke_method_with_block(m, target, args, Some(block))?;
            return Ok(());
        }

        // P2-13: `block` (now an ObjId in a Rust local) is no
        // longer rooted after popping off the stack. Each native
        // iterator driver (`iter_array_filter`, the inline
        // `each` / `map` arms, etc.) pins the block alongside its
        // source receiver, so we don't need a guard at the
        // dispatch boundary itself. The `invoke_method_with_block`
        // path on the no_recv / Object-recv branches doesn't
        // trigger GC before installing the block as the frame's
        // `block_arg`, so the gap is safe there too.
        //
        // `Hash.new { |h, k| ... }` interception. Parallel to the
        // no-block arm in `do_call`. The block becomes the Hash's
        // default-block (stored in `HashObj.default_block` for GC
        // and access). `Hash#[]` consults this slot on missing
        // keys and invokes the block with `(self_hash, key)` —
        // tilt's `Hash.new { |h, k| h[k] = [] }` auto-vivifies.
        //
        // `Hash.new(default) { block }` is an ArgumentError in
        // CRuby ("wrong number of arguments (given 1, expected 0)"
        // from Hash#initialize when both default-arg and block are
        // given). Mirror that explicitly so callers don't see the
        // misleading generic Class.new fallback behaviour.
        // `Module.new { |m| ... }` — anonymous Module with the
        // block evaluated as the module body (`class_eval`-style).
        // The block also receives the new module as its sole arg
        // for explicit-reference shapes like `Module.new { |m|
        // m.define_method(:foo) { ... } }`. Sits BEFORE the
        // `Hash.new` intercept so the Module-class-receiver path
        // isn't swallowed by a hypothetical future shared
        // pattern.
        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Module"
        {
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                // User-defined Module.new singleton wins, parallel
                // to the Hash precedence rule below.
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len(),
                    ),
                }));
            }
            // Build the fresh module shell. Same field set as the
            // no-block `do_call` arm; lifted here so the block can
            // run inside the module body and the result push lands
            // on this control path.
            let new_mod = std::rc::Rc::new(Class {
                name: String::new(),
                is_module: true,
                ivars: std::cell::RefCell::new(HashMap::new()),
                methods: std::cell::RefCell::new(HashMap::new()),
                singleton_methods: std::cell::RefCell::new(HashMap::new()),
                superclass: std::cell::RefCell::new(None),
                includes: std::cell::RefCell::new(Vec::new()),
                prepends: std::cell::RefCell::new(Vec::new()),
                singleton_prepends: std::cell::RefCell::new(Vec::new()),
                singleton_view: std::cell::RefCell::new(None),
                singleton_target: std::cell::RefCell::new(None),
                class_vars: std::cell::RefCell::new(HashMap::new()),
                #[cfg(feature = "cext")]
                cext_alloc_func: std::cell::Cell::new(None),
            });
            let mod_val = Value::Class(new_mod);
            // `as_class_body=true` so `def name; …; end` inside
            // the block lands on the module's methods table. Same
            // machinery `class_eval` uses (`invoke_block_with_self`
            // pushes the module onto class_stack + sets
            // `is_class_body: true` on the new frame). The block
            // receives `mod_val` as its sole positional arg —
            // matches CRuby's `Module.new { |m| ... }` shape.
            self.invoke_block_with_self(
                block,
                mod_val.clone(),
                /*as_class_body=*/ true,
                vec![mod_val],
            )?;
            return Ok(());
        }
        // `Module#define_method(:name) { |args| body }` —
        // dynamically install a block-as-method on the receiver
        // class's instance-methods table. Mirrors the
        // `Op::DefMethodBlock` opcode's install logic but is
        // entered via runtime dispatch rather than a parsed
        // `def`. Both shapes accepted:
        //   - explicit receiver: `cls.define_method(:foo) { ... }`
        //     → recv = Some(Value::Class(target))
        //   - bare-call inside `class_eval do ... end` where
        //     self is the class:
        //     `cls.class_eval { define_method(:foo) { ... } }`
        //     → no_recv = true, frame self_val = the class.
        //     Sinatra/base.rb's `define_singleton` uses this
        //     shape; the block_arg `&content` becomes the
        //     attached block.
        //
        // Closure semantics match DefMethodBlock: the
        // BlockHandle's `captured` Rc is shared with the
        // installed Method so outer-scope locals stay live.
        // CRuby returns the method name as a Symbol.
        // (TRY_RUNS pass-9.7d layer #21.)
        if &*name == "define_method" {
            // Track whether we picked the target via explicit
            // receiver vs no_recv (bare call in class body). The
            // `class_visibility_stack` lexical-visibility lookup
            // below only makes sense for the no_recv path, where
            // the target IS the surrounding class body. For the
            // explicit-receiver path, the surrounding visibility
            // belongs to whatever class body we're currently in —
            // which may be unrelated to `target_cls`. Leaking the
            // caller's `private` onto methods installed on an
            // unrelated class diverges from CRuby
            // (code-review #245 round 7 #1).
            let (target_cls, explicit_recv) = match &recv {
                Some(Value::Class(c)) => (Some(c.clone()), true),
                None => {
                    let self_val = self.frames.last()
                        .expect("ICE: define_method no_recv with empty frames")
                        .self_val
                        .clone();
                    if let Value::Class(c) = self_val { (Some(c), false) } else { (None, false) }
                }
                _ => (None, false),
            };
            // Precedence rule (parallels `Module.new` / `Hash.new`):
            // a user-defined `def self.define_method(...)` on the
            // receiver (or its singleton-prepended chain) wins over
            // the built-in intrinsic. Without this check, override
            // attempts silently shadow into this arm. Only consult
            // when we actually resolved a target class — otherwise
            // fall through to normal dispatch (which will raise
            // NoMethodError on the non-Class receiver).
            if let Some(cls) = &target_cls
                && let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                let recv_val = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, recv_val, args, Some(block));
            }
            if let Some(target_cls) = target_cls {
                // Arity arrangement matches the no-block arm above:
                //   0       → wrong-arity ArgumentError
                //   1       → install the block (path below)
                //   2       → 2-arg Proc/UnboundMethod form NOT
                //             yet supported (even with a block,
                //             CRuby silently drops the block and
                //             uses the proc — too subtle to fake);
                //             raise NoMethodError so the caller
                //             gets a clear "not implemented" signal
                //   3+      → wrong-arity ArgumentError
                // CRuby's wording is `expected 1..2` even when a
                // block is attached, so we use the same message
                // across both arms (PR #245 Copilot round 6 #1).
                match args.len() {
                    1 => {}
                    2 => return Err(self.trap(RubyError::ArgumentError {
                        msg: "the 2-arg Proc/UnboundMethod form of `Module#define_method` is not yet supported by rubyrs Tier-1".into(),
                    })),
                    n => return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", n),
                    })),
                }
                let name_sym = match &args[0] {
                    Value::Sym(s) => *s,
                    Value::Str(s) => {
                        // Same `Config::max_symbols` cap as
                        // `parse_send_target` / `resolve_ivar_name_arg`
                        // — without this, untrusted code could grow
                        // the interner unbounded via
                        // `cls.define_method("dyn_#{i}") {}` in a loop.
                        // Existing symbols always re-resolve; only
                        // fresh names count against the cap.
                        let raw = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&raw) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        self.interner.intern(&raw)
                    }
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Symbol or String)",
                            other.type_name(),
                        ),
                    })),
                };
                let (proto_idx, captured, param_start, n_params) = {
                    let bh = self.heap.block(block);
                    (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
                };
                let proto = &self.protos[proto_idx];
                let params = proto.params.clone();
                // Explicit-receiver path: visibility defaults to
                // Public (the new method's target class doesn't
                // share lexical scope with the caller's visibility
                // stack). No-recv (bare call in class body): inherit
                // the surrounding class's current visibility, since
                // `define_method` and `def` should behave the same
                // way under `private` / `public` modifiers.
                let vis = if explicit_recv {
                    crate::value::Visibility::Public
                } else {
                    self.class_visibility_stack.last().copied()
                        .unwrap_or(crate::value::Visibility::Public)
                };
                let m = std::rc::Rc::new(crate::value::Method {
                    params,
                    proto_idx,
                    fixed_arity: None,
                    // When `target_cls` is an eigenclass shell from
                    // `Class#singleton_class`, the install redirects
                    // into the underlying real class's
                    // singleton_methods; `defining_class` has to
                    // resolve to the same real class so `super`
                    // walks the right ancestor chain.
                    // (Code-review #253 round 1 #1.)
                    defining_class: Some(std::rc::Rc::downgrade(&target_cls.effective_install_class())),
                    visibility: std::cell::Cell::new(vis),
                    closure: Some(crate::value::MethodClosure { captured, param_start, n_params }),
                    builtin: None,
                });
                target_cls.install_method(name_sym, m);
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Sym(name_sym));
                return Ok(());
            }
        }
        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Hash"
        {
            // Same precedence rule as `do_call`'s Hash.new no-
            // block path: a user `def self.new` on Hash (reopened
            // class) wins over the built-in default-block
            // intercept. CRuby treats `Class#new` as a regular
            // method; a reopen-and-override is just normal
            // method-resolution and should be honoured in block-
            // form too. Without this check, `class Hash; def
            // self.new(&b); ...; end; end; Hash.new { ... }`
            // silently returned `{}` from the hardcoded intercept
            // below.
            //
            // `do_call_block`'s generic Value::Class singleton-
            // method dispatch arm further down would catch this
            // for non-Hash classes, but it fires AFTER this Hash
            // intercept, so Hash specifically needs the explicit
            // pre-check.
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
                }));
            }
            // GC rooting: `block` was popped from the stack into a
            // Rust-local ObjId above. Until `hash_set_default_block`
            // installs it into the new Hash (which IS a GC root via
            // `self.stack.push` below), the block is unreachable
            // from the standard roots (stack / frames / pinned).
            // `maybe_gc` could sweep it between the alloc and the
            // store, leaving `hash_set_default_block` pointing at a
            // freed slot. Pin across both maybe_gc + alloc.
            let mut g = PinGuard::new(self);
            g.pin(Value::Block(block));
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
            g.vm.heap.hash_set_default_block(hid, Some(block));
            g.vm.stack.push(Value::Hash(hid));
            return Ok(());
        }
        // `instance_eval` / `class_eval` / `module_eval` — swap
        // `self` for the duration of the block. Intercepted here
        // so the receiver-type dispatch below can't claim them
        // first (e.g. a future `Object#instance_eval` primitive
        // would shadow this). `args.is_empty()` keeps us out of
        // the way of any hypothetical user-defined
        // `instance_eval(arg)` that someone might define.
        if let Some(r) = &recv {
            // `instance_exec(*args) { |*a| ... }` — like instance_eval
            // but the block receives the EXPLICIT args you pass
            // (not `self`). Same self-swap semantics. Variadic args,
            // including zero. Sinatra-shape DSL pattern:
            // `instance.instance_exec(&handler)` runs the captured
            // route block against a fresh request instance so `@ivar`
            // and helper methods (defined on the instance's class)
            // resolve through the swapped self.
            let is_instance_exec = &*name == "instance_exec";
            if is_instance_exec {
                // Override-precedence probe (parity with `send` /
                // `Hash.new` patterns nearby): only fall into the
                // builtin path when there's no user-defined
                // `instance_exec` on the receiver. Without this, a
                // `class C; def instance_exec(...); ...; end; end`
                // override (including on primitive classes like
                // `class String; def instance_exec; end; end`) would
                // be silently shadowed by the builtin.
                let user_override = match r {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                    }
                    Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                    // Primitives — consult the user-class table for
                    // the primitive's stub class (e.g. `String`,
                    // `Integer`). Mirrors the primitive-receiver
                    // fallback in `do_call` at ~line 3066.
                    _ => match self.class_of(r) {
                        Value::Class(cls) => self.lookup_method_cached(&cls, name_id, cache_id).is_some(),
                        _ => false,
                    },
                };
                if !user_override {
                    self.invoke_block_with_self(block, r.clone(), /*as_class_body=*/false, args)?;
                    return Ok(());
                }
            }
            let is_instance_eval = &*name == "instance_eval";
            let is_class_eval = &*name == "class_eval" || &*name == "module_eval";
            if (is_instance_eval || is_class_eval) && args.is_empty() {
                if is_class_eval && !matches!(r, Value::Class(_)) {
                    // Align with the existing wording for `include`
                    // (vm/dispatch.rs:171, :369) so error messages
                    // are consistent across the Module-receiver
                    // family.
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Module)",
                            r.type_name(),
                        ),
                    }));
                }
                // CRuby passes `self` as the sole block arg (so
                // `obj.instance_eval { |o| o == obj }` works);
                // mirror that. The single-arg matches the
                // common DSL shape `cls.class_eval { |k| ... }`.
                let block_args = vec![r.clone()];
                self.invoke_block_with_self(block, r.clone(), is_class_eval, block_args)?;
                return Ok(());
            }
            // String-arg form: `cls.class_eval(source [, file, line])`
            // — parse + compile + run the source. Tier 1 divergence:
            // does NOT switch to the receiver class's class-body
            // context (so bare `Foo.class_eval("def bar; end")`
            // lands `bar` at top level instead of on Foo). Tilt's
            // tilt-2.7.0 `eval_compiled_method` path self-wraps its
            // source in a nested `Tilt::TOPOBJECT.class_eval do
            // def __tilt_xxx; end end`, so the inner block-form
            // (intercepted above) does the actual class context
            // switching. Documented in docs/SUBSET.md.
            // CRuby parity: `class_eval`/`module_eval` is either
            // (a) block-only with 0 args (handled above) OR
            // (b) string-form with 1..3 args and NO block (handled
            // in do_call). The block+args combination raises
            // ArgumentError "wrong number of arguments (given N,
            // expected 0)". Without this guard, passing both
            // would fall through to NoMethodError.
            if is_class_eval && let Value::Class(cls) = r
                && !args.is_empty()
                && self.lookup_class_singleton_method(cls, name_id).is_none()
            {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len()
                    ),
                }));
            }
        }
        if let Some(r) = &recv
            && let Some(v) = self.collection_call_block(r, &name, &args, block)? {
                self.stack.push(v);
                return Ok(());
            }

        if no_recv {
            // `lambda { ... }` / `proc { ... }` / `Proc.new { ... }`-
            // style block-to-Value capture. rubyrs doesn't
            // distinguish Lambda from Proc at runtime (the strict-
            // arity check is the documented gap in SUBSET.md), so
            // both names just hand the attached block back as a
            // Value::Block. `args.is_empty()` keeps us out of the
            // way of user-defined `lambda(arg)` shapes if anyone
            // overrides the name.
            if args.is_empty() && (&*name == "lambda" || &*name == "proc") {
                self.stack.push(Value::Block(block));
                return Ok(());
            }
            if let Some(res) = self.builtin_call(&name, &args) {
                let v = res?;
                // See suppress_call_result_push doc on Vm —
                // mirrors the no_recv path above.
                if self.suppress_call_result_push {
                    self.suppress_call_result_push = false;
                } else {
                    self.stack.push(v);
                }
                return Ok(());
            }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = self.invoke_host_fn(host, &args)?;
                self.stack.push(v);
                return Ok(());
            }
            // Bare `send(:foo) { ... }` / `__send__(:foo) { ... }`
            // — same re-aim as the no_recv arm in `do_call`. See
            // there for the override + visibility rationale.
            if matches!(&*name, "send" | "__send__") {
                let frame_self = self.frames.last()
                    .expect("ICE: do_call_block(no_recv) with empty frames")
                    .self_val.clone();
                let user_override = &*name == "send" && match &frame_self {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                    }
                    Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                    _ => false,
                };
                if !user_override {
                    let target_sym = self.parse_send_target(&args)?;
                    let new_argc = args.len() - 1;
                    self.bypass_visibility_once = true;
                    self.stack.push(Value::Block(block));
                    for a in args.into_iter().skip(1) {
                        self.stack.push(a);
                    }
                    return self.do_call_block(target_sym, new_argc, true, u16::MAX);
                }
            }
            let self_val = self.frames.last().expect("ICE: do_call_block no frame").self_val.clone();
            if let Value::Object(id) = &self_val {
                let cls = self.heap.class_of(*id);
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                    self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                    return Ok(());
                }
            }
            // Block-form parallel of `do_call`'s user-singleton
            // bare-call resolution (~line 2439). Inside
            // `class Foo < Bar; foo do ... end; end`, bare `foo`
            // is dispatched on `self = Foo` and must walk Foo's
            // singleton chain (including Bar's, transitively) so
            // user-defined `def self.foo` and `class << self; def
            // foo; end; end` methods inherited from a parent class
            // resolve identically with or without an attached
            // block. Without this, the Sinatra-shape DSL
            // (`class App < Sinatra::Base; get '/' do ... end`)
            // dies at NoMethodError because the route registrar's
            // block triggers `do_call_block` instead of `do_call`,
            // and the existing block-form Class bridge below only
            // covers hardcoded primitive names.
            if let Value::Class(c) = &self_val
                && let Some(m) = self.lookup_class_singleton_method(c, name_id) {
                self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                return Ok(());
            }
            // Block-form parallel of `do_call`'s bare-call Class
            // bridge (see comments at the no_recv arm around
            // ~line 537). Without this, bare whitelisted Class
            // methods invoked with an attached block from inside
            // a class body would raise NoMethodError even though
            // their blockless counterparts dispatch correctly —
            // breaks the lockstep contract for the block form.
            // Stack restoration matches do_call_block's
            // `[..., recv, block, *args]` shape so re-entry
            // sees the receiver-form layout it expects.
            // PR #196 code-review #3.
            if let Value::Class(cls) = &self_val {
                let in_set = matches!(&*name,
                    "new" | "name" | "to_s" | "inspect"
                    | "method_defined?" | "instance_method" | "undef_method" | "remove_method"
                    | "superclass" | "ancestors" | "include?"
                    | "instance_methods" | "public_instance_methods"
                    | "private_instance_methods" | "protected_instance_methods"
                    | "constants"
                    | "autoload" | "autoload?" | "const_defined?" | "const_get" | "private_constant" | "public_constant"
                    | "deprecate_constant"
                    | "singleton_class"
                    | "class_eval" | "module_eval"
                );
                let allocate_allowed =
                    &*name == "allocate"
                        && !cls.is_module
                        && cls.name != "Module";
                if in_set || allocate_allowed {
                    // `class_eval` / `module_eval` are the ONLY
                    // bridge-set members whose block is load-
                    // bearing. `class C; class_eval { def foo;
                    // end }; end` defines `foo` on `C` via the
                    // block-form intercept in do_call_block's
                    // recv-form path. Re-route through
                    // do_call_block (preserving block) instead
                    // of the do_call discard path the other
                    // bridge names use.
                    if matches!(&*name, "class_eval" | "module_eval") {
                        let argc = args.len();
                        self.stack.push(self_val.clone());
                        self.stack.push(Value::Block(block));
                        for a in args { self.stack.push(a); }
                        return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
                    }
                    // Route through the blockless `do_call`, NOT
                    // `do_call_block` — CRuby silently discards the
                    // block for these Class methods (verified:
                    // `class Bar < Foo; ancestors { ran = true };
                    // end` returns the ancestor array AND `ran`
                    // stays false). do_call_block doesn't have
                    // receiver-form arms for most of these names,
                    // so routing the block form there would
                    // produce NoMethodError. The `allocate` case
                    // already has a do_call_block arm that
                    // discards its block — re-entering do_call
                    // hits the dedicated allocate arm there
                    // instead, with the same fences. Same
                    // outcome, simpler routing.
                    let argc = args.len();
                    self.stack.push(self_val.clone());
                    for a in args { self.stack.push(a); }
                    let _ = block; // explicitly discarded per CRuby
                    return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
                }
            }
            if let Some(m) = self.toplevel_methods.get(&name_id).cloned() {
                self.invoke_method_with_block(m, self_val, args, Some(block))?;
                return Ok(());
            }
            if self.try_method_missing(&self_val, name_id, args, Some(block))? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                kind: crate::error::NoMethodErrorKind::Missing,
                method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
            }));
        }
        let recv = recv.expect("ICE: receiver missing for block call");

        // `obj.send(:name, args...) { ... }` — same dynamic-name
        // re-aim as the block-less arm in `do_call`. `do_call_block`
        // pops args then block then recv (in that drain/pop order),
        // so the stack shape it expects from a caller is
        // `[..., recv, block, *args]`. Put them back in that order
        // and re-enter. cache_id = u16::MAX for the same reason as
        // the block-less arm. User-`def send` override + visibility
        // bypass parity — same rules as the block-less arm; see
        // there for the rationale.
        let user_send_override = &*name == "send" && match &recv {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_cached(&cls, name_id, cache_id).is_some()
            }
            // `def self.send` on a class — singleton-method lookup
            // walking the class's superclass chain. Falls through to
            // the existing `Value::Class` arm which invokes the
            // user's singleton `send`.
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
            _ => false,
        };
        if matches!(&*name, "send" | "__send__") && !user_send_override {
            let target_sym = self.parse_send_target(&args)?;
            let new_argc = args.len() - 1;
            self.bypass_visibility_once = true;
            self.stack.push(recv);
            self.stack.push(Value::Block(block));
            for a in args.into_iter().skip(1) {
                self.stack.push(a);
            }
            return self.do_call_block(target_sym, new_argc, false, u16::MAX);
        }

        // Mirror do_call's Int#+/-/* BigInt-aware intercept so
        // block-form sends (`a.send(:+, big) { ... }`) match the
        // expression form's overflow-promotion. Without this the
        // block path falls through to numeric_call's plain `+`
        // which wraps on overflow.
        #[cfg(feature = "bignum")]
        if args.len() == 1
            && matches!(&recv, Value::Int(_))
            && matches!(&args[0], Value::Int(_))
            && let Some(kind) = crate::bytecode::BinOpKind::from_op_name(&name)
            && matches!(kind,
                crate::bytecode::BinOpKind::Add
                | crate::bytecode::BinOpKind::Sub
                | crate::bytecode::BinOpKind::Mul
            )
        {
            let (Value::Int(x), Value::Int(y)) = (&recv, &args[0]) else { unreachable!() };
            let v = self.apply_int_promote(kind, *x, *y)?;
            self.stack.push(v);
            return Ok(());
        }

        if self.try_push_string_encoding(&recv, &name, &args) {
            return Ok(());
        }
        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes).map_err(|e| self.trap(e))? { self.stack.push(v); return Ok(()); }
        if let Some(v) = self.sym_primitive(&recv, &name, &args)? { self.stack.push(v); return Ok(()); }
        // Mirror do_call's bigint_primitive hook. Without this,
        // block-form calls on BigInt receivers (`big.send(:to_s) { ... }`)
        // raise NoMethodError because primitive_call/sym_primitive
        // are stateless and can't reach the BigInt heap.
        #[cfg(feature = "bignum")]
        if let Some(v) = self.bigint_primitive(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // Block-form `def self.foo` dispatch. Mirrors `do_call`'s
        // `Value::Class` arm at vm/dispatch.rs:1226 — without this,
        // `Foo.bar(args) { … }` where `Foo` carries a user singleton
        // method falls all the way through to `NoMethodError`.
        // Common shape: `StringIO.open("x") do |io| … end`,
        // `Module.send(:include, M) { … }`, any DSL helper a host
        // exposes as a class method that takes a block. Same
        // `lookup_class_singleton_method` helper walks the singleton
        // chain through superclasses; on a hit, we re-enter via
        // `invoke_method_with_block` to thread the block through.
        if let Value::Class(cls) = &recv
            && let Some(m) = self.lookup_class_singleton_method(cls, name_id)
        {
            let target_self = recv.clone();
            return self.invoke_method_with_block(m, target_self, args, Some(block));
        }

        // `Class#allocate` (block form) — CRuby silently ignores
        // a block passed to `allocate`. Without this arm,
        // `Box.allocate { ... }` (or `Box.send(:allocate) { ... }`,
        // which routes here through `do_call_block`) falls through
        // to method_missing/NoMethodError instead of allocating
        // (PR #181 review round 4 Copilot comment #1). Mirrors
        // do_call's allocate arm — same arity / primitive shell /
        // Module-Class fences, same shared allocator helper, with
        // the block discarded.
        //
        // Precedence: this arm sits AFTER the generic
        // `lookup_class_singleton_method` check at line 4601, so
        // a user-defined `def self.allocate` wins. do_call has the
        // matching precedence via its dedicated `allocate`
        // user-singleton arm at line 1184 (fix landed in the same
        // PR's code-review round). The two paths are now
        // symmetric: user override wins in both no-block and
        // block forms.
        if &*name == "allocate"
            && let Value::Class(cls) = &recv {
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
                }));
            }
            // Eigenclass-shell fence — CRuby:
            // `A.singleton_class.allocate` raises TypeError
            // ("can't create instance of singleton class").
            // (Code-review #253 round 9 #1.)
            if cls.singleton_target.borrow().is_some() {
                return Err(self.trap(RubyError::TypeError {
                    msg: "can't create instance of singleton class".into(),
                }));
            }
            if cls.is_module
                || cls.name == "Module"
                || cls.name == "Class"
                || is_primitive_class_name(&cls.name)
            {
                let display = if cls.name.is_empty() {
                    if cls.is_module { "Module" } else { "Class" }
                } else {
                    &cls.name
                };
                return Err(self.trap(RubyError::TypeError {
                    msg: format!("allocator undefined for {}", display),
                }));
            }
            let obj = self.alloc_default_instance(cls)?;
            self.stack.push(obj);
            return Ok(());
        }
        let new_id = self.interner.intern("new");
        if name_id == new_id
            && let Value::Class(cls) = &recv {
                // Eigenclass-shell fence (block-form parallel of
                // the no-block fence in
                // `try_dispatch_class_intrinsics`). CRuby raises
                // TypeError for `A.singleton_class.new { … }` too.
                // (Code-review #253 round 9 #1.)
                if cls.singleton_target.borrow().is_some() {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "can't create instance of singleton class".into(),
                    }));
                }
                // Pin args during the alloc window — see the matching
                // comment in `do_call`'s new-branch for the rationale.
                // Route through `Vm::alloc_default_instance` so the
                // block-call `new` path can't drift from the
                // no-block `new` arm or `Class#allocate` (PR #181
                // review round 2).
                let obj = {
                    let mut g = PinGuard::new(self);
                    for a in &args { g.pin(a.clone()); }
                    g.vm.alloc_default_instance(cls)?
                };
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_uncached(cls, init_id) {
                    self.invoke_method_with_block(m, obj.clone(), args, Some(block))?;
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    self.stack.push(obj);
                }
                return Ok(());
            }
        if let Value::Object(id) = &recv {
            let cls = self.heap.class_of(*id);
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                self.invoke_method_with_block(m, recv.clone(), args, Some(block))?;
                return Ok(());
            }
        }
        if self.try_method_missing(&recv, name_id, args, Some(block))? {
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            kind: crate::error::NoMethodErrorKind::Missing,
            method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&recv)),
        }))
    }


}

/// Identity comparison for Method receivers — heap-managed
/// recvs compare by ObjId / Rc-pointer; primitives compare by
/// value. Matches CRuby's `equal?`-style semantics, narrowed to
/// the cases that can appear in a BoundMethod recv slot.
/// True for class names whose instances are backed by a non-Object
/// `Value` variant. Used by `Class#instance_method` to decide
/// between "real lookup, NameError on miss" (user class — the
/// methods table is the source of truth) and "synthesise an
/// UnboundMethod, let downstream arity / parameters fall back to
/// the builtin sentinel" (primitive — methods live in
/// `primitive_call` arms, not in a per-class table).
///
/// Mirrors `Vm::class_of`'s class-name set (`vm/lookup.rs`),
/// PLUS one intentional extra: `Kernel`. Kernel's instances are
/// Object-backed in CRuby (not a distinct `Value` variant), but
/// we treat it as a sentinel primitive here so
/// `Kernel.instance_method(:foo)` resolves via the same path as
/// real primitives — see the Kernel arm below for the rationale.
/// The two lists are NOT meant to be kept identical; future
/// editors should add new entries to whichever side actually
/// needs them.
/// `Class#method_defined?(name)` resolver. Walks the user-Method
/// table + ancestor chain first; if that misses and `cls` is a
/// primitive class (Integer / String / ...), builds a sentinel
/// receiver of the matching `Value` shape and consults the per-
/// primitive `responds_to` whitelist. This way
/// `String.method_defined?(:nope)` correctly returns `false` while
/// `Symbol.method_defined?(:name)` returns `true` (the
/// `msgpack-ruby/lib/msgpack/symbol.rb` Ruby-2.7+ version-detect
/// path). Excluded primitives that need a non-trivial sentinel
/// (Array/Hash/Range/Regexp/Proc/Method/UnboundMethod) fall back
/// to a permissive `true` — matches CRuby for the broadly-shared
/// Kernel methods and stays out of false-negative territory while
/// the synthesis cost isn't justified.
fn class_method_defined(vm: &mut Vm, cls: &Rc<Class>, sid: SymId) -> bool {
    // Eigenclass-shell: methods installed via
    // `singleton_class.class_eval { def foo; end }` redirect
    // into `target.singleton_methods` rather than the shell's
    // own `methods` table. CRuby's `shell.method_defined?(:foo)`
    // returns true for redirected installs, so walk the
    // target's singleton-method chain when the shell asks.
    // (Code-review #253 round 9 #3.)
    if let Some(target) = cls
        .singleton_target
        .borrow()
        .as_ref()
        .and_then(std::rc::Weak::upgrade)
        && vm.lookup_class_singleton_method(&target, sid).is_some()
    {
        return true;
    }
    if vm.lookup_method_uncached(cls, sid).is_some() {
        return true;
    }
    let sentinel: Option<Value> = match cls.name.as_str() {
        "Integer" => Some(Value::Int(0)),
        "Float" => Some(Value::Float(0.0)),
        "String" => Some(Value::new_str("")),
        // Sym(SymId(0)) is the first interned token — the
        // interner always has at least one entry by the time
        // class objects exist, so this is safe to construct.
        "Symbol" => Some(Value::Sym(SymId(0))),
        "TrueClass" => Some(Value::Bool(true)),
        "FalseClass" => Some(Value::Bool(false)),
        "NilClass" => Some(Value::Nil),
        _ => None,
    };
    match sentinel {
        Some(s) => vm.responds_to(&s, sid),
        // Aggregate / opaque primitives: keep the previously-
        // permissive answer so the gem helper path doesn't trip
        // on Kernel-shared method probes.
        None => is_primitive_class_name(&cls.name),
    }
}

impl Vm {
    /// Resolves a constant path from a starting class. Behavior
    /// matches CRuby's `Module#const_get` / `Module#const_defined?`
    /// dispatch:
    ///   - If the arg is a Symbol, the path is treated as a single
    ///     bare name (no `::` splitting). `:"Foo::Bar"` raises
    ///     `wrong constant name Foo::Bar`.
    ///   - If the arg is a String, `::` separators split the path
    ///     and each segment is walked. A leading `::` rebases to
    ///     the toplevel (Object). Each segment is validated via
    ///     `is_valid_const_name` before lookup.
    ///   - The interner-cap guard applies at every lookup: a
    ///     non-interned qualified key returns Missing without
    ///     calling `intern` (defends `Config::max_symbols`).
    ///
    /// (Copilot review #277 round 4 #3.)
    pub(crate) fn resolve_const_path(
        &mut self,
        start_cls: &std::rc::Rc<crate::value::Class>,
        path: &str,
        split_on_double_colon: bool,
    ) -> ConstPathOutcome {
        let (mut scope_name, segments): (String, Vec<&str>) =
            if split_on_double_colon && path.starts_with("::") {
                // Leading `::` rebases to Object's toplevel scope.
                ("Object".to_string(), path[2..].split("::").collect())
            } else if split_on_double_colon && path.contains("::") {
                (start_cls.name.clone(), path.split("::").collect())
            } else {
                (start_cls.name.clone(), vec![path])
            };
        // CRuby reports the FULL original path in the
        // wrong-name message when the structural issue is
        // visible at parse time — specifically trailing `::`
        // or triple-colon runs (`:::`). For deeper invalid
        // segments inside an otherwise structurally-valid
        // path (e.g. `Foo::lower`), CRuby reports just that
        // segment. We approximate by detecting the structural
        // shapes up front and returning WrongName with the
        // full path; the per-segment loop below handles the
        // segment-only cases.
        //
        // Caveats: CRuby's exact rule depends on path length
        // and resolution success (`Foo::Bar::` with Foo
        // missing reports `uninitialized constant Foo`
        // because the walk fails before validation). We don't
        // model that branch; accepted divergence — covered by
        // Shape 13 of the fixture which exercises CRuby's
        // canonical short-path shapes.
        // (Code-review #277 round 6 #2.)
        if split_on_double_colon
            && (path.ends_with("::") || path.contains(":::"))
        {
            return ConstPathOutcome::WrongName { name: path.to_string() };
        }
        let mut current_value: Option<Value> = None;
        let mut segments_remaining: usize = segments.len();
        for segment in segments {
            if !is_valid_const_name(segment) {
                return ConstPathOutcome::WrongName { name: segment.to_string() };
            }
            let lookup = if scope_name == "Object" {
                segment.to_string()
            } else {
                format!("{}::{}", scope_name, segment)
            };
            if !self.interner.contains(&lookup) {
                return ConstPathOutcome::Missing { missing_qualified: lookup };
            }
            let qid = self.interner.intern(&lookup);
            if let Some(c) = self.classes.get(&qid).cloned() {
                // Update scope_name for the NEXT step's qualified
                // lookup, and remember the value we'd return if
                // this is the final segment.
                scope_name = c.name.clone();
                current_value = Some(Value::Class(c));
                segments_remaining -= 1;
                continue;
            }
            if let Some(v) = self.constants.get(&qid).cloned() {
                segments_remaining -= 1;
                // Non-class constants can't be a parent scope.
                // CRuby's behavior when used as a middle segment:
                //   `Foo::CONST::X` →
                //   `TypeError: Foo::CONST::X does not refer to
                //    class/module`
                // (regardless of whether `Foo::X` would
                // separately resolve). If we ARE the last segment
                // the value is the legitimate result; otherwise
                // the path is structurally invalid and we must
                // raise the CRuby-shape TypeError instead of
                // continuing the walk with the OLD scope_name
                // (which would silently resolve to a sibling
                // under `Foo` or surface as a wrong
                // "uninitialized constant" NameError).
                // (Code-review #277 round 6 #1.)
                if segments_remaining > 0 {
                    return ConstPathOutcome::NotClass { full_path: path.to_string() };
                }
                current_value = Some(v);
                continue;
            }
            return ConstPathOutcome::Missing { missing_qualified: lookup };
        }
        match current_value {
            Some(v) => ConstPathOutcome::Found(v),
            None => ConstPathOutcome::Missing { missing_qualified: path.to_string() },
        }
    }

    /// CRuby-shape arity for a Proto: required positional count
    /// when the signature is fully fixed; `-(required + 1)`
    /// otherwise. Used by `Method#arity` and
    /// `UnboundMethod#arity`. Note: `Proc#arity` does NOT call
    /// this helper — blocks store rest info on `BlockHandle`
    /// (the Proto's `rest_param` field stays empty for them),
    /// and the block arm in `try_dispatch_callable_intrinsics`
    /// computes arity directly from the handle's `n_params` /
    /// `rest_slot`.
    ///
    /// The Proto's parameter layout is
    /// `[required..., optional..., rest?, kw..., kw_rest?, block?]`.
    /// `n_required_positional` covers the leading required slots;
    /// optionals are the gap between that and the rest/kw/block
    /// tail. The `block_param` slot is appended to `proto.params`
    /// so the body sees the local but it must NOT count as an
    /// optional positional for introspection.
    ///
    /// Required keyword (`def f(a:)`) bumps the mandatory count
    /// by 1 (CRuby treats the kwargs bundle as one mandatory
    /// arg). Any optional/rest position OR optional/kw_rest
    /// keyword (when no required-kw is present) flips the result
    /// negative.
    /// (TRY_RUNS layer #24.)
    pub(crate) fn proto_arity(&self, proto_idx: usize) -> i64 {
        let proto = &self.protos[proto_idx];
        let n_req_pos = proto.n_required_positional as usize;
        let rest_count = proto.rest_param.is_some() as usize;
        let kw_count = proto.kw_param_defaults.len();
        let kw_rest_count = proto.kw_rest_param.is_some() as usize;
        let block_count = proto.block_param.is_some() as usize;
        let positional_total = proto.params.len()
            .saturating_sub(rest_count + kw_count + kw_rest_count + block_count);
        let n_opt_pos = positional_total.saturating_sub(n_req_pos);
        let n_req_kw = proto.kw_param_defaults.iter().filter(|d| d.is_none()).count();
        let n_opt_kw = proto.kw_param_defaults.iter().filter(|d| d.is_some()).count();
        let req_kw_present = n_req_kw > 0;
        let effective_req = n_req_pos + req_kw_present as usize;
        let has_pos_optional = n_opt_pos > 0 || rest_count > 0;
        let has_kw_optional = !req_kw_present && (n_opt_kw > 0 || kw_rest_count > 0);
        if has_pos_optional || has_kw_optional {
            -((effective_req + 1) as i64)
        } else {
            effective_req as i64
        }
    }

    /// Default Instance allocator — `maybe_gc` + `check_alloc` +
    /// `heap.alloc(HeapObj::Instance { class, empty ivars, no
    /// singleton })` → `Value::Object`. Shared by `Class#allocate`
    /// and the default branch of `Class.new`'s allocator cascade
    /// so the two paths can't drift on GC/rooting/allocation
    /// behavior (PR #181 review #2 — Copilot flagged duplication
    /// between the two arms).
    ///
    /// Note: the `new` arm calls this through `g.vm` while inside
    /// a `PinGuard`; callers without a PinGuard call it directly
    /// on `&mut self`. Either is safe — this method does NOT pin
    /// its result, so any caller that needs to keep the new
    /// Instance alive across a later `maybe_gc` must pin
    /// (`PinGuard::pin`) before that point.
    ///
    /// Sites that intentionally do NOT use this helper:
    /// - `raise.rs` exception construction (lines 41/63/108/373)
    ///   skips `check_alloc` so a raise during budget exhaustion
    ///   does not re-trap — exception normalization must succeed
    ///   even under OOM-like conditions.
    /// - `match_data.rs:34` (regex MatchData) is a hot path where
    ///   the Instance lives immediately next to a heap-allocated
    ///   capture Array; threading them through this helper would
    ///   trigger an extra `maybe_gc` between two heap.alloc calls
    ///   and sweep the unpinned capture Array.
    ///
    /// These exemptions are intentional; flagged by PR #181
    /// code-review #3.
    pub(crate) fn alloc_default_instance(&mut self, cls: &Rc<Class>) -> Result<Value, Trap> {
        self.maybe_gc();
        self.check_alloc()?;
        let id = self.heap.alloc(HeapObj::Instance(Instance {
            class: cls.clone(),
            ivars: HashMap::new(),
            singleton_class: None,
        }));
        Ok(Value::Object(id))
    }

    /// Does an instance of the primitive class `class_name`
    /// respond to method `sid`? Builds a sentinel `Value` of
    /// the matching shape and consults the per-primitive
    /// `responds_to` whitelist. Aggregate primitives
    /// (Array/Hash/Range/Regexp/...) fall back to permissive
    /// `true`, matching `class_method_defined`'s shape. Used
    /// by `Op::AliasMethod` to decide whether to synthesise a
    /// primitive-forwarder Method when the source name isn't
    /// in the user-Method table.
    pub(crate) fn primitive_class_responds_to(&self, class_name: &str, sid: SymId) -> bool {
        let sentinel: Option<Value> = match class_name {
            "Integer" => Some(Value::Int(0)),
            "Float" => Some(Value::Float(0.0)),
            "String" => Some(Value::new_str("")),
            "Symbol" => Some(Value::Sym(SymId(0))),
            "TrueClass" => Some(Value::Bool(true)),
            "FalseClass" => Some(Value::Bool(false)),
            "NilClass" => Some(Value::Nil),
            _ => None,
        };
        match sentinel {
            Some(s) => self.responds_to(&s, sid),
            None => is_primitive_class_name(class_name),
        }
    }

    /// Build a Method that forwards to a primitive method on
    /// `self`. Emitted as the body of an `alias_method`'d
    /// primitive — when the alias is invoked, the body runs
    /// `LoadSelf; LoadLocal(0); ApplyCall(orig_id, ...); Return`
    /// so any args the caller passed flow through to the
    /// primitive call via the rest-Array slot. The forwarder
    /// Proto is appended to `self.protos` and the index is
    /// stamped into the returned Method.
    pub(crate) fn synth_primitive_forwarder(&mut self, cls: &Rc<Class>, orig_id: SymId) -> Rc<crate::value::Method> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        let proto = Proto {
            name: format!("<primitive-alias-forwarder:{}>", self.interner.resolve(orig_id)),
            // `args` is the rest-arg name; proto.params lists it so
            // `invoke_method`'s arg-binding loop treats slot 0 as
            // the rest collector. n_required_positional = 0 keeps
            // the alias arity-permissive (matches primitive
            // dispatch, which is variadic).
            params: vec!["args".to_string()],
            n_required_positional: 0,
            rest_param: Some("args".to_string()),
            kw_param_defaults: vec![],
            kw_rest_param: None,
            block_param: None,
            n_locals: 1,
            code: vec![
                Op::LoadSelf,
                Op::LoadLocal(0),
                Op::ApplyCall(orig_id, u16::MAX),
                Op::Return,
            ],
            op_spans: vec![Span::ZERO; 4],
            filename: "<primitive-alias>".into(),
            block_body_local_start: u16::MAX,
            byte_literals: vec![],
            const_chains: vec![],
            lexical_scope: vec![],
        };
        let idx = self.protos.len();
        self.protos.push(proto);
        Rc::new(crate::value::Method {
            params: vec!["args".to_string()],
            proto_idx: idx,
            fixed_arity: None,
            defining_class: Some(Rc::downgrade(cls)),
            visibility: std::cell::Cell::new(crate::value::Visibility::Public),
            closure: None,
            builtin: None,
        })
    }
}

/// CRuby's constant-name validation rule: the bare name must
/// start with an ASCII uppercase letter and contain only
/// `[A-Za-z0-9_]`. Empty names are rejected. Used by
/// `Module#const_defined?` / `Module#const_get` to raise the
/// CRuby-shape `NameError("wrong constant name <name>")`
/// distinct from `"uninitialized constant"` (which is for
/// valid-but-absent names). (Copilot review #277 round 3.)
fn is_valid_const_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Outcome of `resolve_const_path` (a single helper that
/// powers both `const_defined?` and `const_get`).
pub(crate) enum ConstPathOutcome {
    /// Path resolved to this Value (Class or other constant).
    Found(Value),
    /// Every name in the path was valid, but some step missed.
    /// `missing_qualified` is the qualified key in CRuby's
    /// `uninitialized constant Foo::Bar` shape for error
    /// reporting.
    Missing { missing_qualified: String },
    /// Some name in the path was not a valid constant identifier.
    WrongName { name: String },
    /// A middle segment of the path resolved to a non-class /
    /// non-module value (e.g. `Foo::CONST::X` where `Foo::CONST`
    /// is `42`). CRuby raises
    /// `TypeError: <full_path> does not refer to class/module`.
    /// Pre-fix the helper continued walking with the previous
    /// scope, which could silently resolve to an unrelated
    /// sibling (`Foo::X`) or surface as a misleading
    /// `uninitialized constant` NameError. (Code-review #277
    /// round 6 #1.)
    NotClass { full_path: String },
}

fn is_primitive_class_name(name: &str) -> bool {
    matches!(
        name,
        "Integer" | "Float" | "String" | "Symbol"
            | "Array" | "Hash" | "Range"
            | "Regexp" | "Proc"
            | "Method" | "UnboundMethod"
            | "TrueClass" | "FalseClass" | "NilClass"
            // Kernel — modeled as a sentinel "primitive" so
            // `Kernel.instance_method(:foo)` resolves without
            // forcing every Kernel method to live in a class
            // table. Real CRuby: Kernel is a Module included in
            // Object, transitively giving every value its method
            // set. We don't have Modules; this sentinel makes
            // the lookup succeed and emits an UnboundMethod that
            // defers resolution to bind+call (where do_call
            // routes to the receiver's primitive method dispatch
            // as if the call were direct).
            | "Kernel"
    )
}

fn method_recv_identity(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => x == y,
        (Value::Hash(x), Value::Hash(y)) => x == y,
        (Value::Range(x), Value::Range(y)) => x == y,
        (Value::Block(x), Value::Block(y)) => x == y,
        (Value::BoundMethod(x), Value::BoundMethod(y)) => x == y,
        (Value::UnboundMethod(x), Value::UnboundMethod(y)) => x == y,
        // ObjId identity, matching `method_recv_hash`. Two BigInt
        // Values are "the same receiver" only when they share an
        // ObjId; canonical-value equality (e.g. comparing two
        // independently-allocated 2^64 BigInts) is intentionally
        // not the relation here — `bound_method == other` only
        // collapses when the underlying receiver is literally the
        // same heap slot.
        #[cfg(feature = "bignum")]
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        // Same ObjId-identity rule as BigInt (see comment above):
        // method receivers collapse only when they point at the
        // literal same heap slot, not canonical-value equality.
        (Value::Rational(x), Value::Rational(y)) => x == y,
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Sym(x), Value::Sym(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        _ => false,
    }
}

/// Hash a Method receiver consistently with `method_recv_identity`.
/// Two receivers that compare equal via `method_recv_identity`
/// must collide here.
fn method_recv_hash(v: &Value) -> i64 {
    match v {
        Value::Object(id) | Value::Array(id) | Value::Hash(id) | Value::Range(id)
        | Value::Block(id) | Value::BoundMethod(id) | Value::UnboundMethod(id)
        | Value::CurriedProc(id) | Value::Rational(id) => id.0 as i64,
        // Two BigInts that hash-equal must collide via ObjId since
        // the heap-side bigint value identity is the ObjId (we
        // never share an ObjId across different BigInt values).
        #[cfg(feature = "bignum")]
        Value::BigInt(id) => id.0 as i64,
        Value::Class(c) => Rc::as_ptr(c) as i64,
        Value::Str(s) => Rc::as_ptr(s) as i64,
        Value::Int(n) => *n,
        Value::Float(f) => f.to_bits() as i64,
        Value::Sym(s) => s.0 as i64,
        Value::Bool(true) => 1,
        Value::Bool(false) => 0,
        Value::Nil => 0xDEAD_BEEF,
        #[cfg(feature = "regex")]
        Value::Regex(r) => Rc::as_ptr(r) as i64,
    }
}

/// Is `s` a syntactically valid CRuby instance-variable name?
///
/// CRuby grammar (from `parse.y` / docs): a single `@` followed by
/// a Ruby identifier — leading char must be ASCII letter or `_`,
/// subsequent chars must be ASCII letter / digit / `_`. Used by
/// `instance_variable_get` / `instance_variable_set` to reject
/// names that CRuby would also reject:
///
///   - bare `@` (no identifier body)
///   - `@@foo` (class-variable shape, double `@`)
///   - `@1foo` (digit start after `@`)
///   - `@foo?` / `@foo=` / `@foo!` (method-name suffixes that
///     aren't legal in ivar names)
///   - non-ASCII bodies (CRuby permits some, rubyrs takes the
///     conservative ASCII-only subset; not load-bearing for
///     any caller surfaced today)
fn is_valid_ivar_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    // Need `@` + at least one identifier char.
    if bytes.len() < 2 || bytes[0] != b'@' {
        return false;
    }
    // First body char: letter or `_`. Rejects `@@x`, `@1x`, `@?x`.
    let first = bytes[1];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    // Remaining: letter / digit / `_`. Rejects `@foo?`, `@foo=`,
    // `@foo!`, `@foo-bar`.
    bytes[2..].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Compute a stable integer id for any `Value`. Backs both
/// `Object#object_id` and `BasicObject#__id__`. Ids are
/// stable for a value while that value is alive; CRuby also
/// reuses heap `object_id` values after GC, and our heap
/// encoding likewise can reuse ids after deallocation
/// (`Heap::alloc` reissues entries from a freelist; Rc
/// pointer identities can also reappear). So we promise
/// "stable while alive", not session-wide uniqueness. CRuby
/// exact values aren't observable beyond equality checks
/// (`a.object_id == b.object_id`), so this encoding diverges
/// from CRuby's exact tags but preserves the contract: same
/// (live) value → same id, distinct (simultaneously live)
/// values →
/// distinct ids (best-effort — Float encoding hashes 64 bits
/// into 60 with collision-resistance ~2^30 distinct floats;
/// distinct floats can in principle collide).
///
/// Encoding contract:
///   - CRuby-exact for the special immediates user code is known
///     to depend on:
///       * nil:   4   (CRuby 3.x — was 8 in 2.x)
///       * true:  20
///       * false: 0
///       * Int n: `n * 2 + 1` (CRuby's Fixnum tag — always odd)
///   - Distinct high-bit type discriminators for the rest, so
///     cross-type collisions are impossible:
///       * Sym:   bit 61 set
///       * Float: bit 60 set
///       * Heap:  bit 62 set, with a 4-bit type subtag at
///         bits 58..61 to distinguish Array vs Object
///         vs Hash etc.
///   - The discriminator bits are far above the range that user
///     code's integer literals reach (`|n| < 2^58` for any
///     practical int produces an id below 2^59, well clear of
///     the Sym/Float/Heap tag bits).
pub(crate) fn object_id_for(v: &crate::value::Value) -> i64 {
    use crate::value::Value;
    /// Heap-managed value id:
    ///   - bit 62        = heap discriminator
    ///   - bits 58..61   = type subtag (4 bits → 16 types)
    ///   - bits 0..57    = payload (58 bits). ObjId-backed
    ///     variants pass a u32 freelist index
    ///     here, which always fits. Rc-backed
    ///     variants (Str/Regex/Class) hash the
    ///     pointer through `scramble_ptr` first
    ///     to avoid leaking host addresses, and
    ///     the resulting 64-bit scramble is
    ///     masked into 58 bits — so two
    ///     simultaneously-live Rc allocations
    ///     can in principle collide
    ///     (~2^29 distinct live allocations
    ///     before a collision is likely).
    fn heap_id(payload: u64, type_subtag: u8) -> i64 {
        debug_assert!(type_subtag < 16, "type subtag must fit in 4 bits");
        let payload_masked = payload & 0x03FF_FFFF_FFFF_FFFF; // 58 bits
        (1i64 << 62) | ((type_subtag as i64) << 58) | (payload_masked as i64)
    }
    match v {
        // CRuby-exact Fixnum encoding `2n+1` for ints in the
        // safe range; falls back to a bit-59 tag otherwise.
        // Safe range:
        //   * `n < 0` — id is negative (sign bit set), distinct
        //     from every type-tagged id (Float/Sym/Heap all set
        //     specific positive bits and clear the sign bit).
        //     Only excluded by overflow of `2n+1` itself
        //     (i.e. `n == i64::MIN`).
        //   * `n >= 0` — id must clear bits 59..62 so it doesn't
        //     collide with Float(bit 60) / Sym(bit 61) /
        //     Heap(bit 62). That means `id < (1<<59)` i.e.
        //     `n < (1<<58)`.
        // Without this guard, e.g. `n = 1<<60` yields
        // `2n+1 = 2^61+1` which collides with `Sym(SymId(1))`.
        Value::Int(n) => match n.checked_mul(2).and_then(|m| m.checked_add(1)) {
            Some(id) if *n < 0 || id < (1i64 << 59) => id,
            _ => {
                // Out-of-range int (|n| > 2^62 roughly): hash
                // the full 64-bit pattern into 59 bits and set
                // bit 59 as the type tag. A raw low-bit mask
                // would collide on inputs with identical low 59
                // bits (e.g. `2**62` and `-(2**62)` both have
                // low-59 == 0). Bit 59 is below
                // Float(60)/Sym(61)/Heap(62) so no cross-type
                // collision; it's above the safe Int range so
                // no collision with regular `2n+1` ids.
                // Collision resistance ~2^30 distinct
                // out-of-range ints — only reachable in builds
                // without bignum promotion.
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                n.hash(&mut h);
                (1i64 << 59) | ((h.finish() & 0x07FF_FFFF_FFFF_FFFF) as i64)
            }
        },
        Value::Bool(true) => 20,
        Value::Bool(false) => 0,
        Value::Nil => 4,
        // Sym: bit 61 set; bits 0..58 = SymId. Distinct from
        // true(20)/false(0)/nil(4) because bit 61 is way above
        // their bit positions; distinct from heap (bit 62) and
        // Float (bit 60).
        Value::Sym(sid) => (1i64 << 61) | (sid.0 as i64),
        // Float: bit 60 set; low 60 bits = a hash of the f64
        // bit pattern. The bit pattern occupies all 64 bits
        // (sign + 11-bit exponent + 52-bit mantissa); a naive
        // `& 0x0FFF...` would strip the sign bit and collapse
        // `1.0` and `-1.0` to the same id. Hashing folds all 64
        // bits into 60 with collision-resistance ~2^30 distinct
        // floats — adequate for any practical workload.
        Value::Float(f) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f.to_bits().hash(&mut h);
            (1i64 << 60) | ((h.finish() & 0x0FFF_FFFF_FFFF_FFFF) as i64)
        }
        // Rc-backed values (Str/Regex/Class): use the raw
        // pointer as the *seed* for an opaque, per-process id,
        // not as the id itself. A naive `Rc::as_ptr(s) as u64`
        // would leak the host virtual address through
        // `object_id` (and through the `to_s`/`inspect`
        // fallback), weakening ASLR for embedders running
        // untrusted Ruby code. Scrambling with a process-local
        // RandomState keeps the identity contract (same Rc →
        // same id while alive) but the resulting payload is
        // not recoverable to the original address. ObjId-backed
        // variants below already use opaque freelist indices,
        // not addresses, so they don't need this treatment.
        Value::Str(s) => heap_id(scramble_ptr(std::rc::Rc::as_ptr(s) as usize), 2),
        Value::Object(id) => heap_id(id.0 as u64, 3),
        Value::Array(id) => heap_id(id.0 as u64, 4),
        Value::Hash(id) => heap_id(id.0 as u64, 5),
        Value::Range(id) => heap_id(id.0 as u64, 6),
        Value::Block(id) => heap_id(id.0 as u64, 7),
        Value::BoundMethod(id) => heap_id(id.0 as u64, 8),
        Value::UnboundMethod(id) => heap_id(id.0 as u64, 9),
        Value::CurriedProc(id) => heap_id(id.0 as u64, 10),
        #[cfg(feature = "regex")]
        Value::Regex(re) => heap_id(scramble_ptr(std::rc::Rc::as_ptr(re) as usize), 11),
        #[cfg(feature = "bignum")]
        Value::BigInt(id) => heap_id(id.0 as u64, 12),
        Value::Class(c) => heap_id(scramble_ptr(std::rc::Rc::as_ptr(c) as usize), 13),
        Value::Rational(id) => heap_id(id.0 as u64, 14),
    }
}

/// Compute the universal `Object#hash` value for `v`. Backs
/// both the `Object#hash` dispatch arm and any container that
/// needs to recurse over its children with the same salt
/// scheme.
///
/// Per-variant type tags (kept stable — changing one would
/// reshuffle every Hash key in user code on upgrade):
///   1 Int, 2 Float, 3 Str, 4 Sym, 5 Bool, 6 Nil,
///   7 heap-identity (default fallback), 8 Range,
///   9 Array (order-sensitive), 10 Hash (order-insensitive).
fn object_hash(v: &Value, heap: &crate::heap::Heap) -> i64 {
    let mut visited = std::collections::HashSet::new();
    object_hash_inner(v, heap, &mut visited)
}

/// Sentinel id returned when `object_hash_inner` re-enters a
/// container it's already inside (`a = []; a << a; a.hash`).
/// Mirrors CRuby's `rb_exec_recursive` substitute — a fixed
/// value used to break the recursion. The exact constant
/// doesn't matter as long as it's stable across runs.
const HASH_RECURSION_SENTINEL: i64 = 0x52_55_42_59_52_53_43_59; // "RUBYRSCY"

fn object_hash_inner(
    v: &Value,
    heap: &crate::heap::Heap,
    visited: &mut std::collections::HashSet<crate::value::ObjId>,
) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match v {
        Value::Int(n) => { 1u8.hash(&mut h); n.hash(&mut h); }
        Value::Float(f) => { 2u8.hash(&mut h); f.to_bits().hash(&mut h); }
        Value::Str(s) => { 3u8.hash(&mut h); s.content.borrow().hash(&mut h); }
        Value::Sym(sid) => { 4u8.hash(&mut h); sid.0.hash(&mut h); }
        Value::Bool(b) => { 5u8.hash(&mut h); b.hash(&mut h); }
        Value::Nil => { 6u8.hash(&mut h); }
        Value::Range(id) => {
            let (begin, end, excl) = {
                let r = heap.range(*id);
                (r.begin.clone(), r.end.clone(), r.exclusive)
            };
            8u8.hash(&mut h);
            object_hash_inner(&begin, heap, visited).hash(&mut h);
            object_hash_inner(&end, heap, visited).hash(&mut h);
            excl.hash(&mut h);
        }
        // Array#hash is order-sensitive — `[1,2].hash !=
        // [2,1].hash`. Feed length plus each element's content
        // hash sequentially. On re-entry (cyclic array) emit
        // the sentinel instead of recursing. We iterate by
        // index + per-step `clone()` of one element rather than
        // cloning the whole Vec up front so a 1M-element array
        // costs O(1) extra memory per hash call.
        Value::Array(id) => {
            9u8.hash(&mut h);
            if !visited.insert(*id) {
                HASH_RECURSION_SENTINEL.hash(&mut h);
            } else {
                let len = heap.array(*id).len();
                (len as u64).hash(&mut h);
                for i in 0..len {
                    let el = heap.array(*id)[i].clone();
                    object_hash_inner(&el, heap, visited).hash(&mut h);
                }
                visited.remove(id);
            }
        }
        // Hash#hash is order-INsensitive — `{a:1,b:2}.hash ==
        // {b:2,a:1}.hash` because the two hashes are `==`. We
        // XOR a per-pair combinator across pairs so pair order
        // can't affect the result, but the combinator itself
        // mixes key and value non-symmetrically (mul-then-add)
        // so a swap of key/value *within* a pair perturbs the
        // result. A bare `kh ^ vh` per pair would collide
        // structurally: e.g. `{1=>2, 2=>1}` and `{1=>1, 2=>2}`
        // both reduce to `acc = 0` despite being `!=`. Length
        // still participates so empty-vs-full disambiguates.
        Value::Hash(id) => {
            10u8.hash(&mut h);
            if !visited.insert(*id) {
                HASH_RECURSION_SENTINEL.hash(&mut h);
            } else {
                let len = heap.hash(*id).len();
                (len as u64).hash(&mut h);
                let mut acc: i64 = 0;
                for i in 0..len {
                    let (k, val) = heap.hash(*id)[i].clone();
                    let kh = object_hash_inner(&k, heap, visited);
                    let vh = object_hash_inner(&val, heap, visited);
                    // (kh * 31 + vh) — non-commutative in kh,vh
                    // so swapping key with value changes the
                    // pair's contribution; XOR across pairs
                    // keeps overall ordering irrelevant.
                    let pair_h = (kh as i128)
                        .wrapping_mul(31)
                        .wrapping_add(vh as i128) as i64;
                    acc ^= pair_h;
                }
                acc.hash(&mut h);
                visited.remove(id);
            }
        }
        // Phase C.1: structural Rational hash. Required to keep
        // the `a.eql?(b) ⇒ a.hash == b.hash` invariant after the
        // companion `ruby_eq` arm in heap.rs treats canonical
        // (num, den) as equality. Without this `Rational(1, 2)`
        // values would compare equal but hash to per-ObjId values,
        // breaking Hash key lookup.
        Value::Rational(id) => {
            let r = *heap.rational(*id);
            11u8.hash(&mut h);
            r.num.hash(&mut h);
            r.den.hash(&mut h);
        }
        _ => { 7u8.hash(&mut h); object_id_for(v).hash(&mut h); }
    }
    h.finish() as i64
}

/// Render a `Proto`'s parameter list in the form CRuby's
/// `Method#inspect` uses — required positional bare,
/// optional positional with `=...`, rest with `*`, required
/// keyword with `:`, optional keyword with `: ...`, kw-rest
/// with `**`, block with `&`. Anonymous rest/kw-rest collapse
/// to bare `*` / `**`. Layout of `Proto.params` (set up in
/// `compile_def`):
///   [0..n_total_pos)    positional (required + optional, in
///                       source order); first
///                       `n_required_positional` are required.
///   if rest_param.is_some():  one slot for the rest name
///   then len(kw_param_defaults) keyword slots
///   if kw_rest_param.is_some(): one slot for the kw-rest name
///   if block_param.is_some():   one slot for the block name
/// Total derived by subtracting the tail counters from
/// `params.len()`.
fn format_method_params(proto: &crate::bytecode::Proto) -> String {
    let mut parts: Vec<String> = Vec::new();
    let n_total = proto.params.len();
    let mut tail = 0usize;
    if proto.rest_param.is_some() { tail += 1; }
    tail += proto.kw_param_defaults.len();
    if proto.kw_rest_param.is_some() { tail += 1; }
    if proto.block_param.is_some() { tail += 1; }
    let n_pos = n_total.saturating_sub(tail);
    let n_req = (proto.n_required_positional as usize).min(n_pos);

    for (i, name) in proto.params[..n_pos].iter().enumerate() {
        if i < n_req {
            parts.push(name.clone());
        } else {
            parts.push(format!("{}=...", name));
        }
    }
    let mut idx = n_pos;
    if let Some(rname) = &proto.rest_param {
        // Anonymous `def f(*)` parses to an empty rest name;
        // collapse to bare `*` to match CRuby.
        parts.push(if rname.is_empty() {
            "*".to_string()
        } else {
            format!("*{}", rname)
        });
        idx += 1;
    }
    for (i, default) in proto.kw_param_defaults.iter().enumerate() {
        let kname = &proto.params[idx + i];
        parts.push(match default {
            None => format!("{}:", kname),
            Some(_) => format!("{}: ...", kname),
        });
    }
    idx += proto.kw_param_defaults.len();
    if let Some(krname) = &proto.kw_rest_param {
        // `def f(**)` compiles with a synthetic
        // `__kw_rest_anon` slot name (compiler.rs:322) —
        // collapse it back to bare `**` for inspect.
        let is_anon = krname.is_empty() || krname == "__kw_rest_anon";
        parts.push(if is_anon {
            "**".to_string()
        } else {
            format!("**{}", krname)
        });
        idx += 1;
    }
    if let Some(bname) = &proto.block_param {
        parts.push(format!("&{}", bname));
    }
    let _ = idx;
    parts.join(", ")
}

/// Scramble a raw pointer into an opaque, process-local u64
/// suitable for embedding in `object_id`. Same pointer → same
/// scrambled value within a process (so identity holds while
/// the value is alive), but the host virtual address isn't
/// recoverable from the result. Uses the std `RandomState`'s
/// process-startup entropy as the hash key.
fn scramble_ptr(ptr: usize) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    use std::sync::OnceLock;
    static SEED: OnceLock<RandomState> = OnceLock::new();
    let rs = SEED.get_or_init(RandomState::new);
    
    
    rs.hash_one(ptr)
}
