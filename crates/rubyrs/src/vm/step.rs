//! The opcode interpreter loop. Mirrors CRuby's vm_exec.c —
//! the main switch over Op variants plus the outer driver
//! that calls `step` until a frame returns or traps.
//!
//! Contents:
//!   - `dispatch` — top-level run loop, returns when the
//!     initial frame returns.
//!   - `dispatch_until` — re-entrant run loop used by
//!     `invoke_block` / `do_call_block` to interpret nested
//!     frames without unwinding.
//!   - `step` — the per-opcode big match. The bulk of the file.

use std::cell::RefCell;
use std::rc::Rc;

use crate::bytecode::{BinOpKind, Op};
use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{BlockHandle, Class, Method, ObjId, Value, Visibility};

use super::{primitive_call, vec_nil, Frame, LoopTransferKind, RescueFilter, RescueHandler, Vm};

/// Translate Onigmo-specific regex constructs into something the
/// Rust `regex` crate accepts. The crate is by design less
/// expressive than Onigmo (no backreferences, no `\G`, no
/// look-behind, ...) — the trade-off is linear-time matching and
/// no catastrophic backtracking.
///
/// Currently handled:
/// - `\G` (match-at-last-position anchor) — context-aware:
///   * Outside a character class: dropped entirely. CRuby uses
///     it for stateful scanning where the engine remembers the
///     end of the previous match; the rubyrs subset mostly
///     slices the input from the current cursor before matching,
///     so the surrounding structural anchors carry the intent.
///   * Inside a character class (`/[\G]/`): translated to bare
///     `G` so the literal-G semantic survives — CRuby treats
///     `\G` in a class as literal G, but the Rust regex crate
///     rejects the escape verbatim.
///
/// Motivating case: MRI's `lib/erb/compiler.rb:460`
/// (`/\G<%#(.*)%>/`) — without translation the LoadRegex op
/// raises SyntaxError on the `\G`.
///
/// Returns a `Cow<'_, str>`: borrowed (zero-alloc fast path) when
/// the pattern doesn't contain `\G` at all (the overwhelmingly
/// common case), owned String when translation happened.
///
/// Other Onigmo features (`\K`, `(?<=...)`, named-group backrefs
/// like `\k<name>`, etc.) still surface as the regex crate's
/// SyntaxError. Adding translations is per-feature on demand.
#[cfg(feature = "regex")]
/// Does `src` contain a named capture group — `(?<name>…)` or
/// `(?'name'…)` — outside a char class / escape? `(?<=…)` and
/// `(?<!…)` (lookbehind) are NOT named groups. Used to decide whether
/// unnamed groups must be demoted to non-capturing (CRuby's rule that
/// a named group disables numbered captures). Tracks `\` escapes and
/// `[…]` classes so a literal `(?<` inside either doesn't false-fire.
fn pattern_has_named_group(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    let mut in_class = false;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == b'[' && !in_class {
            in_class = true;
            i += 1;
            continue;
        }
        if c == b']' && in_class {
            in_class = false;
            i += 1;
            continue;
        }
        if !in_class && c == b'(' && i + 2 < b.len() && b[i + 1] == b'?' {
            match b[i + 2] {
                b'\'' => return true, // (?'name'…)
                // (?<name>…) — but not lookbehind (?<= / (?<!
                b'<' if i + 3 < b.len() && b[i + 3] != b'=' && b[i + 3] != b'!' => {
                    return true
                }
                _ => {}
            }
        }
        i += 1;
    }
    false
}

pub(crate) fn preprocess_regex_pattern(src: &str) -> std::borrow::Cow<'_, str> {
    // When a pattern contains ANY named capture group, Ruby/Onigmo
    // demotes every UNNAMED `(…)` group to non-capturing — only the
    // named groups are numbered. So `/(a)(?<x>b)/` matched against
    // "ab" has `size == 2` (whole + x), `captures == ["b"]`, and `$1`
    // is the `x` group. The linear/fancy engines keep unnamed groups
    // captured, so rewrite `(` → `(?:` here to match CRuby's group
    // numbering. Computed up front so the scanner only fires the
    // rewrite when there's actually a named group present.
    let demote_unnamed = pattern_has_named_group(src);
    // Fast path: most regexes don't use `\G` and have no named
    // group. Skip the whole scan + allocation in that case.
    if !src.contains("\\G") && !demote_unnamed {
        return std::borrow::Cow::Borrowed(src);
    }
    // Tracks whether we are inside an outer character class
    // (single bool, not a depth counter — POSIX subclasses like
    // `[:alpha:]` are skipped as a unit below so they never
    // re-toggle). `\G` is only stripped when it's the Onigmo
    // anchor (outside any `[...]`). Inside a character class —
    // `/[\G]/` — `\G` is a literal `G` in every regex dialect,
    // and dropping the `\\G` would change it to an empty
    // character class (regex compile error) or collapse it
    // with neighbours.
    //
    // POSIX classes (`[:digit:]`, `[:alpha:]`, etc.) need their
    // own pass: the inner `]` that closes `:digit:]` would
    // otherwise prematurely flip `in_class` to false on a
    // pattern like `/[[:digit:]\G]/`. We detect `[:`,
    // `[=`, `[.` after entering a class and skip past the
    // matching `:]`/`=]`/`.]` as a unit.
    // Accumulate as raw bytes so multibyte UTF-8 sequences pass
    // through unchanged. `out.push(c as char)` would write each
    // byte as a separate Latin-1 codepoint, mangling any non-
    // ASCII pattern text (CJK literals, U+FFFD from invalid-byte
    // recovery, etc.). All structural tokens we look for (`\`,
    // `G`, `[`, `]`, `:`, `=`, `.`) are single-byte ASCII so
    // operating at the byte level is safe — UTF-8 multibyte
    // bytes are all 0x80+, never confusable with ASCII.
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let mut i = 0;
    let mut in_class = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            // `\G` outside a char class → drop entirely (Onigmo
            // anchor with no Rust regex equivalent).
            // `\G` inside a char class → CRuby treats as literal
            // `G`. The Rust regex crate rejects the `\G` escape
            // even inside a class, so we translate to bare `G`
            // (`/[\G]/` → `/[G]/`).
            if next == b'G' {
                if in_class {
                    out.push(b'G');
                }
                i += 2;
                continue;
            }
            // Other escapes pass through unchanged. `next` is
            // always ASCII (\d, \s, \., \\, ...); if a multibyte
            // codepoint somehow followed a `\` we'd want to copy
            // all of its bytes — but Ruby/Onigmo doesn't allow
            // `\<multibyte>` as an escape, so the next byte
            // being ASCII is invariant.
            out.push(c);
            out.push(next);
            i += 2;
            continue;
        }
        // POSIX / collating-class skip — only inside a class,
        // and only when the `[` is followed by `:`, `=`, or `.`.
        // Find the matching closer (`:]`, `=]`, `.]`) and copy
        // the whole token verbatim so neither `in_class` nor
        // `\G` handling re-fires inside it.
        if in_class
            && c == b'['
            && i + 1 < bytes.len()
            && matches!(bytes[i + 1], b':' | b'=' | b'.')
        {
            let opener = bytes[i + 1];
            let close = [opener, b']'];
            // Search forward for the closer.
            let mut j = i + 2;
            while j + 1 < bytes.len() && bytes[j..j + 2] != close {
                j += 1;
            }
            // Copy `[` through the closing `]` if found, else
            // bail out and just copy the `[` to let the regex
            // crate report its own error.
            if j + 1 < bytes.len() && bytes[j..j + 2] == close {
                out.extend_from_slice(&bytes[i..j + 2]);
                i = j + 2;
                continue;
            }
        }
        // Demote an unnamed capturing group to non-capturing when the
        // pattern has a named group (see the fast-path comment). Only
        // a bare `(` outside a char class counts: `(?…)` (non-capturing
        // / named / lookaround / flags) and a `(` inside `[…]` (literal)
        // are left untouched.
        if demote_unnamed && !in_class && c == b'(' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'?' {
                out.push(c);
                i += 1;
                continue;
            }
            out.extend_from_slice(b"(?:");
            i += 1;
            continue;
        }
        // Outer character-class bracket tracking. POSIX inner
        // classes are skipped above so their `]` doesn't reach
        // this branch.
        if c == b'[' && !in_class {
            in_class = true;
        } else if c == b']' && in_class {
            in_class = false;
        }
        out.push(c);
        i += 1;
    }
    // SAFETY: input was a &str (valid UTF-8) and every byte
    // operation above either copies an input run verbatim or
    // pushes ASCII (`G`). No way to produce invalid UTF-8.
    std::borrow::Cow::Owned(
        String::from_utf8(out).expect("ICE: preprocess_regex_pattern produced invalid UTF-8")
    )
}

/// Prepend an inline flag group (`(?is)…`) translating Ruby's
/// `i`/`x`/`m` literal flags into the regex-crate letters the
/// linear AND fancy backends both honor. THE TRAP: Ruby `/m`
/// (dot-matches-newline) maps to engine `(?s)` (single-line /
/// dotall), NOT `(?m)` (which in the regex crate is multi-line
/// `^`/`$` anchoring — a different concept Ruby has no flag for).
///
/// `flags == 0` returns the pattern Borrowed (zero-alloc fast
/// path; flagless regexps compile exactly as before). Run AFTER
/// `preprocess_regex_pattern` so the `\G` translation never sees
/// the prefix and the prefix never lands inside the `\G` scan.
#[cfg(feature = "regex")]
pub(crate) fn apply_ruby_flags(pattern: &str, flags: u8) -> std::borrow::Cow<'_, str> {
    use crate::regex_engine::{RB_IGNORECASE, RB_EXTENDED, RB_MULTILINE};
    // Only the three matcher-relevant bits build inline flags.
    // Encoding bits (FIXEDENCODING=16 / NOENCODING=32 — rack's
    // URLMap passes the latter to Regexp.new) carry no matching
    // semantics here; without the mask they'd produce an empty
    // `(?)` group and a SyntaxError.
    let flags = flags & (RB_IGNORECASE | RB_EXTENDED | RB_MULTILINE);
    if flags == 0 {
        return std::borrow::Cow::Borrowed(pattern);
    }
    let mut prefix = String::with_capacity(6 + pattern.len());
    prefix.push_str("(?");
    if flags & RB_IGNORECASE != 0 { prefix.push('i'); }
    if flags & RB_MULTILINE != 0 { prefix.push('s'); } // Ruby /m == engine (?s)
    if flags & RB_EXTENDED != 0 { prefix.push('x'); }
    prefix.push(')');
    prefix.push_str(pattern);
    std::borrow::Cow::Owned(prefix)
}

/// ADR 0025 Phase 2 cext-deferral helper. With `_fiber` on,
/// `cext_depth` exists and is honored; without `_fiber`, the
/// counter doesn't exist (no Fiber to integrate with), so the
/// safe-point treats `cext_depth == 0` as the constant `true`.
/// Production cext entry/exit paths will gate this counter
/// when the cext bridge integration lands (separate work item).
// `#[allow(dead_code)]` on both arms — only the SIGINT safe-point
// check (cfg(unix)) calls this helper. Non-unix builds compile both
// arms for the cfg-fan-out but never reach the call site.
#[cfg(feature = "_fiber")]
#[inline]
#[allow(dead_code)]
fn cext_depth_zero(vm: &crate::vm::Vm) -> bool {
    vm.cext_depth == 0
}
#[cfg(not(feature = "_fiber"))]
#[inline]
#[allow(dead_code)]
fn cext_depth_zero(_vm: &crate::vm::Vm) -> bool {
    true
}

/// ADR 0025 Phase 4b: outcome of the safe-point interrupt
/// check. Constructed by `Vm::safe_point_interrupt_action`,
/// consumed by `InterruptAction::deliver`. Models the three
/// trap-handler outcomes from `SignalHandlerState`:
///   `RaiseInterrupt` — Default state (no trap or "DEFAULT").
///   `Clear`          — Ignore state ("IGNORE" / "SIG_IGN").
///   `InvokeBlock(id)`— user-installed Proc / block.
///
/// Pulled out of the dispatch loop body for two reasons:
///   (1) `dispatch` and `dispatch_until` share the logic;
///   (2) the block-invoke path needs to call back into
///   `dispatch_until` re-entrantly, which is awkward inside
///   the loop body itself.
#[cfg(unix)]
enum InterruptAction {
    RaiseInterrupt,
    Clear,
    InvokeBlock(crate::value::ObjId),
}

#[cfg(unix)]
impl InterruptAction {
    /// Execute the chosen action against the Vm. After this
    /// returns Ok, the dispatch loop should `continue` —
    /// state has been adjusted, the IP may have moved (rescue
    /// handler for RaiseInterrupt; trap block return for
    /// InvokeBlock), and the interrupt flag has been cleared.
    fn deliver(self, vm: &mut crate::vm::Vm) -> Result<(), Trap> {
        use std::sync::atomic::Ordering;
        // Clear the flag regardless — every action consumes
        // it. (For Default + Clear cases the next safe-point
        // re-arms on the next SIGINT; for Block the
        // suppress_interrupt window below holds off any
        // re-entry during the trap.)
        vm.interrupt_pending.store(false, Ordering::Relaxed);
        match self {
            Self::RaiseInterrupt => {
                let exc = match crate::vm::raise::build_interrupt_exception(vm) {
                    Some(v) => v,
                    None => {
                        return Err(vm.trap(RubyError::Interrupt {
                            msg: "interrupt".to_string(),
                        }));
                    }
                };
                vm.unwind_with_exception(exc)
            }
            Self::Clear => Ok(()),
            Self::InvokeBlock(block_id) => {
                // Re-entrant dispatch of the user's trap block.
                // The `SuppressInterruptGuard` increments
                // `suppress_interrupt` on entry and decrements
                // on Drop — so a panic in `invoke_block` or the
                // nested `dispatch_until` CANNOT leak the
                // counter (which would permanently disable
                // SIGINT delivery for the Vm's remaining life).
                // Round-3 review safety finding.
                let _guard = crate::vm::SuppressInterruptGuard::enter(vm);
                let pre_frames = _guard.vm.frames.len();
                _guard.vm.invoke_block(block_id, vec![])?;
                _guard.vm.dispatch_until(pre_frames)?;
                // Block returned. Pop its return value; trap
                // handlers in CRuby return values are ignored
                // (the canonical use is side-effects only,
                // e.g. `exit` or logging).
                _guard.vm.stack.pop();
                Ok(())
            }
        }
    }
}

impl Vm {
    /// ADR 0025 Phase 4b: compute the safe-point interrupt
    /// action, or `None` if no action is warranted at this op.
    ///
    /// `None` for the common case (flag false, or
    /// suppress/cext gate closed). When `Some`, the caller
    /// invokes `.deliver(self)` and `continue`s the dispatch
    /// loop.
    ///
    /// Pulled out of the loop body so `dispatch` and
    /// `dispatch_until` share the same logic without
    /// duplication.
    #[cfg(unix)]
    fn safe_point_interrupt_action(&self) -> Option<InterruptAction> {
        use std::sync::atomic::Ordering;
        if !self.interrupt_pending.load(Ordering::Relaxed) {
            return None;
        }
        if self.suppress_interrupt != 0 {
            return None;
        }
        if !cext_depth_zero(self) {
            return None;
        }
        // Look up the SIGINT trap state. Missing entry =
        // Default behavior.
        let state = self.signal_traps.get(&signal_hook::consts::SIGINT)
            .cloned()
            .unwrap_or(crate::vm::SignalHandlerState::Default);
        Some(match state {
            crate::vm::SignalHandlerState::Default => InterruptAction::RaiseInterrupt,
            crate::vm::SignalHandlerState::Ignore => InterruptAction::Clear,
            crate::vm::SignalHandlerState::Block(id) => InterruptAction::InvokeBlock(id),
        })
    }

    /// Lazily allocate the `$LOAD_PATH` Array on first access.
    /// Idempotent — subsequent calls return the same ObjId so
    /// script mutations (`$LOAD_PATH.unshift(dir)`) land on
    /// the slot the require dispatcher later reads.
    /// `check_alloc` enforces heap caps before allocating;
    /// `maybe_gc` may sweep between calls but `Vm.load_path`
    /// is a GC root (rooted in `gc.rs`) so the slot survives.
    pub(crate) fn ensure_load_path(&mut self) -> Result<crate::value::ObjId, Trap> {
        if let Some(id) = self.load_path {
            return Ok(id);
        }
        self.maybe_gc();
        self.check_alloc()?;
        let id = self.heap.alloc(HeapObj::Array(Vec::new().into()));
        self.load_path = Some(id);
        Ok(id)
    }

    /// Location (`file`, `line`) of the op CURRENTLY executing in the
    /// top frame — the dispatch loop already advanced `ip` past it, so
    /// the live op is `ip - 1`. Used by `Op::DefClass` / `DefModule` /
    /// `StoreConst` to stamp `const_source_locations`. `None` when the
    /// source text isn't tracked (no line resolvable).
    pub(crate) fn current_op_location(&self) -> Option<(std::rc::Rc<str>, u32)> {
        let f = self.frames.last()?;
        let proto = &self.protos[f.proto_idx];
        let op_idx = f.ip.checked_sub(1)?;
        let span = proto.op_spans.get(op_idx).copied()?;
        let src = self.sources.get(proto.filename.as_ref())?;
        let line = crate::error::line_col(src, span.byte_offset).0;
        Some((proto.filename.clone(), line))
    }

    /// Lazily materialise the `$LOADED_FEATURES` / `$"` Array — the
    /// script-visible list of loaded file paths. Twin of
    /// `ensure_load_path`; GC-rooted via `Vm.loaded_features_list`.
    /// `compile_and_run_source` pushes each loaded path here, and
    /// script reads (`$LOADED_FEATURES.last` / `.reject!`) return this
    /// same Array.
    pub(crate) fn ensure_loaded_features_list(&mut self) -> Result<crate::value::ObjId, Trap> {
        if let Some(id) = self.loaded_features_list {
            return Ok(id);
        }
        self.maybe_gc();
        self.check_alloc()?;
        let id = self.heap.alloc(HeapObj::Array(Vec::new().into()));
        self.loaded_features_list = Some(id);
        Ok(id)
    }

    /// The class that owns the current `@@cvar` context, if any.
    /// Resolution mirrors CRuby's "current cref" walk:
    ///   - frame.self_val is `Value::Class(c)` (class body or
    ///     `def self.foo`) → c
    ///   - frame.self_val is `Value::Object(id)` (instance
    ///     method body) → `heap.real_class_of(id)`
    ///   - anything else (toplevel, block-in-toplevel,
    ///     primitive recv) → None, falling through to
    ///     `Vm.toplevel_cvars` at the call site
    pub(crate) fn surrounding_class(&self) -> Option<Rc<Class>> {
        let frame = self.frames.last()?;
        // Block frames carry the lexical class captured at the block's
        // creation. Prefer it so `@@cvar` resolves lexically even when
        // the block runs with a different `self` (instance_eval /
        // class_eval) — CRuby resolves cvars through the cref, not self.
        // Block frames whose lexical class is `None` (created at the top
        // level) fall through to the self_val rule below.
        if frame.is_block && frame.lexical_cvar_class.is_some() {
            return frame.lexical_cvar_class.clone();
        }
        // A METHOD body resolves `@@cvar` through its LEXICAL scope — the class
        // where the method was DEFINED (CRuby's cref) — not `self`. These differ
        // when the method is reached via `extend`/`include`: `self` is the host,
        // but the method (and its cvars) live in the mixed-in module. i18n's
        // `@@normalized_key_cache` is set in `I18n::Base`'s body and read by
        // `Base#normalize_key`, which runs as `I18n.normalize_key` (I18n extends
        // Base) — so `self` is I18n but the cvar lives on Base. Class-BODY frames
        // keep the self rule (the cvar belongs to the class being defined).
        if !frame.is_block
            && !frame.is_class_body
            && let Some(dc) = &frame.defining_class
        {
            if let Some(attached) =
                dc.singleton_target.borrow().as_ref().and_then(|w| w.upgrade())
            {
                return Some(attached);
            }
            return Some(dc.clone());
        }
        let cls = match &frame.self_val {
            Value::Class(c) => c.clone(),
            Value::Object(id) => self.heap.real_class_of(*id),
            _ => return None,
        };
        // A singleton (metaclass) scope's class variables belong to the ATTACHED
        // class — CRuby's `class << self; @@x = …` stores `@@x` on the BASE
        // class (`M.class_variables` includes it), and a singleton method reading
        // `@@x` resolves it there. Map a metaclass scope to its base class so the
        // set and the read agree.
        if let Some(attached) = cls.singleton_target.borrow().as_ref().and_then(|w| w.upgrade()) {
            return Some(attached);
        }
        Some(cls)
    }

    /// The class along `start`'s superclass chain that owns class
    /// variable `name`, or `None` if no ancestor defines it. CRuby
    /// class variables are shared across the hierarchy: a `@@x`
    /// defined in a parent is the SAME variable when read/written
    /// from a subclass. Used by Load/StoreCvar so e.g. kramdown's
    /// `@@parsers` (set in `Kramdown::Parser::Kramdown`) is visible
    /// to its `SmartyPants` subclass's inherited `define_parser`.
    pub(crate) fn cvar_owner_class(
        &self,
        start: &Rc<Class>,
        name: crate::intern::SymId,
    ) -> Option<Rc<Class>> {
        let mut cur = Some(start.clone());
        let mut guard = 0;
        while let Some(c) = cur {
            if c.class_vars.borrow().contains_key(&name) {
                return Some(c);
            }
            guard += 1;
            if guard > 4096 {
                return None;
            }
            cur = c.superclass.borrow().clone();
        }
        None
    }

