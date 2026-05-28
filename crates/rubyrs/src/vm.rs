use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::Proto;
use crate::error::Trap;
use crate::heap::Heap;
use crate::intern::{Interner, SymId};
use crate::value::{Class, FixedArity, Method, ObjId, Value, Visibility};

mod array;
mod bignum;
#[cfg(feature = "_fiber")]
pub(crate) mod fiber;
#[cfg(feature = "cext")]
mod cext;
#[cfg(all(feature = "cext", target_os = "wasi"))]
mod cext_wasi;
mod vm_ptr;
mod dispatch;
mod fileops;
mod gc;
mod hash;
mod iter;
mod kernel;
mod lookup;
#[cfg(feature = "regex")]
mod match_data;
mod numeric;
mod primitive;
mod raise;
mod range;
mod sprintf;
mod step;
mod string;
mod util;
#[cfg(feature = "bignum")]
pub(crate) use bignum::bigint_equals_float_lossless;
// `with_vm_ptr_set` lives in `vm_ptr` (extracted from cext in
// PoC stage 4a). Re-export from here so both the cext bridge
// and the `_http_server` battery's dispatch path find it via
// `super::with_vm_ptr_set`.
#[cfg(any(all(feature = "cext", not(target_os = "wasi")), feature = "_http_server"))]
pub(crate) use vm_ptr::with_vm_ptr_set;
// `current_vm_ptr` is the read-side used by host fn bodies
// that re-enter the Vm. _http_server battery's per-request
// handler uses this to access &mut Vm without the host fn
// signature itself needing to carry one. Stage 4c.3 uses
// this in production code path (handle_request_with_app).
// Also used by `Runtime::reset_between_requests` for a
// cext-invariant debug_assert.
#[cfg(any(
    all(feature = "cext", not(target_os = "wasi")),
    feature = "_http_server",
))]
pub(crate) use vm_ptr::current_vm_ptr;

