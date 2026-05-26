use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::Proto;
use crate::error::Trap;
// `RubyError` is only referenced from the bignum binop paths below;
// the wasi-only `cext_require` alt that previously also needed it
// moved to `vm/cext_wasi.rs`. Keep the gate aligned with the
// remaining bignum consumers.
#[cfg(feature = "bignum")]
use crate::error::RubyError;
use crate::heap::Heap;
use crate::intern::{Interner, SymId};
use crate::value::{Class, Method, ObjId, Value, Visibility};

mod array;
#[cfg(feature = "cext")]
mod cext;
#[cfg(all(feature = "cext", target_os = "wasi"))]
mod cext_wasi;
mod dispatch;
mod fileops;
mod gc;
mod hash;
mod iter;
mod kernel;
mod lookup;
mod numeric;
mod primitive;
mod raise;
mod range;
mod sprintf;
mod step;
mod string;
mod util;
#[cfg(all(feature = "cext", not(target_os = "wasi")))]
pub(crate) use cext::with_vm_ptr_set;
pub(crate) use lookup::{class_is_a, flatten_ancestors, CallCache};
pub(crate) use primitive::primitive_call;
pub(crate) use sprintf::ruby_sprintf;
pub(crate) use util::{value_cmp_v, value_cmp_v_heap, vec_nil, visibility_from_name};

// ---------- VM ----------



pub(crate) struct Frame {
    pub(crate) proto_idx: usize,
    pub(crate) ip: usize,
    pub(crate) locals: Rc<RefCell<Vec<Value>>>,
    pub(crate) self_val: Value,
    pub(crate) base_sp: usize,
    pub(crate) is_class_body: bool,
    pub(crate) swap_return: Option<Value>,
    /// Block passed to this method, as a heap-managed `Value::Block`
    /// id. Used by `yield`. `None` if the method was called without
    /// a block. Since P2-13 the block lives in the GC heap and we
    /// reference it by `ObjId` — earlier code held an
    /// `Rc<BlockHandle>` here which could cycle.
    pub(crate) block_arg: Option<ObjId>,
    /// Defining class of the method this frame is running.
    /// Used by `Op::Super` — `super` lookup starts at this
    /// class's superclass, not at `self.class.superclass` (the
    /// latter would re-find the current method on a sub-class
    /// instance, causing infinite recursion through the
    /// chain). `None` for blocks, toplevel `<main>`, class
    /// bodies; only methods set this.
    pub(crate) defining_class: Option<Rc<Class>>,
    /// True for frames pushed by `Vm::invoke_block` (the frame
    /// for a `do…end` / `{ … }` body). Used by the non-local
    /// `return`-from-block path: when `Op::ReturnMethod` sets
    /// `method_return`, the dispatch loops pop frames while
    /// `is_block` is true, then pop one more frame to exit the
    /// enclosing method. Method frames, class bodies, and the
    /// toplevel `<main>` keep `false`.
    pub(crate) is_block: bool,
    /// Count of positional args the caller supplied. Method
    /// dispatch (`invoke_method_with_block`) sets this to
    /// `positional_take`; `Op::JumpIfArgGiven(slot, off)` consults
    /// it to decide whether `slot` was caller-supplied or left
    /// for the default-arg prologue to fill. Block / class-body
    /// / toplevel frames all use 0 — they don't carry an arity
    /// model that the prologue op would consult.
    pub(crate) n_given_positional: u16,
    pub(crate) rescues: Vec<RescueHandler>,
    /// Stack of `rescues.len()` snapshots, one per enclosing
    /// `while` loop currently active in this frame. `Op::EnterLoop`
    /// pushes; `Op::ExitLoop` pops. `Op::BreakLoop` reads the top
    /// to know how many handler entries to discard before jumping
    /// to the loop's end label. Empty for frames with no active
    /// loop and for frames where `break` instead signals an
    /// iteration-driver / block return (the existing `Op::Break`
    /// path stays untouched).
    pub(crate) loop_rescue_depths: Vec<usize>,
    /// Parallel stack to `loop_rescue_depths`: `stack.len()` at the
    /// moment each enclosing `while`'s `Op::EnterLoop` ran. Used by
    /// `continue_loop_transfer`'s landing path to truncate any
    /// operand-stack residue accumulated inside the body — most
    /// importantly the exception value `unwind_with_exception`
    /// pushes when entering an ensure handler (which `break`/`next`
    /// from inside that handler would otherwise leave stranded).
    /// Kept in lock-step with `loop_rescue_depths`: same push site
    /// (EnterLoop), pop site (ExitLoop), and truncate site
    /// (rescue/ensure match in `unwind_with_exception`).
    pub(crate) loop_stack_depths: Vec<usize>,
}

/// In-flight structured `break`/`next` walking through an
/// `ensure` chain. The `kind` carries the break value (or `Next`
/// for `next`); `target_ip` is the instruction the transfer
/// lands at once every intervening `is_ensure` handler has run;
/// `target_loop_depth` is the `loop_rescue_depths` length the
/// frame should have after the transfer (entries pushed by
/// `EnterLoop`s the transfer is escaping out of get truncated).
/// One slot per VM is enough — break/next transfers are single-
/// frame and complete (or get superseded by a real raise)
/// before any new one can start.
pub(crate) struct LoopTransfer {
    pub(crate) kind: LoopTransferKind,
    pub(crate) target_ip: usize,
    pub(crate) target_rescues_len: usize,
    pub(crate) target_loop_depth: usize,
    /// `stack.len()` at the time `Op::EnterLoop` ran for this
    /// transfer's target loop. On landing the stack is truncated
    /// to this depth before the break value (if any) is pushed —
    /// flushes any operand-stack residue the body accumulated,
    /// including the exception that `unwind_with_exception` pushed
    /// when it entered an ensure handler we're now `break`ing out
    /// of. Without this, `while; begin; raise; ensure; break; end;
    /// end` leaks the exception value on the operand stack until
    /// the surrounding frame pops.
    pub(crate) target_stack_depth: usize,
}

pub(crate) enum LoopTransferKind {
    Break { value: Value },
    Next,
}

/// RAII guard for `Vm.pinned`. Native-side code that needs heap
/// values to survive an intervening `maybe_gc` / `?` early-return
/// constructs one of these, calls `.pin(v)` for every value it
/// wants kept alive, and accesses the VM through `g.vm.foo()` while
/// the guard is in scope. When the guard drops — including on the
/// `?` unwind path — it pops exactly the values it pinned, leaving
/// `pinned` at the same length it had on entry.
///
/// Why this exists: before P0-2, every iterator driver (Array#each,
/// Hash#to_a, the Enumerable filtering family, the
/// `Class.new(args)` allocator) did `self.pinned.push(...); ...; ?;
/// ...; self.pinned.pop();` by hand. The `?` operator could short-
/// circuit past the pop on a raise from a host fn or a fuel trap,
/// leaving dead values on `pinned` that the GC then kept marking as
/// live — slow leak, hard to spot. With this guard the pop is
/// unconditional.
pub(crate) struct PinGuard<'a> {
    pub(crate) vm: &'a mut Vm,
    count: usize,
}

impl<'a> PinGuard<'a> {
    pub(crate) fn new(vm: &'a mut Vm) -> Self {
        Self { vm, count: 0 }
    }
    pub(crate) fn pin(&mut self, v: Value) {
        self.vm.pinned.push(v);
        self.count += 1;
    }
}

impl Drop for PinGuard<'_> {
    fn drop(&mut self) {
        for _ in 0..self.count { self.vm.pinned.pop(); }
    }
}

pub(crate) struct RescueHandler {
    pub(crate) handler_ip: usize,
    pub(crate) stack_depth: usize,
    pub(crate) bind_slot: Option<u16>,
    /// When true this entry was emitted by `Op::PushEnsure` and the
    /// unwinder pushes the exception onto the operand stack (rather than
    /// binding to a local). The ensure body re-raises with `Op::Raise`.
    pub(crate) is_ensure: bool,
    /// `loop_rescue_depths.len()` snapshot at the moment this handler
    /// was pushed. When an exception fires and this handler catches,
    /// the unwinder truncates `loop_rescue_depths` back to this value
    /// so that `Op::EnterLoop` entries pushed by `while` loops the
    /// exception is escaping out of don't leak. Without this, a
    /// later `BreakLoop` would consult the orphan top entry and
    /// pop the wrong number of rescue handlers / jump from the
    /// wrong join point.
    pub(crate) loop_depth_at_push: usize,
    /// Class filter for `rescue`. `None` means catch-all (used for
    /// `ensure` and as a future hook for internal/host-only handlers).
    /// `Some(cls)` means the handler only fires when the raised
    /// exception's class is `cls` or a descendant. Bare `rescue` (no
    /// class listed) populates this with `StandardError`, so any
    /// exception that intentionally lives outside the StandardError
    /// subtree (e.g. `ResourceExhausted`) cannot be silently swallowed
    /// by `rescue => e`. Explicit `rescue ClassName => e` carries the
    /// resolved Class here. Multi-class clauses (`rescue A, B => e`)
    /// emit one handler per class — same handler_ip, same bind_slot —
    /// so each entry holds exactly one filter.
    pub(crate) filter_class: Option<Rc<Class>>,
}

pub(crate) type HostFn = dyn Fn(&[Value]) -> Result<Value, Trap>;
/// v2 host-fn closure shape. Same return/args as `HostFn`, but with
/// a leading `&HostCtx` that exposes heap reads (`resolve_array`,
/// `resolve_hash`). Introduced so embed hosts can consume the
/// heap-y `Value::Array` / `Value::Hash` shapes that the v1
/// `&[Value]`-only signature couldn't reach. See
/// `Runtime::register_fn_v2`.
pub(crate) type HostFnV2 = dyn Fn(&crate::HostCtx, &[Value]) -> Result<Value, Trap>;

/// Storage slot for either signature. Held by `Vm::host_fns` so a
/// single dispatch site can resolve a name without the embed host
/// having to pick between two maps. cext stays on the v1-only
/// type alias (`Rc<HostFn>`) — its registration path predates v2
/// and doesn't need heap reads.
pub(crate) enum HostFnSlot {
    V1(Rc<HostFn>),
    V2(Rc<HostFnV2>),
}

impl Clone for HostFnSlot {
    fn clone(&self) -> Self {
        match self {
            HostFnSlot::V1(f) => HostFnSlot::V1(f.clone()),
            HostFnSlot::V2(f) => HostFnSlot::V2(f.clone()),
        }
    }
}