    pub(crate) fn dispatch(&mut self) -> Result<(), Trap> {
        while !self.frames.is_empty() {
            debug_assert!(
                self.control_signals_synced(),
                "control_signals mask out of sync with signal fields",
            );
            // Folded-signal gate: the three control-flow signal checks
            // below only matter when SOMETHING is pending — one byte
            // test covers method_return / pending_method_break /
            // break_signaled (see Vm::control_signals). Each arm still
            // re-tests its own field; the gate just keeps the common
            // per-op iteration to a single load+branch.
            if self.control_signals != 0 {
                // ADR 0024 Phase A.5: block-break in flight. Op::Yield
                // case (b) parked the break and the Rust iter driver
                // above propagated the value via step_block;
                // continue_method_break now walks intermediate
                // frames + ensures until landing on the yielding
                // method. If the walk drains the frame stack, exit.
                if let Some(mb) = self.pending_method_break.as_ref()
                    && !mb.suspended
                    && self.frames.len() > mb.target_frame_idx
                {
                    self.continue_method_break()?;
                    if self.frames.is_empty() { return Ok(()); }
                    continue;
                }
                // ADR 0024 Phase A.8: break-after-Fiber-resume
                // recovery (see dispatch_until_inner for full
                // rationale).
                if self.break_signaled && self.frames.last()
                    .map(|f| f.pending_yield)
                    .unwrap_or(false)
                {
                    self.break_signaled = false;
                    self.sync_control_signals();
                    let target_idx = self.frames.len() - 1;
                    self.frames[target_idx].pending_yield = false;
                    let value = self.stack.pop().unwrap_or(Value::Nil);
                    self.begin_method_break(value, target_idx)?;
                    if self.frames.is_empty() { return Ok(()); }
                    continue;
                }
                // Non-local return unwind. `Op::ReturnMethod` sets
                // `method_return`; here we honour it by popping any
                // block frames between us and the enclosing method,
                // then popping the method frame and pushing the value
                // as its return. Exit the whole dispatch if we
                // unwound off the bottom of the frame stack.
                if self.method_return.is_some() {
                    // Capture the lexical-owner Rc BEFORE
                    // `take_method_return` clears it (the helper
                    // pairs the value and locals consumption to keep
                    // the invariant that they vanish together).
                    let owner_rc = self.method_return_locals.clone();
                    let val = self.take_method_return().unwrap();
                    // Lexical-aware unwind: walk frames popping
                    // intermediate blocks AND intermediate methods
                    // (the yielding-but-not-defining method, e.g.
                    // `outer` in `outer { ... return ... }` where
                    // the block was defined in caller_method). The
                    // target is the topmost non-block frame whose
                    // `locals` Rc matches the snapshot taken at
                    // `Op::ReturnMethod` time. (TRY_RUNS pass-10
                    // layer #4.)
                    //
                    // Pre-scan to locate the target index. If no
                    // match exists (block escaped its lexical scope
                    // — e.g. stored as a Proc and called from
                    // elsewhere after the owner returned, OR
                    // `method_return_locals` is None because some
                    // path set `method_return` without going through
                    // Op::ReturnMethod), fall back to the legacy
                    // "first non-block" behavior: walk while
                    // `is_block`, then pop exactly one method frame.
                    // The CRuby-correct response is LocalJumpError,
                    // but Tier-1 doesn't model that yet — tracked as
                    // a separate future layer. (Copilot review #285
                    // round 1.)
                    let target_idx: Option<usize> = match &owner_rc {
                        // A lambda frame is a valid return target too —
                        // `find_return_target` may point here for a
                        // `return` inside a lambda (local return).
                        Some(rc) => self.frames.iter().rposition(|f| {
                            (!f.is_block || f.is_lambda)
                                && f.locals
                                    .as_shared()
                                    .is_some_and(|l| std::rc::Rc::ptr_eq(l, rc))
                        }),
                        None => None,
                    };
                    if let Some(owner_idx) = target_idx {
                        // ADR 0024 Phase A.6: route method_return
                        // through the same Phase A.4/A.5 ensure-walk
                        // machinery as block-break. `begin_method_break`
                        // walks the in-flight frame stack from current
                        // top down to + including the owner; for each
                        // frame it pops the `is_ensure` rescue
                        // handlers and runs their bodies before
                        // dropping the frame, then pushes `val` (or
                        // the class for an `is_class_body` owner) onto
                        // the caller's operand stack.
                        //
                        // Pre-A.6 the unwind was a raw-pop loop that
                        // skipped intermediate ensures — `def f;
                        // begin; (1..3).each { |x| return x if x==2
                        // }; ensure; cleanup; end; end` left
                        // `cleanup` un-run.
                        self.begin_method_break(val.clone(), owner_idx)?;
                        if self.frames.is_empty() { return Ok(()); }
                    } else {
                        // ADR 0024 Phase A.6 round 2: stored Proc
                        // tried to `return` after its lexical owner
                        // (the def that created the Proc) has already
                        // returned — `method_return_locals` doesn't
                        // pin down any live frame. CRuby's response is
                        // `LocalJumpError: unexpected return`; route
                        // through `unwind_with_exception` so user
                        // rescue handlers can catch it.
                        let _ = val; // unwind discards the would-be return value
                        let trap = self.trap(RubyError::LocalJumpError {
                            msg: "unexpected return".to_string(),
                        });
                        let exc = match self.trap_to_exception(&trap) {
                            Some(e) => e,
                            None => return Err(trap),
                        };
                        let original_bt = trap.backtrace.clone();
                        let original_class = trap.err.ruby_class_name();
                        let original_msg = trap.err.message();
                        match self.unwind_with_exception(exc) {
                            Ok(()) => continue,
                            Err(_) => return Err(Trap {
                                err: RubyError::Uncaught {
                                    class_name: original_class,
                                    message: original_msg,
                                },
                                backtrace: original_bt,
                            }),
                        }
                    }
                    continue;
                }
            }
            // ADR 0025 Phase 2 + Phase 4b: SIGINT safe-point check.
            // v7 round-3 cosmetic: order placed AFTER `method_return`
            // to match `dispatch_until`'s ordering (method_return →
            // fiber_yield_pending → interrupt → fuel). dispatch has
            // no fiber path so the middle item is absent. See
            // `dispatch_until` below for the full safety rationale.
            // Hoist the cheap pending-flag load inline so the common
            // (no-interrupt) path skips the `safe_point_interrupt_action`
            // function call entirely — it showed up per-op in the
            // call-path profile. SIGINT latency is unchanged: the atomic
            // is still read every op; only the function call (which would
            // re-check the same flag and return None) is elided.
            #[cfg(unix)]
            if self.interrupt_pending.load(std::sync::atomic::Ordering::Relaxed)
                && let Some(action) = self.safe_point_interrupt_action()
            {
                action.deliver(self)?;
                continue;
            }
            // Single frames.last_mut() fetch — see dispatch_until_inner.
            let (proto_idx, ip) = {
                let f = self.frames.last_mut().expect("ICE: dispatch with empty frame stack");
                let pair = (f.proto_idx, f.ip);
                f.ip += 1;
                pair
            };
            let op = self.protos[proto_idx].code[ip];
            match self.step(op, proto_idx) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(trap) => {
                    // Synthetic signal from a nested `dispatch_until`
                    // that already redirected IP to a rescue handler
                    // in this frame. The next op fetch will land on
                    // the handler — just resume.
                    if matches!(trap.err, RubyError::AlreadyCaught) {
                        continue;
                    }
                    // Try routing the trap through the Ruby
                    // rescue machinery so scripts can `rescue`
                    // primitive errors (NoMethodError, KeyError,
                    // ArgumentError, ...). ResourceExhausted /
                    // Uncaught / SyntaxError pass through
                    // unchanged.
                    if let Some(exc) = self.trap_to_exception(&trap) {
                        // Capture the original trap's site before
                        // unwind drains the frame stack — when
                        // unwind synthesises an Uncaught Trap on
                        // miss, its backtrace is empty (frames
                        // already gone). Preserve the call-site
                        // info from the trap that actually fired.
                        let original_bt = trap.backtrace.clone();
                        let original_class = trap.err.ruby_class_name();
                        let original_msg = trap.err.message();
                        match self.unwind_with_exception(exc) {
                            Ok(()) => continue, // handler set up, resume dispatch
                            Err(_) => return Err(Trap {
                                err: RubyError::Uncaught {
                                    class_name: original_class,
                                    message: original_msg,
                                },
                                backtrace: original_bt,
                            }),
                        }
                    }
                    return Err(trap);
                }
            }
        }
        Ok(())
    }



    /// Run dispatch loop until the frame stack returns to `until_depth`.
    pub(crate) fn dispatch_until(&mut self, until_depth: usize) -> Result<(), Trap> {
        // Track our boundary so Op::Raise / Op::EndEnsure can
        // detect when their direct `unwind_with_exception` call
        // crosses out into a caller's frame, and signal the
        // native iter driver above us to bail.
        self.dispatch_until_depths.push(until_depth);
        let len_before = self.dispatch_until_depths.len();
        let r = self.dispatch_until_inner(until_depth);
        // Debug-only nesting check: every push must pair with a
        // pop at the same depth. A mismatch here would leave the
        // boundary stack out of sync with the actual nesting,
        // making subsequent AlreadyCaught checks consult the
        // wrong boundary. The `popped` value is asserted to
        // equal `until_depth` so a future refactor that
        // accidentally pushes inside `dispatch_until_inner`
        // without a matching pop fails loudly in tests/CI.
        let popped = self.dispatch_until_depths.pop();
        debug_assert_eq!(
            self.dispatch_until_depths.len() + 1,
            len_before,
            "dispatch_until_depths nesting mismatch",
        );
        debug_assert_eq!(
            popped, Some(until_depth),
            "dispatch_until_depths top mismatch on pop",
        );
        r
    }

    fn dispatch_until_inner(&mut self, until_depth: usize) -> Result<(), Trap> {
        while self.frames.len() > until_depth {
            debug_assert!(
                self.control_signals_synced(),
                "control_signals mask out of sync with signal fields",
            );
            // Folded-signal gate: the three control-flow signal checks
            // below only matter when SOMETHING is pending — one byte
            // test covers method_return / pending_method_break /
            // break_signaled (see Vm::control_signals). Each arm still
            // re-tests its own field; the gate just keeps the common
            // per-op iteration to a single load+branch.
            if self.control_signals != 0 {
                // Non-local return signal. Two cases, distinguished by
                // whether the return's lexical owner (the method frame it
                // targets) lives WITHIN this dispatch_until scope:
                //
                //  - Owner is at/below `until_depth` (or unknown): we're
                //    about to unwind past our boundary anyway. Exit early
                //    and let the iterator driver (our caller) propagate the
                //    signal — the original behaviour, and the common case
                //    for `coll.each { return }` driven from the top-level
                //    loop, where the target method sits below us.
                //
                //  - Owner is INSIDE our scope (`owner_idx >= until_depth`):
                //    the method being returned-from is one we're driving
                //    (e.g. a Ruby method called from a Rust-invoked Rack
                //    block via `call_ruby_block_sync` → `step_block`). The
                //    top-level `step` loop would consume the return at the
                //    owner frame; we must do the same here, otherwise the
                //    signal escapes all the way to the Rust caller and is
                //    reported as "no enclosing Ruby method to unwind to".
                //    Mirrors the lexical-aware unwind in the main loop and
                //    the in-scope check used for `pending_method_break`
                //    just below.
                if self.method_return.is_some() {
                    let owner_idx: Option<usize> = match &self.method_return_locals {
                        // A lambda frame is a valid return target (local
                        // return from a lambda) — see the main loop.
                        Some(rc) => self.frames.iter().rposition(|f| {
                            (!f.is_block || f.is_lambda)
                                && f.locals
                                    .as_shared()
                                    .is_some_and(|l| std::rc::Rc::ptr_eq(l, rc))
                        }),
                        None => None,
                    };
                    match owner_idx {
                        Some(idx) if idx >= until_depth => {
                            let val = self.take_method_return().unwrap();
                            self.begin_method_break(val, idx)?;
                            if self.frames.len() <= until_depth { return Ok(()); }
                            continue;
                        }
                        _ => return Ok(()),
                    }
                }
                // ADR 0024 Phase A.5/A.9: block-break in flight
                // from an Op::Yield case (b). Fire
                // continue_method_break when the target frame is
                // at-or-above the current top AND within our
                // dispatch scope (target_frame_idx >= until_depth).
                // Cases:
                //   - target == top: A.5 single-method case.
                //   - target < top: A.9 multi-method case — pop
                //     intermediate frames (running their ensures)
                //     until reaching target.
                // If target < until_depth, the target sits in a
                // frame our outer driver owns — bail and let the
                // outer dispatch level fire continue_method_break.
                if let Some(mb) = self.pending_method_break.as_ref()
                    && !mb.suspended
                    && mb.target_frame_idx >= until_depth
                    && self.frames.len() > mb.target_frame_idx
                {
                    self.continue_method_break()?;
                    if self.frames.len() <= until_depth { return Ok(()); }
                    continue;
                }
                // ADR 0024 Phase A.8: break-after-Fiber-resume
                // recovery. The original Op::Yield wrapper that
                // would have observed `break_signaled` was on the
                // Rust stack that Fiber.yield unwound; after a
                // subsequent resume, the block can run more
                // statements + break with no Rust-side observer
                // left. `pending_yield` on the top frame survived
                // the FiberSnapshot stash (per-Frame, deep-copied)
                // — that's our marker that this method's Op::Yield
                // wrapper is gone. Pop the break value off the
                // operand stack (Op::Return pushed it as the
                // block's return), clear the marker, and fire
                // the Phase A.4/A.5 unwind walk.
                if self.break_signaled && self.frames.last()
                    .map(|f| f.pending_yield)
                    .unwrap_or(false)
                {
                    self.break_signaled = false;
                    self.sync_control_signals();
                    let target_idx = self.frames.len() - 1;
                    self.frames[target_idx].pending_yield = false;
                    let value = self.stack.pop().unwrap_or(Value::Nil);
                    self.begin_method_break(value, target_idx)?;
                    if self.frames.len() <= until_depth { return Ok(()); }
                    continue;
                }
            }
            // P1c.2 (ADR 0023): Fiber.yield(v) sets this slot
            // and we exit so `resume_fiber` can observe the
            // suspension. Same shape as method_return — the
            // driver above us reads the flag, stashes value
            // + state, then returns control to whoever called
            // `resume_fiber`. Without this check the bytecode
            // would just continue past the yield point as if
            // nothing happened, defeating cooperative
            // suspension. NOT part of the folded control_signals
            // mask — it must run on every iteration regardless.
            #[cfg(feature = "_fiber")]
            if self.fiber_yield_pending.is_some() { return Ok(()); }
            // ADR 0025 Phase 2 + Phase 4b: SIGINT safe-point check.
            // The POSIX handler (signal-hook, registered in Phase 1)
            // sets `interrupt_pending`; the safe point reads it
            // here and dispatches based on `signal_traps[SIGINT]`:
            //
            // - Default: translate to a Ruby `Interrupt` raise
            //   via `unwind_with_exception` — the canonical
            //   pre-Phase-4 behavior.
            // - Ignore:  clear the flag, continue normally.
            // - Block:   invoke the trap block at the safe
            //   point via re-entrant `invoke_block` +
            //   `dispatch_until`. The `suppress_interrupt`
            //   counter increments around the trap so a
            //   second signal can't recursively fire while
            //   the user's handler is running.
            //
            // Honored only when:
            // - `suppress_interrupt == 0`: must-complete
            //   cleanup windows defer delivery.
            // - `cext_depth == 0` (when `_fiber` is on):
            //   deferred during C-ext frames; mirrors the
            //   existing `Fiber.yield`-in-cext guard.
            //
            // **TODO (Phase 2 follow-up)**: production cext
            // entry/exit paths don't yet increment
            // `cext_depth` (only the Fiber test scaffolding
            // does). Until those land, interrupt-during-cext
            // can still fire mid-cext; documented as a known
            // gap until the cext bridge wires the counter.
            //
            // Memory ordering: Relaxed load is sufficient for
            // a single flag with no paired data — handler uses
            // SeqCst store; reader uses Relaxed. Round-3 review
            // confirmed: composition is sound today *only*
            // because the AtomicBool carries no paired data.
            //
            // **Tripwire**: if a future change adds Vm state
            // that the handler / signal context MUST observe
            // alongside the flag (e.g. a `signal_traps[SIGINT]`
            // ObjId discriminant published from a separate
            // thread), update BOTH sides at once:
            //
            //   1. Upgrade THIS load → Acquire.
            //   2. Upgrade the handler-side store → Release
            //      (replace `signal_hook::flag::register`,
            //      which uses Relaxed, with `register_usize`
            //      or hand-rolled `sigaction` that pairs the
            //      store with a Release fence on the paired
            //      state).
            //   3. Mirror the upgrade at the `kernel.rs` sleep
            //      poller's `flag.load(Relaxed)` site —
            //      otherwise sleep observes the stale paired
            //      state mid-call.
            //
            // The contract is unchanged from Phase 2; this
            // round-3 expansion just makes the upgrade
            // checklist explicit so a future implementer
            // doesn't miss site #3.
            // Same inline pending-flag hoist as the `dispatch` loop — skip
            // the per-op `safe_point_interrupt_action` call when no
            // interrupt is pending. SIGINT latency unchanged.
            #[cfg(unix)]
            if self.interrupt_pending.load(std::sync::atomic::Ordering::Relaxed)
                && let Some(action) = self.safe_point_interrupt_action()
            {
                action.deliver(self)?;
                // Unwind / trap-block dispatch handled by
                // `action.deliver`. Loop back to the top.
                continue;
            }
            // Single frames.last_mut() for the whole fetch — the old
            // shape did a second bounds-check + deref chain just to
            // bump ip (visible per-op in the tight-loop profile).
            let (proto_idx, ip) = {
                let f = self.frames.last_mut().expect("ICE: dispatch_until no frame");
                let pair = (f.proto_idx, f.ip);
                f.ip += 1;
                pair
            };
            let op = self.protos[proto_idx].code[ip];
            match self.step(op, proto_idx) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(trap) => {
                    // `AlreadyCaught` means `unwind_with_exception`
                    // already redirected IP to a rescue handler and
                    // popped frames down to it (the handler is now the
                    // top frame). Whoever's dispatch scope OWNS that
                    // handler frame must consume the signal and resume;
                    // everyone below must bubble it out so the iter
                    // driver(s) between the raise site and the handler
                    // abort cleanly.
                    //
                    // The owner test is "is the handler frame within MY
                    // scope?" — i.e. `frames.len() > until_depth` (the
                    // loop condition). If so, `continue` and the next op
                    // fetch lands on the redirected handler IP. If the
                    // handler sits below us, re-emit.
                    //
                    // Pre-fix this arm re-emitted unconditionally,
                    // assuming the outermost *main-loop* `dispatch`
                    // (step.rs `dispatch` fn) would consume it. That
                    // holds for top-level scripts, but NOT when the
                    // outermost executor is itself a `dispatch_until`
                    // (e.g. a Rack app invoked from Rust via
                    // `call_ruby_block_sync` → `step_block`): there the
                    // signal escaped to the Rust caller and surfaced as
                    // an uncaught "Rack app raised" error even though a
                    // `rescue` was in scope. Mirrors the non-local
                    // `return` fix in this same function.
                    if matches!(trap.err, RubyError::AlreadyCaught) {
                        if self.frames.len() > until_depth {
                            continue;
                        }
                        return Err(trap);
                    }
                    // Same convert-to-rescue dance as `dispatch`.
                    // Without this, a primitive error inside a
                    // block (`arr.each { nil.foo }`) would
                    // bypass every rescue handler all the way
                    // up the call chain.
                    if let Some(exc) = self.trap_to_exception(&trap) {
                        let original_bt = trap.backtrace.clone();
                        let original_class = trap.err.ruby_class_name();
                        let original_msg = trap.err.message();
                        match self.unwind_with_exception(exc) {
                            Ok(()) => {
                                // Unwind found a handler. If that
                                // handler lives at or above our
                                // `until_depth`, then the native
                                // iter driver above us (Array#each,
                                // Hash#any?, …) must stop looping
                                // immediately — otherwise it will
                                // push spurious results / re-raise
                                // and corrupt the rescue's stack
                                // snapshot. Bubble out via
                                // AlreadyCaught so step_block and
                                // every `?` along the way returns
                                // Err; the OUTER dispatch_until
                                // catches it on the line above and
                                // resumes at the redirected handler
                                // IP without double-unwinding.
                                if self.frames.len() <= until_depth {
                                    return Err(self.trap(RubyError::AlreadyCaught));
                                }
                                continue;
                            }
                            Err(_) => return Err(Trap {
                                err: RubyError::Uncaught {
                                    class_name: original_class,
                                    message: original_msg,
                                },
                                backtrace: original_bt,
                            }),
                        }
                    }
                    return Err(trap);
                }
            }
        }
        Ok(())
    }



    /// Raise FrozenError if `self_val` is a frozen object — CRuby's
    /// guard on every instance-variable write. Shared by the
    /// StoreIvar / IncIvar / IncIvarNoPush ops so the three ivar-write
    /// paths stay consistent. A no-op for non-frozen / class /
    /// primitive receivers. rack Builder#freeze_app relies on this:
    /// a frozen handler that sets `@x` during a request must 500.
    pub(crate) fn frozen_ivar_guard(&mut self, self_val: &Value) -> Result<(), Trap> {
        let frozen = match self_val {
            Value::Object(id) => self.heap.instance(*id).frozen.get(),
            Value::Hash(id) => self.heap.hash_frozen(*id),
            Value::Array(id) => self.heap.array_frozen(*id),
            _ => false,
        };
        if !frozen {
            return Ok(());
        }
        let cls_name = match self.class_of(self_val) {
            Value::Class(c) => c.name.clone(),
            _ => "Object".to_string(),
        };
        let shown = self.inspect_value(self_val)?;
        Err(self.trap(crate::error::RubyError::FrozenError {
            msg: format!("can't modify frozen {}: {}", cls_name, shown),
        }))
    }

    /// Execute one op; returns Ok(false) if we just popped the last frame.
    /// `_proto_idx` is reserved for future per-op span lookup; with the
    /// global interner, ops no longer need it for string resolution.
    pub(crate) fn step(&mut self, op: Op, proto_idx: usize) -> Result<bool, Trap> {
        self.check_fuel()?;
        match op {
            Op::LoadConstInt(i) => self.stack.push(Value::Int(i)),
            Op::LoadConstFloat(f) => self.stack.push(Value::Float(f)),
            Op::LoadConstStr(id) => {
                let s = self.interner.resolve(id).clone();
                let v = Value::new_str(s.to_string());
                // Source-encoding re-tag: when the eval'd source wasn't
                // UTF-8 (a template engine eval'ing a US-ASCII /
                // Shift_JIS template), its string literals carry the
                // source's encoding.
                if let Some(enc) = self.protos[proto_idx].source_encoding
                    && let Value::Str(rs) = &v
                {
                    self.retag_literal_to_source_encoding(rs, enc);
                }
                // `# frozen_string_literal: true`: plain literals push
                // frozen. (Interpolated strings don't reach this op, so
                // they stay mutable — CRuby semantics.)
                if self.protos[proto_idx].frozen_string_literal
                    && let Value::Str(rs) = &v
                {
                    rs.frozen.set(true);
                }
                self.stack.push(v);
            }
            Op::LoadConstStrBytes(idx) => {
                // Binary-literal pool lives on the current proto
                // (the interner is UTF-8-only). Clone the Rc<[u8]>
                // slot into a fresh Vec<u8> so each load yields an
                // independent String — mutations via `<<` /
                // `concat` shouldn't bleed into the pool entry that
                // future loads share.
                let bytes: Vec<u8> = self.protos[proto_idx].byte_literals[idx as usize].to_vec();
                let v = Value::new_str_bytes(bytes);
                if let Some(enc) = self.protos[proto_idx].source_encoding
                    && let Value::Str(rs) = &v
                {
                    self.retag_literal_to_source_encoding(rs, enc);
                }
                if self.protos[proto_idx].frozen_string_literal
                    && let Value::Str(rs) = &v
                {
                    rs.frozen.set(true);
                }
                self.stack.push(v);
            }
            #[cfg(feature = "bignum")]
            Op::LoadBigInt(id) => {
                use std::str::FromStr;
                let big = if let Some(b) = self.bigint_lit_cache.get(&id) {
                    (**b).clone()
                } else {
                    let src = self.interner.resolve(id).clone();
                    let parsed = num_bigint::BigInt::from_str(&src).map_err(|e| {
                        self.trap(RubyError::SyntaxError {
                            msg: format!("invalid bigint literal {:?}: {}", src, e),
                        })
                    })?;
                    let rc = Rc::new(parsed);
                    self.bigint_lit_cache.insert(id, rc.clone());
                    (*rc).clone()
                };
                let v = self.bigint_to_value(big)?;
                self.stack.push(v);
            }
            Op::LoadRational(num_id, den_id) => {
                #[cfg(feature = "bignum")]
                {
                    use std::str::FromStr;
                    // Reuse `bigint_lit_cache` — Rational literals
                    // share the same parsed-BigInt surface as
                    // `LoadBigInt`. Each load allocates a fresh
                    // heap `RationalRepr` so ObjId identity stays
                    // per-Value; only the parse work is amortised.
                    let mut parse_or_cached = |id: crate::SymId| -> Result<num_bigint::BigInt, Trap> {
                        if let Some(b) = self.bigint_lit_cache.get(&id) {
                            return Ok((**b).clone());
                        }
                        let src = self.interner.resolve(id).clone();
                        let parsed = num_bigint::BigInt::from_str(&src).map_err(|e| {
                            self.trap(RubyError::SyntaxError {
                                msg: format!("invalid rational component {:?}: {}", src, e),
                            })
                        })?;
                        let rc = Rc::new(parsed);
                        self.bigint_lit_cache.insert(id, rc.clone());
                        Ok((*rc).clone())
                    };
                    let num = parse_or_cached(num_id)?;
                    let den = parse_or_cached(den_id)?;
                    let v = self.make_rational_bigint(num, den)?;
                    self.stack.push(v);
                }
                #[cfg(not(feature = "bignum"))]
                {
                    use std::str::FromStr;
                    let parse_i64 = |id: crate::SymId| -> Result<i64, Trap> {
                        // Don't echo the interned component string back —
                        // ast.rs no-bignum lowering substitutes a u128::MAX
                        // sentinel for literals exceeding u128 (rare but
                        // possible), so the stored text may not match the
                        // user's source. Keep the error message generic.
                        let src = self.interner.resolve(id).clone();
                        i64::from_str(&src).map_err(|_| {
                            self.trap(RubyError::RangeError {
                                msg: "Rational literal component exceeds i64 (rebuild with --features bignum)".to_string(),
                            })
                        })
                    };
                    let num = parse_i64(num_id)?;
                    let den = parse_i64(den_id)?;
                    let v = self.make_rational(num, den)?;
                    self.stack.push(v);
                }
            }
            #[cfg(feature = "regex")]
            Op::LoadRegex(id, flags) => {
                let key = (id, flags);
                let regex_rc = if let Some(r) = self.regex_cache.get(&key) {
                    r.clone()
                } else {
                    let src = self.interner.resolve(id).clone();
                    // `\G`-translate first, then prepend the inline
                    // flag group; the engine sees the prefixed
                    // pattern, but the BARE `translated` is stored as
                    // the regexp's `#source`.
                    let translated = preprocess_regex_pattern(&src);
                    let prefixed = apply_ruby_flags(&translated, flags);
                    let compiled = crate::regex_engine::compile_with_flags(&prefixed, flags, &translated).map_err(|e| {
                        self.trap(RubyError::SyntaxError {
                            msg: format!("invalid regex /{}/: {}", src, e),
                        })
                    })?;
                    let rc = Rc::new(compiled);
                    self.regex_cache.insert(key, rc.clone());
                    rc
                };
                self.stack.push(Value::Regex(regex_rc));
            }
            #[cfg(feature = "regex")]
            Op::CompileRegex(flags) => {
                // Top of stack: a `Value::Str` produced by the
                // InterpolatedRegex build sequence. The assembled
                // pattern is interned so cache lookups can dedup
                // repeated identical expansions (same pattern
                // emitted by different call sites collapses to
                // one compiled Regex).
                let pat_val = self.stack.pop().unwrap_or(Value::Nil);
                let s = match &pat_val {
                    Value::Str(s) => s.clone(),
                    other => {
                        // Defensive: the compiler always emits a
                        // string-producing sequence before this op,
                        // but if a host-defined `to_s`/`String#+`
                        // override returns a non-String we'd rather
                        // raise a Ruby-level TypeError than panic
                        // or miscompile.
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "interpolated regex pattern must be a String, got {}",
                                other.type_name()
                            ),
                        }));
                    }
                };
                // `with_str_lossy` is the borrowed fast path — for
                // valid UTF-8 strings (the common case for regex
                // patterns) the closure sees a `&str` backed by the
                // RubyStr's RefCell content without an owning copy.
                // Cache hits never allocate a String; only the cold
                // path (cache miss → intern → compile) needs to
                // materialise an owned String (interner takes one
                // anyway). Error formatting is also rare and reads
                // through the same borrow.
                let regex_rc = s.with_str_lossy::<Result<Rc<crate::regex_engine::CompiledRegex>, Trap>>(|pat| {
                    // ResourceCap: respect `Config::max_symbols` the
                    // same way `String#to_sym` does. Dynamic patterns
                    // generated in a hot loop (e.g.
                    // `1000.times { |i| /#{i}/ }`) would otherwise
                    // grow the interner — and the SymId-keyed
                    // `regex_cache` — without bound. Skip the check
                    // when the pattern is already interned; a cache
                    // hit costs no new symbol.
                    if let Some(max) = self.max_symbols
                        && !self.interner.contains(pat) && self.interner.len() >= max {
                            return Err(self.trap(RubyError::ResourceExhausted {
                                msg: format!("interner exhausted: {} symbols", max),
                            }));
                        }
                    let id = self.interner.intern(pat);
                    let key = (id, flags);
                    if let Some(r) = self.regex_cache.get(&key) {
                        return Ok(r.clone());
                    }
                    let translated = preprocess_regex_pattern(pat);
                    let prefixed = apply_ruby_flags(&translated, flags);
                    let compiled = crate::regex_engine::compile_with_flags(&prefixed, flags, &translated).map_err(|e| {
                        self.trap(RubyError::SyntaxError {
                            msg: format!("invalid regex /{}/: {}", pat, e),
                        })
                    })?;
                    let rc = Rc::new(compiled);
                    self.regex_cache.insert(key, rc.clone());
                    Ok(rc)
                })?;
                self.stack.push(Value::Regex(regex_rc));
            }
            Op::LoadSymbol(id) => {
                self.stack.push(Value::Sym(id));
            }
            Op::LoadNil => self.stack.push(Value::Nil),
            Op::LoadTrue => self.stack.push(Value::Bool(true)),
            Op::LoadFalse => self.stack.push(Value::Bool(false)),
            Op::LoadSelf => {
                let v = self.frames.last().expect("ICE: LoadSelf no frame").self_val.clone();
                self.stack.push(v);
            }
            Op::LoadLocal(s) => {
                let f = self.frames.last().expect("ICE: LoadLocal no frame");
                let v = match &f.locals {
                    // Arena slots: direct index, no Rc deref / borrow flag.
                    crate::vm::Locals::Stack(base) => {
                        self.locals_arena[*base as usize + s as usize].clone()
                    }
                    crate::vm::Locals::Shared(rc) => rc.borrow()[s as usize].clone(),
                };
                self.stack.push(v);
            }
            Op::StoreLocal(s) => {
                let v = self.stack.pop().expect("ICE: StoreLocal stack underflow");
                let slot = s as usize;
                let frame = self.frames.last().expect("ICE: StoreLocal no frame");
                match &frame.locals {
                    // Stack frames are method frames by construction —
                    // never a block, so no writeback propagation.
                    crate::vm::Locals::Stack(base) => {
                        let idx = *base as usize + slot;
                        self.locals_arena[idx] = v;
                    }
                    crate::vm::Locals::Shared(rc) => {
                        rc.borrow_mut()[slot] = v.clone();
                        // Per-invocation block-locals model: outer-scope
                        // writes (slot < block.param_start) propagate
                        // through every enclosing fresh-Vec back to the
                        // lexical method's locals. `propagate_outer_write`
                        // walks the writeback chain. Without this,
                        // `counter = 0; arr.each { counter += 1 }`
                        // would update only the block frame's fresh Vec
                        // and the method would still see 0 after the
                        // loop. The propagation is a no-op when frame
                        // has no `block_writeback` (method / class-body
                        // / toplevel frames), or when the slot sits in
                        // the current block's own param/body range.
                        let in_outer_scope = frame
                            .block_writeback
                            .as_ref()
                            .is_some_and(|(_, ps)| slot < *ps as usize);
                        if in_outer_scope {
                            self.propagate_outer_write(slot, &v);
                        }
                    }
                }
            }
            Op::IncLocalNoPush(s) => {
                let slot = s as usize;
                let frame = self.frames.last().expect("ICE: IncLocalNoPush no frame");
                let slow_cur = match &frame.locals {
                    crate::vm::Locals::Stack(base) => {
                        let idx = *base as usize + slot;
                        match &mut self.locals_arena[idx] {
                            Value::Int(n) => {
                                *n = (*n).wrapping_add(1);
                                None
                            }
                            cur => Some(cur.clone()),
                        }
                    }
                    crate::vm::Locals::Shared(rc) => {
                        let mut locals = rc.borrow_mut();
                        match &mut locals[slot] {
                            Value::Int(n) => {
                                *n = (*n).wrapping_add(1);
                                None
                            }
                            cur => Some(cur.clone()),
                        }
                    }
                };
                if let Some(cur) = slow_cur {
                    // Slow path: rebind via `+`. push, dispatch, store, drop result.
                    self.stack.push(cur);
                    self.stack.push(Value::Int(1));
                    let plus_id = self.interner.intern("+");
                    self.do_call(plus_id, 1, false, u16::MAX)?;
                    let v = self.stack.pop().unwrap_or(Value::Nil);
                    self.set_local_top(slot, v);
                }
                // Per-invocation block-locals propagation — see
                // Op::StoreLocal. (Stack frames are never blocks —
                // block_writeback is None by construction, so the
                // get_local_top read only ever fires on Shared.)
                let frame = self.frames.last().expect("ICE: IncLocalNoPush no frame");
                let in_outer = frame
                    .block_writeback
                    .as_ref()
                    .is_some_and(|(_, ps)| slot < *ps as usize);
                if in_outer {
                    let v = self.get_local_top(slot);
                    self.propagate_outer_write(slot, &v);
                }
            }
            Op::IncLocal(s) => {
                let slot = s as usize;
                let frame = self.frames.last().expect("ICE: IncLocal no frame");
                let fast_new_n = match &frame.locals {
                    crate::vm::Locals::Stack(base) => {
                        let idx = *base as usize + slot;
                        match &mut self.locals_arena[idx] {
                            Value::Int(n) => {
                                let new_n = (*n).wrapping_add(1);
                                *n = new_n;
                                Some(new_n)
                            }
                            _ => None,
                        }
                    }
                    crate::vm::Locals::Shared(rc) => {
                        let mut locals = rc.borrow_mut();
                        match &mut locals[slot] {
                            Value::Int(n) => {
                                let new_n = (*n).wrapping_add(1);
                                *n = new_n;
                                Some(new_n)
                            }
                            _ => None,
                        }
                    }
                };
                if let Some(new_n) = fast_new_n {
                    self.stack.push(Value::Int(new_n));
                } else {
                    let cur = self.get_local_top(slot);
                    // Slow path: replicate `slot = slot + 1` via BinOp semantics,
                    // including user-defined `+` on the receiver type.
                    self.stack.push(cur);
                    self.stack.push(Value::Int(1));
                    let plus_id = self.interner.intern("+");
                    self.do_call(plus_id, 1, false, u16::MAX)?;
                    let new_val = self.stack.last().expect("ICE: IncLocal slow path no result").clone();
                    self.set_local_top(slot, new_val);
                }
                // Per-invocation block-locals propagation — see
                // Op::StoreLocal.
                let frame = self.frames.last().expect("ICE: IncLocal no frame");
                let in_outer = frame
                    .block_writeback
                    .as_ref()
                    .is_some_and(|(_, ps)| slot < *ps as usize);
                if in_outer {
                    let v = self.get_local_top(slot);
                    self.propagate_outer_write(slot, &v);
                }
            }
            Op::Dup => {
                let v = self.stack.last().expect("ICE: Dup stack underflow").clone();
                self.stack.push(v);
            }
            Op::Pop => { self.stack.pop(); }
            Op::Swap => {
                let n = self.stack.len();
                self.stack.swap(n - 1, n - 2);
            }
            Op::MassignSplat => {
                let v = self.stack.pop().unwrap_or(Value::Nil);
                let coerced = self.massign_coerce_to_array(v)?;
                self.stack.push(coerced);
            }
            Op::LoadIvar(name_id) => {
                // `@foo` reads route to whichever table `self`
                // carries: instance ivars for `Value::Object`,
                // class-level ivars for `Value::Class` (the
                // "class instance variable" CRuby spelling, used
                // by `module Tilt; @default = ...` patterns).
                // Anything else returns nil — matches CRuby's
                // "uninitialized ivar reads as nil" rule.
                let self_val = self.frames.last().expect("ICE: LoadIvar no frame").self_val.clone();
                let v = match &self_val {
                    Value::Object(id) => self.heap.instance(*id).ivars.get(&name_id).cloned().unwrap_or(Value::Nil),
                    Value::Class(c) => c.ivars.borrow().get(&name_id).cloned().unwrap_or(Value::Nil),
                    // Hash-subclass instances carry their own ivar table.
                    Value::Hash(id) => self.heap.hash_ivar_get(*id, name_id).unwrap_or(Value::Nil),
                    // Array-subclass instances likewise.
                    Value::Array(id) => self.heap.array_ivar_get(*id, name_id).unwrap_or(Value::Nil),
                    // String-subclass instances keep ivars in the
                    // `str_ivars` side table (keyed by Rc identity) — the
                    // same table `instance_variable_get/set` use.
                    Value::Str(s) => {
                        let key = std::rc::Rc::as_ptr(s) as usize;
                        self.str_ivars
                            .get(&key)
                            .and_then(|(_, m)| m.get(&name_id).cloned())
                            .unwrap_or(Value::Nil)
                    }
                    _ => Value::Nil,
                };
                self.stack.push(v);
            }
            Op::StoreIvar(name_id) => {
                let v = self.stack.pop().expect("ICE: StoreIvar stack underflow");
                let self_val = self.frames.last().expect("ICE: StoreIvar no frame").self_val.clone();
                self.frozen_ivar_guard(&self_val)?;
                match &self_val {
                    Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, v); }
                    Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, v); }
                    // Hash-subclass instances carry their own ivar table.
                    Value::Hash(id) => { self.heap.hash_ivar_set(*id, name_id, v); }
                    // Array-subclass instances likewise.
                    Value::Array(id) => { self.heap.array_ivar_set(*id, name_id, v); }
                    // String-subclass instances → `str_ivars` side table
                    // (the frozen guard above already handled a frozen
                    // receiver). Keyed by Rc identity; the strong Rc keeps
                    // the string alive (same leak tradeoff as str_singletons).
                    Value::Str(s) => {
                        let key = std::rc::Rc::as_ptr(s) as usize;
                        let keep = s.clone();
                        self.str_ivars
                            .entry(key)
                            .or_insert_with(|| (keep, crate::intern::FxHashMap::default()))
                            .1
                            .insert(name_id, v);
                        self.any_str_ivars = true;
                    }
                    _ => { /* drop — CRuby raises but the toplevel/primitive cases are rare */ }
                }
            }
            Op::LoadCvar(name_id) => {
                // Surrounding class resolution order:
                //   - class body / `def self.foo`: self_val IS the
                //     class → use it directly.
                //   - instance method: self_val is an Object →
                //     `heap.real_class_of` gives the class.
                //   - toplevel / block-in-toplevel: no class on
                //     hand → fall back to Vm.toplevel_cvars.
                // CRuby class variables are shared across the class
                // hierarchy: read resolves to the nearest ancestor
                // that defines `@@name` (so a subclass sees a parent's
                // `@@x`). Falls back to nil if no ancestor has it.
                let cls_opt = self.surrounding_class();
                let v = match cls_opt {
                    Some(cls) => match self.cvar_owner_class(&cls, name_id) {
                        Some(owner) => owner.class_vars.borrow().get(&name_id).cloned().unwrap_or(Value::Nil),
                        None => Value::Nil,
                    },
                    None => self.toplevel_cvars.get(&name_id).cloned().unwrap_or(Value::Nil),
                };
                self.stack.push(v);
            }
            Op::StoreCvar(name_id) => {
                let v = self.stack.pop().expect("ICE: StoreCvar stack underflow");
                let cls_opt = self.surrounding_class();
                match cls_opt {
                    // Write to the ancestor that already owns `@@name`
                    // (shared hierarchy semantics); if none does, the
                    // variable is created on the current class.
                    Some(cls) => {
                        let owner = self.cvar_owner_class(&cls, name_id).unwrap_or(cls);
                        owner.class_vars.borrow_mut().insert(name_id, v);
                    }
                    None => { self.toplevel_cvars.insert(name_id, v); }
                }
            }
            Op::IncIvarNoPush(name_id) => {
                // `@x = @x + 1` fast path, statement form. Mirrors
                // Op::IncIvar but discards the result. Class-level
                // ivars routed via `Value::Class` so the same
                // pattern in a class method bumps the right table.
                let self_val = self.frames.last().expect("ICE: IncIvarNoPush no frame").self_val.clone();
                self.frozen_ivar_guard(&self_val)?;
                let cur = match &self_val {
                    Value::Object(id) => self.heap.instance(*id).ivars.get(&name_id).cloned(),
                    Value::Class(c) => c.ivars.borrow().get(&name_id).cloned(),
                    _ => None,
                };
                let new_v = match cur {
                    Some(Value::Int(n)) => Some(Value::Int(n.wrapping_add(1))),
                    Some(_) | None => {
                        // Slow path — call `+`.
                        let cur_v = cur.unwrap_or(Value::Nil);
                        self.stack.push(cur_v);
                        self.stack.push(Value::Int(1));
                        let plus_id = self.interner.intern("+");
                        self.do_call(plus_id, 1, false, u16::MAX)?;
                        Some(self.stack.pop().unwrap_or(Value::Nil))
                    }
                };
                if let Some(v) = new_v {
                    match &self_val {
                        Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, v); }
                        Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, v); }
                        _ => { /* drop */ }
                    }
                }
            }
            Op::IncIvar(name_id) => {
                // `@x = @x + 1` fast path, expression form. Same as
                // IncIvarNoPush but leaves the new value on stack.
                let self_val = self.frames.last().expect("ICE: IncIvar no frame").self_val.clone();
                self.frozen_ivar_guard(&self_val)?;
                let cur = match &self_val {
                    Value::Object(id) => self.heap.instance(*id).ivars.get(&name_id).cloned(),
                    Value::Class(c) => c.ivars.borrow().get(&name_id).cloned(),
                    _ => None,
                };
                let new_v: Value = match cur {
                    Some(Value::Int(n)) => {
                        let nv = Value::Int(n.wrapping_add(1));
                        match &self_val {
                            Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, nv.clone()); }
                            Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, nv.clone()); }
                            _ => {}
                        }
                        nv
                    }
                    _ => {
                        // Slow path: replicate full `@x = @x + 1`.
                        let cur_v = cur.unwrap_or(Value::Nil);
                        self.stack.push(cur_v);
                        self.stack.push(Value::Int(1));
                        let plus_id = self.interner.intern("+");
                        self.do_call(plus_id, 1, false, u16::MAX)?;
                        let v = self.stack.last().expect("ICE: IncIvar slow path no result").clone();
                        match &self_val {
                            Value::Object(id) => { self.heap.instance_mut(*id).ivars.insert(name_id, v.clone()); }
                            Value::Class(c) => { c.ivars.borrow_mut().insert(name_id, v.clone()); }
                            _ => {}
                        }
                        // Slow path already left value on stack via do_call result.
                        return Ok(true);
                    }
                };
                self.stack.push(new_v);
            }
            Op::StoreConst(name_id) => {
                // `FOO = expr` — compiler emitted Dup before this so
                // the assigned value also remains on the stack as
                // the expression's result (CRuby semantics).
                let v = self.stack.pop().expect("ICE: StoreConst stack underflow");
                // Whether this is the FIRST definition of this key — gates the
                // Module#const_added hook below (CRuby fires it only once).
                let store_was_fresh = !self.constants.contains_key(&name_id)
                    && !self.classes.contains_key(&name_id);
                // CRuby names an anonymous class/module on its first
                // const-assignment (`C = Class.new` ⇒ `C.name ==
                // "C"`). `name_id` is the constant's key — bare at
                // toplevel, qualified (`Foo::Bar`) when written
                // inside a class/module body — which is exactly the
                // name CRuby would stamp. `name_anon_class` is a
                // no-op for an already-named class (alias rebinds
                // don't rename) and recursively re-homes the anon
                // class's nested `const_set` tree into the global
                // qualified maps so `C::X` / `C::Inner::Leaf` reads
                // resolve.
                if let Value::Class(cls) = &v {
                    let bare = self.interner.resolve(name_id).to_string();
                    // CRuby names an anon class with its FULL qualified
                    // path: `module Faraday; X = Struct.new` ⇒
                    // `X.name == "Faraday::X"`, and registers it under
                    // that scoped key — NOT the bare name. Qualify with
                    // the enclosing scope (class_stack top) so:
                    //   (a) a later reopen (`class Faraday::X` /
                    //       `module Faraday; class X`), whose DefClass
                    //       keys by the qualified name, finds and REOPENS
                    //       this same class instead of minting a fresh
                    //       empty one — faraday's `Request = Struct.new(…)
                    //       { extend MiddlewareRegistry }` then
                    //       `module Faraday; class Request` (authorization.rb)
                    //       was dropping the struct members + the extend;
                    //   (b) the bare name doesn't leak to the top level.
                    // Skip when name_id is already qualified (compiler-
                    // emitted `Foo::Bar = …`) or at top level (no scope).
                    let qualified = match self.class_stack.last() {
                        Some(scope) if !bare.contains("::") => {
                            match scope.effective_name() {
                                Some(sn) if !sn.is_empty() => format!("{}::{}", sn, bare),
                                _ => bare.clone(),
                            }
                        }
                        _ => bare.clone(),
                    };
                    self.name_anon_class(cls, &qualified);
                }
                // Const assigned directly inside an eigenclass body
                // (`class << self; FOO = …`): CRuby scopes it under the
                // eigenclass. Mirror the nested module/class arm in
                // Op::DefClass — also register on the eigenclass's own
                // const table so `self::FOO` / `const_get(:FOO, false)` /
                // `const_defined?(:FOO, false)` resolve, while the global
                // `self.constants` write (below) keeps bare reads in the
                // body working. Additive, gated on an eigenclass scope.
                if let Some(scope) = self.class_stack.last()
                    && scope.singleton_target.borrow().is_some()
                {
                    let short = self.interner.resolve(name_id);
                    let short = short.rsplit("::").next().unwrap_or(&short).to_string();
                    let short_id = self.interner.intern(&short);
                    scope.consts.borrow_mut().insert(short_id, v.clone());
                }
                self.constants.insert(name_id, v);
                // Stamp the assignment location (first write wins, so a
                // `class Foo; end`'s DefClass location is preserved over
                // the nested-scope StoreConst the compiler also emits).
                if !self.const_source_locations.contains_key(&name_id)
                    && let Some(loc) = self.current_op_location()
                {
                    self.const_source_locations.insert(name_id, loc);
                }
                self.bump_const_gen();
                // The constant is now defined — drop any consumed-autoload
                // marker for this key (it's no longer an undef slot).
                #[cfg(not(target_os = "wasi"))]
                self.consumed_autoloads.remove(&name_id);
                // `Module#const_added` (CRuby 3.2+) ALSO fires on constant
                // ASSIGNMENT (`C = Class.new`, `M::X = v`), not just class/
                // module defs — zeitwerk's nsfile namespaces are defined as
                // `Widget = Class.new`, and its prepended const_added sets up
                // the namespace's child autoloads here. Owner/cname derivation
                // mirrors the DefClass fire: qualified name → resolved parent +
                // short cname; bare → lexical scope (or Object). Gated on
                // first-definition + const_added being interned.
                if store_was_fresh && self.interner.contains("const_added") {
                    let full = self.interner.resolve(name_id).to_string();
                    // A bare const written INSIDE a class/module body is
                    // compiled as TWO stores — the bare name AND a qualified
                    // `Scope::name` twin (ConstWrite). Both reach here; fire
                    // only on the qualified twin so const_added fires ONCE
                    // (CRuby parity). Top-level bare assignments (no scope) and
                    // explicit `Scope::X =` keep firing.
                    let is_bare_in_body = !full.contains("::") && !self.class_stack.is_empty();
                    if !is_bare_in_body {
                        let (owner, cname) = match full.rfind("::") {
                            Some(pos) => {
                                let parent_id = self.interner.intern(&full[..pos]);
                                let cname_id = self.interner.intern(&full[pos + 2..]);
                                (self.classes.get(&parent_id).cloned(), cname_id)
                            }
                            None => {
                                let o = self.class_stack.last().cloned().or_else(|| {
                                    let obj = self.interner.intern("Object");
                                    self.classes.get(&obj).cloned()
                                });
                                (o, name_id)
                            }
                        };
                        if let Some(owner) = owner {
                            self.fire_const_added(&owner, cname)?;
                        }
                    }
                }
            }
            Op::LoadConst(name_id) => {
                // Explicit `Scope::Const` access to a `private_constant` always
                // raises (CRuby), even from inside the owning module — bare /
                // lexical reads (LoadConstChain) and `const_get` are unaffected.
                // The flat LoadConst key is the qualified `"Scope::Const"`, which
                // is exactly what `record_const_visibility` stored.
                if !self.private_consts.is_empty() && self.private_consts.contains(&name_id) {
                    let full = self.interner.resolve(name_id).to_string();
                    return Err(self.trap(crate::error::RubyError::NameError {
                        msg: format!("private constant {} referenced", full),
                    }));
                }
                // Inline constant cache — resolution below depends only
                // on the global tables, so a per-SymId entry tagged with
                // `const_gen` short-circuits the whole walk. See the
                // field doc on Vm for the invalidation contract.
                if let Some((v, g)) = self.const_cache_flat.get(&name_id)
                    && *g == self.const_gen
                {
                    let v = v.clone();
                    self.stack.push(v);
                    return Ok(true);
                }
                let v = if let Some(c) = self.classes.get(&name_id).cloned() {
                    Value::Class(c)
                } else if let Some(v) = self.constants.get(&name_id).cloned() {
                    v
                } else if self.interner.resolve(name_id).as_ref() == "ENV" {
                    // ADR 0017 Rule 1+2: the ENV map a script sees is
                    // exactly the one the host provided via
                    // `Config::env`. See `env_hash_or_init` for the
                    // lazy-build details. Shared with the chain-walk
                    // fallback in Op::LoadConstChain so a bare `ENV`
                    // inside a nested class body resolves the same
                    // way (PR #234 / pass-9.7c layer #20).
                    Value::Hash(self.env_hash_or_init()?)
                } else {
                    // Phase 1 of issue #224 — autoload trigger.
                    // Before raising the "uninitialized constant"
                    // NameError, check if `name_id` is registered as
                    // a pending toplevel autoload. If so:
                    //   1. Pop the entry FIRST (CRuby semantics —
                    //      prevents re-entry into the same autoload
                    //      while the require is mid-flight; also
                    //      means a require that fails to define the
                    //      constant gets a real NameError on retry
                    //      rather than an infinite require loop).
                    //   2. Call `require` via builtin_call so we go
                    //      through the same path resolution + scope
                    //      gate + LoaderError handling that
                    //      user-level `require` uses. LoadError
                    //      naturally propagates as a Trap.
                    //   3. Re-attempt the classes + constants lookup
                    //      AFTER require completes. If the loaded
                    //      file defined the constant, we resolve and
                    //      push; otherwise fall through to the
                    //      original NameError.
                    //
                    // Wasi-gated: the registry doesn't exist on
                    // wasm32-wasi (no require), so this whole block
                    // compiles out and the original NameError path
                    // is taken.
                    // Try a pending autoload before raising. The
                    // helper walks the exact name then each shorter
                    // `::`-prefix (longest first) across the toplevel
                    // registry (`autoload :Foo` at top level, bare
                    // keys) and the scoped registry (Phase 2 of issue
                    // #224, qualified `Foo::Bar` keys). A qualified
                    // reference AT toplevel compiles to a flat
                    // `LoadConst` keyed by the full name, and a deep
                    // `M5::Inner::THE` whose autoload sits on the
                    // intermediate `M5::Inner` resolves via the prefix
                    // walk. After a require runs, re-check the full
                    // key.
                    #[cfg(not(target_os = "wasi"))]
                    {
                        let name_str = self.interner.resolve(name_id).to_string();
                        if self.fire_pending_autoload(&name_str)? {
                            if let Some(c) = self.classes.get(&name_id).cloned() {
                                self.stack.push(Value::Class(c));
                                return Ok(true);
                            }
                            if let Some(v) = self.constants.get(&name_id).cloned() {
                                self.stack.push(v);
                                return Ok(true);
                            }
                            // require ran but didn't define `name_id`
                            // — fall through to the NameError below.
                        }
                    }
                    // Qualified `Scope::CONST` (`C::FOO`,
                    // `A::B::FOO`, `C::Str::Double`) whose direct
                    // global key missed: resolve the LEADING segment
                    // to its class, then walk the rest of the path
                    // through `resolve_const_path`, which searches the
                    // full ancestor chain (includes / prepends /
                    // superclasses) segment-by-segment. This is what
                    // makes `C::FOO` resolve through an INCLUDED
                    // module (`class C; include M; end` then `C::FOO`
                    // → `M::FOO`), and a chained `C::Str::Double`
                    // resolve `C::Str` to `M::Str` (via include) then
                    // `Double` inside it — CRuby's `rb_const_get`
                    // searches the full ancestry. Splitting on the
                    // FIRST `::` (not the last) is what lets the
                    // intermediate segment go through ancestor
                    // resolution too.
                    {
                        let name_str = self.interner.resolve(name_id).to_string();
                        if let Some((head, rest)) = name_str.split_once("::")
                            && !head.is_empty()
                            && self.interner.contains(head)
                        {
                            let head_id = self.interner.intern(head);
                            if let Some(head_cls) = self.classes.get(&head_id).cloned() {
                                match self.resolve_const_path(&head_cls, rest, true, false) {
                                    crate::vm::dispatch::ConstPathOutcome::Found(v) => {
                                        self.stack.push(v);
                                        return Ok(true);
                                    }
                                    // A scoped-autoload `require` trapped —
                                    // re-raise. The variant is wasi-gated
                                    // (the trigger is), so on wasi the `_`
                                    // arm below covers the remaining cases.
                                    #[cfg(not(target_os = "wasi"))]
                                    crate::vm::dispatch::ConstPathOutcome::Trap(t) => return Err(t),
                                    // Miss / WrongName / NotClass: fall
                                    // through to the NameError below so
                                    // the message keeps the original
                                    // full-path shape.
                                    _ => {}
                                }
                            }
                        }
                    }
                    // `const_missing` hook (CRuby): before raising, give
                    // the owning class/module a chance to materialise the
                    // constant. For a qualified `Scope::CONST`, the
                    // receiver is `Scope` and the name is the final
                    // segment (`Scope.const_missing(:CONST)`); for a bare
                    // toplevel name it's `Object.const_missing(:NAME)`.
                    // try_const_missing pushes the hook's result and
                    // returns true when it fired (result is dynamic, so we
                    // must NOT cache it).
                    {
                        let name_str = self.interner.resolve(name_id).to_string();
                        let (recv_cls, missing) = match name_str.rsplit_once("::") {
                            Some((head, last)) if !head.is_empty() && self.interner.contains(head) => {
                                let head_id = self.interner.intern(head);
                                (self.classes.get(&head_id).cloned(), last.to_string())
                            }
                            Some(_) => (None, name_str.clone()),
                            None => {
                                let obj_id = self.interner.intern("Object");
                                (self.classes.get(&obj_id).cloned(), name_str.clone())
                            }
                        };
                        if let Some(cls) = recv_cls
                            && self.try_const_missing(&cls, &missing)?
                        {
                            return Ok(true);
                        }
                    }
                    // CRuby raises `NameError: uninitialized constant
                    // <name>` for missing constants — silent-nil here
                    // masks real user errors AND lets downstream code
                    // see a Nil where a class/module was expected
                    // (e.g. `nil.new` instead of NameError, which is
                    // confusing to debug). Match CRuby.
                    //
                    // Op-write read positions (`FOO ||= ...`) need
                    // silent-nil — they use `Op::LoadConstOrNil`
                    // instead.
                    let name = self.interner.resolve(name_id).clone();
                    return Err(self.trap(crate::error::RubyError::NameError {
                        msg: format!("uninitialized constant {}", name),
                    }));
                };
                // Fill the IC on the successful main path (classes /
                // constants / ENV hits). Autoload + qualified-path
                // successes return early above and are one-shot — their
                // NEXT read lands in the fast tables and caches here.
                self.const_cache_flat.insert(name_id, (v.clone(), self.const_gen));
                self.stack.push(v);
            }
            Op::LoadConstOrNil(name_id) => {
                // Silent-nil variant of `LoadConst`. See the op's
                // doc comment in bytecode.rs — only the AST `||=`
                // read position emits this. No ENV intercept:
                // `ENV ||= ...` is not idiomatic, and any sane
                // ENV access goes through `LoadConst` where the
                // intercept lives.
                let v = if let Some(c) = self.classes.get(&name_id).cloned() {
                    Value::Class(c)
                } else if let Some(v) = self.constants.get(&name_id).cloned() {
                    v
                } else {
                    Value::Nil
                };
                self.stack.push(v);
            }
            Op::LoadConstChain(chain_idx) => {
                // Cref-walking constant read. The compiler builds the
                // chain at emit time from the lexical class_path
                // (innermost scope first); the runtime walks it in
                // order, taking the first hit in `classes` or
                // `constants`. Used for bare-name reads inside a
                // non-empty class/module scope so `Bar` inside
                // `module Foo` resolves to `Foo::Bar` before falling
                // through to the top-level `Bar`.
                let proto_idx = self.frames.last().expect("ICE: LoadConstChain no frame").proto_idx;
                // Inline constant cache — the chain (and thus the
                // resolution) is static per (proto, chain slot), so the
                // pair keys an entry tagged with `const_gen`. Skips the
                // chain clone, the three-phase walk, and the alloc-heavy
                // `const_via_ancestors` on the steady state.
                let cache_key = (proto_idx as u32, chain_idx);
                if let Some((v, g)) = self.const_cache_chain.get(&cache_key)
                    && *g == self.const_gen
                {
                    let v = v.clone();
                    self.stack.push(v);
                    return Ok(true);
                }
                let chain = self.protos[proto_idx].const_chains[chain_idx as usize].clone();
                // CRuby bare-constant resolution has three ordered
                // phases (see `rb_const_search`):
                //   1. Lexical nesting (`Module.nesting`, innermost
                //      first) — each scope's OWN const table.
                //   2. Ancestors of the INNERMOST lexical cref —
                //      `flatten_ancestors`, each scope's own table.
                //      This is the include/prepend/superclass path.
                //   3. Toplevel (Object).
                // The compiled `chain` is `[scope_inner::bare, …,
                // scope_outer::bare, bare]`: the qualified entries are
                // the `Module.nesting` scopes (phase 1) and the LAST,
                // unqualified entry is the Object/toplevel candidate
                // (phase 3). So we must run phase 2 (the ancestor
                // walk) BETWEEN the qualified lexical entries and the
                // bare entry — otherwise a constant present at BOTH
                // toplevel AND an ancestor module would wrongly bind
                // to toplevel (CRuby picks the ancestor).
                let lex_split = chain.len().saturating_sub(1);
                let mut found: Option<Value> = None;
                // Phase 1: qualified lexical scopes (all but the last,
                // bare entry).
                for sym in &chain[..lex_split] {
                    if let Some(c) = self.classes.get(sym).cloned() {
                        found = Some(Value::Class(c));
                        break;
                    }
                    if let Some(v) = self.constants.get(sym).cloned() {
                        found = Some(v);
                        break;
                    }
                }
                // Phase 2: ancestors of the innermost lexical cref.
                // This is what makes `include M` bring M's constants
                // into scope — `class C; include M; def f = FOO; end`
                // resolves bare `FOO` to `M::FOO` here (rouge's ~240
                // lexers do `include Token::Tokens` then reference
                // bare `Text` / `Str::Double`). The innermost cref is
                // `chain[0]` (innermost-first ordering) minus its
                // trailing `::<bare>` segment; `chain.last()` is the
                // fully-unqualified candidate. A single-segment chain
                // (`[bare]` only) means top-level scope — no enclosing
                // class — so `lex_split == 0` skips this. For a
                // multi-segment bare like `Str::Double`,
                // `const_via_ancestors` probes
                // `<ancestor>::Str::Double`, so an included
                // `Token::Tokens` resolves `Token::Tokens::Str::Double`.
                if found.is_none()
                    && let Some(&bare_sym) = chain.last()
                {
                    let bare_str = self.interner.resolve(bare_sym).to_string();
                    let suffix = format!("::{}", bare_str);
                    // Walk EVERY lexical scope (innermost first), not just
                    // chain.first(): CRuby resolves the HEAD of a
                    // `Head::Rest` reference against the full Module.nesting
                    // chain. `Entity::NAME` inside `class Text` (nesting
                    // [REXML::Text, REXML]) resolves `Entity` to
                    // `REXML::Entity` via the OUTER scope, not
                    // `REXML::Text::Entity` (rexml text.rb:23). Pre-fix only
                    // the innermost cref was tried, so the outer-scope head
                    // was missed.
                    for &lex_qid in &chain[..lex_split] {
                        if found.is_some() { break; }
                        if lex_qid == bare_sym { continue; }
                        let lex_full = self.interner.resolve(lex_qid).to_string();
                        let Some(scope_name) = lex_full.strip_suffix(&suffix) else { continue; };
                        if scope_name.is_empty() || !self.interner.contains(scope_name) { continue; }
                        let scope_id = self.interner.intern(scope_name);
                        let Some(cref) = self.classes.get(&scope_id).cloned() else { continue; };
                        if let Some((head, rest)) = bare_str.split_once("::") {
                            // Multi-segment bare (`Str::Double`, `Entity::NAME`):
                            // resolve the head through this scope's ancestry,
                            // then the rest via `resolve_const_path` on the
                            // resolved class (which walks ITS ancestry too —
                            // `Entity::NAME` → XMLTokens::NAME via include).
                            let head_id = self.interner.intern(head);
                            let mut head_cls = match self.const_via_ancestors(&cref, head_id) {
                                Some(Value::Class(c)) => Some(c),
                                _ => None,
                            };
                            // The head may itself be an autoload not yet loaded
                            // — e.g. top-level `C` in a nested `C::X` reference
                            // where C<B<A are all zeitwerk autoloads. Fire it,
                            // then take the now-defined class (zeitwerk
                            // test_ancestors).
                            #[cfg(not(target_os = "wasi"))]
                            if head_cls.is_none() && self.fire_pending_autoload(head)? {
                                head_cls = self.classes.get(&head_id).cloned();
                            }
                            // `prefer_own_autoload = true`: `rest` is a scoped
                            // lookup under a real resolved scope — a pending
                            // `Head::rest` autoload must win over the toplevel
                            // core name (dry-types' `Types::Array` shadowing
                            // `::Array`; the flag only suppresses the toplevel
                            // fallback WHEN such an autoload is pending).
                            if let Some(head_cls) = head_cls
                                && let crate::vm::dispatch::ConstPathOutcome::Found(v) =
                                    self.resolve_const_path(&head_cls, rest, true, true)
                            {
                                // Explicit `Head::rest` read (the source wrote
                                // `::`) — enforce `private_constant` on the
                                // resolved single-segment const, matching the
                                // flat LoadConst path. Bare lexical reads take
                                // the `else` branch below and are unaffected.
                                if !self.private_consts.is_empty() && !rest.contains("::") {
                                    let pk = format!("{}::{}", head_cls.effective_name().unwrap_or_default(), rest);
                                    let pkid = self.interner.intern(&pk);
                                    if self.private_consts.contains(&pkid) {
                                        return Err(self.trap(crate::error::RubyError::NameError {
                                            msg: format!("private constant {} referenced", pk),
                                        }));
                                    }
                                }
                                found = Some(v);
                            }
                        } else {
                            found = self.const_via_ancestors(&cref, bare_sym);
                            // Ancestor scoped-autoload: `C::X` where X is
                            // autoloaded on an ANCESTOR of C (a superclass or
                            // included module), not on C itself.
                            // const_via_ancestors only sees DEFINED ancestor
                            // consts; walk the ancestry, fire the first pending
                            // `Ancestor::X` autoload, then re-resolve. zeitwerk's
                            // test_ancestors / test_nsfiles need this.
                            #[cfg(not(target_os = "wasi"))]
                            if found.is_none() {
                                let mut fired = false;
                                for anc in super::flatten_ancestors(&cref) {
                                    // effective_name: autovivified zeitwerk
                                    // namespaces have an empty structural name.
                                    let Some(anc_name) = anc.effective_name() else { continue };
                                    if anc_name.is_empty() { continue; }
                                    let key = format!("{}::{}", anc_name, bare_str);
                                    if !self.interner.contains(&key) { continue; }
                                    let kid = self.interner.intern(&key);
                                    if let Some(p) = self.autoloads_scoped.remove(&kid) {
                                        self.consumed_autoloads.insert(kid);
                                        self.invoke_require_for_autoload(Value::new_str(p))?;
                                        if self.classes.contains_key(&kid) || self.constants.contains_key(&kid) {
                                            self.consumed_autoloads.remove(&kid);
                                        }
                                        fired = true;
                                        break;
                                    }
                                }
                                if fired {
                                    found = self.const_via_ancestors(&cref, bare_sym);
                                }
                            }
                        }
                    }
                }
                // Phase 2.5: fire a pending autoload registered at a
                // NEARER LEXICAL scope (`chain[..lex_split]`) BEFORE the
                // toplevel fallback. CRuby resolves a lexically-scoped
                // autoloaded constant (e.g. `Loaders::YAML`) over a
                // same-named TOPLEVEL one (stdlib `::YAML`). Without
                // this, `register YAML` inside `module …FrontMatter::
                // Loaders` bound the stdlib `::YAML` (a registered stub)
                // instead of firing the `autoload :YAML` loader class —
                // so the registry held the wrong class and `loader_class
                // .header?` hit `YAML.header?` (NoMethodError). Wasi has
                // no `require`, so this compiles out.
                #[cfg(not(target_os = "wasi"))]
                if found.is_none() {
                    for sym in &chain[..lex_split] {
                        let cand = self.interner.resolve(*sym).to_string();
                        if self.fire_pending_autoload(&cand)? {
                            if let Some(c) = self.classes.get(sym).cloned() {
                                found = Some(Value::Class(c));
                                break;
                            }
                            if let Some(v) = self.constants.get(sym).cloned() {
                                found = Some(v);
                                break;
                            }
                        }
                    }
                }
                // Phase 3: toplevel / Object (the bare last entry).
                if found.is_none()
                    && let Some(bare_sym) = chain.last()
                {
                    if let Some(c) = self.classes.get(bare_sym).cloned() {
                        found = Some(Value::Class(c));
                    } else if let Some(v) = self.constants.get(bare_sym).cloned() {
                        found = Some(v);
                    }
                }
                let v = match found {
                    Some(v) => v,
                    None => {
                        // Scoped autoload trigger — Phase 2 of issue
                        // #224. Before falling through to the ENV
                        // intercept / NameError, try to satisfy each
                        // chain candidate via a pending autoload.
                        //
                        // Crucially this walks every `::`-PREFIX of each
                        // candidate (via `fire_pending_autoload`), not
                        // just the exact qualified key: a reference like
                        // `Document::DATE_FILENAME_MATCHER` inside
                        // `module Jekyll` compiles to the candidate
                        // `Jekyll::Document::DATE_FILENAME_MATCHER`, but
                        // the pending autoload is registered on the
                        // INTERMEDIATE namespace `Jekyll::Document`
                        // (`autoload :Document, "jekyll/document"`).
                        // Matching only the full key missed it, so the
                        // constant stayed unresolved until something
                        // else happened to touch bare `Document` first.
                        // After each successful require, re-attempt the
                        // whole walk. Wasi has no `require`, so this
                        // compiles out. Discovery: P3 Jekyll spike —
                        // `post_reader.rb` reads
                        // `Document::DATE_FILENAME_MATCHER` cold.
                        //
                        // LOOP until resolved or no autoload fires: a
                        // candidate can be a MULTI-LEVEL autoload chain
                        // where firing the outer autoload registers a
                        // fresh NESTED one. Bridgetown's
                        // `Bridgetown::FrontMatter::RubyFrontMatter`
                        // first fires `autoload :FrontMatter` (loads
                        // front_matter.rb), which only THEN declares
                        // `autoload :RubyFrontMatter` — a single firing
                        // pass left RubyFrontMatter pending and missed.
                        // Each `fire_pending_autoload` consumes one
                        // pending entry (it `.remove`s before requiring),
                        // so the loop is bounded and terminates when a
                        // pass fires nothing new.
                        #[cfg(not(target_os = "wasi"))]
                        loop {
                            // Resolve from already-loaded entries first
                            // (also catches the const a prior iteration's
                            // require just defined).
                            for s2 in &chain {
                                if let Some(c) = self.classes.get(s2).cloned() {
                                    self.stack.push(Value::Class(c));
                                    return Ok(true);
                                }
                                if let Some(cv) = self.constants.get(s2).cloned() {
                                    self.stack.push(cv);
                                    return Ok(true);
                                }
                            }
                            // Fire one pending autoload across the
                            // candidates; stop when a full pass fires none.
                            let mut fired = false;
                            for sym in &chain {
                                let cand = self.interner.resolve(*sym).to_string();
                                if self.fire_pending_autoload(&cand)? {
                                    fired = true;
                                    break;
                                }
                            }
                            if !fired {
                                // requires ran but defined no candidate —
                                // fall through to the NameError below.
                                break;
                            }
                        }
                        // ENV fallback: when the chain walk fails to
                        // find any user-defined constant matching the
                        // qualified candidates, AND the bare-name
                        // (LAST entry in the chain — innermost-first
                        // ordering means the unqualified fallback is
                        // last) is "ENV", lazy-build the env_hash via
                        // the same intercept path Op::LoadConst uses.
                        // Without this, a bare `ENV` reference inside
                        // a nested class body emits LoadConstChain
                        // (which previously didn't know about ENV),
                        // so sinatra-style `ENV['VAR']` at body level
                        // raised \"uninitialized constant
                        // Foo::Bar::ENV\". PR #234 / pass-9.7c
                        // layer #20.
                        let bare = *chain.last().expect("ICE: LoadConstChain with empty chain");
                        if self.interner.resolve(bare).as_ref() == "ENV" {
                            let id = self.env_hash_or_init()?;
                            self.stack.push(Value::Hash(id));
                            return Ok(true);
                        }
                        // Report the INNERMOST-scope qualified form
                        // in the NameError so the user sees the path
                        // CRuby would have searched first — e.g.
                        // `uninitialized constant Foo::Bar::UnresolvedX`
                        // rather than the bare `UnresolvedX`.
                        // chain[0] is the innermost candidate.
                        let name = self.interner.resolve(chain[0]).clone();
                        return Err(self.trap(crate::error::RubyError::NameError {
                            msg: format!("uninitialized constant {}", name),
                        }));
                    }
                };
                // Fill the IC. The autoload path may have bumped
                // `const_gen` mid-op (its require defines constants) —
                // storing with the CURRENT gen is correct: resolution
                // is stable from this point until the next mutation.
                self.const_cache_chain.insert(cache_key, (v.clone(), self.const_gen));
                self.stack.push(v);
            }
            Op::LoadConstChainOrNil(chain_idx) => {
                let proto_idx = self.frames.last().expect("ICE: LoadConstChainOrNil no frame").proto_idx;
                let chain = self.protos[proto_idx].const_chains[chain_idx as usize].clone();
                let mut found: Option<Value> = None;
                for sym in &chain {
                    if let Some(c) = self.classes.get(sym).cloned() {
                        found = Some(Value::Class(c));
                        break;
                    }
                    if let Some(v) = self.constants.get(sym).cloned() {
                        found = Some(v);
                        break;
                    }
                }
                self.stack.push(found.unwrap_or(Value::Nil));
            }
            Op::LoadGlobal(name_id) => {
                // Special-globals intercept. `$$` is the canonical
                // case from tilt/template.rb (`"...-#{$$}"`); add
                // others here as real codebases need them. `$0`
                // returns the script's filename (we use the top
                // frame's proto filename, which Runtime::eval set
                // to whatever the host passed).
                let name = {
                    let resolved = self.interner.resolve(name_id).clone();
                    // `require "English"` aliases the verbose global
                    // names to the punctuation globals (`$POSTMATCH` →
                    // `$'`, `$MATCH` → `$&`, …). Remap up front so every
                    // handler below serves them. Gated on English being
                    // required — CRuby leaves `$POSTMATCH` nil until then.
                    // rss `require "English"` then builds method names
                    // from `$POSTMATCH` (`alias_method "#{$POSTMATCH}?",
                    // name` in install_get_attribute).
                    if self.loaded_stdlib_stubs.contains("English")
                        || self.loaded_stdlib_stubs.contains("english")
                    {
                        match &*resolved {
                            "$MATCH" => std::rc::Rc::from("$&"),
                            "$PREMATCH" => std::rc::Rc::from("$`"),
                            "$POSTMATCH" => std::rc::Rc::from("$'"),
                            "$LAST_PAREN_MATCH" => std::rc::Rc::from("$+"),
                            "$LAST_MATCH_INFO" => std::rc::Rc::from("$~"),
                            "$PROGRAM_NAME" => std::rc::Rc::from("$0"),
                            _ => resolved,
                        }
                    } else {
                        resolved
                    }
                };
                // `$1`, `$2`, ..., `$10`, `$11`, ... — numbered
                // capture references, written by ast.rs as
                // `GVarRead("$N")` (the AST arm for
                // `NumberedReferenceReadNode`). N-th group from the
                // most recent successful match, or nil if no match
                // or the group did not participate. CRuby allows
                // any positive index (`$10` reads the 10th group),
                // so accept all digits after `$` rather than just
                // a single one. `$0` is excluded — it's a separate
                // global (the script filename) handled below.
                // Branched out of the `match` below so it can stay
                // strictly statement-shaped (no allocator call
                // needed — just clones a String).
                #[cfg(feature = "regex")]
                if name.len() >= 2
                    && name.starts_with('$')
                    && name.as_bytes()[1] != b'0'
                    && name.as_bytes()[1..].iter().all(|c| c.is_ascii_digit())
                {
                    let n: usize = name[1..].parse().unwrap_or(0);
                    let v = match self.scoped_last_match() {
                        // BINARY subject: rebuild the capture from raw
                        // bytes + span (ASCII-8BIT) so an invalid byte
                        // survives, instead of the lossy `caps` string.
                        Some(m) if n >= 1 => match &m.binary {
                            Some(bc) => match bc.group_spans.get(n - 1) {
                                Some(Some((a, b))) => {
                                    Value::new_str_bytes_binary(bc.input[*a..*b].to_vec())
                                }
                                _ => Value::Nil,
                            },
                            None => match m.caps.get(n - 1) {
                                Some(Some(cap)) => Value::new_str(cap.clone()),
                                _ => Value::Nil,
                            },
                        },
                        _ => Value::Nil,
                    };
                    self.stack.push(v);
                    return Ok(true);
                }
                // `$&` (whole match) / `$+` (last non-nil capture)
                // / `` $` `` (pre-match) / `$'` (post-match) — the
                // BackReferenceReadNode family, all derived from
                // `last_match`. nil if no match has happened yet
                // (or the last one failed). Pre/post-match read the
                // input slice directly from `LastMatch::input`.
                #[cfg(feature = "regex")]
                match &*name {
                    "$&" => {
                        let v = match self.scoped_last_match() {
                            Some(m) => match &m.binary {
                                Some(bc) => Value::new_str_bytes_binary(
                                    bc.input[m.m_start..m.m_end].to_vec(),
                                ),
                                None => Value::new_str(m.whole.clone()),
                            },
                            None => Value::Nil,
                        };
                        self.stack.push(v);
                        return Ok(true);
                    }
                    "$+" => {
                        // CRuby: last non-nil capture from the last
                        // successful match — `nil` if no match or no
                        // group participated.
                        let v = match self.scoped_last_match() {
                            Some(m) => match &m.binary {
                                // BINARY: last participating group's span.
                                Some(bc) => bc
                                    .group_spans
                                    .iter()
                                    .rev()
                                    .find_map(|s| *s)
                                    .map(|(a, b)| Value::new_str_bytes_binary(bc.input[a..b].to_vec()))
                                    .unwrap_or(Value::Nil),
                                None => m.caps.iter().rev().find_map(|c| c.as_ref())
                                    .map(|s| Value::new_str(s.clone()))
                                    .unwrap_or(Value::Nil),
                            },
                            None => Value::Nil,
                        };
                        self.stack.push(v);
                        return Ok(true);
                    }
                    "$`" => {
                        let v = match self.scoped_last_match() {
                            Some(m) => Value::new_str(m.input[..m.m_start].to_string()),
                            None => Value::Nil,
                        };
                        self.stack.push(v);
                        return Ok(true);
                    }
                    "$'" => {
                        let v = match self.scoped_last_match() {
                            Some(m) => Value::new_str(m.input[m.m_end..].to_string()),
                            None => Value::Nil,
                        };
                        self.stack.push(v);
                        return Ok(true);
                    }
                    _ => {}
                }
                // `$~` — MatchData of the last successful match,
                // or nil. Materialises a fresh MatchData instance
                // on each read (same `@whole`/`@caps` shape as
                // `String#match`'s return value). Branched out so
                // we can call `maybe_gc` + `check_alloc?` cleanly.
                #[cfg(feature = "regex")]
                if &*name == "$~" {
                    // Full MatchData incl. pre/post-match + string,
                    // shared with `Regexp.last_match`.
                    let v = self.materialize_last_match()?;
                    self.stack.push(v);
                    return Ok(true);
                }
                let v = match &*name {
                    // ADR 0017 Rule 1: the script never reads the
                    // host process's real PID. `Config::pid = Some(n)`
                    // → `$$` returns `n`; `None` (default) → returns
                    // `0` as a sentinel. CLI binary `rubyrs` fills
                    // this from `std::process::id()` to preserve
                    // CRuby parity.
                    "$$" => Value::Int(self.pid.unwrap_or(0)),
                    "$0" => {
                        // Bottommost frame = script entry; its
                        // proto's filename is the script's top-level
                        // filename (or "<inline>" for eval calls).
                        let name = self.frames.first()
                            .map(|f| self.protos[f.proto_idx].filename.to_string())
                            .unwrap_or_else(|| "-".to_string());
                        Value::new_str(name)
                    }
                    // `$LOAD_PATH` / `$:` — the require-search-path
                    // Array. Lazily materialised on first read so
                    // scripts that don't touch it pay no startup
                    // cost. The Array is mutable and persistent —
                    // `$LOAD_PATH.unshift(dir)` adds an entry that
                    // subsequent `require` calls consult (see
                    // `Vm::ruby_source_candidates`).
                    "$LOAD_PATH" | "$:" => {
                        let id = self.ensure_load_path()?;
                        Value::Array(id)
                    }
                    // `$LOADED_FEATURES` / `$"` — the Array of loaded
                    // file paths. Lazily materialised; populated by
                    // `compile_and_run_source` on each require/load.
                    "$LOADED_FEATURES" | "$\"" => {
                        let id = self.ensure_loaded_features_list()?;
                        Value::Array(id)
                    }
                    _ => self.globals.get(&name_id).cloned().unwrap_or(Value::Nil),
                };
                self.stack.push(v);
            }
            Op::StoreGlobal(name_id) => {
                // `$foo = expr` — pop the value and store. In
                // statement position the compiler does NOT emit a
                // preceding Dup (mirrors ConstWrite/IVarWrite); in
                // expression position it emits Dup first, so the
                // value remains on the stack as the assignment's
                // result. Special-global writes (`$$ = 42`) are
                // silently accepted into `Vm.globals` but the next
                // read still intercepts and returns the computed
                // value — a documented spike divergence.
                let v = self.stack.pop().expect("ICE: StoreGlobal stack underflow");
                self.globals.insert(name_id, v);
            }
            Op::Jump(off) => {
                let f = self.frames.last_mut().expect("ICE: Jump no frame");
                f.ip = (f.ip as i32 + off) as usize;
            }
            Op::JumpIfFalse(off) => {
                let v = self.stack.pop().expect("ICE: JumpIfFalse stack underflow");
                if !v.is_truthy() {
                    let f = self.frames.last_mut().expect("ICE: JumpIfFalse no frame");
                    f.ip = (f.ip as i32 + off) as usize;
                }
            }
            Op::JumpIfArgGiven(slot, off) => {
                let f = self.frames.last_mut().expect("ICE: JumpIfArgGiven no frame");
                if slot < f.n_given_positional {
                    f.ip = (f.ip as i32 + off) as usize;
                }
            }
            Op::JumpIfKwArgGiven(kw_idx, off) => {
                let f = self.frames.last_mut().expect("ICE: JumpIfKwArgGiven no frame");
                if kw_idx < 64 && (f.kw_given_mask & (1u64 << kw_idx)) != 0 {
                    f.ip = (f.ip as i32 + off) as usize;
                }
            }
            Op::Call(name_id, argc, cache_id) => {
                // Plain call (no keyword syntax): an explicit-brace
                // trailing Hash is POSITIONAL in Ruby 3. Flag it so
                // `invoke_method_with_block` doesn't peel it into kwargs.
                // Cleared after the dispatch (even on error) so it can't
                // leak into the next call.
                self.trailing_hash_positional = true;
                let r = self.do_call(name_id, argc as usize, false, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            // Superinstruction: `LoadLocal(slot); Call(name, 0, cache)`.
            // Push the local receiver (mirrors Op::LoadLocal), then run
            // the same zero-arg dispatch as Op::Call — one dispatch.
            Op::LoadLocalCall(slot, name_id, cache_id) => {
                let f = self.frames.last().expect("ICE: LoadLocalCall no frame");
                let v = match &f.locals {
                    crate::vm::Locals::Stack(base) => {
                        self.locals_arena[*base as usize + slot as usize].clone()
                    }
                    crate::vm::Locals::Shared(rc) => rc.borrow()[slot as usize].clone(),
                };
                self.stack.push(v);
                self.trailing_hash_positional = true;
                let r = self.do_call(name_id, 0, false, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            Op::InterpToS(cache_id) => {
                // Interpolation part: a String stays as-is (CRuby's
                // rb_obj_as_string — user String#to_s NOT consulted);
                // anything else dispatches to_s like a plain call.
                if !matches!(self.stack.last(), Some(Value::Str(_))) {
                    let to_s = self.sym_to_s;
                    self.do_call(to_s, 0, false, cache_id)?;
                }
            }
            Op::CallAset(name_id, argc, cache_id) => {
                // Assignment-syntax dispatch: expression value is the
                // RHS (stack top = last positional arg), never the
                // method's return (CRuby rule, syntactic only). The
                // RHS snapshot stays alive across the dispatch — for
                // the frame path it lands in `swap_return` (a GC
                // root, gc.rs); for the inline path no allocation
                // happens between dispatch return and the replace.
                let rhs = self.stack.last().cloned().unwrap_or(Value::Nil);
                let pre_frames = self.frames.len();
                self.trailing_hash_positional = true;
                let r = self.do_call(name_id, argc as usize, false, cache_id);
                self.trailing_hash_positional = false;
                r?;
                if self.frames.len() > pre_frames {
                    // User-method frame pushed — its eventual return
                    // value is discarded in favour of the RHS (same
                    // mechanism Class.new uses for initialize).
                    if let Some(f) = self.frames.last_mut() {
                        f.swap_return = Some(rhs);
                    }
                } else if let Some(top) = self.stack.last_mut() {
                    // Dispatch completed inline (primitive arm / fast
                    // path / host fn) — replace its pushed result.
                    *top = rhs;
                }
            }
            Op::CallNoRecv(name_id, argc, cache_id) => {
                self.trailing_hash_positional = true;
                let r = self.do_call(name_id, argc as usize, true, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            // Kwarg-trailing variants — argc includes the trailing
            // kwargs Hash. The dispatcher's helper splits the Hash
            // off into a dedicated channel before invoking the
            // method so primitive arms can consume `:foo` keys
            // instead of inspecting the positional Hash heuristically.
            Op::CallKw(name_id, argc, cache_id)
            | Op::CallKwNoRecv(name_id, argc, cache_id) => {
                let no_recv = matches!(op, Op::CallKwNoRecv(_, _, _));
                let mut argc = argc as usize;
                // An EMPTY (or nil) trailing kwsplat contributes nothing —
                // `f(x, **{})` passes just `x` in CRuby. Drop it and
                // re-dispatch as a PLAIN positional call so a preceding
                // brace-hash stays positional. Otherwise `f({a:1}, **{})`
                // peeled the {a:1} into kwargs, leaving the positional
                // `value` unbound ("given 0, expected 1") — which broke
                // `ActiveSupport::MessageEncryptor#encrypt_and_sign(v)`
                // (`create_message(value, **options)` with empty options).
                // Mirrors the CallKwBlock arm; a non-empty kwargs Hash
                // keeps the do_call_kw path below.
                let drop_trailing = argc > 0 && match self.stack.last() {
                    Some(crate::value::Value::Hash(hid)) => self.heap.hash(*hid).is_empty(),
                    Some(crate::value::Value::Nil) => true,
                    _ => false,
                };
                if drop_trailing {
                    self.stack.pop();
                    argc -= 1;
                    // After the drop there are no kwargs, so a trailing
                    // brace-hash is positional — same contract as Op::Call.
                    self.trailing_hash_positional = true;
                    let r = self.do_call(name_id, argc, no_recv, cache_id);
                    self.trailing_hash_positional = false;
                    r?;
                } else {
                    // A non-empty CallKw trailing Hash is ALWAYS keyword
                    // args (`k: v` / `**h`), never a positional brace hash,
                    // so clear the positional-hash flag explicitly. The
                    // plain `Call` ops set it TRUE and reset it after;
                    // relying on the residual-false default broke when the
                    // call runs while an OUTER native call (e.g. `eval`,
                    // whose body dispatches synchronously inside `do_call`)
                    // still holds the flag TRUE — the callee then bound the
                    // kwargs positionally ("given 1, expected 0" against a
                    // kwarg-only method).
                    self.trailing_hash_positional = false;
                    let r = self.do_call_kw(name_id, argc, no_recv, cache_id);
                    self.trailing_hash_positional = false;
                    r?;
                }
            }
            Op::ApplyCall(name_id, cache_id)
            | Op::ApplyCallNoRecv(name_id, cache_id)
            | Op::ApplyCallPrimitive(name_id, cache_id) => {
                // `ApplyCallPrimitive` is the with-recv splat shape with
                // a one-shot "skip the user override, run the primitive"
                // flag — the body of a primitive-alias forwarder.
                if matches!(op, Op::ApplyCallPrimitive(_, _)) {
                    self.force_primitive_dispatch = true;
                }
                // Splat-call: pop the args Array, push its
                // elements back onto the stack as positional args,
                // then dispatch with that dynamic argc. Receiver
                // (when present) sits below the array on the
                // stack — same layout `do_call` expects.
                //
                // GC rooting: between the args-Array pop and the
                // re-push of its elements, the Array (and
                // transitively every heap-shaped element) has NO
                // stack root. STRESS_GC under `invoke_block`'s
                // rest-array assembly could sweep an element like
                // `Value::Hash(hid)` mid-flight, surfacing later
                // as `ICE: heap slot is not a Hash` when the
                // dispatched method dereferences the dangling
                // ObjId. Pin the Array — the GC mark walk traverses
                // its elements, so pinning the Array transitively
                // roots every element through the pop→push window.
                // Repro fixture: `tests/diff/callable_coerce.rb`
                // under STRESS_GC=1 (`.method(:call).to_proc.call(
                // {"VIA" => "x"})` with a Hash arg).
                let no_recv = matches!(op, Op::ApplyCallNoRecv(_, _));
                let arr_val = self.stack.pop().expect("ICE: ApplyCall without arg array");
                let arr_id = match arr_val {
                    Value::Array(id) => id,
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Array (splat arg)", other.type_name()),
                    })),
                };
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(Value::Array(arr_id));
                let elems: Vec<Value> = g.vm.heap.array(arr_id).clone();
                let argc = elems.len();
                for v in elems { g.vm.stack.push(v); }
                drop(g);
                self.do_call(name_id, argc, no_recv, cache_id)?;
            }
            Op::ApplyCallKw(name_id, cache_id)
            | Op::ApplyCallKwNoRecv(name_id, cache_id) => {
                // `foo(*args, **kw)` — the kwsplat Hash is carried
                // SEPARATELY on top of the positional array. Stack
                // (bottom→top): `[recv?, array, kwsplat]`. Expand the
                // array as POSITIONAL args; an EMPTY kwsplat is dropped
                // (so a trailing positional brace-hash stays positional —
                // `f({a:1}, **{})` → value={a:1}), a non-empty one rides
                // as the trailing kwargs arg (trailing_hash_positional
                // false → the binder peels it, not the array's tail).
                let no_recv = matches!(op, Op::ApplyCallKwNoRecv(_, _));
                let kw_val = self.stack.pop().expect("ICE: ApplyCallKw without kwsplat");
                let arr_val = self.stack.pop().expect("ICE: ApplyCallKw without arg array");
                let arr_id = match arr_val {
                    Value::Array(id) => id,
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Array (splat arg)", other.type_name()),
                    })),
                };
                let kw_empty = match &kw_val {
                    Value::Hash(hid) => self.heap.hash(*hid).is_empty(),
                    Value::Nil => true,
                    _ => false,
                };
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(Value::Array(arr_id));
                g.pin(kw_val.clone());
                let elems: Vec<Value> = g.vm.heap.array(arr_id).clone();
                let mut argc = elems.len();
                for v in elems { g.vm.stack.push(v); }
                if !kw_empty {
                    g.vm.stack.push(kw_val);
                    argc += 1;
                }
                drop(g);
                if kw_empty {
                    // No kwargs survive: array tail (if a Hash) is positional.
                    self.trailing_hash_positional = true;
                } else {
                    // The trailing arg is the kwsplat → let the binder peel it.
                    self.trailing_hash_positional = false;
                }
                let r = self.do_call(name_id, argc, no_recv, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            Op::CallBuiltinDirect(name_id) => {
                // Pop the `*args` Array and invoke the Kernel builtin
                // directly via `builtin_call`, bypassing `do_call` (and
                // therefore any user override). See the op doc — this
                // is the alias forwarder for a Kernel global, which
                // must reach the original implementation without
                // re-entering a redefined `require`. Pin the Array so
                // heap-shaped args survive a load-triggered GC.
                let arr_val = self.stack.pop().expect("ICE: CallBuiltinDirect without arg array");
                let arr_id = match arr_val {
                    Value::Array(id) => id,
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Array (splat arg)", other.type_name()),
                    })),
                };
                let name = self.interner.resolve(name_id).to_string();
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(Value::Array(arr_id));
                let elems: Vec<Value> = g.vm.heap.array(arr_id).clone();
                let res = g.vm.builtin_call(&name, &elems);
                drop(g);
                match res {
                    Some(Ok(v)) => self.stack.push(v),
                    Some(Err(t)) => return Err(t),
                    None => return Err(self.trap(RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::Missing,
                        method: name,
                        recv_type: std::borrow::Cow::Borrowed("Object"),
                    })),
                }
            }
            Op::CallBlock(name_id, argc, cache_id) => {
                // `CallBlock` is emitted ONLY for the NON-kwargs block call
                // (`foo(args, &blk)`); a `k: v` / `**h` + block goes to
                // `CallKwBlock` (see emit_method_call). So a trailing brace
                // Hash here is POSITIONAL in Ruby 3 — same contract as the
                // plain `Op::Call`. Set the flag TRUE so the binder doesn't
                // peel it as kwargs (`m({k:1}, &b)` → a={k:1}); a hash var
                // in positional position likewise stays positional.
                self.trailing_hash_positional = true;
                let r = self.do_call_block(name_id, argc as usize, false, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            Op::CallNoRecvBlock(name_id, argc, cache_id) => {
                // Non-kwargs block call (no receiver) — trailing brace Hash
                // is positional, same as CallBlock above.
                self.trailing_hash_positional = true;
                let r = self.do_call_block(name_id, argc as usize, true, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            // `foo(**kw, &blk)` — block-form call with a keyword-splat
            // trailing Hash. An EMPTY/`nil` kwsplat contributes zero
            // args (CRuby: `f(**{}, &b)` passes nothing), so drop the
            // trailing arg before dispatch; the block stays below it on
            // the stack. A non-empty kwargs Hash travels as the trailing
            // arg with `trailing_hash_positional = false`, so a callee
            // declaring kw params binds it (parity with `CallKw`).
            Op::CallKwBlock(name_id, argc, cache_id)
            | Op::CallKwNoRecvBlock(name_id, argc, cache_id) => {
                let no_recv = matches!(op, Op::CallKwNoRecvBlock(_, _, _));
                let mut argc = argc as usize;
                if argc > 0 {
                    let drop_trailing = match self.stack.last() {
                        Some(crate::value::Value::Hash(hid)) => self.heap.hash(*hid).is_empty(),
                        Some(crate::value::Value::Nil) => true,
                        _ => false,
                    };
                    if drop_trailing {
                        // The trailing kwsplat is the top of stack; the
                        // block sits below the positional args, so a bare
                        // pop removes exactly the kwsplat.
                        self.stack.pop();
                        argc -= 1;
                    }
                }
                self.trailing_hash_positional = false;
                let r = self.do_call_block(name_id, argc, no_recv, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            Op::ApplyCallBlock(name_id, cache_id) | Op::ApplyCallNoRecvBlock(name_id, cache_id) => {
                // Splat-call with explicit `&block`. Stack layout
                // (bottom→top): `[recv?, block, array]`. Pop the
                // args Array, expand its elements as positional
                // args (re-establishing the `do_call_block` layout
                // `[recv?, block, arg1, ..., argN]`), then dispatch.
                // GC rooting: same hazard as `Op::ApplyCall` — the
                // Array (and its heap-shaped elements) has no stack
                // root between pop and re-push. Pin via PinGuard.
                let no_recv = matches!(op, Op::ApplyCallNoRecvBlock(_, _));
                let arr_val = self.stack.pop().expect("ICE: ApplyCallBlock without arg array");
                let arr_id = match arr_val {
                    Value::Array(id) => id,
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Array (splat arg)", other.type_name()),
                    })),
                };
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(Value::Array(arr_id));
                let elems: Vec<Value> = g.vm.heap.array(arr_id).clone();
                let argc = elems.len();
                for v in elems { g.vm.stack.push(v); }
                drop(g);
                self.do_call_block(name_id, argc, no_recv, cache_id)?;
            }
            Op::ApplyCallKwBlock(name_id, cache_id)
            | Op::ApplyCallKwNoRecvBlock(name_id, cache_id) => {
                // `foo(*args, **kw, &blk)` — block path of ApplyCallKw.
                // Stack (bottom→top): `[recv?, block, array, kwsplat]`.
                // Expand the array as positional (above the block), drop an
                // EMPTY kwsplat (trailing positional hash stays positional)
                // or ride a non-empty one as the trailing kwargs arg, then
                // dispatch through the block path so the block installs.
                let no_recv = matches!(op, Op::ApplyCallKwNoRecvBlock(_, _));
                let kw_val = self.stack.pop().expect("ICE: ApplyCallKwBlock without kwsplat");
                let arr_val = self.stack.pop().expect("ICE: ApplyCallKwBlock without arg array");
                let arr_id = match arr_val {
                    Value::Array(id) => id,
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Array (splat arg)", other.type_name()),
                    })),
                };
                let kw_empty = match &kw_val {
                    Value::Hash(hid) => self.heap.hash(*hid).is_empty(),
                    Value::Nil => true,
                    _ => false,
                };
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(Value::Array(arr_id));
                g.pin(kw_val.clone());
                let elems: Vec<Value> = g.vm.heap.array(arr_id).clone();
                let mut argc = elems.len();
                for v in elems { g.vm.stack.push(v); }
                if !kw_empty {
                    g.vm.stack.push(kw_val);
                    argc += 1;
                }
                drop(g);
                self.trailing_hash_positional = kw_empty;
                let r = self.do_call_block(name_id, argc, no_recv, cache_id);
                self.trailing_hash_positional = false;
                r?;
            }
            Op::ApplySuper(name_id) => {
                // Pop assembled args Array and drain elements
                // into a Vec<Value>. From here the super-
                // lookup path is identical to Op::Super; the
                // only difference is how the args Vec was
                // produced (splat-assembled at the call site
                // vs. pushed individually by Op::Super).
                let args_val = self.stack.pop().expect("ICE: ApplySuper without args slot");
                let args: Vec<Value> = match args_val {
                    Value::Array(aid) => self.heap.array(aid).clone(),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("ApplySuper expected Array args, got {}", other.type_name()),
                    })),
                };
                self.super_call_with_lifecycle_noop(name_id, args)?;
            }
            Op::ApplySuperBlock(name_id) => {
                // Stack: `[block, array]` (block pushed first, array
                // on top). Same super-lookup path as Op::ApplySuper,
                // but the popped block forwards through
                // `invoke_method_with_block` so the dispatched
                // frame sees an explicit block in the same slot it
                // would for `do ... end`. Used by
                // `def foo(*a, &b); super(*a, &b); end` forwarders
                // — sinatra-contrib/MultiRoute's per-verb methods.
                let args_val = self.stack.pop().expect("ICE: ApplySuperBlock without args slot");
                let args: Vec<Value> = match args_val {
                    Value::Array(aid) => self.heap.array(aid).clone(),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("ApplySuperBlock expected Array args, got {}", other.type_name()),
                    })),
                };
                let block_val = self.stack.pop().expect("ICE: ApplySuperBlock without block slot");
                // `&nil` is the legitimate "no block" shape; map it
                // to a None block slot. A real block forwards as-is; a
                // `&method(:x)` / `&curried_proc` (BoundMethod /
                // CurriedProc) coerces to a forwarder block via
                // `coerce_callable_to_block`, mirroring CallBlock's
                // richer arm. Sinatra's IndifferentHash#transform_values!
                // does `super(&method(:convert_value))`.
                let block_id = match block_val {
                    Value::Block(id) => Some(id),
                    Value::Nil => None,
                    Value::BoundMethod(_) | Value::CurriedProc(_) => {
                        Some(self.coerce_callable_to_block(block_val)?)
                    }
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Proc)",
                            other.type_name()
                        ),
                    })),
                };
                let name_id = self.super_runtime_name(name_id);
                match self.super_lookup(name_id) {
                    Ok((m, self_val)) => {
                        self.invoke_method_with_block(m, self_val, args, block_id)?;
                    }
                    Err(trap) => {
                        // Builtin-substitution twin of the no-block
                        // path's intercept in
                        // super_call_with_lifecycle_noop: minitest
                        // Mock's blank-slate keeps a
                        // `define_method(:send) { |*a, &b|
                        // super(*a, &b) }` passthrough, and
                        // Object#send is a do_call recogniser (no
                        // table Method above the override).
                        // Re-dispatch the FORWARDED name — an
                        // undef'd target then falls to
                        // method_missing, exactly Object#send's
                        // contract. `===` substitutes identity.
                        let is_no_super = matches!(
                            &trap.err,
                            RubyError::NoMethodError {
                                kind: crate::error::NoMethodErrorKind::SuperNoSuperclass,
                                ..
                            },
                        );
                        if !is_no_super {
                            return Err(trap);
                        }
                        let nm = self.interner.resolve(name_id).clone();
                        let cur_self = self.frames.last().map(|f| f.self_val.clone());
                        match (&*nm, cur_self) {
                            // `super(*a, &b)` to the builtin `Class#new` —
                            // twin of the no-block arm in
                            // `super_call_with_lifecycle_noop`. A
                            // `def self.new(*a, &b); super(*a, &b); end`
                            // override (or one `extend`ed via a module,
                            // e.g. concurrent-ruby's `SafeInitialization`
                            // on `Concurrent::Delay`) resolves super to
                            // the inline allocator; allocate + run
                            // initialize (forwarding the block) and yield
                            // the instance.
                            ("new", Some(Value::Class(cls))) => {
                                self.super_builtin_class_new_with_block(&cls, args, block_id)?;
                            }
                            ("allocate", Some(Value::Class(cls))) => {
                                let obj = self.alloc_default_instance(&cls)?;
                                self.stack.push(obj);
                            }
                            ("initialize", Some(Value::Object(_))) => {
                                // BasicObject#initialize no-op (nil).
                                self.stack.push(Value::Nil);
                            }
                            ("send" | "__send__" | "public_send", Some(obj @ Value::Object(_))) => {
                                let mut args = args;
                                if args.is_empty() {
                                    return Err(self.trap(RubyError::ArgumentError {
                                        msg: "no method name given".into(),
                                    }));
                                }
                                let target = args.remove(0);
                                let target_id = match &target {
                                    Value::Sym(s) => *s,
                                    Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                                    other => {
                                        return Err(self.trap(RubyError::TypeError {
                                            msg: format!(
                                                "{} is not a symbol nor a string",
                                                other.type_name()
                                            ),
                                        }));
                                    }
                                };
                                let argc = args.len();
                                match block_id {
                                    Some(bid) => {
                                        self.stack.push(obj);
                                        self.stack.push(Value::Block(bid));
                                        for a in args {
                                            self.stack.push(a);
                                        }
                                        self.do_call_block(target_id, argc, /*no_recv=*/ false, u16::MAX)?;
                                    }
                                    None => {
                                        self.stack.push(obj);
                                        for a in args {
                                            self.stack.push(a);
                                        }
                                        self.do_call(target_id, argc, /*no_recv=*/ false, u16::MAX)?;
                                    }
                                }
                            }
                            ("===", Some(obj @ Value::Object(_))) => {
                                let same = match (&obj, args.first()) {
                                    (Value::Object(a), Some(Value::Object(b))) => a == b,
                                    (_, Some(other)) => obj.ruby_eq(other, &self.heap),
                                    (_, None) => false,
                                };
                                self.stack.push(Value::Bool(same));
                            }
                            // `super(...) { |h, k| ... }` from a
                            // Hash-subclass method. `initialize`
                            // with a block is `Hash.new { ... }`
                            // semantics — install the default_proc
                            // (rack's QueryParser params_class
                            // subclasses `Params < Hash` and supers
                            // with an auto-vivify block). Twin of
                            // the no-block Hash arm in
                            // `super_call_with_lifecycle_noop`;
                            // other names route to the same
                            // primitives (the block is dropped
                            // there — collection_call has no block
                            // plumbing; documented divergence, no
                            // known consumer).
                            (_, Some(Value::Hash(id))) if self.heap.hash_class_tag(id).is_some() => {
                                if &*nm == "initialize" {
                                    self.heap.hash_set_default_block(id, block_id);
                                    if let Some(d) = args.first() {
                                        self.heap.hash_set_default_value(id, Some(d.clone()));
                                    }
                                    self.stack.push(Value::Nil);
                                } else {
                                    let recv = Value::Hash(id);
                                    // Block-form FIRST when a block is
                                    // forwarded: a non-block `fetch` /
                                    // `fetch_values` RAISES KeyError on a
                                    // miss (never returns None), so a
                                    // block-carrying `super` must reach
                                    // the block-form (`fetch { }`) first.
                                    // `super(&method(:convert_value))` from
                                    // IndifferentHash#transform_values! and
                                    // `def fetch(k,*d,&b); super; end` both
                                    // land here. bypass the subclass-override
                                    // deferral (no user super method).
                                    if let Some(bid) = block_id
                                        && let Some(v) =
                                            self.collection_call_block(&recv, &nm, &args, bid, true)?
                                    {
                                        self.stack.push(v);
                                    } else if let Some(v) = self.collection_call(&recv, &nm, &args)? {
                                        self.stack.push(v);
                                    } else {
                                        return Err(trap);
                                    }
                                }
                            }
                            // `super` FROM `method_missing` itself (a
                            // `def method_missing(m, *a, &b); …; super; end`
                            // fallthrough — the bare `super` forwards the
                            // block, so it lands here) reaches
                            // BasicObject#method_missing, which raises
                            // NoMethodError for the ORIGINAL missing method
                            // (args[0]). Routing it back through
                            // try_method_missing would re-invoke the SAME
                            // user method_missing → infinite recursion.
                            ("method_missing", _) => {
                                let missing = match args.first() {
                                    Some(Value::Sym(s)) => self.interner.resolve(*s).to_string(),
                                    Some(Value::Str(s)) => s.to_string_lossy(),
                                    _ => self.interner.resolve(name_id).to_string(),
                                };
                                let recv_desc = self.frames.last()
                                    .map(|f| self.recv_desc_for_error(&f.self_val))
                                    .unwrap_or_else(|| "Object".into());
                                return Err(self.trap(RubyError::NoMethodError {
                                    kind: crate::error::NoMethodErrorKind::Missing,
                                    method: missing,
                                    recv_type: std::borrow::Cow::Owned(recv_desc),
                                }));
                            }
                            // Lifecycle / inclusion hooks: CRuby ships real
                            // empty defaults on Module/Class
                            // (Module#included/extended/prepended,
                            // Class#inherited, method_added family), so a
                            // user hook's bare `super` (forwarding its
                            // block) reaches a no-op. ActiveSupport::Concern
                            // defines `included(base = nil, &block)` whose
                            // `super` (block-carrying → this path) must land
                            // here. Twin of the plain-super lifecycle no-op
                            // in super_call_with_lifecycle_noop.
                            (hook, Some(Value::Class(_)))
                                if matches!(hook,
                                    "included" | "extended" | "prepended" | "inherited"
                                    | "method_added" | "method_removed" | "method_undefined"
                                    | "singleton_method_added" | "singleton_method_removed"
                                    | "singleton_method_undefined" | "const_added") =>
                            {
                                self.stack.push(Value::Nil);
                            }
                            (_, cur) => {
                                // CRuby: `super(*a, &b)` with no superclass
                                // method invokes `method_missing(name, *a,
                                // &b)` on self before raising. Sinatra's
                                // Delegator proxies a delegated method
                                // (`super if respond_to?`) to a mixin's
                                // method_missing this way. The no-block
                                // super path (super_call_with_lifecycle_noop)
                                // already does this; this is its block-form
                                // twin.
                                let recv = cur.unwrap_or(Value::Nil);
                                if !self.try_method_missing(&recv, name_id, args, block_id)? {
                                    return Err(trap);
                                }
                            }
                        }
                    }
                }
            }
            Op::Super(name_id, argc) => {
                let split = self.stack.len() - argc as usize;
                let args: Vec<Value> = self.stack.drain(split..).collect();
                self.super_call_with_lifecycle_noop(name_id, args)?;
            }
            Op::CreateBlock(p_idx, param_start, n_params, rest_slot_raw, kw_rest_slot_raw)
            | Op::CreateLambda(p_idx, param_start, n_params, rest_slot_raw, kw_rest_slot_raw) => {
                // CreateLambda flags the resulting Proc as a lambda
                // (`Proc#lambda?`); CreateBlock is an ordinary block.
                let is_lambda = matches!(op, Op::CreateLambda(..));
                // Snapshot the surrounding frame's captured locals
                // (shared Rc with subsequent invocations of this
                // block) and self before any mutable borrow of
                // `self`, then allocate the BlockHandle into the
                // heap. The stack value is a plain `ObjId`.
                //
                // Per-iteration closure capture fairness is
                // enforced at `invoke_block` (not here): each
                // invocation gets a FRESH locals Vec cloned from
                // `captured`, with a write-back at frame pop. Inner
                // closures captured during that invocation thus
                // hold a Rc to the per-invocation Vec, isolated
                // from subsequent iterations.
                let (captured, self_val, captured_is_method_scope, captured_yield_block) = {
                    let f = self.frames.last().expect("ICE: CreateBlock no frame");
                    let captured = match &f.locals {
                        crate::vm::Locals::Shared(rc) => rc.clone(),
                        // A proto containing Op::CreateBlock is never
                        // Stack-eligible (compile-time escape analysis)
                        // — reaching here would mean the analysis and
                        // the frame disagree.
                        crate::vm::Locals::Stack(_) => {
                            unreachable!("ICE: CreateBlock in a Locals::Stack frame")
                        }
                    };
                    // `yield` inside this block resolves to the block of
                    // the lexically-enclosing METHOD. Capture it now so an
                    // ESCAPED closure (whose defining method has already
                    // returned) can still yield: a method/class-body/
                    // toplevel creating frame contributes its own
                    // `block_arg`; a block creating frame propagates the
                    // binding it already holds (nested blocks share the
                    // enclosing method's block). See `Op::Yield`.
                    let captured_yield_block = if f.is_block {
                        f.captured_yield_block
                    } else {
                        // A method/class-body/toplevel creating frame
                        // contributes its own `block_arg`. A
                        // `define_method` body has `block_arg: None`
                        // (its caller's block is hidden) but may carry
                        // a LEXICALLY-captured yield-block — fall back
                        // to it so a nested block inside the
                        // define_method body (`[..].map { yield }`)
                        // still reaches the enclosing method's block.
                        f.block_arg.or(f.captured_yield_block)
                    };
                    // A non-block creating frame (method / class body /
                    // toplevel) means `captured` is a real outer scope
                    // → the block's outer-write share path is sound.
                    (captured, f.self_val.clone(), !f.is_block, captured_yield_block)
                };
                // Capture the lexical class for `@@cvar` resolution. For
                // a block created inside another block this returns the
                // outer block's captured lexical class (nested blocks
                // share the enclosing cref); inside a method / class body
                // it derives from that frame's self. Computed before the
                // alloc's mutable borrow.
                let lexical_cvar_class = self.surrounding_class();
                let rest_slot = if rest_slot_raw == u16::MAX { None } else { Some(rest_slot_raw) };
                let kw_rest_slot = if kw_rest_slot_raw == u16::MAX { None } else { Some(kw_rest_slot_raw) };
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Block(BlockHandle {
                    proto_idx: p_idx as usize,
                    captured,
                    self_val,
                    lexical_cvar_class,
                    param_start,
                    n_params,
                    rest_slot,
                    kw_rest_slot,
                    captured_is_method_scope,
                    captured_yield_block,
                    is_lambda,
                }));
                self.stack.push(Value::Block(id));
            }
            Op::Yield(_) | Op::ApplyYield => {
                // `yield` resolves to the block of the enclosing
                // METHOD, not the current frame. When yield runs
                // inside a nested block (e.g.
                // `def f; xs.each { |x| yield x }; end`), the
                // current frame is the `each` block; we must walk
                // through `is_block` frames to find the nearest
                // method frame and pick up ITS block_arg.
                //
                // CRuby implements the same lookup via the cfp
                // chain (vm_get_yield_method_cfp). Without the
                // walk, every block-wrapped yield raises
                // "no block given (yield)" — broke ERB's scanner.
                //
                // **ADR 0024 Phase A.1**: Op::Yield now drives
                // the block SYNCHRONOUSLY (recursive
                // `dispatch_until`) so the block's `break val`
                // unwinds back to the yielding method and
                // returns val from it — matching CRuby
                // semantics. v6's fire-and-forget pattern set
                // `break_signaled` but only Rust-level iter
                // drivers (`step_block`) observed it; a Ruby
                // `def f; yield; end; f { break }` was
                // historically broken (infinite loop / silent
                // continue depending on caller shape).
                //
                // The synchronous flow:
                //   1. Locate yielding method's frame index by
                //      LEXICAL scope (ADR 0024 Phase A.7). Blocks
                //      share their captured `locals` Rc with the
                //      defining scope (transitively through
                //      nested blocks); the topmost !is_block
                //      frame whose `locals` Rc-pointer matches
                //      the current top frame's `locals` IS the
                //      lexical owner. Pre-A.7 used "nearest non-
                //      block frame" — incorrect for shapes like
                //      `def f; g { yield }; end; def g; yield;
                //      end; f { ... }` where the inner yield
                //      bound to g (dynamic neighbour) instead of
                //      f (lexical owner) and recursively
                //      re-invoked g's block_arg.
                //   2. Mark the yielding-method frame's
                //      `pending_yield = true` (so a Fiber yield
                //      mid-block can resume correctly).
                //   3. Enter `YieldDepthGuard` (bounded recursion
                //      via `Config::max_yield_recursion`; Drop
                //      decrements panic-safely).
                //   4. `invoke_block` pushes block frame.
                //   5. Inner `dispatch_until(pre_frames)` drives
                //      the block to completion.
                //   6. On normal return: block value is on
                //      stack, IP past Op::Yield; clear
                //      pending_yield; continue.
                //   7. On `break_signaled`: pop value, walk
                //      frames down to + including yielding
                //      method, push value as method's return,
                //      clear break_signaled.
                //   8. On `method_return` / `fiber_yield_pending`:
                //      leave the signal set, let the outer
                //      dispatch loop / Fiber driver handle.
                // Phase A.7: lexical lookup via locals Rc-pointer
                // identity. With the per-invocation block-locals
                // fix (each `invoke_block` installs a fresh
                // locals Vec, retaining the original `captured`
                // Rc on `block_writeback`), the top frame's
                // `locals` is no longer the same Rc the lexical
                // owner uses. `find_lexical_owner_frame` walks
                // the writeback chain to bridge that — each
                // block frame's writeback points one scope
                // outward until a method frame is found.
                // (`lexical_owner_of_top` shortcuts a non-block top
                // frame to itself — required for Locals::Stack method
                // frames, identical behaviour for Shared ones.)
                // Primary: the lexical owner method is still on the
                // stack — read its `block_arg` directly (the common
                // `def f; xs.each { yield }; end` synchronous case),
                // and use its frame index for the pending_yield /
                // break bookkeeping below.
                //
                // Fallback (ESCAPED CLOSURE): the block executing the
                // yield outlived its defining method (`def m(&blk);
                // ->(){ yield }; end` returned, the lambda is called
                // later). The live-frame walk then finds no method
                // frame, so use the yield-block captured at the
                // block's creation and threaded onto its frame
                // (`captured_yield_block`, propagated through nested
                // blocks); CRuby keeps the same binding alive via the
                // closure's captured cref. With no live yielding
                // method, the yield site for break / Fiber-resume
                // bookkeeping is the top (block) frame itself.
                let owner = self.lexical_owner_of_top();
                let (block, yielding_idx) = match owner
                    .and_then(|idx| self.frames[idx].block_arg.map(|b| (b, idx)))
                {
                    Some(pair) => pair,
                    None => match self.frames.last().and_then(|f| f.captured_yield_block) {
                        Some(b) => (b, self.frames.len() - 1),
                        None => return Err(self.trap(RubyError::RuntimeError {
                            msg: "no block given (yield)".to_string(),
                        })),
                    },
                };
                // Static argc for `Op::Yield(n)`; for `Op::ApplyYield`
                // (`yield(*x)`), pop the combined args Array and expand
                // its elements onto the stack (mirrors `Op::ApplyCall`),
                // yielding the dynamic count.
                let argc = match op {
                    Op::Yield(n) => n as usize,
                    Op::ApplyYield => {
                        let arr_val = match self.stack.pop() {
                            Some(v) => v,
                            None => unreachable!("ICE: ApplyYield without args array"),
                        };
                        let arr_id = match arr_val {
                            Value::Array(id) => id,
                            other => {
                                return Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "no implicit conversion of {} into Array",
                                        other.type_name()
                                    ),
                                }));
                            }
                        };
                        let mut g = crate::vm::PinGuard::new(self);
                        g.pin(Value::Array(arr_id));
                        let elems: Vec<Value> = g.vm.heap.array(arr_id).clone();
                        let n = elems.len();
                        for e in elems {
                            g.vm.stack.push(e);
                        }
                        drop(g);
                        n
                    }
                    _ => unreachable!("yield arm only matches Yield | ApplyYield"),
                };
                let split = self.stack.len() - argc;
                let args: Vec<Value> = self.stack.drain(split..).collect();

                let pre_frames = self.frames.len();

                // Bounded recursion guard FIRST so we never
                // mark pending_yield without a matching guard
                // (if enter fails, no cleanup needed).
                let yguard = crate::vm::YieldDepthGuard::enter(self)?;
                yguard.vm.frames[yielding_idx].pending_yield = true;

                // Push block frame + drive to completion.
                if let Err(trap) = yguard.vm.invoke_block(block, args) {
                    yguard.vm.frames[yielding_idx].pending_yield = false;
                    return Err(trap);
                }
                if let Err(trap) = yguard.vm.dispatch_until(pre_frames) {
                    // Block raised; clear pending_yield and
                    // propagate so rescue can catch.
                    if yielding_idx < yguard.vm.frames.len() {
                        yguard.vm.frames[yielding_idx].pending_yield = false;
                    }
                    return Err(trap);
                }

                // dispatch_until returned. Determine why:
                if yguard.vm.method_return.is_some() {
                    // return-from-block: leave method_return
                    // set; outer dispatch loop handles unwind.
                    if yielding_idx < yguard.vm.frames.len() {
                        yguard.vm.frames[yielding_idx].pending_yield = false;
                    }
                    // Guard drops on return → decrements counter.
                    return Ok(true);
                }
                #[cfg(feature = "_fiber")]
                if yguard.vm.fiber_yield_pending.is_some() {
                    // Fiber.yield mid-block. DO NOT clear
                    // pending_yield — it stays set so the
                    // Fiber's stashed FiberSnapshot captures
                    // the in-progress state. On resume the
                    // block continues; eventually it returns
                    // normally OR breaks. We can't see that
                    // far ahead here — let the outer Fiber
                    // driver propagate up; on resume the
                    // dispatch loop re-enters this same
                    // dispatch_until at a level that observes
                    // the post-block state.
                    //
                    // Actually subtle: this `dispatch_until`
                    // call returned. The outer caller is the
                    // dispatch_until that's driving the Fiber.
                    // It also sees fiber_yield_pending and
                    // returns. resume_fiber stashes; later
                    // re-enters dispatch_until. The dispatch
                    // loop will pick up at the block's IP
                    // (top of stack). Block completes,
                    // Op::Return pops it, control resumes at
                    // the yielding-method's IP past Op::Yield —
                    // which is the NEXT op, NOT this same Op::Yield.
                    //
                    // The IP for `self.frames[yielding_idx].ip`
                    // was advanced BEFORE we entered the match
                    // arm (top of the dispatch loop). So past-yield
                    // is already the IP. On resume, dispatch fetches
                    // that next op, not Op::Yield. The synchronous
                    // wrapper's break-check is therefore SKIPPED on
                    // the resume path.
                    //
                    // ADR 0024 Phase A.8: the resume-side recovery
                    // lives in `dispatch_until_inner` /
                    // `dispatch` as a top-of-loop check. When the
                    // resumed block runs `break`, `break_signaled`
                    // gets set and the block frame pops naturally;
                    // the dispatch loop then observes
                    // `break_signaled && top_frame.pending_yield`
                    // and routes the value through
                    // `begin_method_break` — same A.4/A.5
                    // ensure-aware unwind machinery, just driven
                    // from a different entry point because the
                    // original Op::Yield Rust wrapper is gone.
                    return Ok(true);
                }

                // Normal block return or block-break.
                let block_return_value = yguard.vm.stack.pop().unwrap_or(Value::Nil);
                yguard.vm.frames[yielding_idx].pending_yield = false;

                if yguard.vm.break_signaled {
                    // Two cases:
                    //
                    // (a) Current frame IS the yielding method
                    //     (yielding_idx == pre_frames - 1).
                    //     Example: `def f; yield; end; f { break }`.
                    //     No Rust iter driver sits between us and
                    //     `f`, so this wrapper is solely responsible
                    //     for unwinding. Pop the yielding method,
                    //     push the break value as its return — the
                    //     new behavior Phase A.1 adds.
                    //
                    // (b) Yielding method is deeper
                    //     (yielding_idx < pre_frames - 1).
                    //     Example: `def each; 10.times { yield };
                    //     end; obj.each { break }`. A Rust iter
                    //     driver (`Int#times`'s `step_block` loop)
                    //     sits between yield and each. The legacy
                    //     pre-A.1 path already handles this: leave
                    //     `break_signaled` set so the enclosing
                    //     `step_block` returns `BlockStep::Break`
                    //     and `Int#times` aborts naturally,
                    //     propagating the break through `each` as
                    //     its return value. Eating the signal here
                    //     would strand the Rust driver mid-loop.
                    if yielding_idx == pre_frames - 1 {
                        yguard.vm.break_signaled = false;
                        yguard.vm.sync_control_signals();
                        // ADR 0024 Phase A.4: walk the yielding
                        // method's `is_ensure` rescue handlers
                        // before the frame returns. After
                        // dispatch_until returned, frames.len() ==
                        // pre_frames and the topmost frame IS the
                        // yielding method (case a), so
                        // begin_method_break drives the ensure
                        // walk on that frame directly. When no
                        // ensures remain, it pops the frame and
                        // pushes the break value as the method's
                        // return.
                        //
                        // Toplevel case: if the yielding method
                        // is the bottom frame, the walk pops it
                        // and pushes the value as the script's
                        // result. dispatch loop terminates on
                        // empty frames — drop guard FIRST so the
                        // recursion counter decrements before we
                        // bail.
                        let was_toplevel = yielding_idx == 0;
                        yguard.vm.begin_method_break(block_return_value, yielding_idx)?;
                        drop(yguard);
                        if was_toplevel && self.frames.is_empty() {
                            return Ok(false);
                        }
                        return Ok(true);
                    }
                    // Case (b) — ADR 0024 Phase A.5: yielding
                    // method is deeper than current frame's
                    // direct parent. A Rust iter driver (e.g.
                    // `Int#times`'s `step_block` loop) sits
                    // between us and the yielding method.
                    //
                    // Park the break in `pending_method_break`
                    // with `target_frame_idx = yielding_idx` so
                    // the dispatch loop top-of-iteration check
                    // picks it up after the Rust driver returns
                    // and control re-enters bytecode in a frame
                    // above the target. continue_method_break
                    // then pops intermediate method frames
                    // (running their ensures on the way) until
                    // it reaches the yielding method, walks
                    // its ensures, and lands the break value.
                    //
                    // Also push the break value + leave
                    // break_signaled set so the EXISTING Rust
                    // iter driver protocol (step_block → BlockStep
                    // ::Break → driver returns the value) keeps
                    // working end-to-end. Without this, drivers
                    // would treat the block return as a normal
                    // value and keep iterating.
                    //
                    // Phase A.9: don't overwrite an
                    // already-pending break. Multi-method-frame
                    // shapes like `def f; g { |x| yield x }; end;
                    // def g; xs.each { |x| yield x }; end;
                    // f { break }` have several nested Op::Yield
                    // wrappers each running case (b) on the way
                    // out. The INNERMOST one (lexically closest to
                    // the breaking block) has the right target —
                    // outer wrappers should leave that target
                    // alone and just propagate.
                    if yguard.vm.pending_method_break.is_none() {
                        yguard.vm.pending_method_break = Some(crate::vm::MethodBreak {
                            value: block_return_value.clone(),
                            target_frame_idx: yielding_idx,
                            suspended: false,
                        });
                        yguard.vm.sync_control_signals();
                    }
                    yguard.vm.stack.push(block_return_value);
                    drop(yguard);
                    return Ok(true);
                }
                // Normal block return — push the block's value
                // as the yield expression's value.
                yguard.vm.stack.push(block_return_value);
                // Guard drops on fall-through → decrements counter.
            }
            Op::DefMethod(name_id, p_idx) => {
                let proto = &self.protos[p_idx as usize];
                // Capture the defining class (top of class_stack
                // when we're inside `class Foo; def bar; end; end`)
                // so `super` later starts its lookup from the
                // right place. `None` for toplevel defs.
                // Stored as Weak — see Method.defining_class docs.
                // When `class_stack.last()` is an eigenclass shell
                // (from `cls.singleton_class.class_eval { ... }`),
                // `install_method` redirects the install into
                // `cls.singleton_methods`. `defining_class` has to
                // point at the same `cls` so `super_lookup` walks
                // the right ancestor chain — using the shell would
                // miss every node in the receiver's superclass
                // chain. (Code-review #253 round 1 #1.)
                let defining_class = self.class_stack.last().map(|c| Rc::downgrade(&c.effective_install_class()));
                let vis = self.class_visibility_stack.last().copied().unwrap_or(Visibility::Public);
                let params = proto.params.clone();
                let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                let m = Rc::new(Method {
                    params,
                    proto_idx: p_idx as usize,
                    fixed_arity,
                    defining_class,
                    visibility: std::cell::Cell::new(vis),
                    closure: None,
                builtin: None,
                original_name: Some(name_id),
                });
                if let Some(cls) = self.class_stack.last() {
                    cls.install_method(name_id, m.clone());
                    // `module_function` (bare-form) dual-install:
                    // after `M.module_function` in a body, every
                    // subsequent `def name` ALSO installs a public
                    // clone on `cls.singleton_methods` so
                    // `M.name(...)` resolves at call time. The
                    // instance entry above is already stamped
                    // Private by the visibility-stack flip the
                    // module_function arm performs; the singleton
                    // copy carries Public + a fresh
                    // `visibility: Cell` so flipping one doesn't
                    // alias to the other (matches the symbol-arg
                    // arm's per-Method clone in vm/dispatch.rs).
                    // Anchored at the class itself for `super` /
                    // `Method#owner` consistency.
                    let mf_active = self.module_function_active_stack
                        .last()
                        .copied()
                        .unwrap_or(false);
                    if mf_active {
                        let singleton_copy = Rc::new(Method {
                            params: m.params.clone(),
                            proto_idx: m.proto_idx,
                            fixed_arity: m.fixed_arity,
                            defining_class: Some(Rc::downgrade(cls)),
                            visibility: std::cell::Cell::new(Visibility::Public),
                            closure: m.closure.clone(),
                            original_name: m.original_name,
                            builtin: m.builtin.clone(),
                        });
                        cls.singleton_methods.borrow_mut().insert(name_id, singleton_copy);
                    }
                }
                else { self.toplevel_methods.insert(name_id, m); }
                // Conservatively invalidate the inline cache — any previous
                // cache entry could in theory be made stale by this definition.
                self.method_gen = self.method_gen.wrapping_add(1);
                // `Module#method_added(name)` — fires after the
                // install lands. CRuby semantics: Rails / RSpec /
                // many DSLs use this to auto-wrap freshly-defined
                // methods. Toplevel defs are skipped today —
                // CRuby fires `Object.method_added` there, but the
                // hook needs a Class receiver and toplevel installs
                // into `toplevel_methods` instead.
                if let Some(cls) = self.class_stack.last().cloned() {
                    self.fire_method_lifecycle_hook(&cls, "method_added", name_id)?;
                }
                // `def foo` evaluates to the method name Symbol (CRuby) —
                // makes `private def foo` / `module_function def foo` work
                // (the modifier receives `:foo`).
                self.stack.push(Value::Sym(name_id));
            }
            Op::DefSingletonMethod(name_id, p_idx) => {
                // `def self.foo` inside a class body. Installs `foo`
                // on the surrounding class's `singleton_methods`
                // table, dispatched against `Value::Class(c)`
                // receivers in `do_call`. Outside a class body
                // (toplevel singleton has no well-defined target)
                // we fall back to installing on `toplevel_methods`.
                let proto = &self.protos[p_idx as usize];
                // Singleton defs are ALWAYS Public — CRuby's
                // visibility modes (`private` / `protected` /
                // `module_function`) only apply to instance
                // `def`s; `def self.x` after a bare
                // `module_function` stays callable (rack
                // utils.rb defines `def self.param_depth_limit`
                // below its `module_function` line). Demoting a
                // singleton def needs an explicit
                // `private_class_method`.
                let vis = Visibility::Public;
                let params = proto.params.clone();
                let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                // `defining_class` MUST match wherever the method
                // physically lives — `super_lookup` finds it in the
                // receiver's ancestry (`Rc::ptr_eq`) and resumes
                // after it. Compute the install target FIRST, then
                // derive `defining_class` from it. Three cases:
                //   - inside a class body → that class's
                //     `singleton_methods` (when `cls` IS an eigenclass
                //     shell from `singleton_class.class_eval`, the
                //     method lives on the shell's own singleton table
                //     and `defining_class` is the shell, not the real
                //     class — keep `cls` as-is, code-review #253);
                //   - no class body, runtime self is a Class →
                //     `def self.x` inside a method, install on the
                //     class's singleton table;
                //   - no class body, runtime self is an Object →
                //     `def self.x` inside a method/block body (e.g.
                //     minitest's `it` block does `def self.env;
                //     super; end`): install on the object's eigenclass
                //     `methods` table. PRE-FIX BUG: `defining_class`
                //     was taken from `class_stack.last()` while the
                //     method landed on the eigenclass, so `super` from
                //     the singleton method couldn't locate its start
                //     point → spurious "no superclass method".
                enum SingInstall { Singleton(Rc<Class>), Eigen(Rc<Class>), Toplevel }
                let install = if let Some(cls) = self.class_stack.last().cloned() {
                    SingInstall::Singleton(cls)
                } else {
                    match self.frames.last().map(|f| f.self_val.clone()) {
                        Some(Value::Class(c)) => SingInstall::Singleton(c.effective_install_class()),
                        Some(Value::Object(oid)) => {
                            SingInstall::Eigen(self.heap.ensure_singleton_class(oid))
                        }
                        _ => SingInstall::Toplevel,
                    }
                };
                let defining_class = match &install {
                    SingInstall::Singleton(c) | SingInstall::Eigen(c) => Some(Rc::downgrade(c)),
                    SingInstall::Toplevel => None,
                };
                let m = Rc::new(Method {
                    params,
                    proto_idx: p_idx as usize,
                    fixed_arity,
                    defining_class,
                    visibility: std::cell::Cell::new(vis),
                    closure: None,
                builtin: None,
                original_name: Some(name_id),
                });
                match &install {
                    SingInstall::Singleton(c) => {
                        c.singleton_methods.borrow_mut().insert(name_id, m);
                    }
                    SingInstall::Eigen(c) => {
                        c.methods.borrow_mut().insert(name_id, m);
                    }
                    SingInstall::Toplevel => {
                        self.toplevel_methods.insert(name_id, m);
                    }
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                // `singleton_method_added(name)` fires on the
                // surrounding class — `def self.foo` inside
                // `class C` invokes `C.singleton_method_added(:foo)`
                // if the user defined the hook. Toplevel
                // `def self.foo` is skipped (no Class receiver to
                // anchor the hook against).
                if let Some(cls) = self.class_stack.last().cloned() {
                    self.fire_singleton_method_lifecycle_hook(
                        Value::Class(cls),
                        "singleton_method_added",
                        name_id,
                    )?;
                }
                // `def self.foo` also evaluates to `:foo`.
                self.stack.push(Value::Sym(name_id));
            }
            Op::DefObjectSingletonMethod(name_id, p_idx) => {
                // `def obj.name; ...; end` (non-`self` receiver)
                // — instance-level singleton install. Receiver
                // was pushed by the compiler immediately before
                // this op (see `compile_expr`'s Def arm).
                let recv = self.stack.pop()
                    .expect("ICE: DefObjectSingletonMethod stack underflow");
                // `def Foo.bar` / `def Foo::bar` where the receiver is
                // a Class constant — define a CLASS method (singleton
                // method on Foo), exactly like `def self.bar` inside
                // Foo's body. rexml's `def SourceFactory::create_from(
                // arg); …; end` is the motivating case. Eigenclass
                // shells redirect to the real class via
                // `singleton_target`.
                if let Value::Class(cls) = &recv {
                    let proto = &self.protos[p_idx as usize];
                    let params = proto.params.clone();
                    let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                    let anchor = cls.effective_install_class();
                    let m = Rc::new(Method {
                        params,
                        proto_idx: p_idx as usize,
                        fixed_arity,
                        defining_class: Some(Rc::downgrade(&anchor)),
                        visibility: std::cell::Cell::new(Visibility::Public),
                        closure: None,
                        builtin: None,
                        original_name: Some(name_id),
                    });
                    anchor.singleton_methods.borrow_mut().insert(name_id, m);
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.fire_singleton_method_lifecycle_hook(
                        Value::Class(anchor),
                        "singleton_method_added",
                        name_id,
                    )?;
                    // `def obj.foo` also evaluates to `:foo`.
                    self.stack.push(Value::Sym(name_id));
                    return Ok(true);
                }
                // Hash receivers get a real per-instance eigenclass
                // (the openstruct-over-Hash pattern: `def h.method_missing`).
                if let Value::Hash(hid) = recv {
                    let sc = self.ensure_hash_singleton(hid);
                    let proto = &self.protos[p_idx as usize];
                    let params = proto.params.clone();
                    let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                    let m = Rc::new(Method {
                        params,
                        proto_idx: p_idx as usize,
                        fixed_arity,
                        defining_class: Some(Rc::downgrade(&sc)),
                        visibility: std::cell::Cell::new(Visibility::Public),
                        closure: None,
                        builtin: None,
                        original_name: Some(name_id),
                    });
                    sc.methods.borrow_mut().insert(name_id, m);
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.stack.push(Value::Sym(name_id));
                    return Ok(true);
                }
                // String receivers: per-instance eigenclass via the
                // str_singletons side-table (minitest's stub /
                // assertions' diff-test `def s.pretty_print`).
                if let Value::Str(s) = &recv {
                    let s = s.clone();
                    let sc = self.ensure_str_singleton(&s);
                    let proto = &self.protos[p_idx as usize];
                    let params = proto.params.clone();
                    let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                    let m = Rc::new(Method {
                        params,
                        proto_idx: p_idx as usize,
                        fixed_arity,
                        defining_class: Some(Rc::downgrade(&sc)),
                        visibility: std::cell::Cell::new(Visibility::Public),
                        closure: None,
                        builtin: None,
                        original_name: Some(name_id),
                    });
                    sc.methods.borrow_mut().insert(name_id, m);
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.stack.push(Value::Sym(name_id));
                    return Ok(true);
                }
                // Array / Proc receivers: per-instance eigenclass via
                // the heap_singletons side-table (twin of the String
                // arm above). rack's Lock does `def body.close` on an
                // Array body.
                if matches!(&recv, Value::Array(_) | Value::Block(_)) {
                    let sc = self.ensure_heap_singleton(&recv);
                    let proto = &self.protos[p_idx as usize];
                    let params = proto.params.clone();
                    let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                    let m = Rc::new(Method {
                        params,
                        proto_idx: p_idx as usize,
                        fixed_arity,
                        defining_class: Some(Rc::downgrade(&sc)),
                        visibility: std::cell::Cell::new(Visibility::Public),
                        closure: None,
                        builtin: None,
                        original_name: Some(name_id),
                    });
                    sc.methods.borrow_mut().insert(name_id, m);
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.stack.push(Value::Sym(name_id));
                    return Ok(true);
                }
                let obj_id = match recv {
                    Value::Object(id) => id,
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "can't define singleton method on {} (only user-class instances are supported)",
                                other.type_name(),
                            ),
                        }));
                    }
                };
                // Lazily allocate the eigenclass — the receiver
                // pays nothing for objects that never get a
                // singleton method. Repeated `def obj.x` /
                // `def obj.y` on the same object reuse the same
                // singleton class.
                let sc = self.heap.ensure_singleton_class(obj_id);
                let proto = &self.protos[p_idx as usize];
                // `defining_class` points at the eigenclass so
                // `super` from inside walks the eigenclass's
                // superclass chain (= original class), matching
                // CRuby's "module of definition" rule. Stored
                // as `Weak` so the (sc ↔ Method) cycle doesn't
                // pin the eigenclass past the receiver's
                // lifetime — see PR #31 review for the analysis.
                let params = proto.params.clone();
                let fixed_arity = Self::fixed_arity_for_proto(proto, params.len());
                let m = Rc::new(Method {
                    params,
                    proto_idx: p_idx as usize,
                    fixed_arity,
                    defining_class: Some(Rc::downgrade(&sc)),
                    visibility: std::cell::Cell::new(Visibility::Public),
                    closure: None,
                builtin: None,
                original_name: Some(name_id),
                });
                sc.methods.borrow_mut().insert(name_id, m);
                self.method_gen = self.method_gen.wrapping_add(1);
                // `obj.singleton_method_added(:name)` fires after
                // `def obj.foo` lands. Hook lookup walks
                // `class_of(obj)` (the receiver's class), matching
                // CRuby's instance-method-on-class definition.
                self.fire_singleton_method_lifecycle_hook(
                    Value::Object(obj_id),
                    "singleton_method_added",
                    name_id,
                )?;
                // `def obj.foo` evaluates to `:foo`.
                self.stack.push(Value::Sym(name_id));
            }
            Op::AliasMethod(new_id, old_id) => {
                // Resolve `old` along the surrounding class's ancestor
                // chain (or toplevel) and re-insert the same Rc<Method>
                // under `new` in the *current* class. We share the Rc
                // — alias is intentionally semantically identical to
                // the original, including its `defining_class` (so
                // `super` from inside the aliased call walks from the
                // original's super, matching CRuby's "module of
                // definition" rule for aliases).
                //
                // The walk lets `class Child < Parent; alias_method :x,
                // :parent_method; end` work: the source method lives
                // on Parent, the alias name `x` lands on Child.
                // When `class_stack.last()` is an eigenclass shell,
                // `def`/`define_method` redirects the install into
                // the real class's `singleton_methods` — so the
                // source-method lookup for `alias_method` has to
                // walk that same chain via
                // `lookup_class_singleton_method`. Otherwise
                // aliasing a just-defined singleton method inside
                // `singleton_class.class_eval` would miss and
                // raise NameError. (Code-review #253 round 2 #1.)
                let existing = if let Some(cls) = self.class_stack.last() {
                    if let Some(real) = cls.singleton_target.borrow().as_ref().and_then(std::rc::Weak::upgrade) {
                        self.lookup_class_singleton_method(&real, old_id)
                    } else {
                        self.lookup_method_uncached(cls, old_id)
                    }
                } else {
                    self.toplevel_methods.get(&old_id).cloned()
                };
                let m = match existing {
                    Some(m) => m,
                    None => {
                        // Source name not in the user-Method table.
                        // Before raising NameError, check whether the
                        // surrounding class's primitive whitelist
                        // responds to it (`Symbol#name`, `Integer#+`,
                        // ...). If so, synthesise a forwarder Method
                        // whose body is `LoadSelf; LoadLocal(0);
                        // ApplyCall(old_id, ...); Return` — i.e.
                        // call the primitive on `self` with any
                        // forwarded args. This is what lets the
                        // msgpack-ruby `lib/msgpack/symbol.rb`
                        // `alias_method :to_msgpack_ext, :name`
                        // shape work without rewriting upstream.
                        // Variadic forwarding via a rest param so
                        // arities other than 0 also forward
                        // correctly.
                        let cls_ref = self.class_stack.last().cloned();
                        // Eigenclass-shell case: probe the underlying
                        // real class for both the primitive-sentinel
                        // whitelist (e.g. aliasing `:name` works
                        // because Class.name is in the whitelist
                        // even though "Foo" isn't a primitive class
                        // name) AND the Class-method whitelist via
                        // `responds_to(Value::Class(real), …)`. The
                        // install still routes through the shell's
                        // `install_method`, which redirects into
                        // `real.singleton_methods`. (Code-review
                        // #253 round 3 #1.)
                        let probe_cls = cls_ref.as_ref().and_then(|c| {
                            c.singleton_target
                                .borrow()
                                .as_ref()
                                .and_then(std::rc::Weak::upgrade)
                        });
                        let shell_class_whitelist_hit = probe_cls
                            .as_ref()
                            .map(|real| self.responds_to(&Value::Class(real.clone()), old_id, true))
                            .unwrap_or(false);
                        if let Some(cls) = &cls_ref {
                            // Walk the superclass chain looking for
                            // a primitive class that responds to the
                            // source method. `class P < Hash;
                            // alias_method :a, :to_h; end`: cls=P
                            // has name "P" (not a primitive), but
                            // Hash is in the primitive whitelist
                            // and responds to `to_h`. Without the
                            // walk, the immediate-class probe at
                            // primitive_class_responds_to(&cls.name,
                            // ...) misses and we wrongly raise
                            // NameError. rack-3.1.10's
                            // lib/rack/query_parser.rb:197 needs
                            // exactly this:
                            //   class Params < Hash
                            //     alias_method :to_params_hash, :to_h
                            //   end
                            // (TRY_RUNS pass-10 layer #11.)
                            // Universal arms (respond_to? / == /
                            // inspect / ...) answer on EVERY
                            // receiver, so aliasing one from any
                            // user class synthesises the same
                            // forwarder (mock.rb's
                            // `alias __respond_to? respond_to?`).
                            let old_name_str = self.interner.resolve(old_id).to_string();
                            let mut primitive_hit =
                                crate::vm::Vm::universal_arm_name(&old_name_str)
                                || crate::vm::Vm::universal_kernel_private(&old_name_str)
                                || crate::vm::Vm::UNIVERSAL_OBJECT_METHODS
                                    .contains(&old_name_str.as_str());
                            // Rc-pointer visited set defends the
                            // walker against an adversarial cyclic
                            // superclass graph (`A.superclass = B;
                            // B.superclass = A`) that cext or
                            // direct manipulation can construct —
                            // CRuby rejects the cycle at
                            // insertion, rubyrs doesn't enforce
                            // that today. Mirrors the
                            // `Rc::as_ptr` guard
                            // `lookup_method_uncached` (lookup.rs)
                            // already uses for the same shape on
                            // includes/prepends.
                            // (Code-review #320 round 1.)
                            let mut visited: std::collections::HashSet<*const crate::value::Class> =
                                std::collections::HashSet::new();
                            let mut walker: Option<Rc<Class>> = Some(cls.clone());
                            while let Some(c) = walker {
                                if !visited.insert(Rc::as_ptr(&c)) { break; }
                                if self.primitive_class_responds_to(&c.name, old_id) {
                                    primitive_hit = true;
                                    break;
                                }
                                walker = c.superclass.borrow().clone();
                            }
                            if primitive_hit || shell_class_whitelist_hit {
                                let forwarder_cls = probe_cls.as_ref().unwrap_or(cls);
                                let synth = self.synth_primitive_forwarder(forwarder_cls, old_id);
                                cls.install_method(new_id, synth);
                                self.method_gen = self.method_gen.wrapping_add(1);
                                self.fire_method_lifecycle_hook(cls, "method_added", new_id)?;
                                self.stack.push(Value::Nil);
                                return Ok(true);
                            }
                            // Lifecycle hooks: CRuby ships real
                            // empty defaults (Class#inherited,
                            // Module#included/extended/prepended,
                            // method_added family); rubyrs fires
                            // them VM-side with no table Method, so
                            // `alias x inherited` inside a
                            // Class/Module reopen found nothing.
                            // Substitute a variadic no-op — exactly
                            // the default's behavior. minitest's
                            // with_overridden_include saves/restores
                            // Class#inherited this way.
                            if matches!(self.interner.resolve(old_id).as_ref(),
                                "inherited" | "included" | "extended" | "prepended"
                                | "method_added" | "method_removed" | "method_undefined"
                                | "singleton_method_added"
                            ) && matches!(cls.name.as_str(), "Class" | "Module" | "Object") {
                                let synth = self.synth_noop_method(cls, old_id);
                                cls.install_method(new_id, synth);
                                self.method_gen = self.method_gen.wrapping_add(1);
                                self.fire_method_lifecycle_hook(cls, "method_added", new_id)?;
                                self.stack.push(Value::Nil);
                                return Ok(true);
                            }
                        }
                        // CRuby raises NameError ("undefined method ...")
                        // when `alias_method`'s source name isn't found
                        // on the receiver's ancestor chain — not
                        // NoMethodError. NameError is the right shape:
                        // there's no value to call yet (alias is a
                        // class-body operation, not a dispatch site),
                        // so the previous `NoMethodError { recv_type:
                        // "Class" }` was misleading.
                        let name = self.interner.resolve(old_id).to_string();
                        let ctx = self.class_stack.last()
                            .map(|c| format!("class `{}'", c.name))
                            .unwrap_or_else(|| "main".to_string());
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method `{}' for {}", name, ctx),
                        }));
                    }
                };
                if let Some(cls) = self.class_stack.last() {
                    // Same eigenclass-shell redirect as Op::DefMethod:
                    // `alias_method` inside
                    // `cls.singleton_class.class_eval { alias :a :b }`
                    // should install `:a` on `cls.singleton_methods`,
                    // not the shell's instance-methods table.
                    // (Code-review #253 round 1 #8.)
                    cls.install_method(new_id, m);
                } else {
                    self.toplevel_methods.insert(new_id, m);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                // `method_added(new_id)` fires for the alias install
                // too — CRuby invokes the hook regardless of whether
                // the install came from `def` or `alias`.
                if let Some(cls) = self.class_stack.last().cloned() {
                    self.fire_method_lifecycle_hook(&cls, "method_added", new_id)?;
                }
                self.stack.push(Value::Nil);
            }
            Op::AliasSingletonMethod(new_id, old_id) => {
                // `alias new old` inside `class << X` body.
                // Mirrors Op::AliasMethod's shape but resolves
                // `old` via `lookup_class_singleton_method` (walks
                // the surrounding class's singleton_methods chain
                // including its superclass chain) and installs into
                // `singleton_methods`, not `methods`. Outside a
                // class body, falls back to toplevel_methods like
                // the regular alias op — toplevel `class << X` is
                // legal but rarely used and the surface area is
                // identical there.
                let existing = if let Some(cls) = self.class_stack.last() {
                    self.lookup_class_singleton_method(cls, old_id)
                } else {
                    self.toplevel_methods.get(&old_id).cloned()
                };
                let m = match existing {
                    Some(m) => Some(m),
                    None => {
                        // Built-in `Class` method fallback: when
                        // `old_id` isn't a user-defined singleton
                        // method, check whether the surrounding
                        // class advertises it via its primitive
                        // method set (`new` / `name` / `to_s` /
                        // `ancestors` / ... — the same set
                        // lookup.rs's `Value::Class(_)` respond_to
                        // whitelist exposes). If so, synthesise a
                        // forwarder Method whose body is
                        // `LoadSelf; LoadLocal(0); ApplyCall(old_id);
                        // Return` — same shape as Op::AliasMethod's
                        // primitive forwarder. Mirrors the
                        // "msgpack-ruby `alias_method :to_msgpack_ext,
                        // :name`" pattern (PR #182 era) but for
                        // singleton/class methods. Motivating case:
                        // sinatra/base.rb:1659 `class << self;
                        // alias new! new unless method_defined?
                        // :new!; end` — `:new` is `Class#new`, not
                        // a user singleton, so the original lookup
                        // returned None and the alias raised
                        // NameError at load time.
                        // PR #218 (if-modifier) closed the guard
                        // surface; this PR closes the alias-to-
                        // builtin surface uncovered behind it.
                        //
                        // Surface boundary: this fallback only fires
                        // when `class_stack.last()` is Some (a class
                        // body context is active). The arm's outer
                        // comment notes "toplevel `class << X` is
                        // legal but rarely used"; that path falls
                        // through `existing` to `toplevel_methods`
                        // for user-defined names but cannot reach
                        // the synth-forwarder fallback here. Aliasing
                        // a built-in Class method (e.g. `alias new!
                        // new`) at TOPLEVEL `class << X; ...; end`
                        // therefore still raises NameError. The
                        // motivating case (sinatra/base.rb:1659) is
                        // nested, so the asymmetry doesn't surface
                        // — flagged for a future symmetric fix if
                        // someone needs it. PR #229 code-review #3.
                        let cls_ref = self.class_stack.last().cloned();
                        if let Some(cls) = &cls_ref
                            && self.responds_to(&Value::Class(cls.clone()), old_id, true) {
                            // Module fence on Class-only builtins.
                            // `responds_to(Value::Class(_), :new)`
                            // returns true unconditionally (lookup.rs
                            // whitelists `:new` without an `is_module`
                            // gate — unlike `:allocate`, which IS
                            // module-fenced post PR #181). Without
                            // this fence `module M; class << self;
                            // alias mnew new; end; end` would synth a
                            // forwarder that, at call time, dispatches
                            // `Class#new` on the Module and silently
                            // produces an instance. CRuby raises
                            // NameError at the alias because `:new`
                            // isn't a method on a Module's singleton
                            // class. Mirror CRuby's NameError-at-load
                            // by refusing to synth a forwarder for
                            // `:new` on Module receivers. (No other
                            // built-in Class methods in the whitelist
                            // diverge this way — `:name`, `:to_s`,
                            // `:ancestors`, etc. work on Modules in
                            // CRuby. PR #229 code-review #1.)
                            if cls.is_module
                                && self.interner.resolve(old_id).as_ref() == "new"
                            {
                                return Err(self.trap(RubyError::NameError {
                                    msg: format!(
                                        "undefined method `new' for class `{}'",
                                        if cls.name.is_empty() { "Module" } else { &cls.name }
                                    ),
                                }));
                            }
                            let synth = self.synth_primitive_forwarder(cls, old_id);
                            cls.singleton_methods.borrow_mut().insert(new_id, synth);
                            self.method_gen = self.method_gen.wrapping_add(1);
                            self.stack.push(Value::Nil);
                            return Ok(true);
                        }
                        None
                    }
                };
                let m = match m {
                    Some(m) => m,
                    None => {
                        let name = self.interner.resolve(old_id).to_string();
                        // Use the same "class `Foo'" context wording
                        // as Op::AliasMethod's NameError so the two
                        // sites diff cleanly. (CRuby itself spells
                        // these differently in some cases but the
                        // singleton/instance distinction is rarely
                        // load-bearing in real error logs.)
                        let ctx = self.class_stack.last()
                            .map(|c| format!("class `{}'", c.name))
                            .unwrap_or_else(|| "main".to_string());
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method `{}' for {}", name, ctx),
                        }));
                    }
                };
                if let Some(cls) = self.class_stack.last() {
                    cls.singleton_methods.borrow_mut().insert(new_id, m);
                } else {
                    self.toplevel_methods.insert(new_id, m);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(Value::Nil);
            }
            Op::SingletonChainPrepend => {
                // Pop the module/class value and push it onto the
                // surrounding class's `singleton_prepends` chain.
                // The AST recogniser is purely syntactic (it matches
                // any `class << self; prepend Mod; end` regardless
                // of enclosing scope), so the install-target check
                // is enforced HERE at runtime: use
                // `class_stack.last()` when present; trap with
                // SyntaxError otherwise (toplevel / class-eval
                // contexts where there's no class on the stack).
                //
                // CRuby parity:
                // 1. The arg must be a Module — Classes (i.e.
                //    `is_module == false`) raise TypeError. Plain
                //    non-Class values too.
                // 2. Idempotency is ancestor-chain-aware, NOT just
                //    direct-vec — if `M` is already reachable
                //    transitively (e.g. via a prepended-of-prepend
                //    chain), the explicit `prepend M` is a no-op.
                //    Without this, the chain would reorder and
                //    method resolution would diverge from CRuby.
                let arg = self.stack.pop().expect("ICE: SingletonChainPrepend with empty stack");
                let src = match arg {
                    Value::Class(c) if c.is_module => c,
                    Value::Class(_) => return Err(self.trap(RubyError::TypeError {
                        msg: "wrong argument type Class (expected Module)".into(),
                    })),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Module)",
                            other.type_name(),
                        ),
                    })),
                };
                // Install target resolution: prefer the lexical
                // class body on `class_stack` (the common case —
                // `class C; class << self; prepend M; end; end`).
                // Fall back to the current frame's `self` when
                // `self` is itself a Class — that covers the
                // method-body case (`class C; def self.install!;
                // class << self; prepend M; end; end; end`), where
                // CRuby installs on C's eigenclass because `self`
                // inside `install!` is C. Only raise when neither
                // path yields a class — toplevel / instance-method
                // contexts where rubyrs doesn't model the
                // eigenclass distinctly.
                let target = self.class_stack.last().cloned().or_else(|| {
                    self.frames.last().and_then(|f| match &f.self_val {
                        Value::Class(c) => Some(c.clone()),
                        _ => None,
                    })
                });
                let target = match target {
                    Some(c) => c,
                    None => {
                        return Err(self.trap(RubyError::SyntaxError {
                            msg: "`class << self; prepend Mod; end` is not supported outside a class/module body (no singleton-class install target — main's / instance eigenclasses not modelled in rubyrs)".into(),
                        }));
                    }
                };
                // Ancestor-aware dedup: walk every module
                // already in `singleton_prepends`, recursing
                // through each one's own prepends/includes,
                // and skip insertion if `src` is reachable
                // anywhere. Matches the instance-side `prepend`
                // recogniser's `class_is_a` gate.
                if !super::lookup::singleton_chain_contains(&target, &src) {
                    target.singleton_prepends.borrow_mut().insert(0, src);
                    self.bump_const_gen();
                    self.method_gen = self.method_gen.wrapping_add(1);
                }
                self.stack.push(Value::Nil);
            }
            Op::PushClassVisibilityPublic => {
                // Open a `class << <expr>` visibility scope — bare
                // `private` / `public` / `protected` inside the
                // singleton body mutate THIS top entry rather than
                // the enclosing class body's, preventing leakage.
                // Emitted at body start by the AST translator for
                // every SingletonClassNode (receiver-independent —
                // `class << self`, `class << obj`, `class << Const`
                // all wrap their body with Push/Pop). Paired with
                // `PopClassVisibility` at body end via the body's
                // `Begin { ensure: [...] }`.
                // PR #233 code-review #1 / round 3 #1.
                self.class_visibility_stack.push(Visibility::Public);
                self.module_function_active_stack.push(false);
                self.stack.push(Value::Nil);
            }
            Op::PopClassVisibility => {
                // Close a `class << <expr>` visibility scope.
                // Paired with `PushClassVisibilityPublic` at body
                // start and emitted inside the body's
                // `Begin { ensure: [...] }` so the pop runs on
                // both normal exit and exception unwind. Balance
                // is the translator's responsibility; underflow
                // would mean a bytecode-level invariant breakage,
                // so surface it as an ICE.
                //
                // Uses an UNCONDITIONAL `assert!` (not
                // `debug_assert!`) because the project's CI runs
                // `cargo test --release`, where debug assertions
                // are disabled — a debug-only assert would let an
                // unbalanced Pop slip through automated testing
                // and silently corrupt visibility state in release
                // builds. The runtime cost of a single bounds
                // check per `class << ...; end` boundary is
                // negligible, and the alternative (silent state
                // corruption) is significantly worse.
                // PR #233 code-review #2 / round 3 #2.
                assert!(
                    !self.class_visibility_stack.is_empty(),
                    "ICE: PopClassVisibility on empty class_visibility_stack — \
                     translator emitted an unbalanced Pop without a matching Push"
                );
                self.class_visibility_stack.pop();
                self.module_function_active_stack.pop();
                self.stack.push(Value::Nil);
            }
            Op::DefMethodBlock(name_id) => {
                // Pop the BlockHandle the preceding `CreateBlock`
                // pushed, then wrap it as a closure-method. We
                // *share* the BlockHandle's `captured` Rc — the
                // method body and the original lexical scope point
                // at the same locals Vec, so the method can read &
                // write outer-scope variables (CRuby semantics).
                //
                // GC: the captured Rc keeps its slots alive via the
                // Method, which lives in Class.methods (rooted via
                // Vm.classes) or toplevel_methods. `maybe_gc`'s
                // root-gathering loops walk every installed method
                // table and add closure-captured slots to the root
                // set, so Objects/Arrays reachable through the
                // closure survive collections.
                let bv = self.stack.pop().expect("ICE: DefMethodBlock no block on stack");
                let id = if let Value::Block(id) = bv { id } else {
                    panic!("ICE: DefMethodBlock without Block on stack");
                };
                let (proto_idx, captured, param_start, n_params, captured_yield_block) = {
                    let bh = self.heap.block(id);
                    (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params, bh.captured_yield_block)
                };
                let proto = &self.protos[proto_idx];
                let params = proto.params.clone();
                // When `class_stack.last()` is an eigenclass shell
                // (from `cls.singleton_class.class_eval { ... }`),
                // `install_method` redirects the install into
                // `cls.singleton_methods`. `defining_class` has to
                // point at the same `cls` so `super_lookup` walks
                // the right ancestor chain — using the shell would
                // miss every node in the receiver's superclass
                // chain. (Code-review #253 round 1 #1.)
                let defining_class = self.class_stack.last().map(|c| Rc::downgrade(&c.effective_install_class()));
                let vis = self.class_visibility_stack.last().copied().unwrap_or(crate::value::Visibility::Public);
                let m = Rc::new(Method {
                    params,
                    proto_idx,
                    fixed_arity: None,
                    defining_class,
                    visibility: std::cell::Cell::new(vis),
                    closure: Some(crate::value::MethodClosure { captured, param_start, n_params, captured_yield_block }),
                builtin: None,
                original_name: Some(name_id),
                });
                if let Some(cls) = self.class_stack.last() { cls.install_method(name_id, m); }
                else { self.toplevel_methods.insert(name_id, m); }
                self.method_gen = self.method_gen.wrapping_add(1);
                // `method_added(name_id)` fires for the compile-
                // time `define_method(:literal) { … }` intercept too
                // — CRuby invokes the hook regardless of which
                // install path landed the method.
                if let Some(cls) = self.class_stack.last().cloned() {
                    self.fire_method_lifecycle_hook(&cls, "method_added", name_id)?;
                }
                // `Op::DefMethodBlock` is emitted ONLY for the
                // compile-time `define_method(:literal_symbol) { … }`
                // intercept (compiler.rs:209); it is NOT the parsed
                // `def` path. CRuby's `define_method` evaluates to
                // the method name as a Symbol — pushing
                // `Value::Sym(name_id)` aligns this intercept with
                // the runtime-dispatch `Module#define_method` arm
                // in vm/dispatch.rs so `x = define_method(:foo) {}`
                // returns the same value regardless of which
                // intercept fires. Parsed `def name; …; end` still
                // returns `nil` in rubyrs (`Op::DefMethod` pushes
                // Nil) — that's a separate CRuby-divergence not
                // addressed by this PR.
                self.stack.push(Value::Sym(name_id));
            }
            Op::DefObjectSingletonMethodBlock(name_id) => {
                // `recv.define_singleton_method(:foo) { |args| ... }`
                // — closure-method install on the receiver's
                // eigenclass. Compiler pushed `recv` first then
                // the `CreateBlock`-produced block, so pop in
                // that reverse order.
                let bv = self.stack.pop()
                    .expect("ICE: DefObjectSingletonMethodBlock no block on stack");
                let block_id = if let Value::Block(id) = bv { id } else {
                    panic!("ICE: DefObjectSingletonMethodBlock without Block on stack");
                };
                let recv = self.stack.pop()
                    .expect("ICE: DefObjectSingletonMethodBlock no receiver on stack");
                let (proto_idx, captured, param_start, n_params, captured_yield_block) = {
                    let bh = self.heap.block(block_id);
                    (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params, bh.captured_yield_block)
                };
                let proto = &self.protos[proto_idx];
                let params = proto.params.clone();
                // Class receivers install into the class's own
                // `singleton_methods` table (= class method),
                // matching the runtime arm in dispatch.rs. Object
                // receivers install into the eigenclass. Other
                // receivers (primitives) raise TypeError.
                // PR #309 cycle-1: the literal form
                // `C.define_singleton_method(:foo) { ... }`
                // previously rejected Class; aligning both paths
                // now.
                let hook_recv: Value = match &recv {
                    Value::Object(obj_id) => {
                        let sc = self.heap.ensure_singleton_class(*obj_id);
                        let m = Rc::new(Method {
                            params,
                            proto_idx,
                            fixed_arity: None,
                            defining_class: Some(Rc::downgrade(&sc)),
                            visibility: std::cell::Cell::new(Visibility::Public),
                            closure: Some(crate::value::MethodClosure { captured, param_start, n_params, captured_yield_block }),
                            builtin: None,
                            original_name: Some(name_id),
                        });
                        sc.methods.borrow_mut().insert(name_id, m);
                        Value::Object(*obj_id)
                    }
                    Value::Class(cls) => {
                        let m = Rc::new(Method {
                            params,
                            proto_idx,
                            fixed_arity: None,
                            defining_class: Some(Rc::downgrade(cls)),
                            visibility: std::cell::Cell::new(Visibility::Public),
                            closure: Some(crate::value::MethodClosure { captured, param_start, n_params, captured_yield_block }),
                            builtin: None,
                            original_name: Some(name_id),
                        });
                        cls.singleton_methods.borrow_mut().insert(name_id, m);
                        Value::Class(cls.clone())
                    }
                    // Array / Proc receivers: per-instance eigenclass
                    // via the heap_singletons side-table. rack's
                    // Deflater closes over an Array body with
                    // `body.define_singleton_method(:close) { ... }`;
                    // ContentLength does it with `:each` on a Proc.
                    Value::Array(_) | Value::Block(_) => {
                        let sc = self.ensure_heap_singleton(&recv);
                        let m = Rc::new(Method {
                            params,
                            proto_idx,
                            fixed_arity: None,
                            defining_class: Some(Rc::downgrade(&sc)),
                            visibility: std::cell::Cell::new(Visibility::Public),
                            closure: Some(crate::value::MethodClosure { captured, param_start, n_params, captured_yield_block }),
                            builtin: None,
                            original_name: Some(name_id),
                        });
                        sc.methods.borrow_mut().insert(name_id, m);
                        recv.clone()
                    }
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "can't define singleton method on {} (only user-class instances and classes are supported)",
                                other.type_name(),
                            ),
                        }));
                    }
                };
                self.method_gen = self.method_gen.wrapping_add(1);
                // `singleton_method_added(name)` fires on the
                // explicit receiver — Object recv fires the hook
                // looked up on its class; Class recv fires the
                // hook looked up on its singleton chain.
                self.fire_singleton_method_lifecycle_hook(
                    hook_recv,
                    "singleton_method_added",
                    name_id,
                )?;
                // CRuby: `define_singleton_method(:foo) { … }`
                // evaluates to `:foo`. Mirrors the same alignment
                // applied to `Op::DefMethodBlock` above; both
                // intercepts are emitted by compiler.rs for the
                // literal-symbol+block compile-time fast-path and
                // should return the same value as the runtime-
                // dispatch path.
                self.stack.push(Value::Sym(name_id));
            }
            Op::DefClass(name_id, p_idx, qual_id) | Op::DefModule(name_id, p_idx, qual_id) => {
                // `DefModule` distinguishes the source keyword
                // (`module X; end`) so the resulting Class shell
                // gets `is_module: true`. Otherwise identical to
                // DefClass — same body-frame push, same constant-
                // alias plumbing.
                let is_module = matches!(op, Op::DefModule(..));
                // Pop superclass (Nil for "default to Object", a Class for `class Foo < Bar`).
                let parent_val = self.stack.pop().expect("ICE: DefClass without superclass slot");
                let explicit_parent = match parent_val {
                    Value::Class(c) => Some(c),
                    _ => None,
                };
                // CRuby: `class Foo; end` with no explicit parent
                // defaults to Object. Without this default,
                // `Object === Foo.new` returns false and
                // `case scope when Object` fails to match user-class
                // instances — which breaks tilt's render dispatch
                // path (tilt 2.7 template.rb:257 case/when on the
                // render scope, falling back to
                // `Kernel.instance_method(:class).bind_call(scope)`
                // for the non-Object branch).
                //
                // Skip the default for:
                //   - modules (Modules don't have superclass)
                //   - Object itself (otherwise Object < Object cycle)
                //   - reopen of a class that's already been defined
                //     (don't change its superclass post-hoc; the
                //     reopen path below already preserves the
                //     existing chain)
                let object_sym = self.interner.intern("Object");
                let name_str_check = self.interner.resolve(if qual_id.0 == u32::MAX { name_id } else { qual_id }).to_string();
                // CRuby: `class BasicObject < Anything` raises
                // `TypeError: superclass mismatch for class BasicObject`.
                // Without rejecting, `class BasicObject < Object` would
                // create the cycle `Object < BasicObject < Object`,
                // which corrupts ancestor walks (`flatten_ancestors`
                // has cycle detection but the result is still wrong).
                // Fence the explicit-parent path on the top-level
                // BasicObject name; user-defined nested `Foo::BasicObject`
                // is unaffected.
                if name_str_check == "BasicObject" && explicit_parent.is_some() {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "superclass mismatch for class BasicObject".to_string(),
                    }));
                }
                let parent = if explicit_parent.is_some() {
                    explicit_parent
                } else if is_module
                    || name_str_check == "Object"
                    || name_str_check == "BasicObject"
                {
                    // Modules don't have a superclass; Object and
                    // BasicObject sit at/near the root of the chain
                    // (BasicObject is the root with no parent; Object
                    // inherits from BasicObject via the explicit
                    // `class Object < BasicObject` form in
                    // preamble/object.rb). Either way, no default.
                    None
                } else {
                    self.classes.get(&object_sym).cloned()
                };
                // Key the class table by the QUALIFIED SymId when
                // one is supplied (`module Foo; class Bar; end; end`
                // → `qual_id = sym("Foo::Bar")`). Top-level
                // definitions leave the third arg as the
                // `u32::MAX` sentinel and key by the bare name.
                //
                // This separates `class Bar` at top level from a
                // `module Foo; class Bar; end; end` nested define:
                // they hash to different slots, so each gets its
                // own `Class` object with independent method /
                // ivar / superclass tables — matching CRuby's
                // "scope determines identity" model. Re-opening
                // within the same scope still hits the same slot
                // (the qualified SymId is identical) so methods
                // added in a reopen land on the same class.
                // Compact path-name define (`class A::B` inside a
                // scope): the compiler leaves qual_id at the MAX
                // sentinel for `::`-containing names because it can't
                // know at compile time whether the HEAD resolves
                // relative to the current scope or to top level. CRuby
                // resolves the head via a normal (lexical-first)
                // constant lookup, then defines the LAST segment in
                // that namespace. Mirror that: if the head matches a
                // class nested under any enclosing lexical scope, key
                // the new class under `<resolved-head>::<rest>`.
                // Otherwise keep the bare joined name (top-level head,
                // e.g. top-level `class ERB::Compiler`). Without this,
                // `module Parser; class Builders::Default` registered a
                // fresh top-level `Builders::Default`, so
                // `Parser::Builders::Default` stayed undefined (parser
                // gem's AST builder).
                let resolved_path_key: Option<crate::intern::SymId> = {
                    let bare = self.interner.resolve(name_id).to_string();
                    if qual_id.0 == u32::MAX
                        && let Some((head, rest)) = bare.split_once("::")
                    {
                        let lex = self.frames.last()
                            .map(|f| self.protos[f.proto_idx].lexical_scope.clone())
                            .unwrap_or_default();
                        let mut resolved = None;
                        for scope_sym in &lex {
                            let scope_full = self.interner.resolve(*scope_sym).to_string();
                            let cand = format!("{scope_full}::{head}");
                            if self.interner.contains(&cand) {
                                let cand_id = self.interner.intern(&cand);
                                if self.classes.contains_key(&cand_id) {
                                    resolved = Some(self.interner.intern(&format!("{cand}::{rest}")));
                                    break;
                                }
                            }
                        }
                        resolved
                    } else {
                        None
                    }
                };
                let table_key = match resolved_path_key {
                    Some(k) => k,
                    None => if qual_id.0 == u32::MAX { name_id } else { qual_id },
                };
                let name_str = self.interner.resolve(table_key).to_string();
                // Reopening a constant that has a PENDING AUTOLOAD must
                // fire the autoload first (CRuby semantics): `module X`
                // where `X` is autoloaded loads the existing definition
                // — and any nested autoloads that load registers —
                // BEFORE reopening. bridgetown-foundation's object.rb
                // opens `module Bridgetown::Foundation::RefineExt`,
                // which zeitwerk autoloads to a DIRECTORY; firing that
                // dir-autoload is what registers the sibling refine_ext
                // files (hash/module/string/deep_duplicatable). Without
                // this, opening the module minted a fresh empty shell,
                // the siblings never registered, and `eager_load`
                // skipped them (only the explicitly-required object.rb
                // loaded). Gate on THIS exact constant being pending
                // (not a parent prefix) so opening a genuinely-fresh
                // nested module doesn't spuriously fire an ancestor's
                // autoload.
                #[cfg(not(target_os = "wasi"))]
                if !self.classes.contains_key(&table_key)
                    && (self.autoloads_toplevel.contains_key(&table_key)
                        || self.autoloads_scoped.contains_key(&table_key))
                {
                    self.fire_pending_autoload(&name_str)?;
                }
                // First-define vs reopen: CRuby fires the
                // `inherited` callback only on the first
                // `class B < A` definition, not on reopens.
                // Snapshot before the entry()-or_insert.
                let was_fresh = !self.classes.contains_key(&table_key);
                // Stamp the definition location on FIRST define (CRuby's
                // `const_source_location` reports where `class`/`module`
                // first opened; reopens don't move it). `current_op_location`
                // reads the executing DefClass op's span.
                if was_fresh
                    && !self.const_source_locations.contains_key(&table_key)
                    && let Some(loc) = self.current_op_location()
                {
                    self.const_source_locations.insert(table_key, loc);
                }
                // A class/module definition (fresh OR reopen — a reopen
                // can still change what nested bare names resolve to via
                // its body) invalidates the constant ICs.
                self.bump_const_gen();
                let cls = self.classes.entry(table_key).or_insert_with(|| Rc::new(Class {
                    name: name_str,
                    is_module,
                    ivars: RefCell::new(crate::intern::FxHashMap::default()),
                    methods: RefCell::new(crate::intern::FxHashMap::default()),
                    singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
                    superclass: RefCell::new(parent.clone()),
                    includes: RefCell::new(Vec::new()),
                    prepends: RefCell::new(Vec::new()),
                    singleton_prepends: RefCell::new(Vec::new()),
                    singleton_includes: RefCell::new(Vec::new()),
                    singleton_view: RefCell::new(None),
                    singleton_target: RefCell::new(None),
                    undefed: RefCell::new(crate::intern::FxHashSet::default()),
                    anon_serial: std::cell::Cell::new(0),
                    class_vars: RefCell::new(crate::intern::FxHashMap::default()),
            consts: RefCell::new(crate::intern::FxHashMap::default()),
                    assigned_name: RefCell::new(None),
                    class_tag: None,
                    #[cfg(feature = "cext")]
                    cext_alloc_func: std::cell::Cell::new(None),
                })).clone();
                // If the class already existed (reopened) and the user specified a parent
                // this time, update it (only if it wasn't already set to something else).
                if let Some(p) = &parent {
                    let mut sc = cls.superclass.borrow_mut();
                    if sc.is_none() {
                        *sc = Some(p.clone());
                    }
                }
                self.method_gen = self.method_gen.wrapping_add(1); // class structure changed
                // `Class#inherited` callback — CRuby invokes
                // `Parent.inherited(Subclass)` after the
                // Subclass object exists but BEFORE its body
                // runs. Only fires on the FIRST definition of
                // a given class, never on reopen. Modules
                // (`module M; end`) don't inherit, so skip.
                //
                // Discovery: sinatra-4 relies on this in
                // `Sinatra::Base.inherited(subclass)` to call
                // `subclass.reset!` (which initializes
                // `@routes = {}` etc). Without firing it,
                // `class App < Sinatra::Base; get '/' do; end`
                // raises NoMethodError on the nil `@routes`.
                // (TRY_RUNS pass-12 layer #14.)
                //
                // Lookup uses `lookup_class_singleton_method`,
                // which walks the parent's singleton_prepends,
                // own singleton_methods, and then up the
                // superclass-singleton chain (i.e., picks up
                // `inherited` defined via `def self.inherited`
                // on any ancestor of the parent). It does NOT
                // fall through to `Class`'s instance methods —
                // so a user monkey-patch like
                // `class Class; def inherited(sub); end; end`
                // won't fire here. That's a documented
                // divergence shared with the broader class-as-
                // receiver dispatch path (`A.custom_method`
                // also doesn't pick up Class instance-method
                // patches today); a proper fix lives at the
                // dispatch layer, not in this hook. Code-review
                // #337 round 1.
                //
                // CRuby's default `Class#inherited` is a no-op;
                // when no override resolves we skip the
                // dispatch entirely (observationally identical
                // to invoking the no-op default).
                if was_fresh
                    && !is_module
                    && let Some(parent_cls) = &parent
                    // Fast-path: if `"inherited"` has never been
                    // interned, no user code has defined or
                    // referenced an override (the compiler interns
                    // every method name and call site on first
                    // sight). Skip the intern() call entirely so
                    // we don't grow the symbol table — and the
                    // `Config::max_symbols`-guarded paths stay
                    // authoritative. Code-review #337 round 2.
                    && self.interner.contains("inherited") {
                    let inh_id = self.interner.intern("inherited");
                    if let Some(m) = self.lookup_class_singleton_method(parent_cls, inh_id) {
                        let pre_frames = self.frames.len();
                        self.invoke_method(
                            m,
                            Value::Class(parent_cls.clone()),
                            vec![Value::Class(cls.clone())],
                        )?;
                        self.dispatch_until(pre_frames)?;
                        // Discard the callback's return value
                        // (the hook is invoked for its side
                        // effects; CRuby ignores the return).
                        self.stack.pop();
                    }
                }
                // A nested `module`/`class` defined directly inside an
                // eigenclass body (`class << self; module Sync; …`):
                // CRuby scopes the constant under the eigenclass. The
                // flat const model keys it under the surrounding class
                // (compile-time qual_id) so bare reads in the body still
                // resolve — ALSO register it on the eigenclass's own
                // const table so explicit access (`self::Sync`,
                // `const_get(:Sync, false)`, `const_defined?(:Sync,
                // false)`, `singleton_class.const_get`) finds it where
                // CRuby puts it. Additive: the global table + bare-read
                // path are untouched (gated on the enclosing scope being
                // an eigenclass — `singleton_target` set).
                if let Some(scope) = self.class_stack.last()
                    && scope.singleton_target.borrow().is_some()
                {
                    let short = self.interner.resolve(name_id);
                    let short = short.rsplit("::").next().unwrap_or(&short).to_string();
                    let short_id = self.interner.intern(&short);
                    scope.consts.borrow_mut().insert(short_id, Value::Class(cls.clone()));
                    self.bump_const_gen();
                }
                // `Module#const_added` (CRuby 3.2+): fire on the enclosing
                // module the moment a FRESH nested class/module constant
                // appears, BEFORE the body runs — zeitwerk (which
                // `Module.prepend`s a const_added) registers a namespace's
                // child autoloads here, so `class Container; include
                // Container::Mixin` (dry-core) sees Mixin's autoload. Only
                // on first definition (CRuby doesn't re-fire on reopen);
                // skipped for the compact `A::B::C` form (owner ambiguous)
                // and gated on `const_added` being interned at all.
                if was_fresh && self.interner.contains("const_added") {
                    // Owner + short cname. For the compact `class A::B` form
                    // the name is qualified ("A::B"): owner is the resolved
                    // parent path `A`, cname the short last component `B`
                    // (CRuby fires on the parent with the short symbol). This
                    // form was previously SKIPPED, which broke zeitwerk's
                    // namespace child-autoload setup — it `Module.prepend`s a
                    // const_added that registers `Foo/`'s autoloads the moment
                    // `MyGem::Foo` is defined. The bare form ("Foo") fires on
                    // the lexical scope (or Object) as before.
                    let full = self.interner.resolve(name_id).to_string();
                    let (owner, cname) = match full.rfind("::") {
                        Some(pos) => {
                            let parent_id = self.interner.intern(&full[..pos]);
                            let cname_id = self.interner.intern(&full[pos + 2..]);
                            (self.classes.get(&parent_id).cloned(), cname_id)
                        }
                        None => {
                            let o = self.class_stack.last().cloned().or_else(|| {
                                let obj = self.interner.intern("Object");
                                self.classes.get(&obj).cloned()
                            });
                            (o, name_id)
                        }
                    };
                    if let Some(owner) = owner {
                        self.fire_const_added(&owner, cname)?;
                    }
                }
                self.class_stack.push(cls.clone());
                self.class_visibility_stack.push(Visibility::Public);
                self.module_function_active_stack.push(false);
                let proto = &self.protos[p_idx as usize];
                let n_locals = proto.n_locals as usize;
                self.frames.push(Frame {
                    proto_idx: p_idx as usize, ip: 0,
                    locals: crate::vm::Locals::Shared(Rc::new(RefCell::new(vec_nil(n_locals)))),
                    self_val: Value::Class(cls.clone()),
                    base_sp: self.stack.len(),
                    is_class_body: true, swap_return: None, block_arg: None, defining_class: None, lexical_cvar_class: None, #[cfg(feature = "regex")] saved_last_match: None, is_block: false, is_lambda: false, n_given_positional: 0, kw_given_mask: 0, aux: None, pending_yield: false,
                    block_writeback: None,
                    captured_yield_block: None,
                });
            }
            Op::OpenSingletonClass(p_idx) => {
                // `class << <expr>; body; end` run as a REAL
                // eigenclass body — self = the metaclass. The
                // receiver was pushed by the compiler immediately
                // before this op. Materialize its eigenclass, then
                // open a class-body frame on it so `def`, `include`,
                // `private`/`public`, `attr_*`, and `internal def`-
                // style runtime indirection all consistently target
                // the metaclass (= the real class's singleton tables
                // via `singleton_target` redirect). See
                // `Op::OpenSingletonClass` in bytecode.rs.
                let recv = self.stack.pop()
                    .expect("ICE: OpenSingletonClass without receiver slot");
                let eigen: Rc<Class> = match &recv {
                    Value::Class(cls) => cls.ensure_singleton_view(),
                    Value::Object(id) => self.heap.ensure_singleton_class(*id),
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "can't open singleton class of {} (only classes/modules and user-class instances are supported)",
                                other.type_name(),
                            ),
                        }));
                    }
                };
                self.class_stack.push(eigen.clone());
                self.class_visibility_stack.push(Visibility::Public);
                self.module_function_active_stack.push(false);
                let proto = &self.protos[p_idx as usize];
                let n_locals = proto.n_locals as usize;
                self.frames.push(Frame {
                    proto_idx: p_idx as usize, ip: 0,
                    locals: crate::vm::Locals::Shared(Rc::new(RefCell::new(vec_nil(n_locals)))),
                    self_val: Value::Class(eigen),
                    base_sp: self.stack.len(),
                    is_class_body: true, swap_return: None, block_arg: None, defining_class: None, lexical_cvar_class: None, #[cfg(feature = "regex")] saved_last_match: None, is_block: false, is_lambda: false, n_given_positional: 0, kw_given_mask: 0, aux: None, pending_yield: false,
                    block_writeback: None,
                    captured_yield_block: None,
                });
            }
            Op::NewArray(n) => {
                self.maybe_gc();
                self.check_alloc()?;
                let n = n as usize;
                let split = self.stack.len() - n;
                let elems: Vec<Value> = self.stack.drain(split..).collect();
                let id = self.heap.alloc(HeapObj::Array(elems.into()));
                self.stack.push(Value::Array(id));
            }
            Op::NewRange(excl) => {
                self.maybe_gc();
                self.check_alloc()?;
                let end = self.stack.pop().expect("ICE: NewRange end underflow");
                let begin = self.stack.pop().expect("ICE: NewRange begin underflow");
                let id = self.heap.alloc(HeapObj::Range(crate::heap::RangeObj {
                    begin, end, exclusive: excl != 0,
                }));
                self.stack.push(Value::Range(id));
            }
            Op::NewHash(n) => {
                // Body extracted to `op_new_hash` (#[inline(never)]): keeping
                // it OUT of this mega-match means its code can't perturb the
                // instruction layout of step()'s other hot arms. An inline
                // direct-drain version was measured FASTER in isolation
                // (hash construction -30ns) but SLOWER on the full request
                // (+3.7ns) purely from that codegen ripple — extracting
                // captures the win without the ripple.
                self.op_new_hash(n as usize)?;
            }
            Op::PushRescue(off, slot, bind, filter_sym) => {
                let f = self.frames.last().expect("ICE: PushRescue no frame");
                let ip = f.ip;
                let loop_depth = f.loop_depth();
                let begin_depth = f.begin_depth();
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                let bind_slot = if bind != 0 { Some(slot) } else { None };
                // The compiler emits the SymId of the class to filter
                // by — for bare `rescue` that's `StandardError`, for
                // `rescue Foo::Bar` the qualified-form SymId stamped
                // by the lexical dual-write. We resolve through the
                // same fallback chain as `Op::LoadConst`: `classes`
                // first (bare names + dual-write copies), then
                // `constants` (where user `Foo::Bar = …` aliases land).
                // If neither hits, `filter_class` stays `None` and
                // the handler fails every match check — closer to
                // CRuby than silently catching everything.
                // Resolve the rescue-class name through the lexical
                // nesting, exactly like `Op::LoadConstChain` resolves a
                // bare constant read. The compiler stamps only the bare
                // source sym (e.g. `Sig`), but a class defined as
                // `module M; class Sig` is keyed in `self.classes` by
                // its QUALIFIED sym (`M::Sig`) — so a plain
                // `classes.get("Sig")` missed it and `rescue Sig` inside
                // `module M` never matched, letting the exception escape
                // (the `raise` side already resolves via the lexical
                // chain, so the two sides disagreed). Walk the enclosing
                // scopes innermost-first, qualifying the bare name, then
                // fall back to the bare lookup (covers top-level classes
                // and `rescue Foo::Bar` whose sym is already qualified).
                let filter = {
                    // Clone the `Rc<str>` instead of materializing a
                    // fresh `String` — the interner returns
                    // `&Rc<str>` so the clone is a refcount bump.
                    // Defer the lex-walk's `lexical_scope.clone()`
                    // into the relative branch so absolute rescues
                    // don't pay for the Vec copy.
                    let bare_name: std::rc::Rc<str> = self.interner.resolve(filter_sym).clone();
                    // Splatted filter (`rescue *PASSTHROUGH`) — the
                    // marked name is a CONSTANT holding an Array of
                    // classes (minitest's PASSTHROUGH_EXCEPTIONS
                    // idiom). Resolve the constant's VALUE — absolute
                    // or via the same lex-walk as the relative branch
                    // below — and snapshot its class elements.
                    // Unresolved names and non-Array/non-Class values
                    // yield `None` (match nothing): fail-closed,
                    // where the old dropped-splat lowering degraded
                    // to a bare rescue that matched EVERY
                    // StandardError (minitest's passthrough arm then
                    // re-raised every test error and killed the run).
                    if let Some(splat_inner) = crate::const_marker::strip_splat(&bare_name) {
                        let val: Option<Value> = if let Some(abs) = crate::const_marker::strip_absolute(splat_inner) {
                            let abs_sym = self.interner.intern(abs);
                            self.constants.get(&abs_sym).cloned()
                        } else {
                            let proto_idx = self.frames.last().expect("ICE: PushRescue no frame").proto_idx;
                            let lex = self.protos[proto_idx].lexical_scope.clone();
                            let mut found = None;
                            for scope_sym in &lex {
                                let scope_name = self.interner.resolve(*scope_sym).clone();
                                let qualified = format!("{}::{}", scope_name, splat_inner);
                                let qsym = self.interner.intern(&qualified);
                                if let Some(v) = self.constants.get(&qsym) {
                                    found = Some(v.clone());
                                    break;
                                }
                            }
                            found.or_else(|| {
                                let inner_sym = self.interner.intern(splat_inner);
                                self.constants.get(&inner_sym).cloned()
                            })
                        };
                        match val {
                            Some(Value::Array(id)) => {
                                let list: Vec<std::rc::Rc<Class>> = self.heap.array(id).iter()
                                    .filter_map(|v| match v {
                                        Value::Class(c) => Some(c.clone()),
                                        _ => None,
                                    })
                                    .collect();
                                Some(RescueFilter::Any(list))
                            }
                            // `rescue *X` where X holds a single
                            // class — CRuby Array()-coerces, so it
                            // behaves as a one-element list.
                            Some(Value::Class(c)) => Some(RescueFilter::Class(c)),
                            _ => None,
                        }
                    }
                    // Absolute paths (`rescue ::Foo::Bar`) carry a
                    // leading `::` marker from the AST lowering.
                    // CRuby semantics: skip the lex-walk and look up
                    // the joined name at top level only.
                    else if let Some(absolute_bare) = crate::const_marker::strip_absolute(&bare_name) {
                        let abs_sym = self.interner.intern(absolute_bare);
                        self.classes.get(&abs_sym).cloned()
                            .or_else(|| match self.constants.get(&abs_sym) {
                                Some(Value::Class(c)) => Some(c.clone()),
                                _ => None,
                            })
                            .map(RescueFilter::Class)
                    } else {
                        let proto_idx = self.frames.last().expect("ICE: PushRescue no frame").proto_idx;
                        let lex = self.protos[proto_idx].lexical_scope.clone();
                        let mut found = None;
                        if !lex.is_empty() {
                            for scope_sym in &lex {
                                let scope_name = self.interner.resolve(*scope_sym).clone();
                                let qualified = format!("{}::{}", scope_name, bare_name);
                                let qsym = self.interner.intern(&qualified);
                                if let Some(c) = self.classes.get(&qsym).cloned() {
                                    found = Some(c);
                                    break;
                                }
                                if let Some(Value::Class(c)) = self.constants.get(&qsym) {
                                    found = Some(c.clone());
                                    break;
                                }
                            }
                        }
                        found
                            .or_else(|| self.classes.get(&filter_sym).cloned())
                            .or_else(|| match self.constants.get(&filter_sym) {
                                Some(Value::Class(c)) => Some(c.clone()),
                                _ => None,
                            })
                            .map(RescueFilter::Class)
                    }
                };
                self.frames.last_mut().expect("ICE: PushRescue no frame").aux_mut().rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot, is_ensure: false,
                    filter_class: filter, loop_depth_at_push: loop_depth,
                    begin_depth_at_push: begin_depth,
                });
            }
            Op::PushRescueSplatLocal(off, slot, bind, src_slot) => {
                // `rescue *exp` on a local — read the slot NOW (push
                // time) and snapshot its Array elements as the filter
                // list. A single Class coerces to a one-element
                // filter; Nil/other values match nothing
                // (fail-closed). See Op::PushRescue for the shared
                // handler bookkeeping.
                let f = self.frames.last().expect("ICE: PushRescueSplatLocal no frame");
                let ip = f.ip;
                let loop_depth = f.loop_depth();
                let begin_depth = f.begin_depth();
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                let bind_slot = if bind != 0 { Some(slot) } else { None };
                let src = match &f.locals {
                    crate::vm::Locals::Stack(base) => {
                        self.locals_arena[*base as usize + src_slot as usize].clone()
                    }
                    crate::vm::Locals::Shared(rc) => rc.borrow()[src_slot as usize].clone(),
                };
                let filter = match src {
                    Value::Array(id) => {
                        let list: Vec<std::rc::Rc<Class>> = self.heap.array(id).iter()
                            .filter_map(|v| match v {
                                Value::Class(c) => Some(c.clone()),
                                _ => None,
                            })
                            .collect();
                        Some(RescueFilter::Any(list))
                    }
                    Value::Class(c) => Some(RescueFilter::Class(c)),
                    _ => None,
                };
                self.frames.last_mut().expect("ICE: PushRescueSplatLocal no frame").aux_mut().rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot, is_ensure: false,
                    filter_class: filter, loop_depth_at_push: loop_depth,
                    begin_depth_at_push: begin_depth,
                });
            }
            Op::PopRescue => {
                self.frames.last_mut().expect("ICE: PopRescue no frame").pop_rescue();
            }
            Op::EnterBegin => {
                // Snapshot `$!` so `ExitBegin` (or a `return` out of a
                // rescue body) can revert it — CRuby's errinfo is
                // dynamically scoped to the rescue/ensure body, not the
                // whole program. (See `BeginBaseline::saved_dollar_bang`.)
                let saved_dollar_bang =
                    self.globals.get(&self.sym_bang).cloned().unwrap_or(Value::Nil);
                let f = self.frames.last_mut().expect("ICE: EnterBegin no frame");
                let aux = f.aux_mut();
                let baseline = crate::vm::BeginBaseline {
                    rescues_len: aux.rescues.len(),
                    loop_rescue_depths_len: aux.loop_rescue_depths.len(),
                    loop_stack_depths_len: aux.loop_stack_depths.len(),
                    saved_dollar_bang,
                };
                aux.begin_rescue_depths.push(baseline);
            }
            Op::ExitBegin => {
                let baseline = self
                    .frames
                    .last_mut()
                    .expect("ICE: ExitBegin no frame")
                    .aux
                    .as_mut()
                    .and_then(|a| a.begin_rescue_depths.pop())
                    .expect("ICE: ExitBegin without matching EnterBegin");
                // Revert `$!` to its pre-begin value now that this
                // region's rescue/ensure body has completed. A handled
                // exception is no longer "in flight", so a subsequent
                // bare `raise` must not resurface it.
                self.globals.insert(self.sym_bang, baseline.saved_dollar_bang);
            }
            Op::TruncateRescuesToBeginBaseline => {
                let f = self.frames.last_mut().expect("ICE: TruncateRescues no frame");
                let aux = f.aux_mut();
                let baseline = aux
                    .begin_rescue_depths
                    .last()
                    .expect("ICE: retry without matching EnterBegin baseline")
                    .clone();
                // Three-stack cleanup so retry stays balanced
                // whether it fires from a multi-class rescue
                // (rescues truncation) or from inside a `while`
                // loop in the rescue body (loop depths
                // truncation). (Code-review #306 round 3.)
                aux.rescues.truncate(baseline.rescues_len);
                aux.loop_rescue_depths.truncate(baseline.loop_rescue_depths_len);
                aux.loop_stack_depths.truncate(baseline.loop_stack_depths_len);
            }
            Op::PushEnsure(off) => {
                let f = self.frames.last().expect("ICE: PushEnsure no frame");
                let ip = f.ip;
                let loop_depth = f.loop_depth();
                let begin_depth = f.begin_depth();
                let target = (ip as i32 + off) as usize;
                let depth = self.stack.len();
                self.frames.last_mut().expect("ICE: PushEnsure no frame").aux_mut().rescues.push(RescueHandler {
                    handler_ip: target, stack_depth: depth, bind_slot: None, is_ensure: true,
                    filter_class: None, // ensure is unconditional
                    loop_depth_at_push: loop_depth,
                    begin_depth_at_push: begin_depth,
                });
            }
            Op::PopEnsure => {
                self.frames.last_mut().expect("ICE: PopEnsure no frame").pop_rescue();
            }
            Op::Raise => {
                let v = self.stack.pop().unwrap_or(Value::Nil);
                // A user/stub `raise` OVERRIDE (minitest's
                // `obj.stub :raise, nil` installs one on the
                // receiver's eigenclass) must intercept even the
                // bare keyword form — CRuby's raise is an ordinary
                // Kernel method. raise is a cold path, so one
                // class-chain lookup here is acceptable; the
                // kernel-alias forwarder (the saved original)
                // doesn't count as an override or the restore
                // cycle would loop.
                {
                    let raise_sym = self.interner.intern("raise");
                    let self_val = self.frames.last().map(|f| f.self_val.clone()).unwrap_or(Value::Nil);
                    let user_override = match &self_val {
                        Value::Object(id) => {
                            let cls = self.heap.class_of(*id);
                            self.lookup_method_uncached(&cls, raise_sym)
                        }
                        _ => None,
                    };
                    if let Some(m) = user_override
                        && !self.protos[m.proto_idx].name.starts_with("<kernel-alias-forwarder")
                    {
                        // Run the override SYNCHRONOUSLY (raise is
                        // cold; stub bodies are tiny) and discard
                        // its return — the compiler emits
                        // `Raise; LoadNil`, and the LoadNil is the
                        // expression value on the no-unwind path.
                        let argv = if matches!(v, Value::Nil) { vec![] } else { vec![v] };
                        let pre_frames = self.frames.len();
                        self.invoke_method(m, self_val, argv)?;
                        self.dispatch_until(pre_frames)?;
                        self.stack.pop();
                        return Ok(true);
                    }
                }
                self.do_raise_value(v)?;
            }
            Op::Break => {
                // Mark the surrounding native-driven loop to terminate.
                // The value the user passed (or `nil`) stays on the
                // operand stack and rides out with the subsequent
                // Op::Return; collection_call_block reads it then.
                self.break_signaled = true;
                self.sync_control_signals();
            }
            Op::EnterLoop => {
                let stack_depth = self.stack.len();
                let f = self.frames.last_mut().expect("ICE: EnterLoop no frame");
                let aux = f.aux_mut();
                let depth = aux.rescues.len();
                aux.loop_rescue_depths.push(depth);
                aux.loop_stack_depths.push(stack_depth);
            }
            Op::ExitLoop => {
                let aux = self.frames.last_mut().expect("ICE: ExitLoop no frame").aux_mut();
                aux.loop_rescue_depths.pop()
                    .expect("ICE: ExitLoop with empty loop_rescue_depths");
                aux.loop_stack_depths.pop()
                    .expect("ICE: ExitLoop with empty loop_stack_depths");
            }
            Op::BreakLoop(off) => {
                // Compute the loop-target IP at the source site (the
                // dispatcher has already advanced f.ip past this op,
                // so the patched offset lands on the loop's join).
                let f = self.frames.last().expect("ICE: BreakLoop no frame");
                let target_depth = *f.aux.as_ref()
                    .and_then(|a| a.loop_rescue_depths.last())
                    .expect("ICE: BreakLoop outside a while loop");
                let target_ip = (f.ip as i32 + off) as usize;
                // Break value was pushed by the compiler immediately
                // before this op. Take it off so it doesn't pollute
                // the ensure-body stack we may be about to enter,
                // and so we can re-push it once the transfer lands.
                let value = self.stack.pop().expect("ICE: BreakLoop with no value on stack");
                self.begin_loop_transfer(LoopTransferKind::Break { value }, target_ip, target_depth)?;
            }
            Op::NextLoop(off) => {
                // Symmetric to BreakLoop: jumps to iter-check instead
                // of join; no value to carry (while has no iteration
                // value).
                let f = self.frames.last().expect("ICE: NextLoop no frame");
                let target_depth = *f.aux.as_ref()
                    .and_then(|a| a.loop_rescue_depths.last())
                    .expect("ICE: NextLoop outside a while loop");
                let target_ip = (f.ip as i32 + off) as usize;
                self.begin_loop_transfer(LoopTransferKind::Next, target_ip, target_depth)?;
            }
            Op::EndEnsure => {
                // Tail of an ensure handler body. Three paths:
                //   - Loop-transfer in flight: `pending_loop_transfer`
                //     is Some because BreakLoop/NextLoop kicked off a
                //     walk through this ensure. Resume the walk.
                //   - Method-break in flight (ADR 0024 Phase A.4):
                //     `pending_method_break` is Some because Op::Yield's
                //     case (a) kicked off a block-break that has to
                //     walk the yielding method's ensures before the
                //     frame returns. Resume the walk.
                //   - Normal exception unwind: the ensure was entered
                //     by `unwind_with_exception` which pushed the
                //     exception onto the operand stack. Pop and
                //     re-raise so unwind continues.
                if self.pending_loop_transfer.is_some() {
                    self.continue_loop_transfer()?;
                } else if self.pending_method_break.is_some() {
                    self.continue_method_break()?;
                } else {
                    // Stack invariant on the exception path: the
                    // unwinder pushed exactly one exception value
                    // when entering this ensure handler, and the
                    // ensure body is compile_stmt-balanced (every
                    // statement Pops its result). An empty stack
                    // here means stack-balance regression — surface
                    // it loudly rather than silently materialising
                    // a Nil exception.
                    let v = self.stack.pop()
                        .expect("ICE: EndEnsure with empty stack on exception path");
                    let exc = self.normalize_exception(v);
                    self.unwind_with_exception(exc)?;
                    // Same boundary check as Op::Raise — if
                    // unwind crossed out of our dispatch_until's
                    // scope, signal the iter driver above us.
                    if let Some(&d) = self.dispatch_until_depths.last()
                        && self.frames.len() <= d
                    {
                        return Err(self.trap(RubyError::AlreadyCaught));
                    }
                }
            }
            Op::BinOpInt(kind, rhs) => {
                let a = self.stack.pop().expect("ICE: BinOpInt lhs underflow");
                // Str-singleton operator-override gate (see Op::BinOp).
                if self.any_str_singletons
                    && matches!(&a, Value::Str(s)
                        if self.str_singletons.contains_key(&(std::rc::Rc::as_ptr(s) as usize)))
                {
                    self.stack.push(a);
                    self.stack.push(Value::Int(rhs));
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                    return Ok(true);
                }
                if let Value::Int(x) = a {
                    // Int / 0 and Int % 0 raise ZeroDivisionError;
                    // Rust's `wrapping_div` / `wrapping_rem` panic
                    // on rhs=0, so guard before delegating to
                    // `apply_int`.
                    if matches!(kind, BinOpKind::Div | BinOpKind::Mod) && rhs == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let v = match kind.apply_int(x, rhs) {
                        Some(v) => v,
                        // Overflow on Add/Sub/Mul — promote to BigInt.
                        // With bignum off, `apply_int` never returns
                        // None (the arms fall back to wrapping_*).
                        #[cfg(feature = "bignum")]
                        None => self.bigint_arith(kind, &Value::Int(x), &Value::Int(rhs))
                            .expect("ICE: bigint_arith None for Int operands")?,
                        #[cfg(not(feature = "bignum"))]
                        None => unreachable!("apply_int returns None only when bignum is on"),
                    };
                    self.stack.push(v);
                } else {
                    // Cold path: behave as if a generic `<op>` was dispatched
                    // with rhs boxed as an Int.
                    let b_val = Value::Int(rhs);
                    if let Some(v) = self.try_bigint_binop(kind, &a, &b_val)? {
                        // BigInt LHS + Int RHS — promoted arithmetic.
                        self.stack.push(v);
                    } else if let Some(v) = self.try_rational_binop(kind, &a, &b_val)? {
                        // Rational LHS + Int RHS — Phase C.2.
                        self.stack.push(v);
                    } else if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b_val), self.max_value_bytes).map_err(|e| self.trap(e))? {
                        self.stack.push(v);
                    } else if let Some(v) = self.sym_primitive(&a, kind.name(), std::slice::from_ref(&b_val))? {
                        self.stack.push(v);
                    } else {
                        self.stack.push(a);
                        self.stack.push(b_val);
                        let name_id = self.interner.intern(kind.name());
                        self.do_call(name_id, 1, false, u16::MAX)?;
                    }
                }
            }
            Op::BinOp(kind) => {
                let b = self.stack.pop().expect("ICE: BinOp rhs underflow");
                let a = self.stack.pop().expect("ICE: BinOp lhs underflow");
                // A String carrying a per-instance eigenclass may
                // override operators (`def exp.== _; false; end` —
                // minitest's long_invisible test); operator SYNTAX
                // otherwise never consults user tables. Set-once
                // gate keeps this a single false branch normally.
                if self.any_str_singletons
                    && matches!(&a, Value::Str(s)
                        if self.str_singletons.contains_key(&(std::rc::Rc::as_ptr(s) as usize)))
                {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                    return Ok(true);
                }
                // A String-SUBCLASS instance (class_tag, e.g. bcrypt's
                // Password) may override operators — operator SYNTAX
                // otherwise skips user tables. Route to do_call so the
                // subclass method-lookup gate runs (and falls back to the
                // primitive when there's no override). Plain strings have
                // no tag and stay on the fast path below.
                if matches!(&a, Value::Str(s) if s.class_tag.borrow().is_some()) {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                    return Ok(true);
                }
                if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
                    // Same guard as `Op::BinOpInt` — divide / mod
                    // by literal 0 in the Int×Int fast path. Without
                    // this, `n / m` where m happens to be 0 at
                    // runtime would panic the host process.
                    if matches!(kind, BinOpKind::Div | BinOpKind::Mod) && *y == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let v = match kind.apply_int(*x, *y) {
                        Some(v) => v,
                        #[cfg(feature = "bignum")]
                        None => self.bigint_arith(kind, &a, &b)
                            .expect("ICE: bigint_arith None for Int operands")?,
                        #[cfg(not(feature = "bignum"))]
                        None => unreachable!("apply_int returns None only when bignum is on"),
                    };
                    self.stack.push(v);
                } else if let Some(v) = self.try_bigint_binop(kind, &a, &b)? {
                    // BigInt × {Int,BigInt} or Int × BigInt — promoted
                    // arithmetic in arbitrary precision.
                    self.stack.push(v);
                } else if let Some(v) = self.try_rational_binop(kind, &a, &b)? {
                    // Rational × {Int,Rational,Float} (or reverse) —
                    // Phase C.2.
                    self.stack.push(v);
                } else if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b), self.max_value_bytes).map_err(|e| self.trap(e))? {
                    self.stack.push(v);
                } else {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                }
            }
            Op::BinOpLocalLocal(kind, a_slot, b_slot) => {
                // Superinstruction for `<local> <op> <local>`: read both
                // operands straight from the frame's locals instead of
                // doing two LoadLocals + a BinOp through the stack. The
                // body below is byte-for-byte identical to the `Op::BinOp`
                // arm (same Int×Int fast path, bigint/rational promotions,
                // primitive dispatch, fall-to-do_call), only the source of
                // `a`/`b` differs.
                // Frame is always present here (the dispatch loop in
                // `dispatch` / `dispatch_until` only reaches `step` with a
                // non-empty frame stack), so the `None` arm is unreachable
                // rather than a panic — keeps this off the panic budget.
                let (a, b) = match self.frames.last() {
                    Some(frame) => match &frame.locals {
                        crate::vm::Locals::Stack(base) => {
                            let base = *base as usize;
                            (
                                self.locals_arena[base + a_slot as usize].clone(),
                                self.locals_arena[base + b_slot as usize].clone(),
                            )
                        }
                        crate::vm::Locals::Shared(rc) => {
                            let locals = rc.borrow();
                            (locals[a_slot as usize].clone(), locals[b_slot as usize].clone())
                        }
                    },
                    None => unreachable!("BinOpLocalLocal with empty frame stack"),
                };
                // Same str-singleton operator-override gate as
                // Op::BinOp above (assert_equal's `exp == act`
                // compiles to this superinstruction).
                if self.any_str_singletons
                    && matches!(&a, Value::Str(s)
                        if self.str_singletons.contains_key(&(std::rc::Rc::as_ptr(s) as usize)))
                {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                    return Ok(true);
                }
                // A String-SUBCLASS instance (class_tag, e.g. bcrypt's
                // Password) may override operators — operator SYNTAX
                // otherwise skips user tables. Route to do_call so the
                // subclass method-lookup gate runs (and falls back to the
                // primitive when there's no override). Plain strings have
                // no tag and stay on the fast path below.
                if matches!(&a, Value::Str(s) if s.class_tag.borrow().is_some()) {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                    return Ok(true);
                }
                if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
                    // Same divide/mod-by-zero guard as the other BinOp
                    // arms — `n / m` with a zero RHS raises rather than
                    // panicking the host process.
                    if matches!(kind, BinOpKind::Div | BinOpKind::Mod) && *y == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let v = match kind.apply_int(*x, *y) {
                        Some(v) => v,
                        // Overflow on Add/Sub/Mul — promote to BigInt.
                        // `bigint_arith` returns `Some` for Int operands;
                        // the `None` arm is unreachable (kept off the panic
                        // budget via `unreachable!`).
                        #[cfg(feature = "bignum")]
                        None => match self.bigint_arith(kind, &a, &b) {
                            Some(r) => r?,
                            None => unreachable!("bigint_arith None for Int operands"),
                        },
                        #[cfg(not(feature = "bignum"))]
                        None => unreachable!("apply_int returns None only when bignum is on"),
                    };
                    self.stack.push(v);
                } else if let Some(v) = self.try_bigint_binop(kind, &a, &b)? {
                    self.stack.push(v);
                } else if let Some(v) = self.try_rational_binop(kind, &a, &b)? {
                    self.stack.push(v);
                } else if let Some(v) = primitive_call(&a, kind.name(), std::slice::from_ref(&b), self.max_value_bytes).map_err(|e| self.trap(e))? {
                    self.stack.push(v);
                } else {
                    self.stack.push(a);
                    self.stack.push(b);
                    let name_id = self.interner.intern(kind.name());
                    self.do_call(name_id, 1, false, u16::MAX)?;
                }
            }
            Op::Return => {
                // A method/begin `ensure` must run when the frame exits
                // via `return` (CRuby) — the direct pop below would skip
                // it. When the returning frame still has a pending
                // `is_ensure` handler, route through `begin_method_break`:
                // the same ensure-walking machinery used for non-local
                // (block) returns runs every pending ensure body
                // (suspending into each, resumed by Op::EndEnsure) and
                // then pops the frame, pushing the return value. Plain
                // returns (no ensure — the overwhelming majority) keep
                // the fast direct-pop path below.
                let has_ensure = self.frames.last()
                    .and_then(|fr| fr.aux.as_ref())
                    .is_some_and(|a| a.rescues.iter().any(|h| h.is_ensure));
                if has_ensure {
                    let ret = self.stack.pop().unwrap_or(Value::Nil);
                    let target = self.frames.len() - 1;
                    self.begin_method_break(ret, target)?;
                    // Either suspended into an ensure body (frame still
                    // present, ip at the handler) or landed (frame
                    // popped). Continue unless the stack is now empty.
                    return Ok(!self.frames.is_empty());
                }
                let f = self.frames.pop().expect("ICE: Return no frame");
                // Frame-local `$~`: a method frame saved its caller's
                // last-match on entry (block frames carry `None` and
                // are transparent — they share the enclosing method's
                // `$~`). Restore it now so the regex match a callee ran
                // internally doesn't leak back into the caller's `$1`,
                // `$2`, … (CRuby makes `$~` method-local). The save +
                // nil-reset happens in `enter_method_match_scope` at
                // each method-frame push.
                #[cfg(feature = "regex")]
                if let Some(saved) = f.saved_last_match {
                    self.last_match = saved.map(|b| *b);
                }
                // `$!` (errinfo) is dynamically scoped: a `return` out
                // of a rescue body abandons the begin region(s) whose
                // `Op::ExitBegin` would have reverted `$!`. Restore it to
                // the value saved at the OUTERMOST still-open begin in
                // this frame — that snapshot equals `$!` as of method
                // entry, so the caller's errinfo is unaffected by any
                // exception this method handled internally. (No open
                // begin → this method never touched `$!`, nothing to do.)
                if let Some(saved) = f
                    .aux
                    .as_ref()
                    .and_then(|a| a.begin_rescue_depths.first())
                    .map(|b| b.saved_dollar_bang.clone())
                {
                    self.globals.insert(self.sym_bang, saved);
                }
                // Per-invocation block-locals model: writes to
                // outer-scope slots (slot < block.param_start)
                // are propagated AT-WRITE-TIME via the
                // `propagate_outer_write` helper at every
                // `Op::StoreLocal` / `Op::IncLocalNoPush` site,
                // rather than via a bulk write-back here. A bulk
                // copy at Op::Return would CLOBBER outer-slot
                // mutations performed by OTHER code paths
                // (`define_method`-installed closures dispatch
                // through `m.closure.captured`, which IS the
                // outer Rc, so their writes hit the parent
                // directly; a stomp-copy here would replace those
                // with this block frame's stale snapshot). The
                // block_writeback field remains useful for
                // `find_lexical_owner_frame` (Op::Yield /
                // Op::ReturnMethod's lexical-method walk) — that's
                // its remaining role.
                let ret = self.stack.pop().unwrap_or(Value::Nil);
                self.stack.truncate(f.base_sp);
                if f.is_class_body {
                    let cls = self.class_stack.pop().expect("ICE: class_stack empty on class-body return");
                    self.class_visibility_stack.pop();
                    self.module_function_active_stack.pop();
                    // A REAL eigenclass body (`class << obj; …; end` run via
                    // OpenSingletonClass) evaluates to its LAST expression
                    // (CRuby): `(class << obj; ancestors; end)` yields the
                    // ancestors, not the eigenclass. A class's eigenclass
                    // carries `singleton_target`; an object's eigenclass
                    // (heap.ensure_singleton_class) doesn't, but its name is
                    // the `#<Class:…>` form. A regular `class`/`module` body
                    // keeps rubyrs's return-the-class behaviour.
                    if cls.singleton_target.borrow().is_some()
                        || cls.name.starts_with("#<Class:")
                    {
                        self.stack.push(ret);
                    } else {
                        self.stack.push(Value::Class(cls));
                    }
                } else if let Some(replacement) = f.swap_return {
                    self.stack.push(replacement);
                } else {
                    self.stack.push(ret);
                }
                let done = self.frames.is_empty();
                // Recycle this frame's locals cell for the next call
                // (skipped automatically when a closure still shares it —
                // see `recycle_frame_locals`'s strong_count guard), or
                // truncate its arena segment for a Stack frame.
                self.release_frame_locals(f.locals);
                if done {
                    return Ok(false);
                }
            }
            Op::ReturnMethod => {
                // Pop the value but don't pop the frame here —
                // dispatch / dispatch_until's top-of-loop check
                // sees `method_return` and unwinds the right
                // number of frames atomically. Doing it here
                // would skip the block frames between us and the
                // enclosing method.
                let v = self.stack.pop().unwrap_or(Value::Nil);
                self.method_return = Some(v);
                self.sync_control_signals();
                // Snapshot the lexical-owner identity. The current
                // frame is the block where `return` fired; its
                // BlockHandle's `captured` slot points at the
                // locals Vec of the method that lexically created
                // the block. The unwind walker uses `Rc::ptr_eq`
                // to find that method frame — NOT just the nearest
                // method frame (which could be the yielding
                // caller, e.g. `Array#each`'s parent in
                // `outer { return }` shapes). (TRY_RUNS pass-10
                // layer #4.)
                //
                // Since `invoke_block` now installs a FRESH per-
                // invocation locals Vec on the block frame (with
                // the original `captured` retained on
                // `block_writeback`), the top frame's `locals`
                // Rc is no longer the same Rc the lexical owner
                // method uses. Walk the writeback chain — handles
                // arbitrary block nesting (block inside block
                // inside method) where each enclosing block also
                // has a fresh per-invocation Vec — and stash the
                // ULTIMATE owner's Rc. That way the downstream
                // unwind walker's `Rc::ptr_eq(&f.locals, &rc)`
                // hits the method frame directly without needing
                // to repeat the walk.
                // Locals-enum aware version of the seed walk. Every
                // frame this can touch is `Shared` by construction:
                // the top frame is a block / class body / toplevel
                // (method bodies emit Op::Return, not ReturnMethod),
                // and a found owner lexically contains the block →
                // its proto has Op::CreateBlock → never Stack.
                let owner_locals = {
                    let top = self
                        .frames
                        .last()
                        .expect("ICE: ReturnMethod with empty frame stack");
                    match top.locals.as_shared() {
                        Some(rc) => {
                            let seed = rc.clone();
                            // Return-specific walk: stops at the nearest
                            // enclosing LAMBDA frame (lambda `return` is
                            // local) before falling through to the method.
                            match self.find_return_target(&seed) {
                                Some(idx) => self.frames[idx].locals.as_shared().cloned(),
                                // Block escaped its scope (e.g. saved as
                                // a Proc and called after its lexical
                                // method returned) — keep the seed so
                                // the unwind walker's no-match branch
                                // falls back to the legacy behaviour
                                // (LocalJumpError surfaced by Tier-1's
                                // missing model), exactly as before.
                                None => Some(seed),
                            }
                        }
                        // Unreachable by construction (see above) —
                        // degrade to the walkers' None fallback.
                        None => None,
                    }
                };
                self.method_return_locals = owner_locals;
            }
        }
        Ok(true)
    }

    /// Lazy-build (or return cached) ObjId for the script-visible
    /// `ENV` Hash. Shared between `Op::LoadConst("ENV")` and
    /// `Op::LoadConstChain`'s bare-name fallback so the same
    /// constant resolves identically whether referenced bare at
    /// toplevel (`ENV`) or inside a nested class body where the
    /// chain walk previously failed (`Foo::Bar::ENV` → falls
    /// through to bare `ENV`).
    ///
    /// ADR 0017 Rule 1+2: the ENV map a script sees is exactly
    /// the one the host provided via `Config::env` — never the
    /// host process's real env vars. `None` (default) → empty
    /// Hash; the CLI binary `rubyrs` populates from
    /// `std::env::vars()` to preserve `rubyrs script.rb`
    /// ergonomics. Cached for the lifetime of the Vm so all
    /// `ENV` reads see a single object — writes via `ENV[k] = v`
    /// mutate the snapshot but not anything host-side
    /// (documented divergence).
    ///
    /// Order matters: do the fallible `maybe_gc` + `check_alloc()?`
    /// BEFORE consuming `env_override`. Calling `take()` first
    /// and then trapping on heap cap would permanently drop the
    /// host-injected ENV map (the `?` early-return preserves no
    /// override state), and any subsequent `ENV` access would
    /// rebuild as empty — a silent capability loss the host has
    /// no way to recover from.
    pub(crate) fn env_hash_or_init(&mut self) -> Result<ObjId, Trap> {
        if let Some(id) = self.env_hash {
            return Ok(id);
        }
        self.maybe_gc();
        self.check_alloc()?;
        // ADR 0017 Rule 1 requires deterministic iteration.
        // `Config::env: HashMap` has randomised hash order, so
        // collect entries into a key-sorted Vec before
        // materialising the Ruby Hash (which preserves insertion
        // order); otherwise `ENV.each` / `ENV.to_a` / `ENV.inspect`
        // would vary across runs even for identical host injection.
        //
        // `take()` consumes the override on first build (now that
        // we know alloc will succeed): once the Ruby Hash is
        // allocated it IS the canonical ENV, so keeping a second
        // copy on `Vm` would just retain duplicate memory and
        // force per-entry String clones every time. Moving the
        // Strings into `Value::new_str` avoids both.
        let pairs: Vec<(Value, Value)> = match self.env_override.take() {
            Some(map) => {
                let mut entries: Vec<(String, String)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries
                    .into_iter()
                    .map(|(k, v)| (Value::new_str(k), Value::new_str(v)))
                    .collect()
            }
            None => Vec::new(),
        };
        let id = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
        self.env_hash = Some(id);
        Ok(id)
    }

    /// `Op::NewHash` body, extracted out of the step() mega-match so its
    /// code can't perturb the instruction layout of the other hot arms
    /// (see the call site). Drains the k/v values straight off the stack
    /// into the final `pairs` Vec — no intermediate `flat` buffer — then
    /// dedups in place. The drain holds `&mut self.stack`, so ruby_eql
    /// (needs `&self.heap`) runs AFTER it releases. Distinct keys (the
    /// common case) hit no `remove`. Last-write-wins, strict eql?,
    /// first-occurrence position — matches CRuby (`{a:1,a:2}` → `{a:2}`,
    /// `{1.0=>:a, 1=>:b}` keeps size 2).
    #[inline(never)]
    fn op_new_hash(&mut self, n: usize) -> Result<(), Trap> {
        self.maybe_gc();
        self.check_alloc()?;
        let split = self.stack.len() - n * 2;
        // Keys that override `hash`/`eql?` (e.g. zeitwerk's non-hashable test
        // modules) need Ruby-level handling: CRuby calls `key.hash` on insert
        // (so a wrong-arity `hash` raises here) and `key.eql?` for collisions.
        let hash_sym = self.interner.intern("hash");
        let eql_sym = self.interner.intern("eql?");
        let has_user = (0..n)
            .any(|i| self.key_needs_ruby_hash(&self.stack[split + i * 2], hash_sym, eql_sym));
        if has_user {
            // Keys are still on the stack (GC-rooted) for these dispatches.
            for i in 0..n {
                let k = self.stack[split + i * 2].clone();
                if let Some(m) = self.key_user_method(&k, hash_sym) {
                    self.call_resolved_method(m, k, vec![])?;
                }
            }
        }
        let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(n);
        {
            let mut d = self.stack.drain(split..);
            while let (Some(k), Some(v)) = (d.next(), d.next()) {
                pairs.push((k, v));
            }
        }
        if has_user {
            // Pin the drained pairs across the `eql?` dispatch (they left the
            // GC-rooted stack), then dedup with Ruby equality.
            let mut g = crate::vm::PinGuard::new(self);
            for (k, v) in &pairs {
                g.pin(k.clone());
                g.pin(v.clone());
            }
            let mut i = 0;
            while i < pairs.len() {
                let mut j = i + 1;
                while j < pairs.len() {
                    let (ki, kj) = (pairs[i].0.clone(), pairs[j].0.clone());
                    if g.vm.keys_ruby_eql(&ki, &kj, eql_sym)? {
                        pairs[i].1 = pairs[j].1.clone();
                        pairs.remove(j);
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
        } else {
            let mut i = 0;
            while i < pairs.len() {
                let mut j = i + 1;
                while j < pairs.len() {
                    if pairs[j].0.ruby_eql(&pairs[i].0, &self.heap) {
                        pairs[i].1 = pairs[j].1.clone();
                        pairs.remove(j);
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
        }
        let id = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(pairs)));
        self.stack.push(Value::Hash(id));
        Ok(())
    }
}