// `iter::BlockStep` is the result of `step_block`. The
// `_http_server` battery's block-invocation helper
// (stage 4c.2) returns one of these variants. Re-export
// behind the feature gate so http_server.rs can name it
// without reaching into a private mod.
#[cfg(feature = "_http_server")]
pub(crate) use iter::BlockStep;
pub(crate) use numeric::{floor_div_i64, floor_mod_i64, int_cmp_float_lossless};
pub(crate) use lookup::{class_is_a, flatten_ancestors, CallCache};
pub use lookup::IcStats;
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
    /// Mirror of `Config::allow_filesystem_io`. Set by
    /// `apply_config` (and the preamble snapshot path); read by
    /// every script-callable FS-touching site via
    /// `Vm::check_filesystem_io_allowed`. `false` (the default)
    /// makes File.*/require/__dir__ trap; `true` lets them
    /// through.
    pub(crate) allow_filesystem_io: bool,
    /// Mirror of `Config::allowed_paths`. When `Some(prefixes)`,
    /// each FS op's resolved path is checked against the
    /// prefixes before proceeding (see
    /// `Vm::check_path_in_allowlist`). Entries are canonicalized
    /// once by `apply_config` so the per-op check can do a pure
    /// lexical resolve + `starts_with` without further syscalls.
    /// `None` (default) means no narrowing on top of the bool.
    pub(crate) allowed_paths: Option<Vec<std::path::PathBuf>>,
    /// Per-eval working counter; `Some(0)` means exhausted, `None`
    /// means unlimited. Re-anchored at each `Runtime::eval` entry
    /// from `Runtime::fuel_budget` (which `apply_config` writes
    /// from `Config::fuel`); decremented per op by `check_fuel`.
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
    pub(crate) sym_length: SymId,
    pub(crate) sym_size: SymId,
    pub(crate) sym_to_s: SymId,
    pub(crate) sym_inspect: SymId,
    /// Hit/miss counters for the per-call-site IC. ZST + no-op
    /// when the `ic-stats` cargo feature is off; readable via
    /// `Runtime::ic_stats()` when on.
    pub(crate) ic_stats: IcStats,
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
    /// Builtin reflection metadata for the synth Methods that
    /// `Kernel.instance_method(:foo)` returns. Looked up by the
    /// `instance_method` arm when the receiver is Kernel.
    ///
    /// Kept OUT of `Kernel.methods` deliberately: putting them on
    /// the actual chain would re-find them during regular dispatch
    /// (`obj.class` etc.) and re-invoke the synth on every call,
    /// creating either recursion or a spurious user-override
    /// signal. The registry is consulted only for the introspection
    /// surface (`instance_method` / `methods` if we ever add it),
    /// not for dispatch.
    pub(crate) kernel_builtin_metas: std::collections::HashMap<crate::intern::SymId, std::rc::Rc<crate::value::BuiltinMeta>>,
    /// Cached `Kernel` SymId, set at install time. `kernel_builtin_method`
    /// uses this for O(1) HashMap lookup into `classes` instead of a
    /// linear name-string walk.
    pub(crate) kernel_class_sym: Option<crate::intern::SymId>,
    /// BasicObject reflection metadata — same shape as Kernel but
    /// for methods CRuby defines on BasicObject (the root):
    /// `__id__`, `__send__`, `equal?`, `instance_eval`,
    /// `instance_exec`, `==`, `!=`, `!`. Kept off
    /// `BasicObject.methods` for the same reason as Kernel — see
    /// `kernel_builtin_metas`.
    pub(crate) basic_object_builtin_metas: std::collections::HashMap<crate::intern::SymId, std::rc::Rc<crate::value::BuiltinMeta>>,
    /// Cached `BasicObject` SymId — same role as `kernel_class_sym`.
    pub(crate) basic_object_class_sym: Option<crate::intern::SymId>,
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
    pub(crate) fn fixed_arity_for_proto(proto: &Proto, params_len: usize) -> Option<FixedArity> {
        let has_rest = proto.rest_param.is_some();
        let has_kw_rest = proto.kw_rest_param.is_some();
        let has_block_param = proto.block_param.is_some();
        let kw_count = proto.kw_param_defaults.len();
        let positional_max = params_len
            - (if has_rest { 1 } else { 0 })
            - kw_count
            - (if has_kw_rest { 1 } else { 0 })
            - (if has_block_param { 1 } else { 0 });
        let required = proto.n_required_positional as usize;
        if has_rest
            || has_kw_rest
            || has_block_param
            || kw_count != 0
            || required != positional_max
        {
            return None;
        }
        Some(FixedArity {
            required: proto.n_required_positional,
            n_locals: proto.n_locals,
        })
    }

    pub(crate) fn new(protos: Vec<Proto>, mut interner: Interner) -> Self {
        let sym_length = interner.intern("length");
        let sym_size = interner.intern("size");
        let sym_to_s = interner.intern("to_s");
        let sym_inspect = interner.intern("inspect");
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
            // Secure-by-default — matches Config::default's
            // `allow_filesystem_io: false`. CLI / FS-needing
            // embedders flip this via `apply_config`.
            allow_filesystem_io: false,
            // No path narrowing by default — `allow_filesystem_io: false`
            // already covers the secure-by-default sandbox.
            allowed_paths: None,
            fuel: None,
            max_frames: None,
            deadline_at: None,
            op_counter: 0,
            max_symbols: None,
            max_value_bytes: None,
            call_caches: Vec::new(),
            method_gen: 0,
            sym_length,
            sym_size,
            sym_to_s,
            sym_inspect,
            ic_stats: IcStats::new(),
            break_signaled: false,
            callable_forwarder_proto: None,
            method_compose_forwarder_proto: None,
            sources: HashMap::new(),
            method_return: None,
            pending_loop_transfer: None,
            suppress_call_result_push: false,
            bypass_visibility_once: false,
            kernel_builtin_metas: std::collections::HashMap::new(),
            kernel_class_sym: None,
            basic_object_builtin_metas: std::collections::HashMap::new(),
            basic_object_class_sym: None,
        }
    }





}


impl Vm {







    /// Consume the in-flight non-local-return value, clearing any
    /// pending break/next transfer along with it.
    ///
    /// Invariant captured here: a `Op::ReturnMethod` that fires
    /// while a `begin/break` (or `next`) is mid-ensure walk
    /// supersedes that structured transfer (CRuby semantics —
    /// `return` wins, the break value is dropped). The
    /// `pending_loop_transfer` slot has to be cleared at the same
    /// instant `method_return` is consumed, otherwise an EndEnsure
    /// in a surviving frame could later resume into the now-stale
    /// target IP.
    ///
    /// All consume sites (currently `vm/step.rs::dispatch`'s unwind
    /// arm and `vm/kernel.rs::require_in_filescope`'s mimic of that
    /// unwind) must go through this helper rather than
    /// `self.method_return.take()` directly so the invariant cannot
    /// drift apart in one of them. Read-only `is_some()` checks
    /// keep using the field directly — they don't consume, so the
    /// invariant doesn't apply.
    pub(crate) fn take_method_return(&mut self) -> Option<Value> {
        let v = self.method_return.take();
        if v.is_some() {
            self.pending_loop_transfer = None;
        }
        v
    }

    /// Consume the visibility-bypass flag set by `send` /
    /// `__send__` (and the `&nil` block-forward case). Returns
    /// whatever value the flag held and clears it to `false` in
    /// one step.
    ///
    /// The two existing consume sites (`vm/dispatch.rs::do_call`
    /// and `do_call_block` at the dispatch boundary) previously
    /// inlined `mem::replace(&mut self.bypass_visibility_once,
    /// false)`. The `take_*` named helper exists so a future
    /// dispatch-entry path can be added by grepping for `take_*`
    /// rather than knowing to spell out the `mem::replace` idiom
    /// from scratch — same discoverability win as
    /// `take_method_return`. The placement constraint the field's
    /// doc comment warns about (consume at dispatch boundary, NOT
    /// at the visibility-check site, otherwise the flag leaks
    /// when dispatch bottoms out before the Object arm) still
    /// applies regardless of which spelling you use; the helper
    /// doesn't enforce it.
    pub(crate) fn take_bypass_visibility(&mut self) -> bool {
        std::mem::replace(&mut self.bypass_visibility_once, false)
    }