/// Side-channel record of the most recent successful regex match.
/// Holds owned strings so the GC need not walk it; the cost is
/// one `.to_string()` per capture group on each successful match.
/// `caps[i]` is the i-th *parenthesised* group (1-indexed via
/// `$1` etc.); `None` means the group did not participate. The
/// vector length is always `re.captures_len() - 1` after a hit.
#[cfg(feature = "regex")]
#[derive(Debug, Clone)]
pub(crate) struct LastMatch {
    pub(crate) whole: String,
    pub(crate) caps: Vec<Option<String>>,
    /// Original input string the match was performed against, plus
    /// the byte span of the whole match within it. Required to back
    /// `` $` `` (pre-match) and `$'` (post-match) — those return
    /// slices of the input that we'd otherwise have to recompute.
    /// `pre_match` is `input[..m_start]`, `post_match` is
    /// `input[m_end..]`.
    pub(crate) input: String,
    pub(crate) m_start: usize,
    pub(crate) m_end: usize,
}

pub(crate) struct Vm {
    pub(crate) protos: Vec<Proto>,
    pub(crate) interner: Interner,
    pub(crate) classes: HashMap<SymId, Rc<Class>>,
    /// Bare constant assignments (`FOO = expr`), kept in a separate
    /// table from `classes` so `class Foo` and `Foo = 42` can coexist
    /// without collision. `Op::LoadConst` resolves classes first,
    /// then this table, then the `ENV` special-case — chosen for
    /// implementation simplicity, NOT to mirror CRuby (CRuby would
    /// emit "already initialized constant" and reassign). If you
    /// need to shadow a class with a constant, pick a different name.
    pub(crate) constants: HashMap<SymId, Value>,
    /// Files already loaded via `require_relative` — keyed by
    /// canonical path. Suppresses re-loading on subsequent calls
    /// the same way CRuby's `$LOADED_FEATURES` does. The Set
    /// shape (no associated value) is intentional: rubyrs doesn't
    /// expose the list to script code yet, and the "true → false"
    /// return semantic only needs membership.
    ///
    /// Gated to non-wasi: `require` / `require_relative` short-
    /// circuit to a trap on wasm32-wasi (no file I/O), so the
    /// field would be dead code there and trip `-D dead_code`
    /// under `--no-default-features` (the only meaningful wasi
    /// build shape).
    #[cfg(not(target_os = "wasi"))]
    pub(crate) loaded_features: std::collections::HashSet<std::path::PathBuf>,
    /// Set of stdlib stub names (`uri`, `logger`, `json`, ...)
    /// that have been "loaded" via the lenient require stub.
    /// CRuby's `require` returns `true` on first load and
    /// `false` on every subsequent call for the same feature;
    /// rubyrs was returning `true` every time because the
    /// stub didn't track per-name state. Tracked separately
    /// from `loaded_features` (which keys on canonical
    /// `PathBuf`) because stubs have no path. Same wasi-gate
    /// for the same reason — `require` is a trap on wasm32-
    /// wasi so the field would be dead code.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) loaded_stdlib_stubs: std::collections::HashSet<String>,
    /// Per-call-site inline-cache counter. Each compiled `Op::Call`
    /// gets a unique u16 slot id; the Vm side allocates
    /// `call_caches[id]` lazily. Lives on the Vm so kernel
    /// builtins (e.g. `require_relative`) that compile new Ruby
    /// source at runtime can advance the counter without
    /// round-tripping through Runtime.
    pub(crate) cache_counter: u32,
    /// User-defined global variables (`$foo = 1; puts $foo`).
    /// Keyed by SymId of the name including the leading `$`.
    /// Reads of unknown globals return Nil (matches CRuby's
    /// lenient "uninitialized global variable" silent default).
    /// Special globals — `$$` (process pid), `$0` (script name),
    /// regex backrefs `$~` / `$1`–`$9`, separators `$,` / `$;` —
    /// are not stored here; `Op::LoadGlobal` intercepts a known
    /// set and returns the computed value. Plain user globals
    /// fall through to this table.
    pub(crate) globals: HashMap<SymId, Value>,
    pub(crate) toplevel_methods: HashMap<SymId, Rc<Method>>,
    /// Toplevel `@@foo` fallback. CRuby raises RuntimeError on
    /// class-variable use outside a class body; rubyrs takes the
    /// lenient route consistent with our ivar / global handling.
    /// Inside a class body / instance method / class method, the
    /// surrounding `Rc<Class>` owns the cvar; this table catches
    /// the toplevel-only `@@x` writes scripts occasionally use
    /// for cache-like state at file scope.
    pub(crate) toplevel_cvars: HashMap<SymId, Value>,
    /// Heap-allocated `$LOAD_PATH` / `$:` Array. Lazily
    /// initialised on first read so cold-eval scripts that
    /// never touch it pay zero startup cost. Scripts can
    /// `$LOAD_PATH.unshift(dir)` — mutations on this ObjId
    /// land in the same heap Array the require dispatcher
    /// later reads from. GC-rooted in `maybe_gc`.
    pub(crate) load_path: Option<ObjId>,
    pub(crate) host_fns: HashMap<SymId, HostFnSlot>,
    /// C-ext singleton-method dispatch table. Indexed by
    /// `(class joined name, method SymId)`. Populated by
    /// `Vm::cext_require` whenever a C ext calls
    /// `rb_define_singleton_method`; consulted by `do_call` when
    /// the receiver is `Value::Class(c)`.
    #[cfg(feature = "cext")]
    pub(crate) cext_class_methods: HashMap<String, HashMap<SymId, Rc<HostFn>>>,
    /// L3-C: instance-method dispatch table for cext-registered
    /// methods (`rb_define_method`). Mirrors `cext_class_methods`'s
    /// shape but consulted when the receiver is `Value::Object(id)`
    /// whose class joined-name matches. Stores raw registration
    /// data instead of a HostFn closure because the receiver isn't
    /// known at registration time; the dispatch site assembles
    /// `cext_dispatch(..., CextSelfHandle::Object(recv))` inline.
    #[cfg(all(feature = "cext", not(target_os = "wasi")))]
    pub(crate) cext_instance_methods: HashMap<String, HashMap<SymId, crate::vm::cext::CextMethodReg>>,
    pub(crate) class_stack: Vec<Rc<Class>>,
    /// Per-class-body visibility mode, parallel to `class_stack`.
    /// Pushed `Public` on `Op::DefClass` and popped when the class
    /// body returns. Read by `Op::DefMethod` to stamp new methods
    /// with the current visibility, and mutated by the no-arg
    /// `private` / `protected` / `public` calls.
    pub(crate) class_visibility_stack: Vec<Visibility>,
    /// Compiled-regex cache. Keyed by the interned source-string
    /// SymId; first `LoadRegex` for a given pattern compiles and
    /// caches, subsequent loads return the same Rc. Cfg-gated on
    /// the `regex` feature (ADR 0017 Rule 3) — disappears with
    /// `--no-default-features`.
    #[cfg(feature = "regex")]
    pub(crate) regex_cache: HashMap<SymId, Rc<regex::Regex>>,
    /// Parsed-BigInt cache for `Op::LoadBigInt`. Keyed by the
    /// interned decimal-string SymId; first load decodes via
    /// `BigInt::from_str`, subsequent loads return the cached
    /// `Rc<BigInt>` (a fresh `HeapObj::BigInt(b.clone())` is
    /// allocated per load so the heap-side identity stays
    /// per-Value, but the parse work is amortised).
    #[cfg(feature = "bignum")]
    pub(crate) bigint_lit_cache: HashMap<SymId, Rc<num_bigint::BigInt>>,
    /// Last successful regex match — populated by `=~`,
    /// `String#match`, and `Regexp#===` when they hit, cleared
    /// when they miss. Source of truth for `$~` and `$1`..`$N`
    /// (NumberedReferenceReadNode — any positive index, matching
    /// CRuby; `$10`+ are valid too) reads in `LoadGlobal`. Owned
    /// strings rather than
    /// a heap ObjId so we don't have to wire a GC-walk root for
    /// what is conceptually a fast side-channel; `$~` materialises
    /// a fresh MatchData instance on demand. Cfg-gated on `regex`
    /// — without the feature there are no successful matches to
    /// record.
    #[cfg(feature = "regex")]
    pub(crate) last_match: Option<LastMatch>,
    /// Lazily-built ENV Hash, shared across every `ENV`
    /// reference. Set on first `LoadConst("ENV")` and reused
    /// thereafter so script code observes a single mutable
    /// snapshot of the env map the host provided via
    /// `Config::env`. With `Config::env = None`, the lazy build
    /// produces an empty Hash.
    pub(crate) env_hash: Option<ObjId>,
    /// Host-injected ENV map (from `Config::env`). `None` means
    /// "expose an empty ENV Hash" — the script's `ENV[k]` reads
    /// see no host process env vars. ADR 0017 Rule 1+2 closure
    /// for the previous `std::env::vars()` deviation. CLI binary
    /// fills this from `std::env::vars()` to preserve `rubyrs
    /// script.rb` ergonomics.
    pub(crate) env_override: Option<HashMap<String, String>>,
    /// Host-injected PID exposed to scripts via `$$` (from
    /// `Config::pid`). `None` means `$$` returns `0` (sentinel).
    /// ADR 0017 Rule 1 closure for the previous
    /// `std::process::id()` deviation.
    pub(crate) pid: Option<i64>,
    /// Host-injected wall-clock source for `Time.now`. `None`
    /// means `__time_now_raw` raises (deterministic Tier 1
    /// default); CLI binary fills this from
    /// `std::time::SystemTime::now()`. ADR 0017 Rule 1 closure
    /// for the previous "no Time class at all" status.
    pub(crate) time_now: Option<std::sync::Arc<dyn Fn() -> (i64, u32) + Send + Sync>>,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) heap: Heap,
    /// Native-code holding pen for heap values across GC points; see ADR 0005.
    pub(crate) pinned: Vec<Value>,
    pub(crate) stdout: Box<dyn std::io::Write>,
    pub(crate) stress_gc: bool,
    /// Remaining fuel; `Some(0)` means exhausted, `None` means unlimited.
    /// Decremented per op dispatched. Configured by `Config::fuel`.
    pub(crate) fuel: Option<u64>,
    /// Maximum simultaneously-live frames. `frames.push()` checks this
    /// against `frames.len()` before pushing. Default `None` is unlimited.
    pub(crate) max_frames: Option<usize>,
    /// Absolute wall-clock instant past which `eval` traps with
    /// `ResourceExhausted("wall-clock deadline exceeded")`. `None`
    /// means unlimited. Computed at `Runtime::eval` entry from the
    /// `Config::deadline` duration. Checked every 1024 ops (cheap
    /// enough that the syscall amortises out).
    pub(crate) deadline_at: Option<std::time::Instant>,
    /// Lightweight counter incremented per op so deadline checks
    /// only call `Instant::now()` periodically. Wraps; we only
    /// inspect the low bits.
    pub(crate) op_counter: u32,
    /// Cap on distinct interned symbols (P2-14b). `None` means
    /// unlimited. Checked at runtime intern sites (`to_sym`) before
    /// the actual `intern()` call; compile-time intern is not
    /// capped because it's already bounded by source size.
    pub(crate) max_symbols: Option<usize>,
    /// Per-value byte cap (P2-14c). Defends against single
    /// values that hog memory (`"a" * 10_000_000`, `arr <<` in
    /// a tight loop). Checked at mutation sites; see
    /// `Config::max_value_bytes` for the model.
    pub(crate) max_value_bytes: Option<usize>,
    /// Per-call-site monomorphic inline cache for method dispatch on
    /// `Value::Object`. One slot per `Op::Call(...,cache_id)` /
    /// `Op::CallNoRecv` / `Op::CallBlock` / `Op::CallNoRecvBlock` site.
    /// Each entry remembers the (class identity, gen-at-time-of-cache,
    /// resolved Method) of the last successful lookup at that site.
    ///
    /// Lookups compare against the receiver's class pointer AND the
    /// current `method_gen`. Any `Op::DefMethod` bumps `method_gen`,
    /// which effectively invalidates every cache entry — re-fill is
    /// lazy on the next call at each site.
    pub(crate) call_caches: Vec<CallCache>,
    pub(crate) method_gen: u32,
    /// `Op::Break` sets this; iteration drivers check and consume.
    pub(crate) break_signaled: bool,
    /// `Op::ReturnMethod` sets this with the value to return. Both
    /// `dispatch` and `dispatch_until` check it at the top of every
    /// iteration: if `Some`, they unwind frames (block frames first,
    /// then one method frame) and push the value as the method's
    /// return. This is CRuby's non-local-return-from-block
    /// semantics: `return` inside a `do…end` exits the enclosing
    /// method, not just the block.
    pub(crate) method_return: Option<Value>,
    /// In-flight `break`/`next` through `ensure` chain. Set by
    /// `Op::BreakLoop`/`Op::NextLoop` when an `is_ensure` handler
    /// sits between the source and the target; cleared once the
    /// transfer lands at its target loop label. `Op::EndEnsure`
    /// (emitted at the tail of every ensure handler body) reads
    /// this field to decide whether to keep walking the rescue
    /// chain or fall back to normal end-of-ensure exception
    /// re-raise. `unwind_with_exception` clears this field
    /// whenever a real exception starts unwinding — matching
    /// CRuby semantics where a `raise` inside an ensure body
    /// silently drops a pending break/next.
    pub(crate) pending_loop_transfer: Option<LoopTransfer>,
    /// One-shot flag set by a builtin that detected its caller was
    /// unwound past its own call-site (e.g. `require_relative` saw
    /// `unwind_with_exception` route control to an outer
    /// `rescue` handler mid-load). The do_call caller checks +
    /// clears this flag before doing `stack.push(builtin_result)`;
    /// pushing in this state would corrupt the rescue handler's
    /// stack (it's already at `base_sp` after unwind truncation).
    /// Distinct from `method_return` because that path keeps frames
    /// > until_depth, while rescue unwind drops below.
    pub(crate) suppress_call_result_push: bool,
    /// Single-shot flag set by the `send` / `__send__` recogniser
    /// (`vm/dispatch.rs`) right before re-entering dispatch.
    /// Consumed (`mem::replace(..., false)`) at the **dispatch
    /// boundary** — the very top of `do_call` / `do_call_block` —
    /// into a local that the Object-arm visibility check reads.
    /// Consumption is *not* at the check site itself: that would
    /// leak the flag whenever dispatch bottoms out before the
    /// Object arm (e.g. `send(:nonexistent)` on a primitive
    /// raising NoMethodError). CRuby parity: `send` may invoke
    /// methods of any visibility, but the bypass doesn't
    /// transitively apply — anything that method itself calls
    /// runs through the normal check.
    pub(crate) bypass_visibility_once: bool,
    /// Cached index into `protos` of the callable→Block
    /// forwarder. Lazily built on first `&callable` coercion in
    /// `do_call_block` (BoundMethod, CurriedProc, ...). The
    /// forwarder is a tiny proto whose body does
    /// `captured[0].call(*args)`; one instance is shared across
    /// every `&` call site so the allocation cost amortises to
    /// zero.
    pub(crate) callable_forwarder_proto: Option<usize>,
    /// Cached proto for `Method#>>` / `Method#<<`. Body does
    /// `outer.call(inner.(*args))`; three-locals layout
    /// (outer / inner / rest-args). Shared across all composition
    /// sites — same amortisation rationale as the bound-method
    /// forwarder above.
    pub(crate) method_compose_forwarder_proto: Option<usize>,
    /// Filename → source-text map, populated by Runtime before
    /// each `eval`. Used by `Method#source_location` (and any
    /// future Vm-side line-resolution) to convert a Span's
    /// byte offset back to a 1-based line number. Vm-only
    /// readers; Runtime owns the canonical map and clones the
    /// `Rc<str>` source bodies in (cheap, share-pointer).
    pub(crate) sources: std::collections::HashMap<std::rc::Rc<str>, std::rc::Rc<str>>,
}


