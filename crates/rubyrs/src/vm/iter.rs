//! Block-form Enumerable iterators. Mirrors CRuby's `enum.c` —
//! the home for the iterator drivers that walk Array / Hash /
//! Range while yielding to a block.
//!
//! Contents:
//!   - `IterMode` enum + helpers (filter-driver modes).
//!   - `Vm::iter_array_filter` / `iter_hash_filter` /
//!     `iter_range_filter` — shared drivers for select / reject /
//!     find / any? / all? / none?.
//!   - `Vm::collection_call_block` — the big match that
//!     dispatches every block-taking method (each / map /
//!     partition / sort_by / inject / flat_map / chunk / etc.)
//!     over every receiver type.

use crate::error::Trap;
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::{value_cmp_v, PinGuard, Vm};

/// Outcome of a single `block.call(args)` step inside an
/// iterator driver. Returned by `Vm::step_block`.
///
/// The variants encode the three control-flow regimes that
/// affect what a driver should do NEXT:
///
/// - `Value(v)` — block ran to completion and returned `v`
///   (whether `next` was used or the block fell off the end).
///   The driver normally feeds `v` into its accumulator /
///   selector / transform and continues to the next item.
///
/// - `MethodReturn` — non-local `return` from inside the block
///   bubbled `method_return` up. The driver must stop iterating
///   and return immediately; the enclosing dispatch loop will
///   consume `self.method_return` and unwind frames. Drivers
///   that allocate an intermediate (an in-progress map / select
///   / inject accumulator) discard it — the method's return
///   value comes from `method_return`, not the accumulator.
///
/// - `Break(v)` — structured `break v` from inside the block.
///   The break is already "caught" here (`break_signaled` is
///   cleared), so the driver decides how to use `v`. CRuby's
///   universal contract is "break value short-circuits the
///   enumerator and becomes the method's result" — even for
///   predicate methods like `any?` / `all?` / `find`.
///   `[1,2,3].any? { break :tag }` returns `:tag`, NOT `false`
///   (verified against CRuby; see iter_array_filter's existing
///   `early = Some(r)` / `early.unwrap_or(Value::Bool(bool_acc))`
///   shape at line 152/170). So the driver almost always wants
///   `BlockStep::Break(r) => { early = Some(r); break; }` and
///   then returns `early.unwrap_or(<method-default>)`. What
///   *does* vary across methods is the short-circuit on
///   *truthy/falsy block result* (the `BlockStep::Value` arm),
///   NOT how break is handled.
///
/// Why not also a `Next` variant: `next` from a block returns
/// the block normally (with the `next`-supplied value, or nil),
/// no separate signal. From the driver's perspective `next` is
/// just `Value(...)`.
pub(crate) enum BlockStep {
    Value(Value),
    MethodReturn,
    Break(Value),
}

impl Vm {
    /// One synchronous block invocation: push the call frame,
    /// run the dispatch loop until it returns to `pre_frames`,
    /// then classify the outcome into a `BlockStep`. Encapsulates
    /// the PIN-INVOKE-DISPATCH-CHECK boilerplate that every
    /// iterator driver has had to spell out individually.
    ///
    /// Callers are responsible for the **outer** PinGuard
    /// (pinning the receiver, the block, args, and any in-flight
    /// accumulator). This helper does NOT pin — its only job is
    /// to drive one block call to completion and report what
    /// happened. Drivers run the helper in a loop, threading
    /// their own accumulator and per-iteration args through.
    ///
    /// `pre_frames` is the frame count snapshot the driver took
    /// BEFORE the loop started — passed in rather than read here
    /// because the standard convention pins it once before the
    /// loop, not once per iteration.
    ///
    /// See issue #151 for the migration rationale.
    pub(crate) fn step_block(
        &mut self,
        block: ObjId,
        args: Vec<Value>,
        pre_frames: usize,
    ) -> Result<BlockStep, Trap> {
        // P2 #21 follow-up (ADR 0023): silent-corruption
        // guard. When a Fiber yields inside a block invoked
        // from a Rust-level iter driver (Int#times etc.),
        // dispatch_until returns out of the block-body
        // execution back to step_block, which returns to
        // the driver's for-loop — but the for-loop has no
        // visibility into Fiber state and keeps iterating.
        // Without this guard, each subsequent step_block
        // call pushes a NEW block frame on top of the
        // already-suspended Fiber's frame stack; on resume
        // those queued frames all re-emit the LAST
        // iteration's block-parameter value. Observed shape
        // for `5.times { |i| yield i }` inside a Fiber:
        // `0, 4, 4, 4, 4` instead of `0, 1, 2, 3, 4`.
        //
        // Fix: when `fiber_yield_pending` is ALREADY set on
        // entry to step_block, return Nil without invoking
        // the block. The Rust for-loop runs to completion
        // pushing no extra frames; only the FIRST iteration
        // actually delivers a chunk. The remaining
        // iterations are silently dropped — known-limitation
        // documented at `p2_21_known_bug_times_loop_inside_fiber_yield`.
        //
        // Permanent fix (deferred) would replace Rust-level
        // iter loops with bytecode-level iteration so the
        // counter lives in Vm state and FiberStashGuard
        // captures it across yield. User-facing
        // remediation: use a `while`-counter pattern inside
        // Fiber bodies (see examples/sse_server.rb).
        //
        // This is strictly better than the silent-corruption
        // bug it replaces: no more wrong values delivered.
        // Output is truncated, not garbled.
        #[cfg(feature = "_fiber")]
        if self.fiber_yield_pending.is_some() {
            return Ok(BlockStep::Value(Value::Nil));
        }
        self.invoke_block(block, args)?;
        self.dispatch_until(pre_frames)?;
        if self.method_return.is_some() {
            // `method_return` itself stays set — the caller's
            // outer dispatch loop reads it on its way out.
            return Ok(BlockStep::MethodReturn);
        }
        let r = self.stack.pop().unwrap_or(Value::Nil);
        if self.break_signaled {
            self.break_signaled = false;
            return Ok(BlockStep::Break(r));
        }
        Ok(BlockStep::Value(r))
    }
}


/// Which Enumerable predicate-iterator a call dispatches to.
/// `NoneM` is named with a trailing M because `None` collides with
/// `Option::None` in match arms.
#[derive(Copy, Clone, Debug)]
pub(crate) enum IterMode { Select, Reject, Find, Any, All, NoneM, One }

impl IterMode {
    fn bool_init(self) -> bool {
        // For `all?` we start at true and flip to false on first
        // falsy; for `none?` likewise. `any?` starts false.
        // `One` uses a separate counter — `bool_init` is unused
        // for that mode (the `match mode` arm below tallies and
        // sets bool_acc at the end).
        match self {
            IterMode::Any => false,
            IterMode::All | IterMode::NoneM => true,
            _ => false,
        }
    }
}

impl Vm {