    /// Compute the maximum SymId still referenced by long-lived
    /// VM tables that must stay valid across `Runtime::reset` —
    /// `host_fns` (host-registered Ruby methods), and the two
    /// cext method tables. `Runtime::reset` uses this to floor
    /// the interner truncation so post-construction-registered
    /// names don't get their SymIds invalidated.
    ///
    /// `None` when all three tables are empty (no host or cext
    /// registrations) — caller treats this as "truncate to
    /// `snapshot.interner_len` unconditionally".
    ///
    /// Returns `usize` (the underlying repr of SymId) so the
    /// caller can do `keep_len = max(snapshot_len, this + 1)`
    /// directly. Walking all three tables on every reset is
    /// O(num_registered_methods); when cext libraries grow large,
    /// an incremental cache on each register-site would be
    /// cheaper — flagged as future work in PR #212's review.
    pub(crate) fn long_lived_sym_id_max(&self) -> Option<usize> {
        #[allow(unused_mut)]
        let mut max: Option<usize> = self
            .host_fns
            .keys()
            .map(|sym| sym.0 as usize)
            .max();
        // Both cext tables are themselves `#[cfg(feature = "cext")]`
        // (instance methods additionally `not(target_os = "wasi")`);
        // gate the walks the same way so this helper compiles
        // under `--no-default-features` — the fuzz crate
        // disables `cext` to keep the binary lean.
        #[cfg(feature = "cext")]
        for inner in self.cext_class_methods.values() {
            if let Some(m) = inner.keys().map(|sym| sym.0 as usize).max() {
                max = Some(max.map_or(m, |c| c.max(m)));
            }
        }
        #[cfg(all(feature = "cext", not(target_os = "wasi")))]
        for inner in self.cext_instance_methods.values() {
            if let Some(m) = inner.keys().map(|sym| sym.0 as usize).max() {
                max = Some(max.map_or(m, |c| c.max(m)));
            }
        }
        max
    }

    /// Reset every "control flow signal" flag — the per-call
    /// state Op handlers set to communicate break / return /
    /// loop-transfer / suppress-result / bypass-visibility
    /// requests across the dispatch loop. Called from both
    /// `Runtime::eval`'s entry (so a previous eval that left
    /// signals set doesn't bleed into the next) and
    /// `Runtime::reset` (same intent, different trigger). One
    /// helper means a future signal that's added to this set
    /// can't be missed at one site and present at the other —
    /// the kind of drift that's caused real bugs elsewhere in
    /// this codebase.
    pub(crate) fn clear_control_flow_signals(&mut self) {
        self.break_signaled = false;
        self.method_return = None;
        self.pending_loop_transfer = None;
        self.suppress_call_result_push = false;
        self.bypass_visibility_once = false;
    }

    /// Vm-level inner half of `Runtime::reset_between_requests`.
    /// Clears the Vm-owned per-request transient state. The
    /// Runtime wrapper additionally handles the cext debug-
    /// assert (CURRENT_VM_PTR null) and the regex feature-
    /// gated last_match.
    ///
    /// Exposed for callers (the `_http_server` battery's
    /// per-request handler) that hold `&mut Vm` directly
    /// via `current_vm_ptr()` without going back through the
    /// Runtime API.
    #[cfg(feature = "_http_server")]
    pub(crate) fn reset_between_requests_inner(&mut self) {
        self.stack.clear();
        self.frames.clear();
        self.pinned.clear();
        self.class_stack.clear();
        self.class_visibility_stack.clear();
        self.globals.clear();
        self.clear_control_flow_signals();
        #[cfg(feature = "regex")]
        {
            self.last_match = None;
        }
    }

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
                        .find(|(k, _)| k.ruby_eql(key, &self.heap))
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
                    // Use the iter.rs step_block helper (#151);
                    // see `vm/hash.rs::Hash#[]` for the inline
                    // rationale on the BlockStep arms and why
                    // Break maps to LocalJumpError (stored Proc,
                    // not iterator yield).
                    match g.vm.step_block(block_id, vec![Value::Hash(id), key.clone()], pre_frames)? {
                        crate::vm::iter::BlockStep::MethodReturn => {
                            return Ok(Value::Nil);
                        }
                        crate::vm::iter::BlockStep::Break(_) => {
                            return Err(g.vm.trap(crate::error::RubyError::LocalJumpError {
                                msg: "break from proc-closure".into(),
                            }));
                        }
                        crate::vm::iter::BlockStep::Value(r) => {
                            return Ok(r);
                        }
                    }
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