impl Vm {
    pub(crate) fn new(protos: Vec<Proto>, interner: Interner) -> Self {
        Vm {
            protos,
            interner,
            classes: HashMap::new(),
            constants: HashMap::new(),
            #[cfg(not(target_os = "wasi"))]
            loaded_features: std::collections::HashSet::new(),
            #[cfg(not(target_os = "wasi"))]
            loaded_stdlib_stubs: std::collections::HashSet::new(),
            cache_counter: 0,
            globals: HashMap::new(),
            toplevel_methods: HashMap::new(),
            toplevel_cvars: HashMap::new(),
            load_path: None,
            host_fns: HashMap::new(),
            #[cfg(feature = "cext")]
            cext_class_methods: HashMap::new(),
            #[cfg(all(feature = "cext", not(target_os = "wasi")))]
            cext_instance_methods: HashMap::new(),
            class_stack: vec![],
            class_visibility_stack: vec![],
            #[cfg(feature = "regex")]
            regex_cache: HashMap::new(),
            #[cfg(feature = "regex")]
            last_match: None,
            #[cfg(feature = "bignum")]
            bigint_lit_cache: HashMap::new(),
            env_hash: None,
            env_override: None,
            pid: None,
            time_now: None,
            stack: Vec::with_capacity(1024),
            frames: vec![],
            heap: Heap::new(),
            pinned: Vec::new(),
            // ADR 0017 Rule 2 closure: default sink is silent
            // (`std::io::sink()`); hosts that want script output
            // routed somewhere call `Runtime::set_stdout` explicitly.
            // The CLI binary `rubyrs` wires it to process stdout in
            // `main.rs` so `rubyrs script.rb` behaves like CRuby.
            stdout: Box::new(std::io::sink()),
            // Default to false; Config-driven `stress_gc` flows in
            // via `Runtime::apply_config`. The previous `env::var`
            // read here meant `Vm::new` indirectly hit a wasi
            // import on wasm32-wasip1, which would violate wizer's
            // "no imports during init" rule (see PR #116 review)
            // and bake the wizer-time env into the snapshot rather
            // than respecting the user's runtime `STRESS_GC` setting.
            // Now the env read happens exactly once at the CLI
            // boundary (`main.rs::env_lookup("STRESS_GC")`), feeds
            // into `Config.stress_gc`, and reaches the Vm via
            // `apply_config`.
            stress_gc: false,
            fuel: None,
            max_frames: None,
            deadline_at: None,
            op_counter: 0,
            max_symbols: None,
            max_value_bytes: None,
            call_caches: Vec::new(),
            method_gen: 0,
            break_signaled: false,
            callable_forwarder_proto: None,
            method_compose_forwarder_proto: None,
            sources: HashMap::new(),
            method_return: None,
            pending_loop_transfer: None,
            suppress_call_result_push: false,
            bypass_visibility_once: false,
        }
    }





}


impl Vm {







    pub(crate) fn collection_call(&mut self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        Ok(match recv {
            Value::Array(id) => return self.array_collection_call(*id, name, args),
            Value::Hash(id) => return self.hash_collection_call(*id, name, args),
            Value::Str(s) => return self.string_collection_call(s.clone(), name, args),
            Value::Range(id) => return self.range_collection_call(*id, name, args),
            _ => None,
        })
    }