    /// Drives an iterator-with-predicate over an Array. Used by
    /// `select` / `reject` / `find` / `any?` / `all?` / `none?`.
    /// On `break val` (caught via `self.break_signaled`) returns `val`
    /// to match CRuby's "break value short-circuits the enumerator".
    pub(crate) fn iter_array_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Value, Trap> {
        let snapshot: Vec<Value> = self.heap.array(id).clone();
        let mut g = PinGuard::new(self);
        g.pin(Value::Array(id));
        // P2-13: block lives in the GC heap; pin it for the
        // duration of the iteration so any GC fired by the block
        // body doesn't sweep it.
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
            g.pin(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        // `IterMode::One` short-circuits to false on the SECOND
        // truthy match; otherwise the result is `count == 1` after
        // the full walk. Tracked separately so the loop break-on-
        // second-match optimisation matches CRuby's stop point.
        let mut one_count: usize = 0;
        for v in snapshot {
            let r = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                BlockStep::MethodReturn => break,
                BlockStep::Break(r) => { early = Some(r); break; }
                BlockStep::Value(r) => r,
            };
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Reject => if !truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Find => if truthy { find_val = v; break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
                IterMode::One => if truthy {
                    one_count += 1;
                    if one_count > 1 { break; }
                }
            }
        }
        // PinGuard drops at function exit, including the `?` paths above.
        if let Some(e) = early { return Ok(e); }
        Ok(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
            IterMode::One => Value::Bool(one_count == 1),
        })
    }

    /// Same shape as `iter_array_filter`, but the source is a Hash.
    /// Yield convention:
    ///   - `select` / `reject` yield `(k, v)` as TWO args
    ///     (Hash overrides Enumerable here — CRuby parity).
    ///   - `any?` / `all?` / `none?` / `find` yield a SINGLE
    ///     `[k, v]` pair Array (Enumerable-inherited shape),
    ///     so `|pair|` blocks and `&:sym` to_proc work.
    ///
    /// `find` returns the same pair Array it yielded.
    pub(crate) fn iter_hash_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Value, Trap> {
        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
        let mut g = PinGuard::new(self);
        g.pin(Value::Hash(id));
        g.pin(Value::Block(block));
        // Pre-pin every heap-ref k/v from the snapshot so a
        // block that mutates the receiver can't sweep entries
        // held only via the Rust-local Vec.
        for (k, v) in &snapshot {
            if k.is_gc_heap_ref() { g.pin(k.clone()); }
            if v.is_gc_heap_ref() { g.pin(v.clone()); }
        }
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
            g.pin(Value::Hash(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        let is_pair_yield = !matches!(mode, IterMode::Select | IterMode::Reject);
        for (k, v) in snapshot {
            // Build the block arg list. select/reject keep the
            // two-arg shape (Hash#select / #reject override
            // Enumerable in CRuby); everything else yields a
            // single pair Array (Enumerable shape).
            let (block_args, pair_id) = if is_pair_yield {
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let pid = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                g.vm.pinned.push(Value::Array(pid));
                (vec![Value::Array(pid)], Some(pid))
            } else {
                (vec![k.clone(), v.clone()], None)
            };
            let step = g.vm.step_block(block, block_args, pre_frames);
            if pair_id.is_some() { g.vm.pinned.pop(); }
            let r = match step? {
                BlockStep::MethodReturn => break,
                BlockStep::Break(r) => { early = Some(r); break; }
                BlockStep::Value(r) => r,
            };
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Reject => if !truthy { g.vm.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Find => if truthy {
                    // Reuse the per-iter pair_id rather than
                    // allocating a second pair Array.
                    find_val = Value::Array(pair_id.unwrap());
                    break;
                }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
                // Hash#one? routes through its own dedicated arm
                // (vm/iter.rs:3635) because it pre-allocates a
                // pair Array for the block; `iter_hash_filter`
                // isn't called with `IterMode::One` today. Arm
                // present only for exhaustiveness.
                IterMode::One => unreachable!("Hash#one? has its own implementation, not iter_hash_filter"),
            }
        }
        if let Some(e) = early { return Ok(e); }
        Ok(match mode {
            IterMode::Select | IterMode::Reject => Value::Hash(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
            IterMode::One => unreachable!("Hash#one? handled in its own arm"),
        })
    }

    /// Same shape as `iter_array_filter`, but iterates an Int Range.
    /// Returns `None` (Option) to the caller if the range's endpoints
    /// aren't both Ints — callers fall through to NoMethodError.
    pub(crate) fn iter_range_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Option<Value>, Trap> {
        let (bi, ei, excl) = {
            let r = self.heap.range(id);
            match (&r.begin, &r.end) {
                (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                _ => return Ok(None),
            }
        };
        let mut g = PinGuard::new(self);
        g.pin(Value::Range(id));
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
            g.pin(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        let mut one_count: usize = 0;
        let end_inc = if excl { ei - 1 } else { ei };
        let mut i = bi;
        while i <= end_inc {
            let r = match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                BlockStep::MethodReturn => break,
                BlockStep::Break(r) => { early = Some(r); break; }
                BlockStep::Value(r) => r,
            };
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Reject => if !truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Find => if truthy { find_val = Value::Int(i); break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
                IterMode::One => if truthy {
                    one_count += 1;
                    if one_count > 1 { break; }
                }
            }
            i += 1;
        }
        if let Some(e) = early { return Ok(Some(e)); }
        Ok(Some(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
            IterMode::One => Value::Bool(one_count == 1),
        }))
    }

    pub(crate) fn collection_call_block(&mut self, recv: &Value, name: &str, args: &[Value], block: ObjId) -> Result<Option<Value>, Trap> {
        // Object#itself with a block — CRuby ignores the block
        // and returns the receiver unchanged. Sits next to the
        // tap/then/yield_self block path so the universal-arm
        // family stays together. (The no-block path is in
        // dispatch.rs.)
        //
        // Known limitation: this short-circuit (and the existing
        // tap/then/yield_self ones) shadow user-defined overrides
        // because `collection_call_block` runs before user-method
        // lookup in `do_call_block`. A user class that
        // \`def itself; ... end\` won't see its body invoked when
        // a block is attached. Fixing this requires the same
        // user-override probe pattern used by the `send` arm in
        // dispatch.rs (lines ~513-523) but applied uniformly to
        // the whole Object-extras family — out of scope for this
        // PR; tracked as Tier-2 follow-up.
        if name == "itself" {
            if !args.is_empty() {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len()
                    ),
                }));
            }
            return Ok(Some(recv.clone()));
        }
        // Object#tap / #then / #yield_self — universal block
        // helpers. Yield `self` to the block; `tap` discards the
        // result and returns self (debug-style fluent chain),
        // `then` (and its `yield_self` alias) returns whatever
        // the block returned (Kleisli-style transform). Extra
        // args are an ArgumentError on both arities (CRuby
        // checks regardless of block presence).
        if matches!(name, "tap" | "then" | "yield_self") && !args.is_empty() {
            return Err(self.trap(crate::error::RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len()
                ),
            }));
        }
        if args.is_empty() && matches!(name, "tap" | "then" | "yield_self") {
            // step_block migration also surfaces a pre-existing
            // CRuby-parity gap: `1.tap { break :x }` was returning
            // `1` (the receiver) instead of `:x`. CRuby's break-
            // value propagation rule applies to every block-taking
            // method — `tap`/`then` are no exception. Fixed here as
            // a side effect of going through the helper, which
            // forces explicit BlockStep::Break handling.
            //
            // method_return is left set on the Vm; outer dispatch
            // unwinds via `Ok(Some(Value::Nil))` per the sort/sort!
            // / chunk_while / scan precedent locked in by
            // `nonlocal_return_from_block` fixture.
            let pre_frames = self.frames.len();
            let mut g = PinGuard::new(self);
            g.pin(recv.clone());
            g.pin(Value::Block(block));
            match g.vm.step_block(block, vec![recv.clone()], pre_frames)? {
                BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                BlockStep::Break(r) => return Ok(Some(r)),
                BlockStep::Value(r) => {
                    return Ok(Some(if name == "tap" { recv.clone() } else { r }));
                }
            }
        }
        // `s.gsub(/pat/) { |m| ... }` / `s.sub(/pat/) { |m| ... }`.
        // For each match the block is invoked with the matched
        // substring; its return value is converted to a string and
        // spliced in place of the match. gsub iterates all matches;
        // sub does only the first. The block arg stays the full
        // match string — matching CRuby's convention. Capture
        // groups are exposed via `$1` / `$~` / `$&` etc., backed
        // by `last_match` which we update per match before each
        // block invocation. Without that update, `s.gsub(/_(\w)/)
        // { $1.upcase }` would see `$1 == nil` and the
        // ActiveSupport-lite canon couldn't write
        // `String#camelize` the idiomatic way.
        #[cfg(feature = "regex")]
        if let (Value::Str(s), Value::Regex(re), 1) = (recv, args.first().unwrap_or(&Value::Nil), args.len())
            && (name == "gsub" || name == "sub" || name == "gsub!" || name == "sub!") {
                let is_bang = name == "sub!" || name == "gsub!";
                // Bang siblings must reject a frozen receiver
                // before iterating — even when the pattern would
                // not have matched, CRuby raises FrozenError on
                // `freeze.sub! { ... }` rather than returning nil.
                if is_bang && s.frozen.get() {
                    return Err(self.trap(crate::error::RubyError::FrozenError {
                        msg: format!("can't modify frozen String: {:?}", s.content.borrow()),
                    }));
                }
                let source = s.to_string_lossy();
                let only_first = name == "sub" || name == "sub!";
                let mut g = PinGuard::new(self);
                g.pin(recv.clone());
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut out = String::with_capacity(source.len());
                let mut last_end = 0usize;
                let mut any_match = false;
                // CRuby clears `$~` to nil when the gsub call
                // matches nothing — `"x".gsub(/y/) { ... }; $~`
                // returns nil even if `$~` was non-nil before
                // the call. Preserve that surface: if the loop
                // below records no matches, the post-loop
                // cleanup at the bottom of the block clears
                // `last_match`; if at least one match runs, the
                // per-match update inside the loop leaves
                // `last_match` set to the FINAL match (also
                // matches CRuby).
                // Layer #17: dual-engine fallback. Block-form
                // `gsub`/`sub` with regex receiver iterates via
                // `captures_iter`; we haven't migrated this
                // path to the unified dispatcher yet, so trap
                // clearly on fancy-engine patterns until a
                // follow-up wires it through. Native patterns
                // (the overwhelming majority) take the existing
                // fast path unchanged.
                //
                // Check BEFORE \`last_match.take()\` so a trap
                // here doesn't have the side effect of wiping
                // \`$~\` — the operation never ran, so the
                // caller's prior \`$~\` should survive untouched.
                // Code-review #353 round 5.
                let native = re.as_native().ok_or_else(|| g.vm.trap(crate::error::RubyError::RuntimeError {
                    msg: format!(
                        "regex op 'String#sub/gsub block-form' is not yet supported on patterns requiring the fancy-regex engine (pattern: /{}/)",
                        re.as_str(),
                    ),
                }))?;
                let last_match_before = g.vm.last_match.take();
                // `captures_iter` gives us group info per match
                // (find_iter doesn't). Slight cost: each iteration
                // builds a `Captures` rather than a bare `Match`,
                // but the work is what users expect — `$1` /
                // `$~` semantics — and it's amortised against the
                // block dispatch which dominates per-match cost
                // anyway.
                for caps in native.captures_iter(&source) {
                    any_match = true;
                    let m = caps.get(0).expect("ICE: captures.get(0) is always Some on a successful match");
                    out.push_str(&source[last_end..m.start()]);
                    // Populate `$~` / `$1..$N` for the block body.
                    // Mirrors the snapshot shape `str_bracket_regex`
                    // (~line 2179) builds for `s[/pat/, N]`.
                    let m_start = m.start();
                    let m_end = m.end();
                    let whole = m.as_str().to_string();
                    let mut group_caps: Vec<Option<String>> = Vec::with_capacity(caps.len().saturating_sub(1));
                    for i in 1..caps.len() {
                        group_caps.push(caps.get(i).map(|cm| cm.as_str().to_string()));
                    }
                    g.vm.last_match = Some(crate::vm::LastMatch {
                        whole: whole.clone(),
                        caps: group_caps,
                        input: source.clone(),
                        m_start,
                        m_end,
                    });
                    let r = match g.vm.step_block(block, vec![Value::new_str(whole)], pre_frames)? {
                        // Non-local `return` from the block —
                        // `Ok(Some(Value::Nil))` marks the primitive
                        // as matched so the outer dispatch loop
                        // unwinds via `method_return`. `Ok(None)`
                        // would route to NoMethodError.
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        // CRuby semantics: `break val` from inside a
                        // gsub block returns val as the call's
                        // result (not the partially-built string).
                        BlockStep::Break(r) => return Ok(Some(r)),
                        BlockStep::Value(r) => r,
                    };
                    let r_str = r.to_display(&g.vm.heap, &g.vm.interner);
                    out.push_str(&r_str);
                    last_end = m_end;
                    if only_first { break; }
                }
                // No-match case: restore the pre-call `$~`
                // snapshot, BUT then explicitly clear it.
                // CRuby's behaviour is "set $~ to nil" when the
                // call matched nothing — the snapshot-restore +
                // clear shape keeps `last_match` correctly None
                // whether or not the block was invoked.
                if !any_match {
                    let _ = last_match_before;
                    g.vm.last_match = None;
                }
                out.push_str(&source[last_end..]);
                // Bang siblings return nil when the pattern never
                // matched (block was never invoked); otherwise
                // mutate self in place and return self. The
                // equality check skips a buffer swap when the
                // block produced bytes identical to the match
                // (e.g. `"a".sub!(/a/) { |m| m }`).
                if is_bang {
                    if !any_match { return Ok(Some(Value::Nil)); }
                    let new_bytes = out.into_bytes();
                    if *s.borrow() != new_bytes {
                        *s.borrow_mut() = new_bytes;
                    }
                    return Ok(Some(Value::Str(s.clone())));
                }
                return Ok(Some(Value::new_str(out)));
            }
        // `s.each_byte { |b| ... }` — yield each byte (Int 0..255)
        // to the block, then return the receiver String. CRuby
        // returns an Enumerator when called without a block;
        // Tier 1 doesn't model Enumerator (ADR 0017 row
        // "Fiber / Enumerator" is Tier 2), so only the
        // block-given shape is reachable here — `do_call`'s
        // block-less path falls through to NoMethodError as
        // before. The byte snapshot decouples iteration from
        // any in-block mutation of the receiver (matches CRuby:
        // `each_byte` yields the bytes that existed at call
        // time, even if the block mutates the String).
        if let Value::Str(s) = recv
            && name == "each_byte" && args.is_empty()
        {
            // step_block migration fixes a latent CRuby-parity
            // bug: `"abc".each_byte { break :x }` was returning
            // `"abc"` (the receiver) instead of `:x` because the
            // pre-migration loop dropped the break_signaled check
            // entirely. Non-local return was masked by the same
            // gap; step_block's explicit BlockStep variants make
            // both paths unmissable.
            let bytes: Vec<u8> = s.borrow().clone();
            let mut g = PinGuard::new(self);
            g.pin(recv.clone());
            g.pin(Value::Block(block));
            let pre_frames = g.vm.frames.len();
            let mut early: Option<Value> = None;
            for b in bytes {
                match g.vm.step_block(block, vec![Value::Int(b as i64)], pre_frames)? {
                    BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                    BlockStep::Break(r) => { early = Some(r); break; }
                    BlockStep::Value(_) => {}
                }
            }
            return Ok(Some(early.unwrap_or_else(|| recv.clone())));
        }
        // `s.scan(/pat/) { |m| ... }` / `s.scan(string) { |m| ... }`
        // — yield each match to the block (capture-group Array if
        // the regex has groups, the matched substring otherwise).
        // Returns the receiver String, matching CRuby.
        if let (Value::Str(s), 1) = (recv, args.len()) && name == "scan" {
            let source: Vec<u8> = s.borrow().clone();
            // Only the cfg(feature = "regex") arms below consume this
            // String. Computing it unconditionally would dead-code-
            // warn under --no-default-features.
            #[cfg(feature = "regex")]
            let source_str = String::from_utf8_lossy(&source).into_owned();
            let mut g = PinGuard::new(self);
            g.pin(recv.clone());
            g.pin(Value::Block(block));
            let pre_frames = g.vm.frames.len();
            let mut early: Option<Value> = None;
            match &args[0] {
                #[cfg(feature = "regex")]
                Value::Regex(re) => {
                    // Layer #17: scan with regex receiver
                    // hasn't been migrated to the dual-engine
                    // dispatcher yet. Native patterns take the
                    // existing fast path; fancy patterns trap
                    // clearly until the follow-up wires it.
                    let native = re.as_native().ok_or_else(|| g.vm.trap(crate::error::RubyError::RuntimeError {
                        msg: format!(
                            "regex op 'String#scan' is not yet supported on patterns requiring the fancy-regex engine (pattern: /{}/)",
                            re.as_str(),
                        ),
                    }))?;
                    let has_groups = native.captures_len() > 1;
                    // The pre-migration code did `pop(); if break_signaled
                    // { early = pop(); }` — TWO pops, with the second one
                    // happening AFTER the first had already discarded the
                    // block's return value. So break_signaled captured
                    // whatever stack residue lived UNDER the block result
                    // (typically Nil) — `"abcabc".scan(/a/) { break :tag }`
                    // was returning `nil` instead of `:tag`. step_block
                    // pops exactly once and classifies, fixing the
                    // double-pop bug as a side effect.
                    if has_groups {
                        for caps in native.captures_iter(&source_str) {
                            let mut group_vec: Vec<Value> = Vec::with_capacity(caps.len() - 1);
                            for i in 1..caps.len() {
                                let v = caps.get(i)
                                    .map(|m| Value::new_str(m.as_str()))
                                    .unwrap_or(Value::Nil);
                                group_vec.push(v);
                            }
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let gid = g.vm.heap.alloc(HeapObj::Array(group_vec));
                            // Pin the freshly-allocated capture-
                            // groups Array for the duration of the
                            // step_block call only. `invoke_block`
                            // may run `maybe_gc()` before copying
                            // `args` into the block's locals, and
                            // `args` at that point is a Rust-local
                            // `Vec<Value>` — the only root for
                            // `gid` until invoke_block writes it
                            // into a frame slot. Without the pin,
                            // STRESS_GC sweeps `gid` and the block
                            // sees a use-after-free (stack-overflow
                            // ICE on the next GC).
                            //
                            // Manual push/pop instead of an outer
                            // `g.pin(...)` so the pin doesn't
                            // accumulate across iterations (a long
                            // `scan` would otherwise keep every
                            // capture Array pinned until the loop
                            // ends — O(matches) memory pressure).
                            //
                            // The `?` short-circuit is moved AFTER
                            // the pop by binding `step_block(...)`'s
                            // Result to a local first (`let step_result =
                            // ...`), then popping, then `match
                            // step_result?`. Without this dance an
                            // Err from step_block would skip the pop
                            // and leave `gid` permanently pinned —
                            // the historical PinGuard footgun its
                            // doc-comment warns about (vm.rs:147).
                            g.vm.pinned.push(Value::Array(gid));
                            let step_result = g.vm.step_block(block, vec![Value::Array(gid)], pre_frames);
                            g.vm.pinned.pop();
                            match step_result? {
                                BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                                BlockStep::Break(r) => { early = Some(r); break; }
                                BlockStep::Value(_) => {}
                            }
                        }
                    } else {
                        for m in native.find_iter(&source_str) {
                            match g.vm.step_block(block, vec![Value::new_str(m.as_str())], pre_frames)? {
                                BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                                BlockStep::Break(r) => { early = Some(r); break; }
                                BlockStep::Value(_) => {}
                            }
                        }
                    }
                }
                Value::Str(pat) => {
                    let pat_owned: Vec<u8> = pat.borrow().clone();
                    if !pat_owned.is_empty() {
                        let bytes: &[u8] = &source;
                        let pat_bytes: &[u8] = &pat_owned;
                        let plen = pat_bytes.len();
                        let mut i = 0;
                        while i + plen <= bytes.len() {
                            if &bytes[i..i + plen] == pat_bytes {
                                match g.vm.step_block(block, vec![Value::new_str_bytes(pat_owned.clone())], pre_frames)? {
                                    BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                                    BlockStep::Break(r) => { early = Some(r); break; }
                                    BlockStep::Value(_) => {}
                                }
                                i += plen;
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
                _ => return Ok(None),
            }
            return Ok(Some(early.unwrap_or_else(|| recv.clone())));
        }
        Ok(match (recv, name, args) {
            // `arr.first` / `arr.first(n)` / `arr.last` / `arr.last(n)`
            // with an attached block — CRuby silently discards the
            // block (these methods don't yield). Without an arm
            // here the call falls through to NoMethodError on
            // `[1,2,3].first(2) { ... }`, surprising callers who
            // expect Ruby's "block silently ignored when the
            // method doesn't use one" convention. Delegate to the
            // non-block dispatcher so both paths share one source
            // of truth (and pick up future tweaks — e.g. Float /
            // BigInt n coercion — in lockstep). The receiver
            // doesn't need pinning here: `array_collection_call`
            // pins anything it allocates.
            (Value::Array(id), "first" | "last", _) => {
                return self.array_collection_call(*id, name, args);
            }
            // `Array#delete(obj) { ... }`: CRuby yields `obj` on
            // no-match and returns the block's result. rubyrs's
            // Tier 1 stub silently drops the block (documented at
            // the impl site in `array.rs`). The delegation is
            // still required so wrong-arity arr.delete(){...} /
            // arr.delete(a,b){...} raises ArgumentError instead
            // of NoMethodError.
            (Value::Array(id), "delete", _) => {
                return self.array_collection_call(*id, name, args);
            }
            // `arr.clear { ... }` — CRuby silently discards the
            // block. Without this delegation the block-given
            // routing fails over to NoMethodError because
            // dispatch consults this table first and won't fall
            // through to array.rs.
            (Value::Array(id), "clear", _) => {
                return self.array_collection_call(*id, name, args);
            }
            // Same shape for `Range#first / #last` (and arity-1
            // forms). PR #146 added the arity-1 arms only to the
            // non-block dispatcher (`range_collection_call`),
            // reopening the gap PR #140 closed for Array.
            // `(1..5).first(2) { ... }` would otherwise fall
            // through to NoMethodError; CRuby silently ignores
            // the block.
            (Value::Range(id), "first" | "last", _) => {
                return self.range_collection_call(*id, name, args);
            }
            (Value::Array(id), "each", []) => {
                // Pilot migration to `step_block` per #151.
                // The driver only cares about: did break fire?
                // (use the break value). Otherwise continues to
                // the next element. method_return propagates up
                // by leaving `method_return` set on the Vm.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}  // each ignores per-iter result
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            // `arr.reverse_each { |v| … }` — `each` walking the
            // snapshot in reverse order. Returns the receiver.
            // Used by msgpack/bigint.rb's `from_msgpack_ext` to
            // accumulate the limb chunks back into a single integer
            // (LSB-first storage; reverse_each visits MSB-first).
            (Value::Array(id), "reverse_each", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot.into_iter().rev() {
                    match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "map", []) | (Value::Array(id), "collect", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    let r = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    g.vm.heap.array_mut(result_id).push(r);
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `Array#map!` / `Array#collect!` — in-place variant.
            // Mutates the receiver, returns self. Block-form only
            // in Tier-1; CRuby's no-block form returns an
            // Enumerator (`#<Enumerator: arr:map!>`), which the
            // rubyrs subset doesn't model.
            //
            // Break semantics: the elements already mapped stay
            // mapped; the remaining elements keep their pre-call
            // values; the call's return is the break expression.
            // Implemented by writing back each result via
            // `arr[idx]` (where `arr` is `heap.array_mut(*id)`)
            // rather than rebuilding a fresh Vec, so a break
            // mid-iteration leaves the tail untouched. The
            // snapshot insulates the iteration
            // from any concurrent in-block writes to the same
            // Array (CRuby's behaviour is to iterate over the
            // values present at call time).
            //
            // Frozen-receiver check intentionally omitted: rubyrs
            // doesn't yet model Array#freeze (verified: `.freeze.
            // frozen?` returns false), so we can't raise the
            // FrozenError CRuby would here. Documented divergence
            // shared with every other in-place Array primitive.
            // (TRY_RUNS pass-13 layer #16 — sinatra-4 hits this
            // via rack's middleware chain.)
            (Value::Array(id), "map!", []) | (Value::Array(id), "collect!", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                // GC safety: snapshot elements need their own
                // pins. The receiver is pinned (so heap-backed
                // children stay reachable WHILE the receiver
                // still holds them), but the block can mutate
                // the receiver — `arr.clear` / `pop` / `shift`
                // drops the original children from the receiver,
                // and the snapshot Vec is on the Rust stack so
                // the GC tracer doesn't see it. The next block
                // call may run `maybe_gc` and sweep them,
                // leaving dangling `ObjId`s in the snapshot.
                // Pin each ObjId-backed element through the
                // loop. (`Value::Str` / `Value::Class` are
                // `Rc`-counted and live independently of GC, so
                // they're skipped to keep the pinned Vec tight.)
                // Code-review #348 round 3.
                for v in snapshot.iter() {
                    if matches!(
                        v,
                        Value::Array(_) | Value::Hash(_) | Value::Range(_)
                            | Value::Block(_)
                            | Value::BoundMethod(_) | Value::UnboundMethod(_)
                            | Value::CurriedProc(_) | Value::Object(_)
                            | Value::Rational(_)
                    ) {
                        g.pin(v.clone());
                    }
                    #[cfg(feature = "bignum")]
                    if matches!(v, Value::BigInt(_)) {
                        g.pin(v.clone());
                    }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (idx, v) in snapshot.into_iter().enumerate() {
                    let r = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    // In-place write at the iteration index. The
                    // receiver array might have shrunk if the
                    // block mutated it (rare but legal); guard
                    // against the index falling off the end so
                    // we don't panic in that case.
                    let arr = g.vm.heap.array_mut(*id);
                    if idx < arr.len() {
                        arr[idx] = r;
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            // `flat_map { ... }` = map then flatten(1). Same
            // driver as map, but each block result that's an
            // Array gets spread into the result.
            // `arr.filter_map { |x| ... }` — map + select in one
            // pass. Block return is kept iff truthy; nil/false are
            // dropped (not "false is included" — strict truthiness,
            // matching CRuby).
            (Value::Array(id), "filter_map", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    let r = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() {
                        g.vm.heap.array_mut(result_id).push(r);
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Array(id), "flat_map", []) | (Value::Array(id), "collect_concat", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    let r = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    match r {
                        Value::Array(rid) => {
                            let items: Vec<Value> = g.vm.heap.array(rid).clone();
                            for it in items { g.vm.heap.array_mut(result_id).push(it); }
                        }
                        other => g.vm.heap.array_mut(result_id).push(other),
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `chunk { |x| key }` groups consecutive elements
            // sharing the same key. Returns
            // `[[key, [vals...]], ...]`. `nil` key drops the
            // element AND ends the current group (so equal keys
            // on either side of a `nil` land in separate groups,
            // see the `separator_just_hit` flag below). `false`
            // is a normal key — its run shows up in the output.
            // CRuby also recognises the `:_separator` Symbol as a
            // separator and `:_alone` as "this element gets its
            // own group" — neither is modelled here (documented
            // Tier 1 divergence).
            (Value::Array(id), "chunk", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                // Defensive pin of every heap-slot element. Without
                // this, if the block mutates the receiver mid-
                // iteration (`arr.shift` / `slice!` / etc.),
                // elements held only in the Rust-local `snapshot` /
                // `groups` Vecs are no longer reachable through the
                // pinned receiver and a subsequent step_block-
                // triggered maybe_gc would sweep them. CRuby
                // disallows concurrent mutation entirely; we
                // instead keep the elements alive defensively so
                // the primitive completes without ICE'ing.
                //
                // Narrowed to GC-tracked heap variants via
                // `Value::is_gc_heap_ref` — immediates
                // (Int/Float/Bool/Nil/Sym) and Rc-shared variants
                // (Str/Class/Regex) aren't GC-managed. `maybe_gc`
                // clones every `vm.pinned` entry into the marking
                // root set (vm/gc.rs:113-115), so blanket pinning
                // would add O(n) GC scan work for large arrays.
                //
                // The sort driver (iter.rs:1713) uses a blanket pin
                // of the same shape; left as-is pending its own
                // perf pass to keep this PR scoped to chunk.
                // Pre-existing gap surfaced by Copilot review on
                // PR #187.
                for v in &snapshot {
                    if v.is_gc_heap_ref() {
                        g.pin(v.clone());
                    }
                }
                let pre_frames = g.vm.frames.len();
                let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
                let mut early = None;
                // True when the previous yielded key was the
                // separator sentinel (`nil`). CRuby's chunk treats
                // `nil` as "drop this element AND end the current
                // group" — without resetting this state, a sequence
                // like `[1, nil-key, 1]` would merge the two 1s
                // into a single group across the separator, which
                // violates the "consecutive elements" rule.
                // Surfaced by Copilot review on PR #187.
                let mut separator_just_hit = false;
                for v in snapshot {
                    let key = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        // Return immediately (don't fall through to
                        // the post-loop `maybe_gc / check_alloc / heap.alloc`
                        // — a Trap from any of those would clobber the
                        // in-flight `method_return` state). `Ok(Some(Value::Nil))`
                        // marks the primitive as matched so the outer
                        // dispatch loop unwinds via `method_return`.
                        // Matches the shape used by gsub / bsearch /
                        // sort below.
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(k) => k,
                    };
                    // `nil` key: drop this element AND end the
                    // current group. See the chunk-arm header
                    // comment for the documented divergence vs
                    // CRuby's `:_separator` / `:_alone` symbols.
                    if matches!(key, Value::Nil) {
                        separator_just_hit = true;
                        continue;
                    }
                    // Skip the merge-into-previous-group check if a
                    // separator was just hit — even when the new
                    // key equals the previous group's key, they
                    // must be split into two groups.
                    let same_as_last = !separator_just_hit
                        && groups.last()
                        .map(|(k, _)| k.ruby_eq(&key, &g.vm.heap))
                        .unwrap_or(false);
                    separator_just_hit = false;
                    if same_as_last {
                        groups.last_mut().unwrap().1.push(v);
                    } else {
                        // Pin block-returned `key` for the rest of
                        // the primitive — but only when it's a
                        // GC-tracked heap reference. `groups` is a
                        // Rust-local Vec, not part of scan_roots;
                        // if `key` is a heap-slot Value (Array /
                        // Hash / Object / Range / Block /
                        // BoundMethod / UnboundMethod / CurriedProc
                        // / BigInt returned by the block), the next
                        // iteration's step_block can fire maybe_gc
                        // and sweep it, leaving `groups.last()` /
                        // `ruby_eq` / post-loop materialization
                        // reading freed memory. Immediate variants
                        // (Int / Float / Bool / Nil / Sym) and Rc-
                        // shared variants (Str / Class / Regex) are
                        // not GC-managed and don't need pinning;
                        // pinning them would just grow the pinned
                        // root set + GC scan budget. `v` itself is
                        // safe regardless (transitively rooted via
                        // the pinned source Array), so only `key`
                        // needs the check.
                        //
                        // O(distinct_heap_keys) pin growth —
                        // bounded by the output size, which is the
                        // natural cost ceiling for this primitive.
                        // Pre-existing gap surfaced by Copilot
                        // review on PR #187.
                        if key.is_gc_heap_ref() {
                            g.pin(key.clone());
                        }
                        groups.push((key, vec![v]));
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let mut out: Vec<Value> = Vec::with_capacity(groups.len());
                for (key, items) in groups {
                    let items_id = g.vm.heap.alloc(HeapObj::Array(items));
                    g.pin(Value::Array(items_id));
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![key, Value::Array(items_id)]));
                    g.pin(Value::Array(pair_id));
                    out.push(Value::Array(pair_id));
                }
                let oid = g.vm.heap.alloc(HeapObj::Array(out));
                Some(Value::Array(oid))
            }
            // `Hash#each_key { |k| ... }` / `Hash#each_value { |v| ... }`
            // — narrower variants of `each` that yield only one
            // side of each pair. Same snapshot + break/return
            // unwinding shape as `each` below; same return value
            // (the receiver Hash). CRuby's no-block form returns
            // an Enumerator, which Tier 1 doesn't model — the
            // block-less path falls through to NoMethodError.
            (Value::Hash(id), m @ ("each_key" | "each_value"), []) => {
                let id = *id;
                let key_only = m == "each_key";
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    let yielded = if key_only { k } else { v };
                    match g.vm.step_block(block, vec![yielded], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "each", []) | (Value::Hash(id), "each_pair", []) => {
                // CRuby yields each pair as a single 2-elem Array
                // `[k, v]`. Two-param blocks (`|k, v|`) auto-splat
                // it into k / v; single-destructure blocks
                // (`|(k, v)|`) receive the pair and unpack via the
                // F4 compile-time prologue. Yielding two separate
                // args would defeat the destructure path because
                // the block's anonymous slot would get just `k`,
                // not the pair Array.
                //
                // GC discipline: `snapshot` is a Rust-local Vec.
                // If the block mutates the receiver hash (e.g.
                // `h.delete(k)`), heap-ref k/v lose their only
                // GC root. Per-iteration pinning only protects
                // the CURRENT pair — if the block deletes a not-
                // yet-visited entry and allocates (triggering GC
                // inside step_block), later iterations would read
                // dangling ObjIds out of `snapshot`. Pin all heap-
                // ref k/v up-front via `g.pin` (PinGuard's Drop
                // handles cleanup on any exit path including the
                // `?` propagations below).
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    // Scoped pin: step_block's args→locals copy can
                    // call maybe_gc (block with rest param has to
                    // alloc a rest Array), and pair_id is only
                    // reachable via this Rust-local Vec until then.
                    // Push/pop around the single call so we don't
                    // accumulate pins across iterations.
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "each_with_index", []) => {
                // Block invocation per CRuby: `(pair, idx)` where
                // `pair` is the fresh `[k, v]` Array. The block
                // running with a single param gets `pair` (an
                // Array). Two-param destructured form
                // (`|pair, idx|`) is what users usually want.
                //
                // Same snapshot-vs-hash-mutation GC discipline
                // as `Hash#each` above: pin all heap-ref k/v
                // up-front so a block-driven mutation of a
                // not-yet-visited entry can't sweep it before
                // this loop reaches it.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (i, (k, v)) in snapshot.into_iter().enumerate() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    // Scoped pin for the freshly-allocated pair
                    // Array — only reachable via this Rust local
                    // until step_block copies it to the block's
                    // slot.
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id), Value::Int(i as i64)], pre_frames);
                    g.vm.pinned.pop();
                    match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "map", []) | (Value::Hash(id), "collect", []) => {
                // `h.map { |pair| ... }` / `h.map { |k, v| ... }` —
                // CRuby yields each entry as a single 2-elem Array
                // `[k, v]`. Two-param blocks auto-destructure via
                // the F4 prologue; single-param blocks receive the
                // pair Array as their lone arg (the bug this fix
                // closes — pre-fix we yielded two separate args,
                // so `h.collect { |m| m }` returned `[:a, :b]`
                // instead of `[[:a,1],[:b,2]]`). Mirrors the Hash#each
                // shape exactly so the auto-destructure rules
                // compose identically.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                // Pre-pin every heap-ref k/v from the snapshot —
                // same discipline as Hash#each: a block that mutates
                // the receiver can't sweep entries held only via the
                // Rust-local Vec.
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    g.vm.heap.array_mut(result_id).push(r);
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `h.transform_keys { |k| ... }` — new Hash with keys
            // mapped through the block. Values preserved. On
            // collision (block maps two distinct keys to the same
            // new key), later wins, matching CRuby.
            // `h.filter_map { |k, v| ... }` — yields each (k, v),
            // collects truthy block results into a fresh Array.
            // Like Array#filter_map but on Hash entries; the
            // result is NOT a Hash (CRuby behaviour).
            (Value::Hash(id), "filter_map", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step_result? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() {
                        g.vm.heap.array_mut(result_id).push(r);
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Hash(id), "transform_keys", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    let new_key = match g.vm.step_block(block, vec![k], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    // Last-wins collision: overwrite existing slot
                    // if the new_key equals one already present;
                    // otherwise append. Matches CRuby's iteration-
                    // order semantics.
                    let existing = g.vm.heap.hash(result_id).iter()
                        .position(|(k2, _)| k2.ruby_eql(&new_key, &g.vm.heap));
                    if let Some(p) = existing {
                        g.vm.heap.hash_mut(result_id)[p] = (new_key, v);
                    } else {
                        g.vm.heap.hash_mut(result_id).push((new_key, v));
                    }
                }
                Some(early.unwrap_or(Value::Hash(result_id)))
            }
            // `h.transform_values { |v| ... }` — new Hash with the
            // same keys but values mapped through the block. No
            // collision possible (keys unchanged); order preserved.
            (Value::Hash(id), "transform_values", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::with_capacity(snapshot.len()))));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    let new_v = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    g.vm.heap.hash_mut(result_id).push((k, new_v));
                }
                Some(early.unwrap_or(Value::Hash(result_id)))
            }
            // `h.transform_keys! { |k| ... }` — in-place key map; same
            // last-wins collision semantics as `transform_keys`, but
            // mutates the receiver and returns it. Core Ruby 3.0+.
            // DIVERGENCE: on `break` the new pairs are built in a
            // scratch Vec committed only on normal completion, so the
            // receiver is left fully untouched; CRuby commits the
            // entries processed before the break. Documented in
            // SUBSET.md; `break` mid-transform_keys! is rare.
            (Value::Hash(id), "transform_keys!", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut new_pairs: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
                let mut early = None;
                for (k, v) in snapshot {
                    let new_key = match g.vm.step_block(block, vec![k], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    let existing = new_pairs.iter()
                        .position(|(k2, _)| k2.ruby_eql(&new_key, &g.vm.heap));
                    if let Some(p) = existing {
                        new_pairs[p] = (new_key, v);
                    } else {
                        new_pairs.push((new_key, v));
                    }
                }
                match early {
                    Some(r) => Some(r),
                    None => {
                        *g.vm.heap.hash_mut(id) = new_pairs;
                        Some(Value::Hash(id))
                    }
                }
            }
            // `h.transform_values! { |v| ... }` — in-place value map;
            // keys unchanged, mutates the receiver and returns it.
            // Core Ruby 2.6+. DIVERGENCE on `break`: same scratch-Vec
            // commit-on-normal-completion as transform_keys! above, so
            // the receiver is left untouched rather than partially
            // committed. Documented in SUBSET.md.
            (Value::Hash(id), "transform_values!", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut new_vals: Vec<Value> = Vec::with_capacity(snapshot.len());
                let mut early = None;
                for (_k, v) in &snapshot {
                    let new_v = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    new_vals.push(new_v);
                }
                match early {
                    Some(r) => Some(r),
                    None => {
                        let new_pairs: Vec<(Value, Value)> = snapshot.into_iter()
                            .map(|(k, _)| k).zip(new_vals).collect();
                        *g.vm.heap.hash_mut(id) = new_pairs;
                        Some(Value::Hash(id))
                    }
                }
            }
            // `h.merge!(other) { |key, old, new| ... }` /
            // `h.update(...) { ... }` — in-place merge whose block
            // resolves key collisions (its result becomes the value).
            // New keys append in `other`'s order. Mutates and returns
            // self. Core Ruby. The blockless form is in vm/hash.rs.
            (Value::Hash(id), "merge!" | "update", [Value::Hash(other)]) => {
                let id = *id;
                let other = *other;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Hash(other));
                g.pin(Value::Block(block));
                let extra: Vec<(Value, Value)> = g.vm.heap.hash(other).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in extra {
                    let pos = g.vm.heap.hash(id).iter()
                        .position(|(ek, _)| ek.ruby_eql(&k, &g.vm.heap));
                    if let Some(p) = pos {
                        let old = g.vm.heap.hash(id)[p].1.clone();
                        let resolved = match g.vm.step_block(block, vec![k.clone(), old, v], pre_frames)? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break; }
                            BlockStep::Value(r) => r,
                        };
                        g.vm.heap.hash_mut(id)[p].1 = resolved;
                    } else {
                        g.vm.heap.hash_mut(id).push((k, v));
                    }
                }
                match early {
                    Some(r) => Some(r),
                    None => Some(Value::Hash(id)),
                }
            }
            // `h.merge(other) { |key, old, new| ... }` — block-form of
            // `merge`: like the blockless version (vm/hash.rs) but the
            // block resolves collisions. Returns a NEW hash; self is
            // untouched. The result inherits the RECEIVER's default
            // block, matching CRuby and the blockless `merge` arm — so
            // `Hash.new { ... }.merge(x) { ... }` still auto-vivifies.
            // Core Ruby.
            (Value::Hash(id), "merge", [Value::Hash(other)]) => {
                let id = *id;
                let other = *other;
                let default_block = self.heap.hash_default_block(id);
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Hash(other));
                g.pin(Value::Block(block));
                if let Some(bid) = default_block {
                    g.pin(Value::Block(bid));
                }
                let mut out: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let extra: Vec<(Value, Value)> = g.vm.heap.hash(other).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in extra {
                    let pos = out.iter().position(|(ek, _)| ek.ruby_eql(&k, &g.vm.heap));
                    if let Some(p) = pos {
                        let old = out[p].1.clone();
                        let resolved = match g.vm.step_block(block, vec![k.clone(), old, v], pre_frames)? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break; }
                            BlockStep::Value(r) => r,
                        };
                        out[p].1 = resolved;
                    } else {
                        out.push((k, v));
                    }
                }
                if let Some(r) = early {
                    Some(r)
                } else {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let nid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(out)));
                    if default_block.is_some() {
                        g.vm.heap.hash_set_default_block(nid, default_block);
                    }
                    Some(Value::Hash(nid))
                }
            }
            (Value::Hash(id), "fetch", [k]) => {
                // Block form: `h.fetch(k) { |k| default_expr }`.
                // Block is invoked only on miss; CRuby ignores the
                // 2-arg fetch + block combo (warns); we silently
                // accept it (handled in non-block path too).
                let id = *id;
                let pos = self.heap.hash(id).iter()
                    .position(|(key, _)| key.ruby_eql(k, &self.heap));
                if let Some(p) = pos {
                    return Ok(Some(self.heap.hash(id)[p].1.clone()));
                }
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                g.pin(k.clone());
                let pre_frames = g.vm.frames.len();
                // Single-shot block — the call's result IS the
                // block's return (whether reached via fall-off,
                // explicit value, or `break`). step_block's
                // Value / Break variants both surface that value;
                // method_return propagates via Ok(Some(Nil)) per
                // the established pattern.
                match g.vm.step_block(block, vec![k.clone()], pre_frames)? {
                    BlockStep::MethodReturn => Some(Value::Nil),
                    BlockStep::Break(r) | BlockStep::Value(r) => Some(r),
                }
            }
            (Value::Int(start), "upto", [Value::Int(stop)]) => {
                // Pin the block — the body may allocate freely
                // (e.g. `1.upto(10) { (1..1000).to_a }`), and GC
                // would otherwise sweep the block ObjId mid-loop.
                // Same fix shape Int#times already used; Int#upto
                // and Int#downto were missing it (pre-existing GC
                // bug — STRESS_GC reproduces the
                // "ICE: heap slot is not a Block" panic at
                // heap.rs without these pins). Empirically
                // verified pre-fix / post-fix on this branch.
                let start = *start;
                let stop = *stop;
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut i = start;
                while i <= stop {
                    match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            // Float endpoint — CRuby `1.upto(13.3)` yields up to
            // floor(13.3) == 13. NaN floor-cast produces 0; if
            // start > 0 the loop yields nothing (matches CRuby).
            // Infinity / large-finite saturate to i64::MAX via
            // `as i64`, so `5.upto(Float::INFINITY)` runs to
            // i64::MAX iterations. CRuby has the same runaway
            // (its loop doesn't special-case Infinity either);
            // hosts that need bounded execution should set
            // `Config::fuel` or `Config::deadline` (both opt-in
            // — a default Config doesn't trap this).
            (Value::Int(start), "upto", [Value::Float(stop_f)]) => {
                let start = *start;
                let stop = stop_f.floor() as i64;
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut i = start;
                while i <= stop {
                    match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            (Value::Int(start), "downto", [Value::Int(stop)]) => {
                // Same pin rationale as Int#upto above.
                let start = *start;
                let stop = *stop;
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut i = start;
                while i >= stop {
                    match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    i -= 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            // Mirror image of `upto` with a Float endpoint:
            // CRuby `9.downto(1.3)` yields down to ceil(1.3) == 2.
            (Value::Int(start), "downto", [Value::Float(stop_f)]) => {
                let start = *start;
                let stop = stop_f.ceil() as i64;
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut i = start;
                while i >= stop {
                    match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    i -= 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            (Value::Int(n), "times", []) => {
                // Pin the block: the body may allocate freely (eg.
                // `N.times { a = [1,2,3] }`) which can trigger GC,
                // and the block ObjId is no longer on the stack at
                // this point — without a pin, the block slot gets
                // swept mid-iteration and the next invoke_block
                // panics with use-after-free (heap.rs:115).
                // Matches the pin pattern used by Array#each /
                // Range#each / map etc.
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let n_val = *n;
                for i in 0..n_val {
                    match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Int(n_val)))
            }
            // BigInt iteration: `times` / `upto` / `downto` with at
            // least one BigInt operand. Counter lives as a native
            // `num_bigint::BigInt` on the Rust stack; per-iteration
            // `bigint_to_value` demotes to `Value::Int` whenever the
            // current count fits i64 (the common in-range case for
            // `(big - 5).upto(big)` etc.). The yielded Value is then
            // pinned via `vm.pinned.push` / `.pop` around the
            // `step_block` call — required because `invoke_block`'s
            // rest-args path (dispatch.rs::invoke_block) calls
            // `maybe_gc` with only the Block pinned, which would
            // sweep the freshly-allocated yield BigInt sitting in
            // the local args Vec. Discovered as a STRESS_GC use-
            // after-free in PR #174 cycle 1; pinned by
            // `bigint_iter_yield_pinned_across_rest_param_gc_window`.
            //
            // The outer PinGuard scopes recv / stop / block for the
            // whole loop. Per-iteration pin uses raw `vm.pinned`
            // push/pop (PinGuard's accumulate-and-drop model would
            // leak per-iteration entries; the manual pair scopes the
            // pin to exactly one step_block call, with pop placed
            // BEFORE the `?` on step_result so Trap propagation also
            // unpins cleanly).
            //
            // DoS protection for unbounded counters (a literal
            // `(2**100).times` would run essentially forever, exactly
            // as in CRuby) comes from the two existing runtime caps,
            // neither of which needs special-casing here. They differ
            // in lifecycle:
            //   - `Config::fuel` — **per-eval**. The host's ceiling
            //     lives on `Runtime::fuel_budget`; each
            //     `Runtime::eval` re-anchors `vm.fuel` from that
            //     ceiling at entry, so a Runtime reused across many
            //     evals gets a fresh budget per call (symmetric with
            //     `deadline` below). `vm.fuel` is decremented per
            //     dispatched op inside `step_block`'s invoke_block
            //     path; every iteration calls the block, which runs
            //     at least one op (its own return), so fuel ticks
            //     every iteration regardless of how trivial the body
            //     is. Trips with `ResourceExhausted: "out of fuel"`.
            //   - `Config::deadline` — **per-eval**. The stored value
            //     is a `Duration` (not an Instant), so each
            //     `Runtime::eval` recomputes the absolute
            //     `Instant::now() + duration` at entry; the budget
            //     restarts every call. The check itself lives in
            //     `vm/gc.rs::check_fuel` and runs unconditionally
            //     every 1024 ops (sharing the function with fuel only
            //     for cadence reasons — one `Instant::now()` syscall
            //     per 1024 ops). Trips with
            //     `ResourceExhausted: "wall-clock deadline exceeded"`.
            // A host that configures NEITHER cap accepts unbounded
            // CPU consumption as a documented trade-off (consistent
            // with the rest of the runtime's "explicit opt-in" cap
            // model). Pinned by
            // `integer_iter_loops_trap_under_fuel_cap` in
            // `tests/embed/resource_caps.rs`.
            //
            // Lives BELOW the Int×Int arms above so pure-Int cases
            // keep using the optimized i64 fast path.
            #[cfg(feature = "bignum")]
            (Value::BigInt(_), "times", []) => {
                use num_bigint::BigInt;
                let recv_owned = self.heap.bigint(match recv { Value::BigInt(id) => *id, _ => unreachable!() }).clone();
                // Negative `recv.times` is 0 iterations in CRuby —
                // mirrors the Int path (`(-5).times { ... }` → no
                // calls). For any negative BigInt we'd skip the
                // loop entirely; bail early to avoid the
                // alloc/check pattern.
                if recv_owned.sign() == num_bigint::Sign::Minus {
                    return Ok(Some(recv.clone()));
                }
                let mut g = PinGuard::new(self);
                g.pin(recv.clone());
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut counter = BigInt::from(0);
                while counter < recv_owned {
                    let yield_val = g.vm.bigint_to_value(counter.clone())?;
                    // Pin the yielded BigInt across step_block: if
                    // the block has a rest param, invoke_block
                    // builds the rest-args Array via heap.alloc,
                    // which runs maybe_gc with only the Block pinned
                    // — leaving the freshly-allocated yield_val
                    // reachable only from the local args Vec, which
                    // GC doesn't see. Push/pop directly on
                    // `vm.pinned` (avoiding PinGuard's
                    // accumulate-and-drop model) so the per-
                    // iteration pin doesn't leak.
                    g.vm.pinned.push(yield_val.clone());
                    let step_result = g.vm.step_block(block, vec![yield_val], pre_frames);
                    g.vm.pinned.pop();
                    match step_result? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    counter += 1;
                }
                Some(early.unwrap_or_else(|| recv.clone()))
            }
            // `big.upto(stop) { |i| ... }` — counts up from recv to
            // stop inclusive. Fires when either operand is BigInt;
            // the Int×Int case is handled by the arm above. CRuby:
            // returns recv at the end (or the break value).
            #[cfg(feature = "bignum")]
            (recv_v @ (Value::Int(_) | Value::BigInt(_)), "upto", [stop_v @ (Value::Int(_) | Value::BigInt(_))])
                if matches!(recv_v, Value::BigInt(_)) || matches!(stop_v, Value::BigInt(_)) =>
            {
                let start = self.as_bigint(recv_v).expect("guarded");
                let stop = self.as_bigint(stop_v).expect("guarded");
                let mut g = PinGuard::new(self);
                g.pin(recv_v.clone());
                g.pin(stop_v.clone());
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut counter = start;
                while counter <= stop {
                    let yield_val = g.vm.bigint_to_value(counter.clone())?;
                    // See `times` arm above for the per-iteration
                    // pin rationale (invoke_block's rest-args path
                    // runs maybe_gc with only the Block pinned).
                    g.vm.pinned.push(yield_val.clone());
                    let step_result = g.vm.step_block(block, vec![yield_val], pre_frames);
                    g.vm.pinned.pop();
                    match step_result? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    counter += 1;
                }
                Some(early.unwrap_or_else(|| recv_v.clone()))
            }
            // `big.downto(stop) { |i| ... }` — counts down from recv
            // to stop inclusive. Same shape as upto with `>=` /
            // `-=` 1.
            #[cfg(feature = "bignum")]
            (recv_v @ (Value::Int(_) | Value::BigInt(_)), "downto", [stop_v @ (Value::Int(_) | Value::BigInt(_))])
                if matches!(recv_v, Value::BigInt(_)) || matches!(stop_v, Value::BigInt(_)) =>
            {
                let start = self.as_bigint(recv_v).expect("guarded");
                let stop = self.as_bigint(stop_v).expect("guarded");
                let mut g = PinGuard::new(self);
                g.pin(recv_v.clone());
                g.pin(stop_v.clone());
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut counter = start;
                while counter >= stop {
                    let yield_val = g.vm.bigint_to_value(counter.clone())?;
                    // See `times` arm above for the per-iteration
                    // pin rationale.
                    g.vm.pinned.push(yield_val.clone());
                    let step_result = g.vm.step_block(block, vec![yield_val], pre_frames);
                    g.vm.pinned.pop();
                    match step_result? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    counter -= 1;
                }
                Some(early.unwrap_or_else(|| recv_v.clone()))
            }
            // Arity / coerce guards for BigInt receivers. The loop
            // arms above only match exact-arity, integer-arg shapes
            // (`big.times` with no args; `big.upto(int_or_big)` with
            // one Integer arg). Without these guards, wrong shapes
            // fall past iter.rs entirely and surface as
            // NoMethodError ('undefined method for Integer') —
            // diverging from CRuby's ArgumentError (wrong arity)
            // and TypeError (non-Integer arg). \`respond_to?\` says
            // the methods exist (see lookup.rs whitelist), so user
            // code's \`rescue ArgumentError\` keys on the wrong
            // class without these arms.
            //
            // Limited to BigInt receivers — the parallel Int-recv
            // gaps (`5.times(99)`, `5.upto(3.14)`) are pre-existing
            // and out of this PR's scope.
            #[cfg(feature = "bignum")]
            (Value::BigInt(_), "times", _) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len(),
                    ),
                }));
            }
            #[cfg(feature = "bignum")]
            (Value::BigInt(_), "upto" | "downto", []) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: "wrong number of arguments (given 0, expected 1)".to_string(),
                }));
            }
            #[cfg(feature = "bignum")]
            (Value::BigInt(_), "upto" | "downto", [other]) => {
                // The loop arm above matched Int/BigInt stop; if we
                // reach here the stop is some other type. CRuby
                // raises ArgumentError ("comparison of Integer
                // with X failed") — see the Int sibling arm below
                // for the rationale.
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "comparison of Integer with {} failed",
                        crate::vm::numeric::type_name_for_coerce(other),
                    ),
                }));
            }
            #[cfg(feature = "bignum")]
            (Value::BigInt(_), "upto" | "downto", many) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        many.len(),
                    ),
                }));
            }
            // Arity / coerce guards for Int receivers. Sibling to the
            // BigInt guards above; fire only when the happy-path arms
            // (Int×Int / Int×BigInt-via-the-BigInt-arm) failed to
            // match. Without these, wrong shapes fall past iter.rs
            // entirely and surface as NoMethodError ('undefined
            // method for Integer') — diverging from CRuby's
            // ArgumentError (wrong arity) and TypeError (non-Integer
            // arg). \`respond_to?(:times|:upto|:downto)\` says the
            // methods exist on every Integer (see lookup.rs), so
            // user code's \`rescue ArgumentError\` keys on the wrong
            // class without these.
            //
            // Int recv with BigInt arg is handled by the BigInt arm
            // above (it gates on either-side-is-BigInt), so by the
            // time control reaches these arms the arg is either
            // missing, multiple, or non-Integer.
            (Value::Int(_), "times", _) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len(),
                    ),
                }));
            }
            (Value::Int(_), "upto" | "downto", []) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: "wrong number of arguments (given 0, expected 1)".to_string(),
                }));
            }
            (Value::Int(_), "upto" | "downto", [other]) => {
                // Single-arg shape but arg is non-numeric (String,
                // nil, Symbol, …). CRuby raises ArgumentError
                // ("comparison of Integer with X failed") here,
                // not TypeError — the upto/downto loop uses `<=>`
                // internally, and the comparison failure surfaces
                // as ArgumentError. Float endpoints are accepted
                // by the Float arms above, so by this point the
                // arg is genuinely non-numeric. BigInt arg would
                // already be handled by the BigInt arm above.
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "comparison of Integer with {} failed",
                        crate::vm::numeric::type_name_for_coerce(other),
                    ),
                }));
            }
            (Value::Int(_), "upto" | "downto", many) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        many.len(),
                    ),
                }));
            }
            // `(b..e).step(n) { |i| ... }` — yields each step value.
            // Returns the receiver Range, matching CRuby.
            (Value::Range(id), "step", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("step can't be {}", n),
                    }));
                }
                let (b, e, excl) = {
                    let r = self.heap.range(*id);
                    (r.begin.clone(), r.end.clone(), r.exclusive)
                };
                let (bi, ei) = match (&b, &e) {
                    (Value::Int(a), Value::Int(c)) => (*a, *c),
                    _ => return Ok(None),
                };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                let step = *n;
                let mut early = None;
                while i <= end_inc {
                    match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                    i = i.saturating_add(step);
                }
                Some(early.unwrap_or(Value::Range(*id)))
            }
            (Value::Range(id), "each", []) => {
                // Two endpoint shapes drive iteration: Int+Int (the
                // common case, integer counting) and Str+Str (the
                // alphabetic `('a'..'z').each` case, driven by
                // String#succ). Other shapes fall through to
                // NoMethodError.
                let (b, e, excl) = {
                    let r = self.heap.range(*id);
                    (r.begin.clone(), r.end.clone(), r.exclusive)
                };
                match (&b, &e) {
                    (Value::Int(a), Value::Int(c)) => {
                        let bi = *a; let ei = *c;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Range(*id));
                        g.pin(Value::Block(block));
                        let pre_frames = g.vm.frames.len();
                        let mut early = None;
                        let end_inc = if excl { ei - 1 } else { ei };
                        let mut i = bi;
                        while i <= end_inc {
                            match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                                BlockStep::MethodReturn => break,
                                BlockStep::Break(r) => { early = Some(r); break; }
                                BlockStep::Value(_) => {}
                            }
                            i += 1;
                        }
                        Some(early.unwrap_or(Value::Range(*id)))
                    }
                    (Value::Str(_), Value::Str(_)) => {
                        // Walk via String#succ, comparing
                        // lexicographically — matches CRuby's
                        // ('a'..'z').each iteration model.
                        let start = if let Value::Str(s) = &b { s.to_string_lossy() } else { unreachable!() };
                        let stop = if let Value::Str(s) = &e { s.to_string_lossy() } else { unreachable!() };
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Range(*id));
                        g.pin(Value::Block(block));
                        let pre_frames = g.vm.frames.len();
                        let mut early = None;
                        let mut cur = start;
                        loop {
                            // Inclusive: stop when cur > stop. Exclusive: stop when cur >= stop.
                            let done = if excl { cur >= stop } else { cur > stop };
                            if done { break; }
                            match g.vm.step_block(block, vec![Value::new_str(cur.clone())], pre_frames)? {
                                BlockStep::MethodReturn => break,
                                BlockStep::Break(r) => { early = Some(r); break; }
                                BlockStep::Value(_) => {}
                            }
                            // succ; if the result is longer than `stop`, no further
                            // String less-than-or-equal can be true with our lex
                            // ordering — bail to avoid an unbounded loop for cases
                            // like ('a'..'9') where succ rolls into a longer string.
                            let next = super::string::str_succ(&cur);
                            if next.len() > stop.len() { break; }
                            cur = next;
                        }
                        Some(early.unwrap_or(Value::Range(*id)))
                    }
                    _ => return Ok(None),
                }
            }
            // `(b..e).each_slice(n) { |slice| ... }` — yield each
            // consecutive group of n Ints from the Range as one
            // Array argument; return the receiver Range. Same
            // closure + `vm.pinned.truncate(baseline)` per-iter
            // scope pattern as Hash / Array each_slice (PRs
            // #311 / #312). Only Int+Int endpoints supported —
            // matches `iter_range_filter` convention; Str+Str
            // ranges fall through to NoMethodError.
            // Float coerce — CRuby truncates `each_slice(2.5)` to
            // 2 (Integer cast). NaN / ±Inf raise RangeError via
            // `float_to_int_arg`. Re-dispatch with the converted
            // Int so the existing Int arm owns the rest of the
            // logic. Same pattern repeats across the 5 sibling
            // arms (each_slice/each_cons × Array/Hash/Range).
            (Value::Range(_), "each_slice", [Value::Float(f)]) => {
                let n = self.float_to_int_arg(*f)?;
                return self.collection_call_block(recv, name, &[Value::Int(n)], block);
            }
            (Value::Range(id), "each_slice", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("invalid slice size: {}", n),
                    }));
                }
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let id = *id;
                let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                // Exclusive end at i64::MIN means an empty range
                // (`min...min`). `saturating_sub` would underflow
                // to `min` and yield once; checked_sub maps it to
                // an empty-range early return (matches the
                // no-block arm in range.rs and the Range#sum arm
                // pattern).
                let end_inc = if excl {
                    match ei.checked_sub(1) {
                        Some(v) => v,
                        None => return Ok(Some(Value::Range(id))),
                    }
                } else { ei };
                let mut current: Vec<Value> = Vec::with_capacity(n_usz.min(64));
                let mut i = bi;
                while i <= end_inc {
                    current.push(Value::Int(i));
                    if current.len() == n_usz {
                        let iter_baseline = g.vm.pinned.len();
                        let chunk = std::mem::take(&mut current);
                        let step_result: Result<BlockStep, Trap> = (|| {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let slice_id = g.vm.heap.alloc(HeapObj::Array(chunk));
                            g.vm.pinned.push(Value::Array(slice_id));
                            g.vm.step_block(block, vec![Value::Array(slice_id)], pre_frames)
                        })();
                        g.vm.pinned.truncate(iter_baseline);
                        match step_result? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break; }
                            BlockStep::Value(_) => {}
                        }
                        current = Vec::with_capacity(n_usz.min(64));
                    }
                    // Bail before overflow on `i += 1` when end_inc == i64::MAX.
                    if i == end_inc { break; }
                    i += 1;
                }
                // Trailing partial chunk — only when no break fired.
                if early.is_none() && !current.is_empty() {
                    let iter_baseline = g.vm.pinned.len();
                    let chunk = std::mem::take(&mut current);
                    let step_result: Result<BlockStep, Trap> = (|| {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let slice_id = g.vm.heap.alloc(HeapObj::Array(chunk));
                        g.vm.pinned.push(Value::Array(slice_id));
                        g.vm.step_block(block, vec![Value::Array(slice_id)], pre_frames)
                    })();
                    g.vm.pinned.truncate(iter_baseline);
                    match step_result? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Range(id)))
            }
            // Wrong-arity / non-Int for Range#each_slice block form.
            (Value::Range(_), "each_slice", _) => {
                return Err(self.arity_error_arg1_int(name, args));
            }
            (Value::Range(_), "each_cons", [Value::Float(f)]) => {
                let n = self.float_to_int_arg(*f)?;
                return self.collection_call_block(recv, name, &[Value::Int(n)], block);
            }
            // `(b..e).each_cons(n) { |window| ... }` — sliding
            // window of n consecutive Ints; return receiver.
            // Maintains an n-element ring buffer (cheap on Int-
            // sized usize), yields when full. No yields when
            // range length < n.
            (Value::Range(id), "each_cons", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("invalid size: {}", n),
                    }));
                }
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let id = *id;
                let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                // See each_slice arm for the i64::MIN edge.
                let end_inc = if excl {
                    match ei.checked_sub(1) {
                        Some(v) => v,
                        None => return Ok(Some(Value::Range(id))),
                    }
                } else { ei };
                // Early-return when range_len < n — no windows
                // can be yielded and walking the full range
                // would buffer up to range_len ints for nothing
                // (mirrors Array#each_cons' `len >= n` guard).
                // Overflow on `end_inc - bi + 1` (e.g.
                // `i64::MIN..i64::MAX`) → range_len is larger
                // than any i64, so don't early-return.
                let too_short = if bi > end_inc {
                    true
                } else {
                    match end_inc.checked_sub(bi).and_then(|d| d.checked_add(1)) {
                        Some(len) => len < *n,
                        None => false,
                    }
                };
                if too_short { return Ok(Some(Value::Range(id))); }
                let mut window: std::collections::VecDeque<Value> =
                    std::collections::VecDeque::with_capacity(n_usz.min(64));
                let mut i = bi;
                'outer: while i <= end_inc {
                    if window.len() == n_usz {
                        window.pop_front();
                    }
                    window.push_back(Value::Int(i));
                    if window.len() == n_usz {
                        let iter_baseline = g.vm.pinned.len();
                        let win_vec: Vec<Value> = window.iter().cloned().collect();
                        let step_result: Result<BlockStep, Trap> = (|| {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let wid = g.vm.heap.alloc(HeapObj::Array(win_vec));
                            g.vm.pinned.push(Value::Array(wid));
                            g.vm.step_block(block, vec![Value::Array(wid)], pre_frames)
                        })();
                        g.vm.pinned.truncate(iter_baseline);
                        match step_result? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break 'outer; }
                            BlockStep::Value(_) => {}
                        }
                    }
                    if i == end_inc { break; }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Range(id)))
            }
            // Wrong-arity / non-Int for Range#each_cons block form.
            (Value::Range(_), "each_cons", _) => {
                return Err(self.arity_error_arg1_int(name, args));
            }
            // `(b..e).chunk_while { |a, b| pred(a, b) }` —
            // partition the Range into runs of consecutive Ints
            // where the block returns truthy for the adjacent
            // pair (a=prev, b=current). Falsy starts a new chunk.
            // Returns an Array of Array-chunks (NOT the receiver
            // — unlike each_slice/each_cons). Walks lazily by
            // Int counter so huge ranges don't materialise
            // upfront. Only Int+Int endpoints supported;
            // Str+Str raises RuntimeError to keep `respond_to?`
            // (Vm::responds_to in lookup.rs) consistent with the
            // dispatcher — same fallback as each_slice/each_cons.
            (Value::Range(id), "chunk_while", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(id));
                g.pin(Value::Block(block));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                // Exclusive end at i64::MIN means empty range —
                // see each_slice arm for the checked_sub
                // rationale (Range#sum precedent).
                let end_inc = if excl {
                    match ei.checked_sub(1) {
                        Some(v) => v,
                        None => return Ok(Some(Value::Array(result_id))),
                    }
                } else { ei };
                if bi > end_inc {
                    return Ok(Some(Value::Array(result_id)));
                }
                let pre_frames = g.vm.frames.len();
                let mut current_chunk: Vec<Value> = vec![Value::Int(bi)];
                let mut early: Option<Value> = None;
                let mut prev = bi;
                // Loop only fires for ranges with at least 2
                // elements; single-element ranges flush
                // `current_chunk` as the only chunk via the
                // trailing flush below.
                if bi < end_inc {
                    let mut cur = bi + 1;
                    'outer: loop {
                        // No per-iter pins to manage (the args
                        // are Int Values, not heap-ref) — call
                        // step_block directly, matching the
                        // Array/Hash chunk_while arms.
                        let r = match g.vm.step_block(block, vec![Value::Int(prev), Value::Int(cur)], pre_frames)? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break 'outer; }
                            BlockStep::Value(r) => r,
                        };
                        if r.is_truthy() {
                            current_chunk.push(Value::Int(cur));
                        } else {
                            // Flush current_chunk, start fresh.
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let chunk_id = g.vm.heap.alloc(HeapObj::Array(std::mem::take(&mut current_chunk)));
                            g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                            current_chunk.push(Value::Int(cur));
                        }
                        prev = cur;
                        if cur == end_inc { break; }
                        cur += 1;
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                // Trailing chunk (always non-empty on a non-
                // empty range — we seeded it with `Value::Int(bi)`).
                if !current_chunk.is_empty() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let chunk_id = g.vm.heap.alloc(HeapObj::Array(current_chunk));
                    g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                }
                Some(Value::Array(result_id))
            }
            // Wrong-arity for Range#chunk_while (takes 0 args).
            (Value::Range(_), "chunk_while", many) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        many.len()
                    ),
                }));
            }
            (Value::Array(id), "each_with_index", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (i, v) in snapshot.into_iter().enumerate() {
                    match g.vm.step_block(block, vec![v, Value::Int(i as i64)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "each_with_object", [seed]) => {
                // `arr.each_with_object(memo) { |elem, memo| ... }`.
                // CRuby threads `memo` unchanged across iterations
                // (unlike inject which uses the block's return as the
                // next accumulator). The block's return value is
                // ignored; users mutate `memo` for side effects.
                //
                // GC discipline: `snapshot` is a Rust-local Vec.
                // If the block mutates the receiver array (e.g.
                // `arr.clear` or deletes a future element), the
                // not-yet-visited heap-ref entries lose their only
                // root before `step_block` (which can trigger GC
                // during args→locals copy / rest-Array alloc).
                // Pin all heap-ref elements up-front via `g.pin`
                // so the whole snapshot stays rooted for the
                // full iteration; PinGuard's Drop cleans up on
                // any exit path including `?` propagations.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                g.pin(seed.clone());
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                for v in &snapshot {
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    match g.vm.step_block(block, vec![v, seed.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or_else(|| seed.clone()))
            }
            (Value::Array(id), "partition", []) => {
                // `arr.partition { |x| pred(x) }` returns
                // `[truthy_array, falsy_array]` — exactly two new
                // Arrays. Used a lot in routing / grouping idioms.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let yes_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(yes_id));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let no_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(no_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    let r = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() {
                        g.vm.heap.array_mut(yes_id).push(v);
                    } else {
                        g.vm.heap.array_mut(no_id).push(v);
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![
                    Value::Array(yes_id), Value::Array(no_id),
                ]));
                Some(Value::Array(pair_id))
            }
            // `arr.take_while { |x| ... }` / `#drop_while` — prefix
            // partitioning. `take_while` returns the prefix while
            // the block is truthy and stops at the first falsy
            // return. `drop_while` skips that prefix and returns
            // the rest unchanged (block isn't invoked past the
            // crossing point).
            (Value::Array(id), "take_while" | "drop_while", []) => {
                let want_take = name == "take_while";
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early: Option<Value> = None;
                let mut crossed = false;
                let mut crossing_idx: Option<usize> = None;
                for (i, v) in snapshot.iter().enumerate() {
                    let r = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if !r.is_truthy() {
                        crossed = true;
                        crossing_idx = Some(i);
                        break;
                    }
                    if want_take {
                        g.vm.heap.array_mut(result_id).push(v.clone());
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if !want_take {
                    // drop_while: copy from the crossing point to
                    // the end. If we never crossed, drop_while
                    // returns []; the result_id is already empty.
                    if let Some(start) = crossing_idx {
                        for w in &snapshot[start..] {
                            g.vm.heap.array_mut(result_id).push(w.clone());
                        }
                    } else if !crossed {
                        // Block was truthy for every element →
                        // drop_while drops the whole array. Already
                        // empty result_id is correct.
                    }
                }
                Some(Value::Array(result_id))
            }
            // `arr.bsearch { |x| ... }` — binary search a sorted
            // Array. Two modes, distinguished by the block's return
            // type at runtime:
            //   find-minimum (block returns Bool / nil): returns the
            //     smallest element for which the block is truthy,
            //     nil if none. Array must be partitioned false...true.
            //   find-any (block returns Int): 0 = match, <0 means
            //     "x too large" (search left), >0 means "x too
            //     small" (search right). Returns the matching
            //     element or nil. Array must be sorted in the
            //     comparison direction.
            // Other block-return types raise TypeError, matching
            // CRuby.
            (Value::Array(id), "bsearch", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut low = 0usize;
                let mut high = snapshot.len();
                let mut saw_int = false;
                let mut int_match: Option<Value> = None;
                while low < high {
                    let mid = low + (high - low) / 2;
                    let elem = snapshot[mid].clone();
                    let r = match g.vm.step_block(block, vec![elem.clone()], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => return Ok(Some(r)),
                        BlockStep::Value(r) => r,
                    };
                    match r {
                        Value::Bool(true) => high = mid,
                        Value::Bool(false) | Value::Nil => low = mid + 1,
                        Value::Int(n) => {
                            saw_int = true;
                            if n == 0 { int_match = Some(elem); break; }
                            else if n < 0 { high = mid; }
                            else { low = mid + 1; }
                        }
                        other => return Err(g.vm.trap(crate::error::RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (must be numeric, true, false or nil)",
                                other.type_name(),
                            ),
                        })),
                    }
                }
                if let Some(m) = int_match { return Ok(Some(m)); }
                if saw_int { return Ok(Some(Value::Nil)); }
                Some(if low < snapshot.len() { snapshot[low].clone() } else { Value::Nil })
            }
            // `arr.each_slice(n) { |slice| ... }` — yield each
            // consecutive group of n elements as a single Array
            // argument; return the receiver (CRuby parity, post-
            // 2.7). Last slice may be shorter than n. The cycle-6
            // closure + `vm.pinned.truncate(baseline)` per-iter
            // scope pattern (from Hash#each_slice) releases the
            // slice pin at end of each iteration, on every exit
            // path including check_alloc / step_block traps.
            (Value::Array(_), "each_slice", [Value::Float(f)]) => {
                let n = self.float_to_int_arg(*f)?;
                return self.collection_call_block(recv, name, &[Value::Int(n)], block);
            }
            (Value::Array(id), "each_slice", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("invalid slice size: {}", n),
                    }));
                }
                let id = *id;
                let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                let snapshot: Vec<Value> = self.heap.array(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(id));
                g.pin(Value::Block(block));
                // Snapshot elements need their own pins: slices
                // are built per-chunk inside the loop, so between
                // iterations only the receiver Array is pinned —
                // a block that mutates the receiver (`arr.clear`,
                // `arr.shift`) would otherwise drop element roots.
                for v in &snapshot {
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                'outer: for chunk in snapshot.chunks(n_usz) {
                    let iter_baseline = g.vm.pinned.len();
                    let step_result: Result<BlockStep, Trap> = (|| {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let slice_id = g.vm.heap.alloc(HeapObj::Array(chunk.to_vec()));
                        g.vm.pinned.push(Value::Array(slice_id));
                        g.vm.step_block(block, vec![Value::Array(slice_id)], pre_frames)
                    })();
                    g.vm.pinned.truncate(iter_baseline);
                    match step_result? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break 'outer; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Array(id)))
            }
            // Catch-all for wrong-arity / non-Int arg in Array#each_slice
            // block form. Previously fell through to NoMethodError, which
            // contradicts `respond_to?(:each_slice) == true` and CRuby's
            // ArgumentError / TypeError surface. See `arity_error_arg1_int`.
            (Value::Array(_), "each_slice", _) => {
                return Err(self.arity_error_arg1_int(name, args));
            }
            // `arr.each_cons(n) { |window| ... }` — sliding window
            // of n consecutive elements; return the receiver. No
            // yields when receiver has fewer than n elements.
            // Windows share element identity automatically (each
            // `win.to_vec()` clones the Copy `Value`s — heap-ref
            // Values keep their ObjId, so identity is preserved).
            (Value::Array(_), "each_cons", [Value::Float(f)]) => {
                let n = self.float_to_int_arg(*f)?;
                return self.collection_call_block(recv, name, &[Value::Int(n)], block);
            }
            (Value::Array(id), "each_cons", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("invalid size: {}", n),
                    }));
                }
                let id = *id;
                let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                let snapshot: Vec<Value> = self.heap.array(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(id));
                g.pin(Value::Block(block));
                for v in &snapshot {
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                if snapshot.len() >= n_usz {
                    'outer: for win in snapshot.windows(n_usz) {
                        let iter_baseline = g.vm.pinned.len();
                        let step_result: Result<BlockStep, Trap> = (|| {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let wid = g.vm.heap.alloc(HeapObj::Array(win.to_vec()));
                            g.vm.pinned.push(Value::Array(wid));
                            g.vm.step_block(block, vec![Value::Array(wid)], pre_frames)
                        })();
                        g.vm.pinned.truncate(iter_baseline);
                        match step_result? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break 'outer; }
                            BlockStep::Value(_) => {}
                        }
                    }
                }
                Some(early.unwrap_or(Value::Array(id)))
            }
            // Wrong-arity / non-Int for Array#each_cons block form.
            (Value::Array(_), "each_cons", _) => {
                return Err(self.arity_error_arg1_int(name, args));
            }
            // `arr.chunk_while { |a, b| pred(a, b) }` — partition
            // into runs of consecutive elements where the block
            // returns truthy for the pair (a=prev, b=current).
            // Falsy starts a new chunk. Empty input → `[]`;
            // single-element → `[[elem]]`.
            (Value::Array(id), "chunk_while", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                // Defensive pin of every heap-slot element. If the
                // block mutates the receiver mid-iteration (`shift`
                // / `slice!` / etc.), the snapshot's heap-Value
                // elements lose their transitive root through the
                // pinned receiver. They're then reachable only via
                // the Rust-local `snapshot` Vec (and, after the
                // first chunk flushes, via `current_chunk`) which
                // scan_roots can't see. The next `maybe_gc()` —
                // either inside `step_block` or at the chunk-flush
                // alloc below — would sweep them. Same family as
                // the chunk / group_by defensive snapshot pins
                // earlier in this file. Narrowed via
                // `is_gc_heap_ref` so immediate / Rc-shared
                // variants don't grow the pinned-roots set.
                //
                // Once snapshot elements are pinned, `current_chunk`'s
                // `pair[1].clone()` pushes point at the same ObjIds
                // and inherit the pin; no separate per-element
                // current_chunk pin is needed.
                for v in &snapshot {
                    if v.is_gc_heap_ref() {
                        g.pin(v.clone());
                    }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                if snapshot.is_empty() {
                    return Ok(Some(Value::Array(result_id)));
                }
                let pre_frames = g.vm.frames.len();
                let mut current_chunk: Vec<Value> = vec![snapshot[0].clone()];
                let mut early: Option<Value> = None;
                for pair in snapshot.windows(2) {
                    let r = match g.vm.step_block(block, vec![pair[0].clone(), pair[1].clone()], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() {
                        current_chunk.push(pair[1].clone());
                    } else {
                        // Flush current chunk and start a fresh one.
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let chunk_id = g.vm.heap.alloc(HeapObj::Array(std::mem::take(&mut current_chunk)));
                        g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                        current_chunk.push(pair[1].clone());
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                // Flush the trailing chunk.
                if !current_chunk.is_empty() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let chunk_id = g.vm.heap.alloc(HeapObj::Array(current_chunk));
                    g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                }
                Some(Value::Array(result_id))
            }
            // Wrong-arity for Array#chunk_while (takes 0 args).
            (Value::Array(_), "chunk_while", many) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        many.len()
                    ),
                }));
            }
            // `arr.min_by(n) { |x| key(x) }` / `arr.max_by(n) { ... }`
            // — top-n form. Returns an Array of `n` extremes
            // sorted by key (ascending for min_by, descending for
            // max_by). `n <= 0` yields `[]`; `n > len` yields all
            // elements sorted. Uses a full sort_by then truncate,
            // not a heap — O(n log n) is fine at the input sizes
            // we see in our niche.
            (Value::Array(id), "min_by", [Value::Int(n)])
            | (Value::Array(id), "max_by", [Value::Int(n)]) => {
                let want_min = name == "min_by";
                if *n < 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("negative size ({})", n),
                    }));
                }
                let n_take = *n as usize;
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                if n_take == 0 || snapshot.is_empty() {
                    return Ok(Some(Value::Array(result_id)));
                }
                let pre_frames = g.vm.frames.len();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
                let mut early: Option<Value> = None;
                for v in snapshot {
                    let key = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    pairs.push((key, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let interner = &g.vm.interner;
                // CRuby treats incomparable keys here as an error;
                // we keep the same shape as sort_by — return None
                // and let the caller surface NoMethodError.
                let mut incomparable = false;
                pairs.sort_by(|(ka, _), (kb, _)| {
                    match value_cmp_v(ka, kb, interner) {
                        Some(o) => o,
                        None => { incomparable = true; std::cmp::Ordering::Equal }
                    }
                });
                if incomparable { return Ok(None); }
                let take = n_take.min(pairs.len());
                let result_vec: Vec<Value> = if want_min {
                    pairs.into_iter().take(take).map(|(_, v)| v).collect()
                } else {
                    // Largest n: reverse-sorted prefix.
                    pairs.into_iter().rev().take(take).map(|(_, v)| v).collect()
                };
                *g.vm.heap.array_mut(result_id) = result_vec;
                Some(Value::Array(result_id))
            }
            (Value::Array(id), "min_by", []) | (Value::Array(id), "max_by", []) => {
                // For each element, call the block once to produce a
                // key. Track the running winner. Returns nil for an
                // empty array (matching CRuby). Block-keys that
                // aren't mutually comparable surface as NoMethodError
                // via `value_cmp_v` returning None for one of them —
                // same shape as sort_by.
                let want_min = name == "min_by";
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut best: Option<(Value, Value)> = None;
                for v in snapshot {
                    let key = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    best = Some(match best {
                        None => (key, v),
                        Some((bk, bv)) => match value_cmp_v(&key, &bk, &g.vm.interner) {
                            Some(std::cmp::Ordering::Less) if want_min => (key, v),
                            Some(std::cmp::Ordering::Greater) if !want_min => (key, v),
                            // Equal or wrong direction — keep prior.
                            Some(_) => (bk, bv),
                            // Incomparable keys — fall through to None below.
                            None => return Ok(None),
                        },
                    });
                }
                if let Some(e) = early { return Ok(Some(e)); }
                Some(best.map(|(_, v)| v).unwrap_or(Value::Nil))
            }
            (Value::Array(id), "group_by", []) => {
                // Group elements into a Hash keyed by the block's
                // return value. Insertion order matches first
                // appearance of each key — CRuby semantics. Values
                // collect into a fresh Array per key.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                // Defensive pin of every heap-slot element. Without
                // this, a block that mutates the receiver mid-
                // iteration (`arr.shift` / `slice!` / etc.) would
                // leave the snapshot's heap-Value elements rooted
                // only via this Rust-local Vec — the subsequent
                // bucket alloc's maybe_gc would sweep them and the
                // freshly-built bucket `Array(vec![v])` would
                // contain a dangling ObjId. CRuby's exact behaviour
                // under receiver mutation during enumeration is
                // unspecified / implementation-defined (in practice
                // it stops processing without raising); rubyrs
                // keeps the elements alive defensively so the
                // primitive completes without ICE'ing regardless
                // of what the user-level semantics turn out to be.
                // Mirrors the chunk
                // driver's defensive snapshot pin earlier in this
                // file. Narrowed via `is_gc_heap_ref` to avoid
                // O(n) GC scan growth for immediate/Rc-shared
                // element types.
                for v in &snapshot {
                    if v.is_gc_heap_ref() {
                        g.pin(v.clone());
                    }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    let key = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        // `break;` (NOT immediate
                        // `return Ok(Some(Value::Nil))` like chunk /
                        // bsearch / sort use) is safe here because
                        // the post-loop tail (`if let Some(e) = early
                        // ...; Some(Value::Hash(result_id))`) is
                        // strictly heap-reads — no `maybe_gc`, no
                        // `check_alloc?`, no `heap.alloc`. There's
                        // no Trap path that could clobber the
                        // in-flight `method_return`. If a future
                        // edit adds an allocation to the tail,
                        // switch this arm to
                        // `return Ok(Some(Value::Nil))` to match
                        // the chunk-arm pattern.
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    // Find or create the bucket array for this key.
                    let pos = g.vm.heap.hash(result_id).iter()
                        .position(|(k, _)| k.ruby_eql(&key, &g.vm.heap));
                    if let Some(p) = pos {
                        if let Value::Array(arr_id) = g.vm.heap.hash(result_id)[p].1 {
                            g.vm.heap.array_mut(arr_id).push(v);
                        }
                    } else {
                        // Pin the block-returned `key` across
                        // `maybe_gc()`. Between step_block returning
                        // and the eventual `hash_mut.push((key, ...))`,
                        // `key` lives only in this Rust local. If
                        // it's a heap-slot Value (Array / Hash /
                        // Object / Range / Block / BoundMethod /
                        // UnboundMethod / CurriedProc / BigInt),
                        // `maybe_gc` would sweep it and the
                        // subsequent `hash_mut.push` would insert a
                        // dangling ObjId into result_id. Same
                        // family as the chunk driver's GC pin.
                        //
                        // Pop discipline mirrors the step_block
                        // dance: the pin scope is JUST `maybe_gc()`,
                        // popped BEFORE `check_alloc()?` so an Err
                        // propagating via `?` doesn't skip the pop
                        // and leak a permanent pin. `check_alloc`
                        // doesn't trigger GC and `heap.alloc`
                        // doesn't either (and is infallible), so
                        // they run safely with `key` unpinned.
                        //
                        // Pin-stack discipline: this push/pop pair
                        // bypasses `PinGuard::pin`'s internal
                        // `count` accounting (the PinGuard's Drop
                        // pops exactly `count` items). The pair is
                        // safe because (a) it's balanced within a
                        // single straight-line block with no `?`
                        // between push and pop, (b) `maybe_gc()`
                        // reads `vm.pinned` but never mutates it,
                        // so the pop is guaranteed to remove the
                        // just-pushed `key`. DO NOT insert any
                        // `g.pin(...)` call (PinGuard-counted)
                        // between this push and pop — that would
                        // make the pop remove a PinGuard-tracked
                        // slot and PinGuard::Drop would under-pop,
                        // leaking a permanent pin.
                        let pin_key = key.is_gc_heap_ref();
                        if pin_key { g.vm.pinned.push(key.clone()); }
                        g.vm.maybe_gc();
                        if pin_key { g.vm.pinned.pop(); }
                        g.vm.check_alloc()?;
                        let arr_id = g.vm.heap.alloc(HeapObj::Array(vec![v]));
                        g.vm.heap.hash_mut(result_id).push((key, Value::Array(arr_id)));
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                Some(Value::Hash(result_id))
            }
            (Value::Array(id), "sort", []) | (Value::Array(id), "sort!", []) => {
                // Block-form sort: the block is the comparator,
                // called with `(a, b)` on every comparison and
                // returning a value whose sign decides ordering
                // (negative → prev<curr, zero → equal, positive →
                // swap). Accepted result types: Int, Float, and (with
                // `bignum`) BigInt — see the match below. CRuby's
                // `rb_cmpint` coerces any numeric to an integer cmp
                // axis; we replicate that for the types we model.
                //
                // Same insertion-sort shape as the no-block arms
                // in `array_collection_call`, but each comparison
                // routes through the block. PinGuard wraps the
                // whole impl: the `copy` Vec holds element ObjIds
                // with no other GC root once the receiver is no
                // longer on the stack, AND each block invocation
                // may trigger maybe_gc.
                //
                // sort! mutates in place and returns self; sort
                // allocates a fresh Array. Tilt's `template.rb:252`
                // uses sort! with a block on locals-key arrays.
                let is_bang = name == "sort!";
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let mut copy: Vec<Value> = g.vm.heap.array(*id).clone();
                // Pin every element. If the comparator block mutates
                // the receiver (e.g. `arr.clear` mid-sort) and triggers
                // maybe_gc, elements referenced only from this Rust-
                // local `copy` Vec would otherwise be unrooted and
                // could be swept — leading to dangling ObjIds when the
                // next comparison reads them. CRuby disallows
                // concurrent mutation entirely; we instead keep the
                // elements alive defensively so the sort completes
                // without ICE'ing.
                for v in &copy { g.pin(v.clone()); }
                let pre_frames = g.vm.frames.len();
                let n = copy.len();
                let mut early: Option<Value> = None;
                'outer: for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        // Compare copy[j-1] vs copy[j] via the block.
                        // Block result: Int — negative → prev < curr
                        // (already in order); zero → equal (in order);
                        // positive → prev > curr (swap).
                        let a = copy[j - 1].clone();
                        let b = copy[j].clone();
                        // Non-local `return` from the comparator:
                        // `Ok(Some(Value::Nil))` marks the primitive
                        // as matched so the outer dispatch loop
                        // unwinds via `method_return`. `Ok(None)`
                        // would route through do_call_block to
                        // NoMethodError, because this arm IS the
                        // block-form sort!/sort primitive.
                        // Empirically verified vs CRuby:
                        // `def foo; [3,1,2].sort!{return :x};
                        // :unreached; end; foo` → `:x`.
                        let result = match g.vm.step_block(block, vec![a, b], pre_frames)? {
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break 'outer; }
                            BlockStep::Value(r) => r,
                        };
                        // Non-Integer block result mirrors CRuby's
                        // `comparison of X with 0 failed`
                        // (ArgumentError, NOT TypeError — CRuby
                        // routes the result through `<=>`-style
                        // comparison against the integer 0 to
                        // determine ordering, and the failure
                        // surfaces from Comparable#>). The class
                        // name in the message is the block-return
                        // type, not the operand types — those are
                        // both Integer in the common probe.
                        //
                        // BigInt path: with `bignum` enabled, a
                        // comparator that subtracts large-magnitude
                        // operands legitimately returns a BigInt —
                        // e.g. `(a * 2**100) - (b * 2**100)` yields
                        // a BigInt result that's still semantically
                        // a sign-bearing integer. CRuby's `<=>`
                        // itself returns small -1/0/1, but `<=>`
                        // isn't the only valid comparator return;
                        // the `(a-b)` shape (idiomatic for floats
                        // and ints alike) is what produces BigInt.
                        // Sort by sign, same as Int.
                        let ord = match &result {
                            Value::Int(n) if *n > 0 => std::cmp::Ordering::Greater,
                            Value::Int(n) if *n < 0 => std::cmp::Ordering::Less,
                            Value::Int(_) => std::cmp::Ordering::Equal,
                            // Float comparator results — CRuby's
                            // `rb_cmpint` accepts any numeric that
                            // compares against 0 (common shape:
                            // `arr.sort { |a, b| a - b }` on floats).
                            // Sort by sign; NaN treated as Equal
                            // (CRuby is also undefined-ish there).
                            Value::Float(f) if *f > 0.0 => std::cmp::Ordering::Greater,
                            Value::Float(f) if *f < 0.0 => std::cmp::Ordering::Less,
                            Value::Float(_) => std::cmp::Ordering::Equal,
                            #[cfg(feature = "bignum")]
                            Value::BigInt(id) => {
                                // O(n²) sort — avoid allocating
                                // BigInt::from(0) per comparison. The
                                // heap-stored bigint exposes
                                // `.sign()` directly (num_bigint API).
                                use num_bigint::Sign;
                                match g.vm.heap.bigint(*id).sign() {
                                    Sign::Plus => std::cmp::Ordering::Greater,
                                    Sign::Minus => std::cmp::Ordering::Less,
                                    Sign::NoSign => std::cmp::Ordering::Equal,
                                }
                            }
                            _ => {
                                let result_class = match g.vm.class_of(&result) {
                                    Value::Class(c) => c.name.clone(),
                                    _ => result.type_name().to_string(),
                                };
                                return Err(g.vm.trap(crate::error::RubyError::ArgumentError {
                                    msg: format!(
                                        "comparison of {} with 0 failed",
                                        result_class,
                                    ),
                                }));
                            }
                        };
                        match ord {
                            std::cmp::Ordering::Greater => {
                                copy.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if is_bang {
                    *g.vm.heap.array_mut(*id) = copy;
                    Some(Value::Array(*id))
                } else {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let nid = g.vm.heap.alloc(HeapObj::Array(copy));
                    Some(Value::Array(nid))
                }
            }
            (Value::Array(id), "sort_by", []) => {
                // PinGuard wraps the entire impl — the previous code
                // dropped the guard after the key-collection loop,
                // leaving `pairs` (a Rust local) to carry ObjId-
                // bearing element Values through `user_cmp` insertion
                // sort and the trailing `maybe_gc()` with no GC root.
                // Symptom: `.to_a.sort_by` chains where the receiver
                // Array of pairs has no other anchor → pair Arrays
                // swept → dangling slots in the result.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let arr = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(arr.len());
                let mut early: Option<Value> = None;
                for v in arr {
                    let key = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    // Narrowed via `is_gc_heap_ref`: only pin
                    // GC-managed heap variants. `maybe_gc` clones
                    // every `vm.pinned` entry into the marking
                    // root set; blanket-pinning every key+v even
                    // when they're immediates / Rc-shared variants
                    // adds O(n) GC scan work for no safety benefit.
                    if key.is_gc_heap_ref() { g.pin(key.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                    pairs.push((key, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let n = pairs.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let (k_prev, k_curr) = {
                            let (a, b) = pairs.split_at(j);
                            (a[j - 1].0.clone(), b[0].0.clone())
                        };
                        let ord = g.vm.user_cmp(&k_prev, &k_curr)?;
                        match ord {
                            None => return Ok(None),
                            Some(std::cmp::Ordering::Greater) => {
                                pairs.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                let sorted: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let nid = g.vm.heap.alloc(HeapObj::Array(sorted));
                Some(Value::Array(nid))
            }
            (Value::Array(id), "inject", []) | (Value::Array(id), "reduce", []) => {
                // Pilot migration to `step_block` per #151.
                // Empty-array short-circuit + accumulator =
                // structurally different from `each` above; if
                // the helper signature works here it should
                // work for the rest of the inject / reduce /
                // each_with_object family.
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = snapshot[0].clone();
                let mut early = None;
                for v in &snapshot[1..] {
                    match g.vm.step_block(block, vec![acc.clone(), v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => { acc = r; }
                    }
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "inject", [init]) | (Value::Array(id), "reduce", [init]) => {
                // Pilot migration. The `init` variant shares the
                // accumulator pattern with the no-arg form above
                // — different only in the initial-acc source.
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                for v in &snapshot {
                    match g.vm.step_block(block, vec![acc.clone(), v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => { acc = r; }
                    }
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "count", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                for v in snapshot {
                    let r = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() { n += 1; }
                }
                Some(early.unwrap_or(Value::Int(n)))
            }
            (Value::Range(id), "inject", []) | (Value::Range(id), "reduce", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                if bi > end_inc { return Ok(Some(Value::Nil)); }
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = Value::Int(bi);
                let mut early = None;
                let mut i = bi + 1;
                while i <= end_inc {
                    match g.vm.step_block(block, vec![acc.clone(), Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => { acc = r; }
                    }
                    i += 1;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Range(id), "inject", [init]) | (Value::Range(id), "reduce", [init]) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    match g.vm.step_block(block, vec![acc.clone(), Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => { acc = r; }
                    }
                    i += 1;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Range(id), "count", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    let r = match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() { n += 1; }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(n)))
            }

            (Value::Array(id), "select", []) | (Value::Array(id), "filter", []) => Some(self.iter_array_filter(*id, IterMode::Select, block)?),
            (Value::Array(id), "reject", []) => Some(self.iter_array_filter(*id, IterMode::Reject, block)?),
            (Value::Array(id), "find", []) | (Value::Array(id), "detect", []) => Some(self.iter_array_filter(*id, IterMode::Find, block)?),
            (Value::Array(id), "any?", []) => Some(self.iter_array_filter(*id, IterMode::Any, block)?),
            (Value::Array(id), "all?", []) => Some(self.iter_array_filter(*id, IterMode::All, block)?),
            (Value::Array(id), "none?", []) => Some(self.iter_array_filter(*id, IterMode::NoneM, block)?),
            (Value::Array(id), "one?", []) => Some(self.iter_array_filter(*id, IterMode::One, block)?),
            // `a.find_index { |x| ... }` / `a.index { |x| ... }`
            // — Int index of the first element whose block result
            // is truthy, or nil. Positional-arg form lives in
            // array.rs; passing both a positional arg AND a block
            // routes there too (CRuby silently discards the block
            // when an arg is given — but emits a warning; we
            // skip the warning).
            // Positional-arg form with a block-given — CRuby
            // honours the arg and silently discards the block
            // (with a `given block not used` warning). Without
            // this arm dispatch would fall through to a
            // NoMethodError because array.rs's positional arm
            // only runs in the no-block path. We skip the
            // warning emission.
            (Value::Array(id), "find_index", [target]) | (Value::Array(id), "index", [target]) => {
                let id = *id;
                let target = target.clone();
                let len = self.heap.array(id).len();
                let mut found: Option<i64> = None;
                for i in 0..len {
                    let el = self.heap.array(id)[i].clone();
                    if el.ruby_eq(&target, &self.heap) {
                        found = Some(i as i64);
                        break;
                    }
                }
                Some(match found {
                    Some(i) => Value::Int(i),
                    None => Value::Nil,
                })
            }
            (Value::Array(id), "find_index", []) | (Value::Array(id), "index", []) => {
                let id = *id;
                let snapshot: Vec<Value> = self.heap.array(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(id));
                g.pin(Value::Block(block));
                for v in &snapshot {
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut found: Option<i64> = None;
                let mut early = None;
                for (i, v) in snapshot.into_iter().enumerate() {
                    let r = match g.vm.step_block(block, vec![v], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() { found = Some(i as i64); break; }
                }
                Some(early.unwrap_or_else(|| match found {
                    Some(idx) => Value::Int(idx),
                    None => Value::Nil,
                }))
            }

            // Hash#min_by / #max_by — yield (k, v) to the block,
            // pick the pair whose block-returned key is the
            // extremum. Result is the winning [k, v] as a fresh
            // 2-element Array, matching CRuby. Empty hash → nil.
            (Value::Hash(id), op @ ("min_by" | "max_by"), []) => {
                let want_max = op == "max_by";
                let pairs: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                if pairs.is_empty() { return Ok(Some(Value::Nil)); }
                let mut best: Option<(Value, Value, Value)> = None;
                let mut early: Option<Value> = None;
                {
                    let mut g = PinGuard::new(self);
                    g.pin(Value::Hash(*id));
                    g.pin(Value::Block(block));
                    for (k, v) in &pairs {
                        if k.is_gc_heap_ref() { g.pin(k.clone()); }
                        if v.is_gc_heap_ref() { g.pin(v.clone()); }
                    }
                    let pre_frames = g.vm.frames.len();
                    for (k, v) in pairs {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                        g.vm.pinned.push(Value::Array(pair_id));
                        let step_result = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                        g.vm.pinned.pop();
                        let key = match step_result? {
                            BlockStep::MethodReturn => break,
                            BlockStep::Break(r) => { early = Some(r); break; }
                            BlockStep::Value(r) => r,
                        };
                        best = match best {
                            None => Some((k, v, key)),
                            Some((bk, bv, bkey)) => {
                                let ord = match value_cmp_v(&key, &bkey, &g.vm.interner) {
                                    Some(o) => o,
                                    None => return Ok(None),
                                };
                                let want_replace = if want_max {
                                    ord == std::cmp::Ordering::Greater
                                } else {
                                    ord == std::cmp::Ordering::Less
                                };
                                if want_replace { Some((k, v, key)) }
                                else { Some((bk, bv, bkey)) }
                            }
                        };
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if let Some((k, v, _)) = best {
                    // PinGuard the winning pair across the explicit
                    // `maybe_gc`: previously k/v were Rust locals
                    // with no root, so STRESS_GC could sweep them
                    // before the new Array was alloc'd → dangling
                    // ObjIds inside the result.
                    let mut g = PinGuard::new(self);
                    g.pin(k.clone());
                    g.pin(v.clone());
                    g.vm.maybe_gc();
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    Some(Value::Array(pid))
                } else {
                    Some(Value::Nil)
                }
            }

            // Hash#sort_by — yield (k, v), use returned key as the
            // sort key, return an Array of [k, v] pairs in key
            // order. Stability preserved via insertion sort.
            (Value::Hash(id), "sort_by", []) => {
                // PinGuard wraps the *entire* impl, not just the
                // block-invocation phase. Previously the guard
                // dropped before the post-loop `maybe_gc`, leaving
                // `keyed` (a Rust local) holding ObjId-bearing
                // Values with no GC root → STRESS_GC swept them and
                // the resulting Array<[k,v]> had dangling slots
                // that exploded inside `to_display`.
                let pairs_in: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let mut keyed: Vec<(Value, Value, Value)> = Vec::with_capacity(pairs_in.len());
                let mut early: Option<Value> = None;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(*id));
                g.pin(Value::Block(block));
                for (k, v) in &pairs_in {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                for (k, v) in pairs_in {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let key = match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if key.is_gc_heap_ref() { g.pin(key.clone()); }
                    keyed.push((key, k, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let n = keyed.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let ord = {
                            let a = keyed[j - 1].0.clone();
                            let b = keyed[j].0.clone();
                            g.vm.user_cmp(&a, &b)?
                        };
                        match ord {
                            None => return Ok(None),
                            Some(std::cmp::Ordering::Greater) => {
                                keyed.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                g.vm.maybe_gc();
                let mut out: Vec<Value> = Vec::with_capacity(keyed.len());
                for (_, k, v) in keyed {
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    let pv = Value::Array(pid);
                    g.pin(pv.clone());
                    out.push(pv);
                }
                let oid = g.vm.heap.alloc(HeapObj::Array(out));
                Some(Value::Array(oid))
            }

            // Hash#group_by — bucket pairs by the block's return.
            // Each bucket is an Array of [k, v] pairs; the result
            // is a Hash from group-key → Array.
            (Value::Hash(id), "group_by", []) => {
                // Same GC root-hole pattern as sort_by above: the
                // previous impl scoped PinGuard only across the
                // block invocation, then dropped it and ran more
                // alloc work (with `maybe_gc`) over `buckets` and
                // each freshly-built pair Array. Extend the guard
                // and pin each new ObjId as it's created.
                let pairs_in: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let mut buckets: Vec<(Value, Vec<Value>)> = Vec::new();
                let mut early: Option<Value> = None;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(*id));
                g.pin(Value::Block(block));
                for (k, v) in &pairs_in {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                for (k, v) in pairs_in {
                    // `group_by` yields a single pair Array (CRuby's
                    // Enumerable shape) — same as `map`/`each`. The
                    // SAME `pair_id` Array is BOTH the block arg AND
                    // the value pushed into the bucket below, so the
                    // pinning needs to span the entire window.
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    let pair = Value::Array(pid);
                    g.pin(pair.clone());
                    let step_result = g.vm.step_block(block, vec![pair.clone()], pre_frames);
                    let group = match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if group.is_gc_heap_ref() { g.pin(group.clone()); }
                    let pos = buckets.iter().position(|(gk, _)| gk.ruby_eql(&group, &g.vm.heap));
                    match pos {
                        Some(p) => buckets[p].1.push(pair),
                        None => buckets.push((group, vec![pair])),
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                let mut hash_pairs: Vec<(Value, Value)> = Vec::with_capacity(buckets.len());
                for (gk, vs) in buckets {
                    let aid = g.vm.heap.alloc(HeapObj::Array(vs));
                    let av = Value::Array(aid);
                    g.pin(av.clone());
                    hash_pairs.push((gk, av));
                }
                let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(hash_pairs)));
                Some(Value::Hash(hid))
            }

            (Value::Hash(id), "select", []) | (Value::Hash(id), "filter", []) => Some(self.iter_hash_filter(*id, IterMode::Select, block)?),
            (Value::Hash(id), "reject", []) => Some(self.iter_hash_filter(*id, IterMode::Reject, block)?),
            (Value::Hash(id), "find", []) | (Value::Hash(id), "detect", []) => Some(self.iter_hash_filter(*id, IterMode::Find, block)?),
            (Value::Hash(id), "any?", []) => Some(self.iter_hash_filter(*id, IterMode::Any, block)?),
            (Value::Hash(id), "all?", []) => Some(self.iter_hash_filter(*id, IterMode::All, block)?),
            (Value::Hash(id), "none?", []) => Some(self.iter_hash_filter(*id, IterMode::NoneM, block)?),
            // Hash#min / #max block-form (comparator) is out of
            // subset — only the no-block form (hash.rs) is
            // implemented. Without this guard arm the
            // block-given call would fall through every Hash
            // iter.rs arm and surface as
            // `NoMethodError: undefined method min/max for Hash`
            // even though `respond_to?(:min)` returns true
            // (lookup.rs widens both names). Raise a clear
            // ArgumentError naming the gap so users know to
            // either drop the block or wait for the block-form.
            (Value::Hash(_), "min" | "max", []) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "Hash#{name} block-form (comparator) is not supported in this subset; \
                         use the no-block form or sort_by + first/last",
                    ),
                }));
            }
            // `h.one? { |pair| ... }` / `{ |k, v| ... }` — true
            // iff exactly one entry yields truthy. CRuby yields
            // a single `[k, v]` Array per entry (matching
            // Hash#each); `|k, v|` blocks auto-splat. Standalone
            // arm rather than an IterMode extension because the
            // count-then-compare shape doesn't fit the
            // Any/All/NoneM short-circuit loop in
            // `iter_hash_filter`.
            (Value::Hash(id), "one?", []) => {
                let id = *id;
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut count: i64 = 0;
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() {
                        count += 1;
                        // Short-circuit: more than one truthy
                        // means we already know the answer.
                        if count > 1 { break; }
                    }
                }
                Some(early.unwrap_or(Value::Bool(count == 1)))
            }
            // `h.partition { |pair| ... }` / `{ |k, v| ... }` —
            // returns `[truthy_pairs_array, falsy_pairs_array]`.
            // Each pair is materialised as a fresh `[k, v]`
            // Array. CRuby yields a single pair Array per entry
            // (matching Hash#each); single-param blocks receive
            // the pair, two-param blocks auto-splat.
            (Value::Hash(id), "partition", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let yes_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(yes_id));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let no_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(no_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    let target = if r.is_truthy() { yes_id } else { no_id };
                    g.vm.heap.array_mut(target).push(Value::Array(pair_id));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![
                    Value::Array(yes_id), Value::Array(no_id),
                ]));
                Some(Value::Array(pair_id))
            }
            // `h.take_while { |pair| ... }` / `{ |k, v| ... }`
            // — prefix where the block is truthy; stops at the
            // first falsy return (block NOT invoked after).
            // `h.drop_while { |pair| ... }` — suffix AFTER the
            // crossing point; block invoked until the first
            // falsy then the rest passes through unconditionally.
            // Both yield a single `[k, v]` pair Array per entry
            // (matches Hash#each / partition / one? convention).
            (Value::Hash(id), "take_while" | "drop_while", []) => {
                let id = *id;
                let is_take = name == "take_while";
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut crossed = false;
                for (k, v) in snapshot {
                    if crossed {
                        // drop_while past the crossover: append
                        // remaining pairs without invoking block.
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                        g.vm.heap.array_mut(result_id).push(Value::Array(pair_id));
                        continue;
                    }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    let truthy = r.is_truthy();
                    if is_take {
                        if truthy {
                            g.vm.heap.array_mut(result_id).push(Value::Array(pair_id));
                        } else {
                            break;
                        }
                    } else {
                        // drop_while: keep dropping until first
                        // falsy, then collect THIS pair + rest.
                        if !truthy {
                            crossed = true;
                            g.vm.heap.array_mut(result_id).push(Value::Array(pair_id));
                        }
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `h.each_slice(n) { |slice| ... }` — yield each
            // consecutive group of n `[k, v]` pair Arrays as a
            // single Array argument; return the receiver Hash
            // (CRuby parity — block-form returns the receiver,
            // not nil). Last slice may be shorter than n.
            (Value::Hash(_), "each_slice", [Value::Float(f)]) => {
                let n = self.float_to_int_arg(*f)?;
                return self.collection_call_block(recv, name, &[Value::Int(n)], block);
            }
            (Value::Hash(id), "each_slice", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("invalid slice size: {}", n),
                    }));
                }
                let id = *id;
                let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                'outer: for chunk in snapshot.chunks(n_usz) {
                    // Per-iter scope: snapshot pinned-len at iter
                    // start, push transient pair/slice pins
                    // directly, then truncate back unconditionally
                    // after the closure returns (Ok OR Err).
                    // Wrapping the body in a closure routes any
                    // `?` early-return through the post-closure
                    // truncate, so per-iter pins are released
                    // at end of each chunk on every path — no
                    // accumulation across iterations, no leak on
                    // check_alloc trap. `g.pin()` isn't used here
                    // because PinGuard's Drop only fires at
                    // function exit (would keep all iters'
                    // pins alive — O(snapshot.len()) growth).
                    let iter_baseline = g.vm.pinned.len();
                    let step_result: Result<BlockStep, Trap> = (|| {
                        let mut pair_ids: Vec<Value> = Vec::with_capacity(chunk.len());
                        for (k, v) in chunk {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let pid = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                            g.vm.pinned.push(Value::Array(pid));
                            pair_ids.push(Value::Array(pid));
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let slice_id = g.vm.heap.alloc(HeapObj::Array(pair_ids));
                        g.vm.pinned.push(Value::Array(slice_id));
                        g.vm.step_block(block, vec![Value::Array(slice_id)], pre_frames)
                    })();
                    g.vm.pinned.truncate(iter_baseline);
                    match step_result? {
                        // Non-local `return` from inside the block:
                        // bubble out immediately as Nil so the outer
                        // dispatch loop reads `vm.method_return` and
                        // unwinds. Matching the chunk_while
                        // convention; `break 'outer` here would
                        // swallow the return signal and push the
                        // receiver onto the stack mid-unwind.
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break 'outer; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            // Wrong-arity / non-Int for Hash#each_slice block form.
            (Value::Hash(_), "each_slice", _) => {
                return Err(self.arity_error_arg1_int(name, args));
            }
            // `h.each_cons(n) { |window| ... }` — sliding window
            // of n consecutive `[k, v]` pair Arrays. No yields
            // when receiver has fewer than n pairs. Returns
            // receiver Hash (CRuby parity).
            (Value::Hash(_), "each_cons", [Value::Float(f)]) => {
                let n = self.float_to_int_arg(*f)?;
                return self.collection_call_block(recv, name, &[Value::Int(n)], block);
            }
            (Value::Hash(id), "each_cons", [Value::Int(n)]) => {
                if *n <= 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("invalid size: {}", n),
                    }));
                }
                let id = *id;
                let n_usz = usize::try_from(*n).unwrap_or(usize::MAX);
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                // Pre-materialise pair Arrays once; CRuby shares
                // pair identity across overlapping windows. Each
                // pinned pair Array transitively pins its `k` /
                // `v` contents, so no per-entry snapshot pin is
                // needed (would just inflate vm.pinned for large
                // hashes — GC root-walk cost).
                let mut pair_vals: Vec<Value> = Vec::with_capacity(snapshot.len());
                for (k, v) in &snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                    g.pin(Value::Array(pid));
                    pair_vals.push(Value::Array(pid));
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                if pair_vals.len() >= n_usz {
                    'outer: for win in pair_vals.windows(n_usz) {
                        // Per-iter scope: see each_slice arm
                        // above for the closure+truncate
                        // rationale. The window pin is released
                        // at end of each iteration on every
                        // path, keeping `vm.pinned` size bounded
                        // by `pair_vals.len()` (already pre-
                        // pinned) instead of growing
                        // O(number_of_windows).
                        let iter_baseline = g.vm.pinned.len();
                        let step_result: Result<BlockStep, Trap> = (|| {
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let wid = g.vm.heap.alloc(HeapObj::Array(win.to_vec()));
                            g.vm.pinned.push(Value::Array(wid));
                            g.vm.step_block(block, vec![Value::Array(wid)], pre_frames)
                        })();
                        g.vm.pinned.truncate(iter_baseline);
                        match step_result? {
                            // See each_slice arm above — non-local
                            // `return` must bubble out as Nil so
                            // outer dispatch reads `method_return`.
                            BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                            BlockStep::Break(r) => { early = Some(r); break 'outer; }
                            BlockStep::Value(_) => {}
                        }
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            // Wrong-arity / non-Int for Hash#each_cons block form.
            (Value::Hash(_), "each_cons", _) => {
                return Err(self.arity_error_arg1_int(name, args));
            }
            // `h.chunk_while { |a, b| pred(a, b) }` — partition
            // entries into runs where the block (called with two
            // adjacent `[k, v]` pair Arrays) returns truthy.
            // Falsy starts a new chunk. Result is an Array of
            // chunk Arrays, each chunk containing pair Arrays.
            // Empty hash → `[]`; single-pair hash → `[[[k,v]]]`.
            (Value::Hash(id), "chunk_while", []) => {
                let id = *id;
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                // Pre-materialise pair Arrays so adjacent
                // chunks see the same identity (CRuby parity
                // with Array#chunk_while where adjacent
                // elements are the same Value). Each pinned
                // pair transitively pins its `k` / `v`
                // contents, so no per-entry snapshot pin
                // is needed.
                let mut pair_vals: Vec<Value> = Vec::with_capacity(snapshot.len());
                for (k, v) in &snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                    g.pin(Value::Array(pid));
                    pair_vals.push(Value::Array(pid));
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                if pair_vals.is_empty() {
                    return Ok(Some(Value::Array(result_id)));
                }
                let pre_frames = g.vm.frames.len();
                let mut current_chunk: Vec<Value> = vec![pair_vals[0].clone()];
                let mut early: Option<Value> = None;
                for pair in pair_vals.windows(2) {
                    let r = match g.vm.step_block(block, vec![pair[0].clone(), pair[1].clone()], pre_frames)? {
                        BlockStep::MethodReturn => return Ok(Some(Value::Nil)),
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() {
                        current_chunk.push(pair[1].clone());
                    } else {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let chunk_id = g.vm.heap.alloc(HeapObj::Array(std::mem::take(&mut current_chunk)));
                        g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                        current_chunk.push(pair[1].clone());
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if !current_chunk.is_empty() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let chunk_id = g.vm.heap.alloc(HeapObj::Array(current_chunk));
                    g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                }
                Some(Value::Array(result_id))
            }
            // Wrong-arity for Hash#chunk_while (takes 0 args).
            (Value::Hash(_), "chunk_while", many) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        many.len()
                    ),
                }));
            }
            // `h.find_index { |pair| ... }` — returns the Int
            // index of the first entry whose block result is
            // truthy, or nil. Same yield shape as one? /
            // partition (single pair Array per entry).
            (Value::Hash(id), "find_index", []) => {
                let id = *id;
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut found: Option<i64> = None;
                let mut early = None;
                for (i, (k, v)) in snapshot.into_iter().enumerate() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() { found = Some(i as i64); break; }
                }
                Some(early.unwrap_or_else(|| match found {
                    Some(idx) => Value::Int(idx),
                    None => Value::Nil,
                }))
            }
            // Wrong-arity for block-form uniq — CRuby's uniq
            // takes no positional args (just an optional
            // block). Without this guard, `h.uniq(1) { ... }`
            // falls through and surfaces as NoMethodError.
            (Value::Hash(_), "uniq", many) if !many.is_empty() => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        many.len(),
                    ),
                }));
            }
            // `h.tally { ... }` (block-given) — CRuby silently
            // discards the block and returns the same Hash as
            // the no-block tally. Without this arm, dispatch
            // routes block-given to iter.rs first, sees no
            // `tally` arm, and lands at NoMethodError —
            // contradicting respond_to?(:tally). Delegate to
            // the no-block hash.rs arm by calling
            // `hash_collection_call` directly with empty args.
            (Value::Hash(id), "tally", []) => {
                return self.hash_collection_call(*id, "tally", &[]);
            }
            // `h.zip(*args) { |tuple| ... }` block form is out
            // of subset. Without this guard the block would be
            // silently ignored: dispatch routes block-given to
            // iter.rs first, sees no `zip` arm here, then falls
            // through to hash.rs which DOES match the no-block
            // arm and returns the result, discarding the block.
            // Raise a clear "not supported" error instead.
            (Value::Hash(_), "zip", _) => {
                return Err(self.trap(crate::error::RubyError::ArgumentError {
                    msg: "Hash#zip block form (yielding each tuple) is not supported \
                          in this subset".to_string(),
                }));
            }
            // `arr.uniq { |x| key }` — block-form Array#uniq.
            // Block return is the uniqueness key (compared via
            // `ruby_eql`). First-seen wins on collision.
            // Mirrors the Hash#uniq block-arm pattern below:
            // seen-keys stored in a pinned heap-backed Array so
            // heap-ref keys survive across maybe_gc.
            (Value::Array(id), "uniq", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(id).clone();
                for v in &snapshot {
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                // Heap-backed seen-keys list (same pattern as
                // Hash#uniq block-form to prevent UAF on
                // swept block-return values).
                let seen_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(seen_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    let key = match g.vm.step_block(block, vec![v.clone()], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    let already_seen = g.vm.heap.array(seen_id).iter()
                        .any(|s| s.ruby_eql(&key, &g.vm.heap));
                    if !already_seen {
                        g.vm.heap.array_mut(seen_id).push(key);
                        g.vm.heap.array_mut(result_id).push(v);
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `h.uniq { |pair| key }` — block-form uniq.
            // Yields a single `[k, v]` pair Array per entry;
            // the block return is the uniqueness key (compared
            // via `ruby_eql`). First-seen entry wins on
            // collision. Returns Array<[k, v]> in insertion
            // order. The no-block form lives in hash.rs (every
            // Hash entry is eql?-unique already).
            (Value::Hash(id), "uniq", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                // `seen_id` is a heap-backed Array of block
                // return values — heap-ref keys (Array / Hash /
                // String / BigInt / Object) MUST be rooted
                // across iterations, otherwise the next iter's
                // maybe_gc sweeps them and the subsequent
                // `ruby_eql` scan reads use-after-free slots.
                // Storing in a pinned Array gives them a real
                // root via the GC walker.
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let seen_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(seen_id));
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let key = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    // First-seen wins: only push if no
                    // previously-seen key is `ruby_eql` to this
                    // one. seen_id is heap-backed + pinned so
                    // its contents stay rooted across the next
                    // iter's maybe_gc.
                    let already_seen = g.vm.heap.array(seen_id).iter()
                        .any(|s| s.ruby_eql(&key, &g.vm.heap));
                    if !already_seen {
                        g.vm.heap.array_mut(seen_id).push(key);
                        g.vm.heap.array_mut(result_id).push(Value::Array(pair_id));
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `h.count { |k, v| pred }` — count pairs whose block
            // result is truthy. Same shape as Array#count block,
            // but the per-iter pair is the `[k, v]` Array (yielded
            // identically to `each` so two-param `|k, v|` blocks
            // auto-splat).
            //
            // GC discipline: pin all heap-ref k/v from `snapshot`
            // up-front so a block-driven mutation of a not-yet-
            // visited entry can't sweep it before the loop
            // reaches it. PinGuard's Drop handles cleanup on any
            // exit path including `?` propagations.
            (Value::Hash(id), "count", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    if r.is_truthy() { n += 1; }
                }
                Some(early.unwrap_or(Value::Int(n)))
            }
            // `h.flat_map { |pair| ... }` / `{ |k, v| ... }` —
            // like map then one-level flatten. CRuby yields a
            // single `[k, v]` Array per entry (matching
            // Hash#each); single-param blocks receive the pair
            // Array, two-param blocks get the destructured key
            // and value via auto-splat. Passing `k` and `v` as
            // separate args would bind a single-param `|pair|`
            // to just the key.
            (Value::Hash(id), "flat_map", []) | (Value::Hash(id), "collect_concat", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                // Defensive pin of every heap-slot k/v before the
                // block runs — block can mutate the receiver and
                // sweep elements held only in Rust-local Vecs.
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    let r = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    match r {
                        Value::Array(rid) => {
                            let items: Vec<Value> = g.vm.heap.array(rid).clone();
                            for it in items { g.vm.heap.array_mut(result_id).push(it); }
                        }
                        other => g.vm.heap.array_mut(result_id).push(other),
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `h.reduce { |acc, (k, v)| ... }` — no-init form.
            // First (k, v) pair seeds `acc` as a fresh `[k, v]`
            // Array; subsequent pairs are passed as the second
            // argument (also packaged as `[k, v]`). Empty Hash
            // returns nil. The block's second arg is destructurable
            // as `|acc, (k, v)|`.
            (Value::Hash(id), "reduce", []) | (Value::Hash(id), "inject", []) => {
                let id = *id;
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let first_id = g.vm.heap.alloc(HeapObj::Array(vec![snapshot[0].0.clone(), snapshot[0].1.clone()]));
                g.pin(Value::Array(first_id));
                let pre_frames = g.vm.frames.len();
                let mut acc = Value::Array(first_id);
                let mut early = None;
                for (k, v) in &snapshot[1..] {
                    // Pin the current accumulator BEFORE the
                    // loop-top maybe_gc / pair-Array alloc:
                    // after iter 1, `acc` is whatever the block
                    // returned, which may be a heap-backed
                    // Array / Hash / BigInt held only in this
                    // Rust local. The maybe_gc + heap.alloc
                    // sequence below would otherwise sweep it
                    // before we get a chance to pin it
                    // (reproducible under STRESS_GC=1 as
                    // `ICE: use-after-free`).
                    let acc_heap = acc.is_gc_heap_ref();
                    if acc_heap { g.vm.pinned.push(acc.clone()); }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![acc.clone(), Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    if acc_heap { g.vm.pinned.pop(); }
                    match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => { acc = r; }
                    }
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Hash(id), "reduce", [init]) | (Value::Hash(id), "inject", [init]) => {
                let id = *id;
                let init = init.clone();
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                g.pin(init.clone());
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut acc = init;
                let mut early = None;
                for (k, v) in &snapshot {
                    // Pin `acc` BEFORE the loop-top maybe_gc —
                    // see no-init form's comment for the
                    // STRESS_GC=1 use-after-free rationale.
                    let acc_heap = acc.is_gc_heap_ref();
                    if acc_heap { g.vm.pinned.push(acc.clone()); }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![acc.clone(), Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    if acc_heap { g.vm.pinned.pop(); }
                    match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => { acc = r; }
                    }
                }
                Some(early.unwrap_or(acc))
            }
            // `h.sum { |pair| expr }` / `{ |k, v| expr }` —
            // sums block return values. Default initial
            // accumulator is Int(0) (CRuby); an Int init seeds
            // from there. Mirrors Array#sum's dispatch via
            // `apply_int_promote` / `try_bigint_binop` so
            // Bignum overflow promotion works. CRuby yields a
            // single `[k, v]` Array per entry (matching
            // Hash#each); see the flat_map comment above for
            // the rationale.
            (Value::Hash(id), "sum", []) | (Value::Hash(id), "sum", [Value::Int(_)]) => {
                let id = *id;
                let init: i64 = match args { [Value::Int(n)] => *n, _ => 0 };
                let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let kind = crate::bytecode::BinOpKind::Add;
                let mut acc: Value = Value::Int(init);
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in &snapshot {
                    // Pin `acc` BEFORE the loop-top maybe_gc —
                    // once apply_int_promote / try_bigint_binop
                    // widens `acc` to a freshly-allocated
                    // BigInt, the maybe_gc + heap.alloc sequence
                    // below would sweep it before we get a
                    // chance to pin it (reproducible under
                    // STRESS_GC=1 as `ICE: heap slot is not a
                    // BigInt`). Int / Symbol accumulators
                    // short-circuit via is_gc_heap_ref so the
                    // all-Int hot path pays nothing.
                    let acc_heap = acc.is_gc_heap_ref();
                    if acc_heap { g.vm.pinned.push(acc.clone()); }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k.clone(), v.clone()]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step = g.vm.step_block(block, vec![Value::Array(pair_id)], pre_frames);
                    g.vm.pinned.pop();
                    if acc_heap { g.vm.pinned.pop(); }
                    let r = match step? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    match (&acc, &r) {
                        (Value::Int(x), Value::Int(y)) => {
                            acc = g.vm.apply_int_promote(kind, *x, *y)?;
                        }
                        _ => {
                            #[cfg(feature = "bignum")]
                            if let Some(next) = g.vm.try_bigint_binop(kind, &acc, &r)? {
                                acc = next;
                                continue;
                            }
                            return Ok(None);
                        }
                    }
                }
                Some(early.unwrap_or(acc))
            }
            // `h.each_with_object(memo) { |(k, v), memo| ... }`.
            // Mirrors `Array#each_with_object` but yields a pair
            // Array. Block return is ignored; `memo` is the
            // observable result. Same up-front pin discipline as
            // `Hash#count` above.
            (Value::Hash(id), "each_with_object", [seed]) => {
                let id = *id;
                let seed = seed.clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                g.pin(seed.clone());
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                for (k, v) in &snapshot {
                    if k.is_gc_heap_ref() { g.pin(k.clone()); }
                    if v.is_gc_heap_ref() { g.pin(v.clone()); }
                }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.pinned.push(Value::Array(pair_id));
                    let step_result = g.vm.step_block(block, vec![Value::Array(pair_id), seed.clone()], pre_frames);
                    g.vm.pinned.pop();
                    match step_result? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(_) => {}
                    }
                }
                Some(early.unwrap_or(seed))
            }

            (Value::Range(id), "select", []) | (Value::Range(id), "filter", []) => self.iter_range_filter(*id, IterMode::Select, block)?,
            (Value::Range(id), "reject", []) => self.iter_range_filter(*id, IterMode::Reject, block)?,
            (Value::Range(id), "find", []) | (Value::Range(id), "detect", []) => self.iter_range_filter(*id, IterMode::Find, block)?,
            (Value::Range(id), "any?", []) => self.iter_range_filter(*id, IterMode::Any, block)?,
            (Value::Range(id), "all?", []) => self.iter_range_filter(*id, IterMode::All, block)?,
            (Value::Range(id), "none?", []) => self.iter_range_filter(*id, IterMode::NoneM, block)?,
            (Value::Range(id), "one?", []) => self.iter_range_filter(*id, IterMode::One, block)?,

            (Value::Range(id), "map", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let count = if excl { (ei - bi).max(0) } else { (ei - bi + 1).max(0) };
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(count as usize)));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                while i <= end_inc {
                    let r = match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                        BlockStep::MethodReturn => break,
                        BlockStep::Break(r) => { early = Some(r); break; }
                        BlockStep::Value(r) => r,
                    };
                    g.vm.heap.array_mut(result_id).push(r);
                    i += 1;
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }

            // Range Enumerable fallback: materialize as an Array
            // and re-dispatch through the Array arms above. This
            // gets each_with_index / each_with_object / partition
            // / min_by / max_by / group_by / sort_by "for free"
            // and keeps a single source of truth for the
            // iteration semantics. Cost: one Vec<Value::Int>
            // allocation. Only Int-bounded ranges (the common
            // case) qualify — heterogeneous ranges would need
            // their own dispatch.
            (Value::Range(id), name, args) if matches!(name,
                "each_with_index" | "each_with_object" |
                "partition" | "min_by" | "max_by" |
                "group_by" | "sort_by"
            ) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        // Str+Str ranges (e.g. ('a'..'z')) are
                        // supported by Range#each via str_succ
                        // but not yet by each_slice / each_cons.
                        // Returning Ok(None) here used to fall
                        // through to NoMethodError — but
                        // `respond_to?(:each_slice)` is true
                        // for any Range, so that contradicted
                        // the lockstep contract documented at
                        // lookup.rs:756. Raise RuntimeError
                        // instead (same fallback shape as the
                        // zero-arg find_index path in
                        // array.rs:357 / PR #308 cycle 3).
                        _ => return Err(self.trap(crate::error::RubyError::RuntimeError {
                            msg: format!(
                                "Range#{name} with non-Int endpoints is not yet implemented in rubyrs"
                            ),
                        })),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut elems: Vec<Value> = Vec::new();
                let mut v = bi;
                while v <= end_inc {
                    elems.push(Value::Int(v));
                    v += 1;
                }
                // Pin the block AND every incoming arg FIRST: a
                // STRESS_GC pass triggered by `maybe_gc` below could
                // otherwise sweep the block-handle slot or an arg
                // value (e.g. the memo Hash passed to
                // `each_with_object({})`) — neither is necessarily
                // on the operand stack at this point, only borrowed
                // through `&[Value]` from the dispatch caller, which
                // doesn't count as a GC root. Symptoms were the
                // "ICE: heap slot is not a Block" and "is not a Hash"
                // panics in `range_enumerable`.
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                for a in args { g.pin(a.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let arr_id = g.vm.heap.alloc(HeapObj::Array(elems));
                g.pin(Value::Array(arr_id));
                let arr_val = Value::Array(arr_id);
                return g.vm.collection_call_block(&arr_val, name, args, block);
            }
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Block-iteration short-circuit semantics — `break` /
    //! `next` / non-local `return` inside iter_array_filter
    //! and friends. These are integration-shape tests (drive
    //! `Runtime::eval` with small scripts and assert stdout)
    //! because the per-driver short-circuit machinery is
    //! deeply tied to dispatch / step, and unit-testing it in
    //! isolation would mean reconstructing most of the
    //! evaluator.
    //!
    //! The diff_cruby `enumerable_filter.rb` /
    //! `control_flow.rb` fixtures cover the end-to-end CRuby
    //! match; these tests pin the contract module-locally so a
    //! regression surfaces here first.
    use crate::{Config, Runtime};

    /// Configure a Runtime with stdout pointing at a shared
    /// in-memory buffer, run `src`, return the captured stdout.
    fn capture(src: &str) -> String {
        use std::sync::{Arc, Mutex};
        // Box<dyn Write> doesn't expose the inner buffer, so wrap
        // an Arc<Mutex<Vec<u8>>> behind a small adapter that
        // implements `io::Write` and clones the Arc.
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut rt = Runtime::with_config(Config::default());
        rt.set_stdout(Box::new(Sink(buf.clone())));
        let _ = rt.eval(src, "test.rb").expect("eval succeeded");
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn array_each_break_returns_break_value() {
        // `break` from inside Array#each makes the call return the
        // break-arg.
        let out = capture(r#"
            r = [1, 2, 3, 4].each { |x| break "stop at #{x}" if x == 2 }
            puts r
        "#);
        assert_eq!(out, "stop at 2\n");
    }

    #[test]
    fn array_each_break_no_value_returns_nil() {
        let out = capture(r#"
            r = [1, 2, 3].each { |x| break if x == 2 }
            puts r.nil?
        "#);
        assert_eq!(out, "true\n");
    }

    #[test]
    fn array_map_next_replaces_iteration_value() {
        // `next val` makes the current iteration's contribution be
        // `val` (Array#map collects what each block call returns).
        let out = capture(r#"
            r = [1, 2, 3].map { |x| next 0 if x == 2; x }
            p r
        "#);
        assert_eq!(out, "[1, 0, 3]\n");
    }

    #[test]
    fn array_select_break_truncates() {
        let out = capture(r#"
            r = [1, 2, 3, 4, 5].select { |x| break [-1] if x == 3; x.even? }
            p r
        "#);
        // Break short-circuits the select entirely, returning the
        // break value (not the partial selection).
        assert_eq!(out, "[-1]\n");
    }

    #[test]
    fn array_each_with_index_break_value() {
        let out = capture(r#"
            r = ["a", "b", "c"].each_with_index { |v, i| break i if v == "b" }
            puts r
        "#);
        assert_eq!(out, "1\n");
    }

    #[test]
    fn hash_each_next_continues() {
        // `next` (no value) just skips to the next iteration.
        let out = capture(r#"
            sum = 0
            { a: 1, b: 2, c: 3 }.each { |(_k, v)| next if v == 2; sum = sum + v }
            puts sum
        "#);
        assert_eq!(out, "4\n");
    }

    #[test]
    fn range_each_break_returns_value() {
        let out = capture(r#"
            r = (1..10).each { |i| break "early at #{i}" if i > 3 }
            puts r
        "#);
        assert_eq!(out, "early at 4\n");
    }

    #[test]
    fn nonlocal_return_from_block_exits_enclosing_method() {
        // `return` from inside a driver block escapes the entire
        // method, not just the block. Different from `break`,
        // which only exits the driver and returns to the method.
        let out = capture(r#"
            def find
              [1, 2, 3].each { |x| return "got #{x}" if x == 2 }
              "not found"
            end
            puts find
        "#);
        assert_eq!(out, "got 2\n");
    }

    // ---------- Per-invocation closure-locals contract ----------
    //
    // Companion to `tests/diff/closure_in_iter_capture.rb` (gem-
    // oracle byte-diff). These module-local tests pin the same
    // contract that `step_block` + `invoke_block`'s fresh-clone
    // path enforce, the `find_lexical_owner_frame` + writeback-
    // chain walker fixes from `923adc51` / `d397eaa2`. A
    // regression in any of:
    //
    //   * invoke_block fresh-clone path (per-iter isolation)
    //   * `propagate_outer_write` chain walk (counter / nested
    //     write-through to outer-method locals)
    //   * Op::Yield's `find_lexical_owner_frame` seed (yield from
    //     nested block must find the enclosing method even though
    //     the block frame's `locals` is no longer Rc-shared with
    //     the method)
    //   * Op::ReturnMethod's owner-locals stash (non-local return
    //     from nested block must locate the lexical owner)
    //
    // surfaces in iter.rs's coverage gate too — the original
    // coverage drop from `923adc51` (93% → 88%) traced to these
    // exact branches.

    #[test]
    fn per_iter_lambda_capture_returns_distinct_values() {
        // The headline shape — `.map { |s| -> { s } }` must
        // return one lambda per iteration with the per-iter
        // value, NOT the last iteration leaking to every lambda.
        // Pre-`923adc51` this returned `[:c, :c, :c]`.
        let out = capture(r#"
            ls = [:a, :b, :c].map { |s| -> { s } }
            p ls.map(&:call)
        "#);
        assert_eq!(out, "[:a, :b, :c]\n");
    }

    #[test]
    fn counter_aggregation_through_each_writes_back_to_outer() {
        // The `propagate_outer_write` contract — block-frame
        // StoreLocal/IncLocal/IncLocalNoPush on a slot in the
        // surrounding method's scope must reach the method's
        // locals so the post-loop read sees the accumulated
        // value, NOT the pre-loop snapshot.
        let out = capture(r#"
            counter = 0
            [1, 2, 3].each { |x| counter += x }
            puts counter
        "#);
        assert_eq!(out, "6\n");
    }

    #[test]
    fn nested_block_writes_propagate_to_method_locals() {
        // `propagate_outer_write`'s chain walk past intermediate
        // block frames. Inner block writes `result = :found`;
        // the value must reach the surrounding method's `result`
        // slot through the outer block frame's writeback Rc, not
        // stop at the outer block's fresh per-invocation Vec.
        let out = capture(r#"
            def nested_writer
              result = nil
              [1].each do
                [1].each do
                  result = :found
                end
              end
              result
            end
            p nested_writer
        "#);
        assert_eq!(out, ":found\n");
    }

    #[test]
    fn yield_from_nested_block_resolves_enclosing_method() {
        // `Op::Yield`'s `find_lexical_owner_frame` walk. The
        // method's `block_arg` is reachable from inside `.times`
        // (nested block) even though the block frame's `locals`
        // is no longer Rc-shared with the method (fresh-clone
        // path). Pre-fix this raised
        // `no block given (yield)`.
        let out = capture(r#"
            class Body
              def each
                10.times { |i| yield i if i < 3 }
              end
            end
            collected = []
            Body.new.each { |v| collected << v }
            p collected
        "#);
        assert_eq!(out, "[0, 1, 2]\n");
    }

    #[test]
    fn nonlocal_return_from_nested_block_finds_enclosing_method() {
        // `Op::ReturnMethod`'s owner-locals walk. Non-local
        // return through nested-blocks must locate the method
        // whose lexical scope created the OUTERMOST block,
        // not just any non-block frame on the stack.
        let out = capture(r#"
            def find_in_nested
              [1, 2].each do
                [10, 20, 30].each do |v|
                  return "got #{v}" if v == 20
                end
              end
              "not found"
            end
            puts find_in_nested
        "#);
        assert_eq!(out, "got 20\n");
    }

    #[test]
    fn define_method_block_capture_per_iter_distinct() {
        // M27 A4 contract — `define_method` inside an iterator
        // block must capture each iteration's loop variable
        // independently. Reused as the surface real Sinatra
        // plugins reach for (sinatra_plugin_smoke validates the
        // same shape).
        let out = capture(r#"
            class Greeter
              [:formal, :casual, :friendly].each do |style|
                define_method("greet_#{style}") do |name|
                  "[#{style}] #{name}"
                end
              end
            end
            g = Greeter.new
            puts g.greet_formal("Alice")
            puts g.greet_casual("Bob")
            puts g.greet_friendly("Cara")
        "#);
        assert_eq!(
            out,
            "[formal] Alice\n[casual] Bob\n[friendly] Cara\n",
        );
    }

    #[test]
    fn block_local_var_captured_per_iter() {
        // Block-local var first-assigned inside the body (not a
        // block parameter, not from outer scope). Each `.times`
        // iteration creates a fresh `local`; the lambda captured
        // that iter must hold ITS local, not the last iteration's.
        let out = capture(r#"
            collected = []
            3.times do |i|
              local = i * 10
              collected << -> { local }
            end
            p collected.map(&:call)
        "#);
        assert_eq!(out, "[0, 10, 20]\n");
    }
}