    /// Compare two values using built-in types first, then falling
    /// back to invoking the left-hand side's user-defined `<=>`.
    /// Returns `None` for incomparable pairs (built-in cross-type
    /// mismatches, or a user `<=>` that returns `nil`). Used by
    /// `Array#sort` so user classes that define `<=>` (typically
    /// via `include Comparable`) sort sensibly. Synchronously
    /// dispatches the user method by pushing a frame and running
    /// `dispatch_until` — the same pattern iterator drivers use.
    /// One step of nested lookup for `Hash#dig` / `Array#dig`.
    /// Hash receivers use `ruby_eq` key lookup; Array uses Int
    /// index (negative wraps from end). Anything else → nil so
    /// the caller can short-circuit cleanly.
    pub(crate) fn dig_step(&mut self, recv: &Value, key: &Value) -> Result<Value, Trap> {
        match recv {
            Value::Hash(id) => {
                let id = *id;
                // Direct hit first.
                {
                    let h = self.heap.hash(id);
                    if let Some(v) = h.iter()
                        .find(|(k, _)| k.ruby_eq(key, &self.heap))
                        .map(|(_, v)| v.clone())
                    {
                        return Ok(v);
                    }
                }
                // Missing key — CRuby's `Hash#dig` walks via `[]`
                // per step, which consults default_value first, then
                // default-block. Mirrors the `Hash#[]` missing-key
                // arm: scalar default returned as-is, block fired
                // with `(self_hash, key)` if no scalar default.
                if let Some(v) = self.heap.hash_default_value(id) {
                    return Ok(v);
                }
                if let Some(block_id) = self.heap.hash_default_block(id) {
                    let pre_frames = self.frames.len();
                    let mut g = PinGuard::new(self);
                    g.pin(Value::Hash(id));
                    g.pin(key.clone());
                    g.pin(Value::Block(block_id));
                    g.vm.invoke_block(block_id, vec![Value::Hash(id), key.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() {
                        return Ok(Value::Nil);
                    }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    // Same Proc-break-LocalJumpError semantics as
                    // the `Hash#[]` arm. See its comment for the
                    // rationale (stored block, not iterator yield).
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        return Err(g.vm.trap(crate::error::RubyError::LocalJumpError {
                            msg: "break from proc-closure".into(),
                        }));
                    }
                    return Ok(r);
                }
                Ok(Value::Nil)
            }
            Value::Array(id) => {
                if let Value::Int(i) = key {
                    let a = self.heap.array(*id);
                    let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                    Ok(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                } else {
                    Ok(Value::Nil)
                }
            }
            _ => Ok(Value::Nil),
        }
    }

    pub(crate) fn user_cmp(&mut self, a: &Value, b: &Value) -> Result<Option<std::cmp::Ordering>, Trap> {
        // Heap-aware fast path so Array#sort works on BigInt
        // arrays — value_cmp_v alone would return None for any
        // BigInt operand and force fall-through to the user `<=>`
        // method dispatch, which doesn't exist for primitives.
        if let Some(ord) = value_cmp_v_heap(a, b, &self.interner, &self.heap) {
            return Ok(Some(ord));
        }
        // Try the receiver's `<=>` method (user-defined). Only
        // Value::Object can have user methods; other receivers
        // would have been resolved by value_cmp_v above.
        if let Value::Object(id) = a {
            let cls = self.heap.class_of(*id);
            let spaceship = self.interner.intern("<=>");
            if let Some(m) = self.lookup_method_uncached(&cls, spaceship) {
                let pre_frames = self.frames.len();
                let mut g = PinGuard::new(self);
                g.pin(a.clone());
                g.pin(b.clone());
                g.vm.invoke_method(m, a.clone(), vec![b.clone()])?;
                g.vm.dispatch_until(pre_frames)?;
                let result = g.vm.stack.pop().unwrap_or(Value::Nil);
                drop(g);
                return Ok(match result {
                    Value::Int(n) if n < 0 => Some(std::cmp::Ordering::Less),
                    Value::Int(0) => Some(std::cmp::Ordering::Equal),
                    Value::Int(_) => Some(std::cmp::Ordering::Greater),
                    _ => None,
                });
            }
        }
        Ok(None)
    }


}


/// MatchData materialization — shared between `String#match`
/// (vm/string.rs) and the `$~` read path (vm/step.rs). Keeps one
/// source of truth for the @whole/@caps ivar shape, the
/// two-allocation cap accounting, and the "MatchData class not
/// loaded → nil" fallback. Cfg-gated on `regex` along with every
/// other consumer of `last_match`.
#[cfg(feature = "regex")]
impl Vm {
    pub(crate) fn materialize_match_data(
        &mut self,
        whole: String,
        caps: Vec<Value>,
    ) -> Result<Value, Trap> {
        self.maybe_gc();
        self.check_alloc()?;
        let caps_arr = self.heap.alloc(crate::heap::HeapObj::Array(caps));
        let cls_id = self.interner.intern("MatchData");
        let cls = match self.classes.get(&cls_id).cloned() {
            Some(c) => c,
            None => return Ok(Value::Nil),
        };
        // Second alloc — re-check the cap so a tight `heap.max_live`
        // budget that admitted `caps_arr` but not the Instance traps
        // cleanly rather than sneaking past the limit.
        self.check_alloc()?;
        let obj_id = self.heap.alloc(crate::heap::HeapObj::Instance(crate::value::Instance {
            class: cls,
            ivars: HashMap::new(),
            singleton_class: None,
        }));
        let whole_ivar = self.interner.intern("@whole");
        let caps_ivar = self.interner.intern("@caps");
        let inst = self.heap.instance_mut(obj_id);
        inst.ivars.insert(whole_ivar, Value::new_str(whole));
        inst.ivars.insert(caps_ivar, Value::Array(caps_arr));
        Ok(Value::Object(obj_id))
    }
}

/// Dispatch helpers used by the reduce-style accumulators in
/// `Array#sum`/`Range#sum`/`Array#inject`/`Range#inject`. Promotes
/// to BigInt on overflow when the feature is on; falls back to
/// wrapping when it's off. (The main `Op::BinOp` path doesn't go
/// through this helper — it inlines the same logic in step.rs
/// with the BinOpInt/BinOp fast paths because each instruction
/// already has the operands unwrapped in locals; routing through
/// a helper would add an avoidable match on the i64 fast path.
/// If those two paths ever drift apart, refactor both onto this
/// helper.)
impl Vm {
    /// Apply an Int×Int op, promoting to BigInt on Add/Sub/Mul
    /// overflow when `bignum` is on. Use this instead of calling
    /// `kind.apply_int` directly anywhere the result needs to be
    /// pushed back as a Value.
    pub(crate) fn apply_int_promote(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        x: i64,
        y: i64,
    ) -> Result<Value, Trap> {
        if let Some(v) = kind.apply_int(x, y) {
            return Ok(v);
        }
        // None can only happen under `feature = "bignum"`.
        #[cfg(feature = "bignum")]
        {
            self.bigint_arith(kind, &Value::Int(x), &Value::Int(y))
                .expect("ICE: bigint_arith None for Int operands")
        }
        #[cfg(not(feature = "bignum"))]
        unreachable!("apply_int returns None only when bignum is on");
    }
}

/// Dispatch helper: tries Int/BigInt arithmetic or comparison
/// for the `Op::BinOp` cold path (operands include at least one
/// BigInt, or are non-Int shapes that this method declines).
/// With `bignum` off this is a no-op that always returns `None`,
/// so the caller falls through to `primitive_call` exactly as
/// before.
impl Vm {
    pub(crate) fn try_bigint_binop(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        a: &Value,
        b: &Value,
    ) -> Result<Option<Value>, Trap> {
        #[cfg(not(feature = "bignum"))]
        {
            let _ = (kind, a, b);
            Ok(None)
        }
        #[cfg(feature = "bignum")]
        {
            use crate::bytecode::BinOpKind;
            // Decline unless at least one operand is a BigInt
            // (Int×Int went through the fast path above already).
            if !matches!(a, Value::BigInt(_)) && !matches!(b, Value::BigInt(_)) {
                return Ok(None);
            }
            // Float ↔ BigInt mixed: coerce the BigInt to f64 (lossy
            // at extreme magnitudes — matches CRuby's "Float wins
            // on mix" rule and Integer#to_f's documented precision
            // loss past 2^53). Without this branch, `2.0 + big`
            // raised NoMethodError because primitive_call's Float
            // arms only handle Int/Float rhs.
            if matches!(a, Value::Float(_)) || matches!(b, Value::Float(_)) {
                let to_f = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(n) => Some(*n as f64),
                        Value::BigInt(id) => self.heap.bigint(*id).to_string().parse::<f64>().ok(),
                        _ => None,
                    }
                };
                let (af, bf) = match (to_f(a), to_f(b)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => return Ok(None),
                };
                let result = match kind {
                    BinOpKind::Add => Value::Float(af + bf),
                    BinOpKind::Sub => Value::Float(af - bf),
                    BinOpKind::Mul => Value::Float(af * bf),
                    BinOpKind::Div => Value::Float(af / bf),
                    BinOpKind::Mod => Value::Float(af.rem_euclid(bf)),
                    BinOpKind::Lt => Value::Bool(af < bf),
                    BinOpKind::Le => Value::Bool(af <= bf),
                    BinOpKind::Gt => Value::Bool(af > bf),
                    BinOpKind::Ge => Value::Bool(af >= bf),
                    BinOpKind::Eq => Value::Bool(af == bf),
                    BinOpKind::Ne => Value::Bool(af != bf),
                };
                return Ok(Some(result));
            }
            // Both operands must be integers (Int or BigInt); if
            // not, decline and let primitive_call try (e.g. for
            // String * BigInt later). Use `as_bigint_ref` to
            // borrow heap-side BigInts rather than cloning — only
            // Int→BigInt coercions allocate, and comparison ops
            // run entirely from refs.
            let ax_cow = match self.as_bigint_ref(a) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bx_cow = match self.as_bigint_ref(b) {
                Some(v) => v,
                None => return Ok(None),
            };
            // Comparison ops return Bool directly (run against the
            // borrowed BigInts via Cow's Deref impl — no clones).
            match kind {
                BinOpKind::Lt => return Ok(Some(Value::Bool(*ax_cow < *bx_cow))),
                BinOpKind::Le => return Ok(Some(Value::Bool(*ax_cow <= *bx_cow))),
                BinOpKind::Gt => return Ok(Some(Value::Bool(*ax_cow > *bx_cow))),
                BinOpKind::Ge => return Ok(Some(Value::Bool(*ax_cow >= *bx_cow))),
                BinOpKind::Eq => return Ok(Some(Value::Bool(*ax_cow == *bx_cow))),
                BinOpKind::Ne => return Ok(Some(Value::Bool(*ax_cow != *bx_cow))),
                _ => {}
            }
            drop(ax_cow);
            drop(bx_cow);
            // Arithmetic: delegate to bigint_arith which handles
            // zero-division traps and CRuby-style floor div / mod.
            match self.bigint_arith(kind, a, b) {
                Some(res) => Ok(Some(res?)),
                None => Ok(None),
            }
        }
    }

    /// `**` exponentiation with BigInt promotion and DoS cap.
    /// Returns:
    /// - `Some(v)` for any Int/BigInt × {Int (non-negative), Float,
    ///   negative Int, BigInt-when-|base|≤1} where we can produce a
    ///   value. Float / negative-Int exponents on Int receivers are
    ///   normally handled by numeric_call BEFORE reaching this fn;
    ///   we cover them here only when the receiver is a BigInt
    ///   (otherwise NoMethodError despite `respond_to?(:**)` being
    ///   true) and for the |base|≤1 short-circuit.
    /// - `Err(...)` for BigInt exponents with |base|>1 — the result
    ///   would need at least 2^63 bits of storage so we trap
    ///   `ResourceExhausted` rather than attempting to compute or
    ///   silently falling through.
    /// - `None` for operand shapes outside this branch's scope
    ///   (non-integer recv, or Int recv + Float/negative exp where
    ///   numeric_call handles it); the caller falls through.
    ///
    /// DoS protection: result bit count is approximately
    /// `bit_length(base) * exp` (tight as `(bit_length-1) * exp + 1`
    /// when |base| is a power of two). A few bytes of input can ask
    /// for many GB of output, so we pre-estimate and trap
    /// `ResourceExhausted` before calling `BigInt::pow`. The estimate
    /// rounds up to the BigInt limb size (u64 = 8 bytes) plus a small
    /// allocator-header overhead so the cap reflects actual heap
    /// storage, not just the minimal bit count. Honours
    /// `Config::max_value_bytes` (same cap that bounds String /
    /// Array growth); falls back to a 1 MB safety ceiling when no
    /// cap is configured.
    #[cfg(feature = "bignum")]
    pub(crate) fn try_bigint_pow(
        &mut self,
        recv: &Value,
        exp_arg: &Value,
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::{BigInt, Sign};
        let recv_is_bigint = matches!(recv, Value::BigInt(_));
        let exp_is_bigint = matches!(exp_arg, Value::BigInt(_));
        // Float / negative-exp paths need to fire here whenever
        // either operand is BigInt, since numeric_call only covers
        // pure Int×Int. Without this, `2 ** -(2**100)` or
        // `1 ** -(2**100)` (Int recv + negative BigInt exp) would
        // fall through to NoMethodError.
        let need_float_handling = recv_is_bigint || exp_is_bigint;
        // Read base sign + bit-length via borrowed Cow — avoids
        // the O(n) magnitude clone `as_bigint` would do for BigInt
        // receivers. The Cow borrow ends with the block; later
        // &mut self calls (`trap`, `bigint_to_value`) are free to
        // re-borrow. The full base is re-borrowed only at the
        // single `pow` site below.
        let (base_sign, base_bits) = {
            let base_cow = match self.as_bigint_ref(recv) {
                Some(v) => v,
                None => return Ok(None),
            };
            (base_cow.sign(), base_cow.bits())
        };
        // `base_is_pow2` is only consulted by the positive-exp
        // DoS estimator below. Defer the O(n) `count_ones()` scan
        // until we know we're in that branch so Float / negative-
        // exp / short-circuit paths don't pay for it.
        // Compute parity / sign / zero of the exponent up front so
        // every branch below dispatches on one vocabulary.
        let (exp_is_negative, exp_is_zero, exp_is_odd, exp_is_float) = match exp_arg {
            Value::Int(n) => (*n < 0, *n == 0, *n & 1 != 0, false),
            Value::BigInt(id) => {
                let big = self.heap.bigint(*id);
                let s = big.sign();
                (s == Sign::Minus, s == Sign::NoSign, big.bit(0), false)
            }
            Value::Float(_) => (false, false, false, true),
            _ => return Ok(None),
        };
        // Float exponent: coerce base to f64 (bounded) and use
        // powf. Int receivers go through numeric_call's Int×Float
        // arm BEFORE reaching here, so this fires only for BigInt
        // receivers (where the alternative would be NoMethodError
        // despite `respond_to?(:**)` returning true).
        if exp_is_float {
            if !recv_is_bigint { return Ok(None); }
            if let Value::Float(f) = exp_arg {
                let base_f = self.bigint_recv_to_f64_bounded(recv);
                return Ok(Some(Value::Float(base_f.powf(*f))));
            }
            unreachable!("exp_is_float ⇒ Value::Float(_)");
        }
        // Short-circuit |base| ≤ 1 — constant-size results,
        // dispatch only on sign + parity, safe for any exp shape.
        if base_bits <= 1 {
            match base_sign {
                Sign::NoSign => {
                    // base == 0. 0**0 == 1; 0**n (n>0) == 0;
                    // 0**n (n<0) raises ZeroDivisionError in CRuby
                    // — match that for ALL operand shapes. The
                    // previous behaviour returned `Float::INFINITY`
                    // for BigInt-flavoured operands (and Int recv
                    // × Int neg exp deferred to numeric.rs's powf,
                    // which silently produced inf too). Both paths
                    // now raise so the error surfaces explicitly
                    // instead of poisoning downstream arithmetic.
                    if exp_is_negative {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let r = if exp_is_zero { BigInt::from(1) } else { BigInt::from(0) };
                    return Ok(Some(self.bigint_to_value(r)?));
                }
                Sign::Plus => {
                    // base == 1: always 1. Negative exp → Float(1.0)
                    // for BigInt-flavoured operands (Int×Int still
                    // defers to numeric.rs's parity-preserving ±1
                    // arm).
                    if exp_is_negative {
                        if need_float_handling {
                            return Ok(Some(Value::Float(1.0)));
                        }
                        return Ok(None);
                    }
                    return Ok(Some(self.bigint_to_value(BigInt::from(1))?));
                }
                Sign::Minus => {
                    // base == -1: parity decides sign. Negative
                    // exponent: |result| = 1, sign from parity.
                    if exp_is_negative {
                        if need_float_handling {
                            return Ok(Some(Value::Float(if exp_is_odd { -1.0 } else { 1.0 })));
                        }
                        return Ok(None);
                    }
                    let r = if exp_is_odd { BigInt::from(-1) } else { BigInt::from(1) };
                    return Ok(Some(self.bigint_to_value(r)?));
                }
            }
        }
        // |base| > 1 from here on.
        // Negative Int / BigInt exp: Float reciprocal. Pure
        // Int×Int neg-exp goes through numeric.rs first; we cover
        // every other shape here (BigInt recv with any neg exp,
        // OR Int recv with negative BigInt exp) so dispatch
        // doesn't NoMethodError on `2 ** -(2**100)` and friends.
        if exp_is_negative {
            if need_float_handling {
                let exp_f = match exp_arg {
                    Value::Int(n) => *n as f64,
                    // BigInt-negative exp: result tends toward 0
                    // for |base|>1. Coerce via the bounded helper
                    // (caps the intermediate string at f64-range,
                    // ~310 bytes max).
                    Value::BigInt(id) => {
                        let big = self.heap.bigint(*id);
                        Self::bigint_to_f64_bounded(big)
                    }
                    _ => unreachable!(),
                };
                // Compute on |base| so a negative base + non-
                // integer / non-finite exp can't NaN out of
                // libm's powf. Re-apply the sign from the
                // already-computed base_sign + exp_is_odd, which
                // preserve parity from the original i64 / BigInt
                // rather than the f64 round.
                let base_f = self.bigint_recv_to_f64_bounded(recv);
                let mag = base_f.abs().powf(exp_f);
                let signed = if base_sign == Sign::Minus && exp_is_odd { -mag } else { mag };
                return Ok(Some(Value::Float(signed)));
            }
            return Ok(None);
        }
        // Exponent identities — return cheap results before the
        // DoS estimator (which itself adds a 32-byte header to
        // est_bytes, so `big ** 0` under an aggressively tight
        // `max_value_bytes` would otherwise trap even though the
        // correct answer is the immediate `Int(1)`). Skip the pow
        // allocation entirely for `** 0` and `** 1`.
        if exp_is_zero {
            return Ok(Some(Value::Int(1)));
        }
        if matches!(exp_arg, Value::Int(1)) {
            return Ok(Some(recv.clone()));
        }
        // Positive exp from here on. BigInt exponent with |base|>1
        // → trap (would need ≥ 2**63 bits).
        if matches!(exp_arg, Value::BigInt(_)) {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: "integer ** BigInt exponent exceeds u32::MAX".to_string(),
            }));
        }
        let exp_i64 = match exp_arg {
            Value::Int(n) => *n, // ≥ 2 (0, 1, negative handled above)
            _ => unreachable!("non-Int/BigInt/Float exp returned earlier"),
        };
        let exp_u32: u32 = match u32::try_from(exp_i64) {
            Ok(v) => v,
            Err(_) => {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("integer exponent {} exceeds u32::MAX", exp_i64),
                }));
            }
        };
        // Estimate result size and trap before allocating GBs.
        // The true bit-length of `base ** exp` is
        // `floor(exp * log2(|base|)) + 1`. For a power-of-two
        // base, `log2(|base|) == base_bits - 1` exactly, so the
        // tight bound is `(base_bits - 1) * exp + 1` — using
        // `base_bits * exp` here would overshoot 2× on the
        // canonical `2 ** n` shape (e.g. `2 ** 10_000_000`
        // really is ~1.25MB but a `2 * 10_000_000 = 20M-bit`
        // estimate would falsely trap a 2MB cap). For non-pow2
        // bases we fall back to `base_bits * exp` as a safe
        // upper bound (log2(base) < base_bits for any base).
        // Ceil-div in u64; compare against `cap as u64` so the
        // check doesn't silently truncate on 32-bit targets.
        // Compute power-of-two flag lazily here — earlier paths
        // (Float exp, negative exp, |base|≤1 short-circuit) all
        // return before reaching the estimator, so they avoid
        // the O(n) `count_ones()` scan over the BigInt magnitude.
        let base_is_pow2 = {
            let base_cow = match self.as_bigint_ref(recv) {
                Some(v) => v,
                None => unreachable!("recv shape validated at fn entry"),
            };
            base_cow.magnitude().count_ones() == 1
        };
        let est_bits: u64 = if base_is_pow2 {
            (base_bits.saturating_sub(1))
                .saturating_mul(exp_u32 as u64)
                .saturating_add(1)
        } else {
            base_bits.saturating_mul(exp_u32 as u64)
        };
        // Round up to BigInt limb storage (u64 limbs = 8 bytes each)
        // plus a small allocator-header overhead so the cap reflects
        // actual heap storage rather than just the minimal bit count.
        // This keeps `max_value_bytes` semantically aligned with the
        // Array/String paths (which count backing-storage bytes) and
        // closes a small word-boundary bypass on inputs that landed
        // just under the previous min-bytes estimate.
        const BIGINT_HEADER_BYTES: u64 = 32;
        let est_limbs: u64 = est_bits.saturating_add(63) / 64;
        let est_bytes: u64 = est_limbs.saturating_mul(8).saturating_add(BIGINT_HEADER_BYTES);
        let cap = self.max_value_bytes.unwrap_or(1 << 20);
        if est_bytes > cap as u64 {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!(
                    "integer ** exp would need ~{} bytes, exceeding cap {}",
                    est_bytes, cap
                ),
            }));
        }
        // Borrow base once more for the actual pow; `(&BigInt).pow`
        // returns an owned BigInt without consuming the receiver,
        // so a BigInt-receiver path computes pow against a
        // borrowed magnitude rather than a clone.
        let result = match self.as_bigint_ref(recv) {
            Some(c) => c.pow(exp_u32),
            None => unreachable!("recv shape validated earlier"),
        };
        Ok(Some(self.bigint_to_value(result)?))
    }

    /// BigInt → f64 with the intermediate decimal string bounded
    /// by a bits()-based pre-check. f64::MAX ≈ 2^1024, so any
    /// BigInt past that is already out of f64 range — return ±∞
    /// without materialising a string. Below the threshold the
    /// decimal form is at most ~310 digits, well under any
    /// `max_value_bytes` cap we care about. Centralises the
    /// dispatch.rs Range coercion pattern in one place that the
    /// `**` Float / negative-exp paths can share without
    /// allocating O(magnitude) strings on a hostile big input.
    #[cfg(feature = "bignum")]
    pub(crate) fn bigint_to_f64_bounded(b: &num_bigint::BigInt) -> f64 {
        use num_bigint::Sign;
        if b.bits() > 1024 {
            return if b.sign() == Sign::Minus { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        b.to_string().parse::<f64>().unwrap_or(f64::NAN)
    }

    /// Receiver-side helper around [`Self::bigint_to_f64_bounded`]:
    /// borrows the BigInt out of the heap via `as_bigint_ref`,
    /// then defers to the bounded coercion. Returns `NaN` if the
    /// receiver isn't an integer (caller already validated this;
    /// the NaN is a defensive fallback, not a reachable path).
    #[cfg(feature = "bignum")]
    pub(crate) fn bigint_recv_to_f64_bounded(&self, recv: &Value) -> f64 {
        match self.as_bigint_ref(recv) {
            Some(c) => Self::bigint_to_f64_bounded(&c),
            None => f64::NAN,
        }
    }

    /// Unary `-@` / `+@` / `abs` for BigInt receivers, plus the
    /// Int(i64::MIN) auto-promotion case (where the i64 cannot
    /// represent its own negation or absolute value, so numeric.rs
    /// declines and we materialise the BigInt 2^63 here). For Int
    /// receivers other than i64::MIN we return `None` so dispatch
    /// stays on numeric.rs's existing wrapping arms. `+@` on
    /// BigInt is a no-op clone; on Int it shouldn't even reach
    /// here (numeric.rs handles it) — included for completeness.
    #[cfg(feature = "bignum")]
    pub(crate) fn try_bigint_unary(
        &mut self,
        recv: &Value,
        name: &str,
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::{BigInt, Sign};
        match recv {
            Value::BigInt(id) => {
                // Compute the owned result in a borrow scope, then
                // drop the borrow before calling bigint_to_value
                // (&mut self). `+@` just hands back the receiver
                // unchanged — no demote needed. The identity
                // shortcut is sound ONLY because every
                // `Value::BigInt(id)` is allocated through
                // `bigint_to_value`, which demotes any in-i64
                // magnitude to `Value::Int(n)` — see the
                // `debug_assert!` below. If a future cext/FFI path
                // ever bypasses `bigint_to_value` and stores an
                // in-i64 magnitude as `HeapObj::BigInt`, this
                // shortcut would leak a non-canonical
                // `Value::BigInt(small)` whose dispatch semantics
                // drift from `Value::Int(small)`.
                if name == "+@" {
                    debug_assert!(
                        i64::try_from(self.heap.bigint(*id)).is_err(),
                        "non-canonical BigInt reached try_bigint_unary +@: \
                         magnitude fits i64 but wasn't demoted by bigint_to_value",
                    );
                    return Ok(Some(recv.clone()));
                }
                // `abs` on an already-non-negative BigInt is the
                // identity: skip both the BigInt clone and the
                // bigint_to_value allocation by handing back
                // `recv` unchanged (same shape as `+@`). Only the
                // Minus branch needs a fresh BigInt + demote-on-fit
                // funnel.
                if name == "abs" {
                    let sign = self.heap.bigint(*id).sign();
                    if sign != Sign::Minus {
                        debug_assert!(
                            i64::try_from(self.heap.bigint(*id)).is_err(),
                            "non-canonical BigInt reached try_bigint_unary abs: \
                             magnitude fits i64 but wasn't demoted by bigint_to_value",
                        );
                        return Ok(Some(recv.clone()));
                    }
                }
                let result = {
                    let b = self.heap.bigint(*id);
                    match name {
                        "-@" => -b,
                        "abs" => -b, // sign == Minus from check above
                        _ => return Ok(None),
                    }
                };
                Ok(Some(self.bigint_to_value(result)?))
            }
            Value::Int(n) if *n == i64::MIN => {
                // i64::MIN.abs() and -i64::MIN both overflow i64 by
                // exactly one (the magnitude is 2^63, one past
                // i64::MAX). Promote via BigInt — bigint_to_value
                // will keep it as BigInt since it doesn't fit.
                match name {
                    "abs" | "-@" => {
                        let promoted = -BigInt::from(i64::MIN);
                        Ok(Some(self.bigint_to_value(promoted)?))
                    }
                    "+@" => Ok(Some(Value::Int(i64::MIN))),
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// `Integer#pow(exp[, mod])`. 1-arg form is exactly `recv ** exp`
    /// — delegated to `try_bigint_pow`. 2-arg form is modular
    /// exponentiation: computes `(recv ** exp) mod modulus` without
    /// materialising the intermediate (so the DoS cap that bounds
    /// the plain `**` path is unnecessary here — the result is
    /// already bounded by `|modulus|`).
    ///
    /// CRuby semantics for the 2-arg form:
    /// - `modulus == 0` → ZeroDivisionError.
    /// - `exp < 0` → RangeError (modular inverse may not exist; we
    ///   don't compute it).
    /// - Otherwise the result follows Ruby's floor-mod convention
    ///   (same sign as `modulus`). `num_bigint::BigInt::modpow`
    ///   already returns a value with the same sign as the modulus,
    ///   matching this convention exactly — no post-adjustment.
    /// - `exp` and `modulus` must both be Integer (Int / BigInt);
    ///   Float / String etc. raise TypeError.
    #[cfg(feature = "bignum")]
    pub(crate) fn try_bigint_pow_method(
        &mut self,
        recv: &Value,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::Sign;
        // 1-arg form ≡ `recv ** exp`. Reuse try_bigint_pow's full
        // shape handling (Float exp, negative exp, BigInt exp,
        // DoS cap, identity short-circuits, ZeroDivisionError on
        // 0**-n, etc.). Non-numeric exponents (String, Symbol,
        // nil, …) raise TypeError matching CRuby — `try_bigint_pow`
        // would otherwise decline (`Ok(None)`) and dispatch would
        // surface NoMethodError, which is the wrong error class.
        // Mirrors the Int-receiver guard in numeric.rs::pow.
        if args.len() == 1 {
            let arg = &args[0];
            let acceptable = matches!(arg, Value::Int(_) | Value::Float(_) | Value::BigInt(_));
            if !acceptable {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "{} can't be coerced into Integer",
                        crate::vm::numeric::type_name_for_coerce(arg),
                    ),
                }));
            }
            return self.try_bigint_pow(recv, arg);
        }
        // 2-arg form: pow(exp, mod). Validate shapes first using
        // immutable borrows (no clones). The error paths short-
        // circuit before the modpow allocation; the success path
        // borrows the three BigInts via `as_bigint_ref` (Cow) and
        // runs `modpow` inside the borrow scope so BigInt operands
        // don't pay an O(n) clone before the computation.
        //
        // All Cow-dependent work (shape checks, sign reads, modpow)
        // runs inside one labelled block so each operand is borrowed
        // exactly once. Int operands still pay one `BigInt::from(n)`
        // alloc per `as_bigint_ref` call (unavoidable); BigInt
        // operands stay as `Cow::Borrowed` (no clone). The block
        // exits with Ok(Some(result)) / Ok(None) (decline) / Err
        // (trap). Trap construction (which needs `&mut self`)
        // happens AFTER the borrows expire, when the block returns.
        let pre: Result<Option<num_bigint::BigInt>, RubyError> = 'classify: {
            let Some(base) = self.as_bigint_ref(recv) else {
                // Non-Integer recv → decline so dispatch falls
                // through to NoMethodError (Float etc. have no
                // `.pow(exp, mod)`).
                break 'classify Ok(None);
            };
            // Match CRuby's exact TypeError message text so user
            // code pattern-matching on `e.message` keeps working.
            let Some(exp) = self.as_bigint_ref(&args[0]) else {
                break 'classify Err(RubyError::TypeError {
                    msg: "Integer#pow() 2nd argument not allowed unless a 1st argument is integer".to_string(),
                });
            };
            let Some(modulus) = self.as_bigint_ref(&args[1]) else {
                break 'classify Err(RubyError::TypeError {
                    msg: "Integer#pow() 2nd argument not allowed unless all arguments are integers".to_string(),
                });
            };
            // Sign checks read the held Cows directly (no extra
            // borrow / no extra `BigInt::from(n)` for Int operands).
            if modulus.sign() == Sign::NoSign {
                break 'classify Err(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                });
            }
            if exp.sign() == Sign::Minus {
                break 'classify Err(RubyError::RangeError {
                    msg: "Integer#pow() 1st argument cannot be negative when 2nd argument specified".to_string(),
                });
            }
            // BigInt::modpow returns a value with the same sign as
            // modulus — matches Ruby's floor-mod semantics exactly,
            // no post-adjustment.
            Ok(Some(base.modpow(&exp, &modulus)))
        };
        // Borrows expired with the block. Safe to call &mut self.
        match pre {
            Ok(None) => Ok(None),
            Ok(Some(result)) => Ok(Some(self.bigint_to_value(result)?)),
            Err(err) => Err(self.trap(err)),
        }
    }

    /// `Integer#digits([base = 10])` — array of digits in the given
    /// base, least-significant first. Returns `Some(Value::Array)`
    /// for BigInt receivers; `Ok(None)` for Int receivers (so the
    /// i64 fast path in `vm/dispatch.rs::Integer#digits` runs
    /// instead — keeps small Int×Int#digits off the BigInt
    /// arithmetic path) and for non-Integer recv (lets dispatch
    /// fall through to NoMethodError). Traps:
    /// - Negative receiver → ArgumentError "out of domain"
    ///   (CRuby raises Math::DomainError; the established subset
    ///   pattern uses ArgumentError as the substitute since
    ///   Math::DomainError isn't modelled — same convention as
    ///   the Range #cover? / numeric-out-of-domain arms in
    ///   `Vm::do_call`).
    /// - Base < 0 → ArgumentError "negative radix".
    /// - Base < 2 → ArgumentError "invalid radix N".
    /// - Non-Integer base → TypeError "no implicit conversion of
    ///   X into Integer".
    /// - Result-array estimate exceeds the active cap → trap
    ///   ResourceExhausted before allocation. The cap is
    ///   `Config::max_value_bytes` when set, otherwise a 1 MB
    ///   safety ceiling (same fallback as `try_bigint_pow`'s
    ///   estimator — so hostless / default-config users still get
    ///   a bound on this allocation path). The bound itself uses
    ///   an integer approximation: `est_count = floor((recv_bits
    ///   - 1) / log2_lower) + 1`, where `log2_lower = max(1,
    ///   base.bits() - 1)` is a lower bound on `log2(base)` (since
    ///   `base >= 2^(base.bits() - 1)`). Dividing by a smaller log
    ///   gives a safe upper bound on the count without floating-
    ///   point. Multiply by `size_of::<Value>()` for bytes.
    #[cfg(feature = "bignum")]
    pub(crate) fn try_integer_digits(
        &mut self,
        recv: &Value,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::{BigInt, Sign};
        // BigInt receivers only — Int receivers route through
        // `dispatch.rs`'s existing i64 fast path (no BigInt
        // arithmetic for small Int×Int). Non-Integer recv: decline
        // so dispatch can fall through to NoMethodError.
        // Returning `Ok(None)` for Int recv lets `bigint_primitive`
        // continue through the arity guard (which still fires for
        // `args.len() > 1` regardless of recv type) and then
        // through to `dispatch.rs`'s Int#digits handler. The Int
        // fast path now shares error message text with this BigInt
        // path (see the matching dispatch.rs edits).
        let (recv_bits, recv_sign) = match recv {
            Value::BigInt(id) => {
                let b = self.heap.bigint(*id);
                (b.bits(), b.sign())
            }
            _ => return Ok(None),
        };
        // Negative receiver: out of domain.
        if recv_sign == Sign::Minus {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "out of domain".to_string(),
            }));
        }
        // Resolve base. Default 10; reject non-Integer args; reject
        // <2 (with CRuby's two distinct messages).
        let base: BigInt = match args.first() {
            None => BigInt::from(10),
            Some(Value::Int(r)) => {
                if *r < 0 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: "negative radix".to_string(),
                    }));
                }
                if *r < 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("invalid radix {}", r),
                    }));
                }
                BigInt::from(*r)
            }
            Some(Value::BigInt(id)) => {
                let b = self.heap.bigint(*id);
                // BigInt radix is always > i64::MAX > 1, so >= 2.
                // Negative BigInt radix would have been demoted to
                // Int by bigint_to_value if it fit. For BigInts
                // outside i64 range, we know sign from b.sign().
                if b.sign() == Sign::Minus {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: "negative radix".to_string(),
                    }));
                }
                b.clone()
            }
            Some(other) => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        crate::vm::numeric::type_name_for_coerce(other),
                    ),
                }));
            }
        };
        // Pre-estimate array length to avoid building a multi-GB
        // Vec on hostile input. The exact digit count is
        // `floor(log_base(recv)) + 1`; rewriting via base-2:
        // `floor((recv_bits - 1) / log2(base)) + 1` (since
        // `log2(recv) ≈ recv_bits - 1` for recv > 0). We use the
        // integer lower bound `log2(base) >= base.bits() - 1`
        // (since `base >= 2^(base.bits() - 1)`); dividing by a
        // smaller log gives a safe upper bound on the count
        // without floating-point.
        //
        // Base = 2:   log2_lower = 1, est = recv_bits (exact).
        // Base = 10:  log2_lower = 3, est ≈ recv_bits/3 + 1.
        // Base = 256: log2_lower = 8, est ≈ recv_bits/8 + 1.
        //
        // recv_bits == 0 case (`Sign::NoSign`) sets est_count = 1
        // explicitly below — the cap check still runs but is
        // trivially satisfied for any non-pathological cap (a
        // single-Value array is `size_of::<Value>()` bytes).
        const VALUE_BYTES: u64 = std::mem::size_of::<Value>() as u64;
        let log2_lower: u64 = base.bits().saturating_sub(1).max(1);
        let est_count: u64 = if recv_bits == 0 {
            1
        } else {
            // ceil-form: `(recv_bits - 1) / log2_lower + 1`.
            // Previous form `recv_bits / log2_lower + 1`
            // overcounted by 1 for base = 2 (recv_bits = N gave
            // est = N+1 instead of N) and similarly off-by-one
            // for any base where `recv_bits % log2_lower == 0`.
            (recv_bits - 1) / log2_lower + 1
        };
        let est_bytes: u64 = est_count.saturating_mul(VALUE_BYTES);
        let cap = self.max_value_bytes.unwrap_or(1 << 20) as u64;
        if est_bytes > cap {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!(
                    "Integer#digits would need ~{} bytes, exceeding cap {}",
                    est_bytes, cap
                ),
            }));
        }
        // Build the digit array. Clone the heap BigInt as the
        // working value; we mutate `n` via repeated `n = &n / &base`
        // in the loop below, so an owned BigInt is required.
        let mut n: BigInt = match recv {
            Value::BigInt(id) => self.heap.bigint(*id).clone(),
            _ => unreachable!("recv shape narrowed to BigInt at fn entry"),
        };
        // GC rooting: every `bigint_to_value` call below invokes
        // `maybe_gc()`. For Int radix (the common case) rem is
        // always small and demotes to `Value::Int`, no rooting
        // needed. For BigInt radix, rem can be a heap-backed
        // `Value::BigInt(id)`; without pinning, an iteration N+1
        // GC could sweep the BigInts pushed during 1..N before
        // the Array allocation roots them, leaving dangling
        // ObjIds in the returned Array. Pin every Value::BigInt
        // digit as it's produced; the PinGuard drops after the
        // Array is allocated (heap.alloc itself triggers the
        // final GC walk, which now sees both the pinned digits
        // and the freshly-allocated Array as reachable).
        let mut guard = PinGuard::new(self);
        // Pre-reserve up to `est_count` (already capped against
        // `max_value_bytes` above, so safe to truncate to usize).
        // Avoids the geometric reallocation pattern Vec would
        // otherwise use during the loop on large digit arrays.
        let cap_count = est_count.min(usize::MAX as u64) as usize;
        let mut digits: Vec<Value> = Vec::with_capacity(cap_count);
        if recv_sign == Sign::NoSign {
            digits.push(Value::Int(0));
        } else {
            use num_integer::Integer;
            while n.sign() != Sign::NoSign {
                // `div_rem` returns (quotient, remainder) in a
                // single division step — half the per-iteration
                // BigInt work vs separate `&n / &base` + `&n %
                // &base`. `Integer` is impl'd for `BigInt` by
                // num-bigint. rem fits i64 when base fits i64;
                // for BigInt base we go through bigint_to_value
                // so the demote-on-fit funnel handles either.
                let (quot, rem) = n.div_rem(&base);
                n = quot;
                let digit_val = guard.vm.bigint_to_value(rem)?;
                if matches!(digit_val, Value::BigInt(_)) {
                    guard.pin(digit_val.clone());
                }
                digits.push(digit_val);
            }
        }
        guard.vm.maybe_gc();
        guard.vm.check_alloc()?;
        let arr_id = guard.vm.heap.alloc(crate::heap::HeapObj::Array(digits));
        // `guard` drops here, unpinning the digits — but the
        // Array now holds them as roots, so the next GC walk
        // still sees them as reachable.
        Ok(Some(Value::Array(arr_id)))
    }
}

/// BigInt method dispatch — covers the calls `primitive_call`
/// can't satisfy (it's stateless; BigInt needs heap access for
/// the decimal-string read). Hooked from `Vm::do_call` after
/// the regular primitive paths. Phase A surface:
///
/// - `to_s` / `inspect` — heap-read paths handled inline.
/// - Operator method-call shape (`big.+(x)`, `big.send(:==, y)`)
///   — name parsed by `BinOpKind::from_op_name`, then routed
///   through `try_bigint_binop` so the answer matches the
///   `Op::BinOp` path exactly.
///
/// The expression-form arithmetic (`big + 1` compiled as
/// `Op::BinOp`) still goes through `try_bigint_binop` directly
/// without entering this helper.
#[cfg(feature = "bignum")]
impl Vm {
    pub(crate) fn bigint_primitive(
        &mut self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        // Entry conditions, in order of precedence:
        // 1. `**` exponentiation fires for ANY Int/BigInt operand
        //    combo, including Int×Int — numeric_call's `**` arm
        //    declines on i64 overflow so we get the chance to
        //    promote here (`2 ** 100`). Handled before the guard
        //    below so the Int×Int overflow case isn't filtered out.
        // 2. Unary `-@` / `+@` / `abs` — fires for BigInt recv OR
        //    Int(i64::MIN) recv. numeric_call declines on i64::MIN
        //    under `bignum` so this arm can promote to the
        //    BigInt 2^63. Also sits ahead of the recv-or-arg-is-
        //    BigInt guard for the same reason as `**`.
        // 3. `pow(exp[, mod])` method form — 1-arg aliases `**`;
        //    2-arg routes through `BigInt::modpow` for modular
        //    exponentiation. Fires for any Integer recv (including
        //    Int×Int×Int), so it sits ahead of the recv-or-arg
        //    guard. No DoS cap on the 2-arg form: modpow never
        //    materialises the intermediate, and the result is
        //    bounded by |mod|.
        // 4. `digits([base])` — produces a `Value::Array` so it
        //    needs `&mut Vm` (can't live in stateless numeric_call).
        //    Two sub-checks fire ahead of the dispatch in CRuby
        //    precedence order: negative recv → ArgumentError "out
        //    of domain" (Math::DomainError substitute), then arity
        //    guard for >1 args → ArgumentError. The dispatch
        //    itself narrows the helper to BigInt receivers; Int
        //    receivers fall through to dispatch.rs's i64 fast
        //    path. Sits ahead of the recv-or-arg guard so Int
        //    receivers don't get filtered out.
        // 5. Recv is BigInt: covers `big.to_s`, `big.+(x)`, etc.
        // 6. Recv is Int AND a BigInt is among args: covers the
        //    inverse-receiver operator method-call shape
        //    `1.+(2**63)`, which goes through the Int-side
        //    dispatch path and would otherwise miss BigInt
        //    arithmetic entirely (the expression form `1 + big`
        //    works because Op::BinOp already routes via
        //    try_bigint_binop on either-operand-is-BigInt).
        //
        // When adding a new entry path that needs to fire for
        // Int receivers without a BigInt arg (e.g. another auto-
        // promotion shape), place it BEFORE the
        // `recv_is_bigint || arg_is_bigint` guard below.
        //
        // Fall through to the rest of bigint_primitive when
        // `try_bigint_pow` declines. Decline cases narrow to
        // Int recv × Int (positive) exp where `numeric_call`
        // already produced a value, or operand shapes that aren't
        // integer at all (the latter never reaches bigint_primitive
        // in practice — `primitive_call`'s Int arm would have
        // matched first). Float and negative-Int exponents are
        // handled inside `try_bigint_pow` itself for BigInt-
        // flavoured operands; Int×Int Float/neg-exp is owned by
        // `numeric_call` before we get here.
        if args.len() == 1 && name == "**"
            && let Some(v) = self.try_bigint_pow(recv, &args[0])?
        {
            return Ok(Some(v));
        }
        // Cond 2 — see entry-conditions doc above.
        if args.is_empty() && matches!(name, "-@" | "+@" | "abs")
            && let Some(v) = self.try_bigint_unary(recv, name)?
        {
            return Ok(Some(v));
        }
        // `pow(exp[, mod])` method form — 1-arg is an alias for `**`,
        // 2-arg is modular exponentiation via BigInt::modpow. Fires
        // ahead of the recv-or-arg guard so Int×Int×Int shapes work
        // too. No DoS cap needed for the 2-arg form: modpow never
        // materialises the intermediate, and the result is bounded
        // by |mod|.
        if name == "pow" && (args.len() == 1 || args.len() == 2)
            && let Some(v) = self.try_bigint_pow_method(recv, args)?
        {
            return Ok(Some(v));
        }
        // CRuby precedence: a negative receiver for `Integer#digits`
        // raises `Math::DomainError: out of domain` BEFORE any
        // arity / base validation. Match that ordering by checking
        // recv sign first, ahead of the arity guard and digits
        // dispatch below. The Math::DomainError substitute is
        // ArgumentError (same convention as other numeric-out-of-
        // domain arms in Vm::do_call). Concrete examples (CRuby vs
        // pre-fix rubyrs): `(-5).digits(10, 2)` should raise
        // "out of domain", not the arity error;
        // `(-5).digits("foo")` should raise "out of domain", not
        // a TypeError on the base; etc.
        if name == "digits" {
            let neg_recv = match recv {
                Value::Int(n) => *n < 0,
                Value::BigInt(id) => self.heap.bigint(*id).sign() == num_bigint::Sign::Minus,
                _ => false,
            };
            if neg_recv {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "out of domain".to_string(),
                }));
            }
        }
        // `Integer#digits` produces a `Value::Array`, which needs
        // heap allocation — can't live in stateless `numeric_call`.
        // Fires for ANY Int/BigInt receiver (recv-side check is in
        // the helper, which now narrows to BigInt only — Int
        // receivers continue through and hit dispatch.rs's i64
        // fast path). Sits ahead of the recv-or-arg guard so Int
        // receivers don't get filtered out. By the time we reach
        // here, `recv` is non-negative (the precedence check above
        // already trapped the negative case).
        if name == "digits" && (args.is_empty() || args.len() == 1)
            && let Some(v) = self.try_integer_digits(recv, args)?
        {
            return Ok(Some(v));
        }
        // Arity guard for `digits` — CRuby raises ArgumentError
        // ("wrong number of arguments (given N, expected 0..1)")
        // for arities outside {0, 1}. Without this, `5.digits(10, 2)`
        // falls through to NoMethodError despite `respond_to?(:digits)`
        // being true. Fires for any Int/BigInt receiver.
        if name == "digits"
            && matches!(recv, Value::Int(_) | Value::BigInt(_))
            && args.len() > 1
        {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len(),
                ),
            }));
        }
        // Arity guard for BigInt-receiver `pow` — numeric.rs's
        // arity guard only catches Int×*, so `big.pow` /
        // `big.pow(1,2,3)` would otherwise fall through to
        // NoMethodError despite `respond_to?(:pow)` being true.
        // Match CRuby's exact ArgumentError message text.
        if name == "pow" && matches!(recv, Value::BigInt(_)) && args.len() != 2 && args.len() != 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 1..2)",
                    args.len(),
                ),
            }));
        }
        let recv_is_bigint = matches!(recv, Value::BigInt(_));
        let arg_is_bigint = args.iter().any(|a| matches!(a, Value::BigInt(_)));
        if !recv_is_bigint && !arg_is_bigint {
            return Ok(None);
        }
        // Phase A heap-read operations — only meaningful on a BigInt
        // receiver (Int#to_s already handled by numeric_call).
        if recv_is_bigint && args.is_empty()
            && let Value::BigInt(id) = recv
        {
            use num_bigint::Sign;
            let b = self.heap.bigint(*id);
            match name {
                    "to_s" | "inspect" => {
                        // BigInt decimal can grow arbitrarily (consider
                        // `n = 2 ** 1_000_000; n.to_s`), so the
                        // String materialised here must obey the same
                        // `Config::max_value_bytes` cap that other
                        // primitive_call arms enforce. Without this
                        // check a script could DoS the host by
                        // converting a huge BigInt to string.
                        let s = b.to_string();
                        if let Some(max) = self.max_value_bytes
                            && s.len() > max
                        {
                            return Err(self.trap(RubyError::ResourceExhausted {
                                msg: format!("value size {} bytes > cap {}", s.len(), max),
                            }));
                        }
                        return Ok(Some(Value::new_str(s)));
                    }
                    // Pure read-only predicates — fit cleanly in
                    // Phase A because they don't need heap mutation.
                    // (CRuby Integer uniformity: any predicate the
                    // i64 Int receiver supports should work on the
                    // unified Integer class regardless of magnitude.)
                    "to_i" => return Ok(Some(recv.clone())),
                    "to_f" => {
                        // Lossy at extreme magnitudes; matches CRuby.
                        return Ok(Some(Value::Float(
                            b.to_string().parse::<f64>().unwrap_or(f64::INFINITY)
                        )));
                    }
                    "zero?" => return Ok(Some(Value::Bool(b.sign() == Sign::NoSign))),
                    "positive?" => return Ok(Some(Value::Bool(b.sign() == Sign::Plus))),
                    "negative?" => return Ok(Some(Value::Bool(b.sign() == Sign::Minus))),
                    "even?" => return Ok(Some(Value::Bool((b & num_bigint::BigInt::from(1)) == num_bigint::BigInt::from(0)))),
                    "odd?" => return Ok(Some(Value::Bool((b & num_bigint::BigInt::from(1)) != num_bigint::BigInt::from(0)))),
                    // `Integer#bit_length` on BigInt. For non-
                    // negatives: bit position of the highest set
                    // bit (== `bits()`). For negatives: CRuby's
                    // two's-complement convention gives the bit
                    // position of the highest 0-bit, equivalent to
                    // `bit_length(~n) = bit_length(-n - 1) =
                    // bits(|n| - 1)`. `bits()` returns u64; cap at
                    // i64::MAX in case of pathological 2^63-bit
                    // BigInts (unreachable under our DoS caps, but
                    // future-proofs the cast).
                    "bit_length" => {
                        let bits: u64 = match b.sign() {
                            Sign::NoSign => 0,
                            Sign::Plus => b.bits(),
                            Sign::Minus => {
                                // |n| - 1 in BigInt land, then bit count.
                                (b.magnitude() - 1u32).bits()
                            }
                        };
                        let n = i64::try_from(bits).unwrap_or(i64::MAX);
                        return Ok(Some(Value::Int(n)));
                    }
                    _ => {}
            }
        }
        // Operator method-call shape — `big.+(1)`, `1.+(big)`,
        // `big.send(:==, x)`. Route through `try_bigint_binop` so
        // the answer matches the `Op::BinOp` path exactly (same
        // arithmetic / floor-div semantics, same comparison Bool,
        // same overflow-promotion-then-demote rule).
        if args.len() == 1
            && let Some(kind) = crate::bytecode::BinOpKind::from_op_name(name)
            && let Some(v) = self.try_bigint_binop(kind, recv, &args[0])?
        {
            return Ok(Some(v));
        }
        // `<=>` — universal three-way comparison. Not in BinOpKind
        // (it returns Int not Bool, so the BinOp machinery doesn't
        // model it), so we handle it here for Int/BigInt operands.
        // CRuby's Integer#<=> returns nil for incomparable rhs
        // (e.g. `1 <=> "foo"`); we do the same by deferring to the
        // numeric_call path via None.
        if args.len() == 1 && name == "<=>"
            && let (Some(ax), Some(bx)) = (
                self.as_bigint_ref(recv),
                self.as_bigint_ref(&args[0]),
            )
        {
            let ord = ax.cmp(&bx);
            let n = match ord {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            return Ok(Some(Value::Int(n)));
        }
        Ok(None)
    }
}

/// BigInt arithmetic surface — shared by the i64-overflow promotion
/// path in `Op::BinOp` / `Op::BinOpInt` and by the cold-path
/// dispatch for already-BigInt operands. Cfg-gated on `bignum`
/// alongside the `Value::BigInt` variant. ADR 0018 BigInt placement.
#[cfg(feature = "bignum")]
impl Vm {
    /// Wraps a `BigInt` as a `Value`, demoting to `Value::Int` if
    /// it fits in i64. Every arithmetic path that can produce a
    /// BigInt result funnels through here so that
    /// post-overflow-shrink results land as `Int(n)` (matching
    /// CRuby's `Fixnum == Bignum` equality on the natural Int
    /// path) rather than `BigInt(n)` with a different ObjId per
    /// computation.
    pub(crate) fn bigint_to_value(&mut self, b: num_bigint::BigInt) -> Result<Value, Trap> {
        if let Ok(n) = i64::try_from(&b) {
            return Ok(Value::Int(n));
        }
        self.maybe_gc();
        self.check_alloc()?;
        Ok(Value::BigInt(self.heap.alloc(crate::heap::HeapObj::BigInt(b))))
    }

    /// Resolves an Int / BigInt operand to its `num_bigint::BigInt`
    /// form. Non-integer Values return `None` so the caller can
    /// fall through to the regular dispatch path (e.g. method-missing
    /// for `String + BigInt`). Owned form — clones the heap-side
    /// BigInt because the caller will consume it (arithmetic moves).
    /// For comparisons / read-only paths prefer `as_bigint_ref`.
    pub(crate) fn as_bigint(&self, v: &Value) -> Option<num_bigint::BigInt> {
        match v {
            Value::Int(n) => Some(num_bigint::BigInt::from(*n)),
            Value::BigInt(id) => Some(self.heap.bigint(*id).clone()),
            _ => None,
        }
    }

    /// Borrowed form of `as_bigint`. BigInt operands flow as
    /// `Cow::Borrowed(&BigInt)` (no clone); Int operands wrap
    /// in `Cow::Owned(BigInt::from(n))` because we have to
    /// materialise the conversion somewhere. The borrowed result
    /// is tied to `&self.heap`, so the caller must drop it before
    /// any `&mut self` calls. Used by `try_bigint_binop` for
    /// comparison ops where both sides run from refs.
    pub(crate) fn as_bigint_ref<'a>(
        &'a self,
        v: &'a Value,
    ) -> Option<std::borrow::Cow<'a, num_bigint::BigInt>> {
        use std::borrow::Cow;
        match v {
            Value::Int(n) => Some(Cow::Owned(num_bigint::BigInt::from(*n))),
            Value::BigInt(id) => Some(Cow::Borrowed(self.heap.bigint(*id))),
            _ => None,
        }
    }

    /// Performs Add/Sub/Mul/Div/Mod on Int/BigInt operands in
    /// arbitrary precision. Returns `None` for operands that
    /// aren't integers (the caller dispatches normally). Div/Mod
    /// by zero returns `Some(Err(...))` for the trap.
    pub(crate) fn bigint_arith(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        a: &Value,
        b: &Value,
    ) -> Option<Result<Value, Trap>> {
        use crate::bytecode::BinOpKind;
        use num_bigint::BigInt;
        let ax = self.as_bigint(a)?;
        let bx = self.as_bigint(b)?;
        let result: BigInt = match kind {
            BinOpKind::Add => ax + bx,
            BinOpKind::Sub => ax - bx,
            BinOpKind::Mul => ax * bx,
            BinOpKind::Div | BinOpKind::Mod => {
                use num_bigint::Sign;
                if bx.sign() == Sign::NoSign {
                    return Some(Err(self.trap(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    })));
                }
                // CRuby's Integer#/ floors toward negative infinity
                // (BigInt's default `Div` truncates toward zero).
                // Same correction for `%`: result has rhs's sign.
                let (q, r) = (&ax / &bx, &ax % &bx);
                let needs_correction = (r.sign() == Sign::Minus && bx.sign() == Sign::Plus)
                    || (r.sign() == Sign::Plus && bx.sign() == Sign::Minus);
                if matches!(kind, BinOpKind::Div) {
                    if needs_correction { q - 1 } else { q }
                } else {
                    if needs_correction { r + &bx } else { r }
                }
            }
            // Comparison ops are handled inline in
            // `try_bigint_binop` (which returns Bool directly via
            // BigInt's PartialOrd/PartialEq); they never reach this
            // arithmetic match.
            _ => return None,
        };
        Some(self.bigint_to_value(result))
    }
}

// cext-reentrance machinery (CURRENT_VM_PTR + VmPtrGuard + with_vm_ptr_set)
// moved to `vm/cext.rs`.
// `file_class_dispatch` moved to `vm/fileops.rs`.
//
// (The `with_caught_unwind` helper that used to live here was
// removed in Spike L3-A — once the cext call routes through the
// pure-C `rubyrs_jmp_invoke`, there's no Rust frame between
// setjmp and the cext fn that a catch_unwind could meaningfully
// cover. Re-introducing it on master appears to have been an
// accidental revert in the kernel.rs extraction refactor; this
// branch drops it again to keep the panic-budget honest and
// -D warnings green.)





