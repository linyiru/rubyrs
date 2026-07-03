//! Native Commissioner traversal driver ("commdrv") — ADR 0034, RuboCop
//! cop-walk machinery.
//!
//! Replaces the interpreted hot loop of `RuboCop::Cop::Commissioner#walk`
//! (rubocop 1.88 / rubocop-ast 1.49): the per-node `on_<type>` dispatch
//! cascade, `trigger_responding_cops` / `trigger_restricted_cops` block
//! plumbing, the megamorphic `public_send(:"on_#{type}")`, and the
//! per-invocation `with_cop_error_handling` begin/rescue wrapper. The cop
//! callback BODIES still run as ordinary interpreted methods — this module
//! eliminates dispatch/name-resolution/trampoline layers only.
//!
//! ## Shape
//!
//! - **Seal** (`__rubyrs_commdrv_seal`, once per `require "rubocop"`):
//!   resolves and pins the stock `Commissioner#on_<type>` methods plus the
//!   trigger/error-handling sentinels and the `AST::Node#type/#children` /
//!   `SendNode#method_name` accessors. Per-walk these are re-resolved and
//!   ptr-compared whenever `method_gen` moved — any monkey-patch or
//!   subclass interposition makes the driver decline to the interpreted
//!   path. The per-type `triggers` flag (does `on_<type>` resolve to
//!   Commissioner's generated method, or fall through to Traversal's
//!   bare visitor?) is derived from the live resolution, so it tracks
//!   whatever `Parser::Meta::NODE_TYPES` the loaded parser gem has.
//! - **Start** (`__rubyrs_commdrv_start(commissioner, root)`): validates
//!   the `@callbacks` / `@restricted_map` shapes, resolves every
//!   (cop, callback) pair ONCE into method handles (public-visibility
//!   checked — `public_send` fidelity), then runs a native prepass DFS
//!   over the AST that (a) validates every node (Instance, `@type` Sym,
//!   `@children` Array, no eigenclass, stock accessors) and (b) flattens
//!   the visit order into a linear program of (node, callback-list)
//!   trigger items in EXACTLY the order the interpreted
//!   Commissioner/Traversal pair produces. Any surprise → decline, which
//!   is always safe here because no callback has fired yet.
//! - **Run** (`__rubyrs_commdrv_run(handle)`): executes the program,
//!   invoking each cop callback through the ordinary
//!   `invoke_method` + `dispatch_until` VM entry (same path
//!   `Hash#user-eql?` etc. use). Returns `true` when the program is
//!   exhausted.
//!
//! ## Error-handling protocol (the `with_cop_error_handling` contract)
//!
//! The interpreted wrapper is `begin; cop.public_send(cb, node); rescue
//! StandardError => e; ...record...; end` per invocation. Natively we get
//! byte-exact rescue semantics by NOT rescuing in Rust at all: the hook's
//! `walk` runs `__rubyrs_commdrv_run` inside an interpreted
//! `begin/rescue StandardError` loop. When a cop callback raises, the
//! VM's unwinder finds that interpreted handler (it is live on the frame
//! stack below the native boundary), `dispatch_until` hands the driver an
//! `AlreadyCaught`/raw trap, the driver saves its program counter +
//! pending (cop, node) and re-emits the trap out of the host fn. The
//! rescue body records the error exactly like `with_cop_error_handling`
//! (including `@options[:raise_error]` / `[:raise_cop_error]` and `$!`
//! scoping — it IS an interpreted rescue), then calls run again, which
//! resumes from the saved position. Non-StandardError exceptions and
//! `throw` carriers unwind PAST the hook's rescue exactly as they unwind
//! past the interpreted `rescue StandardError`, abandoning the walk; the
//! hook's `ensure` frees the native state.
//!
//! ## GC rooting
//!
//! The walk state holds ObjIds for AST nodes and cop instances across
//! callback invocations that allocate (and so can GC). All of them are
//! reachable from the hook's `walk` frame for the state's whole lifetime:
//! nodes via the root-node argument local (the AST is a frozen tree —
//! `@children` arrays are ivar-reachable), cops via the commissioner
//! (`self`) → `@callbacks` / `@restricted_map` ivar hashes. The driver
//! therefore adds no pins of its own; `STRESS_GC=1` on a walk fixture is
//! the regression gate for this reasoning.
//!
//! ## Fidelity notes (deliberate, unreachable-in-practice divergences)
//!
//! - Traversal visits `always`-slot and `many_node_children` children
//!   unconditionally; a `nil` in such a slot raises NoMethodError
//!   mid-walk on the interpreted path (grammar-impossible — the builder
//!   never produces it). The native prepass skips nil slots instead.
//! - A cop that mutates the (frozen-by-the-builder) AST mid-walk would
//!   diverge from the prepass' flattened order. Frozen node instances
//!   make this impossible without `instance_variable_set` gymnastics no
//!   cop performs.
//! - ASTs nested deeper than `MAX_DEPTH` decline to the interpreted path
//!   (which SystemStackErrors on them, same as CRuby).
//!
//! Kill switch: `RUBYRS_COMMDRV_NO_NATIVE=1`. Decline diagnostics:
//! `RUBYRS_COMMDRV_DEBUG=1`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{NoMethodErrorKind, RubyError, Trap};
use crate::heap::HeapObj;
use crate::intern::{FxHashMap, SymId};
use crate::value::{Class, Method, ObjId, Value, Visibility};
use crate::vm::Vm;

pub(crate) const HOOK_RB: &str = include_str!("commdrv_hook.rb");

/// Decline result: `Err(Decline)` bubbles a reason string up to the
/// host-fn boundary, which returns `nil` to the hook (→ interpreted walk).
struct Decline(&'static str);
type DRes<T> = Result<T, Decline>;

fn decline<T>(why: &'static str) -> DRes<T> {
    Err(Decline(why))
}

fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUBYRS_COMMDRV_DEBUG").is_some())
}

// ---------------------------------------------------------------------------
// Per-type traversal plans — mirrors rubocop-ast 1.49's Traversal templates.
// ---------------------------------------------------------------------------

/// How Traversal's generated `on_<type>` visits the node's children.
#[derive(Clone, Copy, PartialEq)]
enum Plan {
    /// `NO_CHILD_NODES` in Commissioner: no `super`, so no children AND no
    /// `after_<type>` triggers.
    NoChild,
    /// Visit the listed child indices (absent or nil slots skipped).
    /// Covers the positional templates (`:always` / `:nil?` slots are both
    /// nil-skipped — see the module doc's fidelity note).
    Idx(&'static [u8]),
    /// Visit every child (nil-skipped) — `many_node_children` +
    /// `many_opt_node_children`.
    All,
    /// `send` / `csend`: every child except index 1 (the method-name
    /// symbol), nil-skipped. These two types also fire the RESTRICTED
    /// callback tables.
    SendLike,
}

/// `(type name, plan)` for every type rubocop-ast 1.49's Traversal defines
/// an `on_<type>` for. A node whose type is NOT in this table declines the
/// walk (the interpreted path then reproduces whatever
/// NoMethodError/forward-compat behavior applies).
const TYPE_TABLE: &[(&str, Plan)] = &[
    // --- NO_CHILD_NODES (no_children + opt_symbol_child + literal_child) ---
    ("true", Plan::NoChild), ("false", Plan::NoChild), ("nil", Plan::NoChild),
    ("self", Plan::NoChild), ("cbase", Plan::NoChild), ("zsuper", Plan::NoChild),
    ("redo", Plan::NoChild), ("retry", Plan::NoChild),
    ("forward_args", Plan::NoChild), ("forwarded_args", Plan::NoChild),
    ("match_nil_pattern", Plan::NoChild), ("forward_arg", Plan::NoChild),
    ("forwarded_restarg", Plan::NoChild), ("forwarded_kwrestarg", Plan::NoChild),
    ("lambda", Plan::NoChild), ("empty_else", Plan::NoChild),
    ("kwnilarg", Plan::NoChild), ("blocknilarg", Plan::NoChild),
    ("__FILE__", Plan::NoChild), ("__LINE__", Plan::NoChild),
    ("__ENCODING__", Plan::NoChild),
    ("restarg", Plan::NoChild), ("kwrestarg", Plan::NoChild),
    ("int", Plan::NoChild), ("float", Plan::NoChild), ("complex", Plan::NoChild),
    ("rational", Plan::NoChild), ("str", Plan::NoChild), ("sym", Plan::NoChild),
    ("lvar", Plan::NoChild), ("ivar", Plan::NoChild), ("cvar", Plan::NoChild),
    ("gvar", Plan::NoChild), ("nth_ref", Plan::NoChild), ("back_ref", Plan::NoChild),
    ("arg", Plan::NoChild), ("blockarg", Plan::NoChild), ("shadowarg", Plan::NoChild),
    ("kwarg", Plan::NoChild), ("match_var", Plan::NoChild),
    // --- single node child at index 0 ---
    ("splat", Plan::Idx(&[0])), ("kwsplat", Plan::Idx(&[0])),
    ("match_rest", Plan::Idx(&[0])),
    ("not", Plan::Idx(&[0])), ("match_current_line", Plan::Idx(&[0])),
    ("defined?", Plan::Idx(&[0])), ("arg_expr", Plan::Idx(&[0])),
    ("pin", Plan::Idx(&[0])), ("if_guard", Plan::Idx(&[0])),
    ("unless_guard", Plan::Idx(&[0])),
    ("match_with_trailing_comma", Plan::Idx(&[0])),
    ("block_pass", Plan::Idx(&[0])), ("preexe", Plan::Idx(&[0])),
    ("postexe", Plan::Idx(&[0])),
    ("const", Plan::Idx(&[0])),
    // --- symbol then node ---
    ("lvasgn", Plan::Idx(&[1])), ("ivasgn", Plan::Idx(&[1])),
    ("cvasgn", Plan::Idx(&[1])), ("gvasgn", Plan::Idx(&[1])),
    ("optarg", Plan::Idx(&[1])), ("kwoptarg", Plan::Idx(&[1])),
    // --- node then optional node ---
    ("while", Plan::Idx(&[0, 1])), ("until", Plan::Idx(&[0, 1])),
    ("module", Plan::Idx(&[0, 1])), ("sclass", Plan::Idx(&[0, 1])),
    // --- symbol-only children, but NOT in NO_CHILD_NODES (after_ fires) ---
    ("regopt", Plan::Idx(&[])),
    // --- many node children (unconditional in Traversal) ---
    ("dstr", Plan::All), ("dsym", Plan::All), ("xstr", Plan::All),
    ("regexp", Plan::All), ("array", Plan::All), ("hash", Plan::All),
    ("pair", Plan::All), ("mlhs", Plan::All), ("masgn", Plan::All),
    ("or_asgn", Plan::All), ("and_asgn", Plan::All), ("rasgn", Plan::All),
    ("mrasgn", Plan::All), ("undef", Plan::All), ("alias", Plan::All),
    ("args", Plan::All), ("super", Plan::All), ("yield", Plan::All),
    ("or", Plan::All), ("and", Plan::All), ("while_post", Plan::All),
    ("until_post", Plan::All), ("match_with_lvasgn", Plan::All),
    ("begin", Plan::All), ("kwbegin", Plan::All), ("return", Plan::All),
    ("in_match", Plan::All), ("match_alt", Plan::All), ("break", Plan::All),
    ("next", Plan::All), ("match_as", Plan::All), ("array_pattern", Plan::All),
    ("array_pattern_with_tail", Plan::All), ("hash_pattern", Plan::All),
    ("const_pattern", Plan::All), ("find_pattern", Plan::All),
    ("index", Plan::All), ("indexasgn", Plan::All), ("procarg0", Plan::All),
    ("kwargs", Plan::All),
    // --- many OPTIONAL node children (nil-guarded in Traversal too) ---
    ("case", Plan::All), ("rescue", Plan::All), ("resbody", Plan::All),
    ("ensure", Plan::All), ("for", Plan::All), ("when", Plan::All),
    ("case_match", Plan::All), ("in_pattern", Plan::All),
    ("irange", Plan::All), ("erange", Plan::All),
    ("match_pattern", Plan::All), ("match_pattern_p", Plan::All),
    ("iflipflop", Plan::All), ("eflipflop", Plan::All),
    // --- positional specials ---
    ("casgn", Plan::Idx(&[0, 2])), ("op_asgn", Plan::Idx(&[0, 2])),
    ("numblock", Plan::Idx(&[0, 2])), ("itblock", Plan::Idx(&[0, 2])),
    ("class", Plan::Idx(&[0, 1, 2])), ("if", Plan::Idx(&[0, 1, 2])),
    ("block", Plan::Idx(&[0, 1, 2])),
    ("def", Plan::Idx(&[1, 2])),
    ("defs", Plan::Idx(&[0, 2, 3])),
    // --- send family (restricted-callback types) ---
    ("send", Plan::SendLike), ("csend", Plan::SendLike),
];

/// Interpreted `walk` recursion consumes a few frames per AST level;
/// rubyrs SystemStackErrors around 10k frames. Decline well below that so
/// pathological nestings keep their interpreted (raising) behavior.
const MAX_DEPTH: u32 = 1000;

// ---------------------------------------------------------------------------
// Seal — hook-time sentinel capture + per-mgen verification
// ---------------------------------------------------------------------------

struct SealType {
    on_sym: SymId,
    /// Commissioner-chain resolution of `on_<type>` captured at seal time.
    /// `None`: the loaded parser/rubocop-ast pair doesn't define it — a
    /// node of this type declines (interpreted path raises NoMethodError).
    on_method: Option<Rc<Method>>,
    /// `on_<type>` resolves to Commissioner's generated trigger method
    /// (true) vs falling through to Traversal's bare visitor (false — the
    /// type is outside `Parser::Meta::NODE_TYPES`, children are visited
    /// but NO cop callbacks fire).
    triggers: bool,
}

struct Seal {
    commissioner_class: Rc<Class>,
    types: Vec<SealType>,
    /// `@callbacks` key → (TYPE_TABLE index, phase 0=on/1=after).
    cb_index: FxHashMap<SymId, (u16, u8)>,
    /// `@type` sym → TYPE_TABLE index.
    type_index: FxHashMap<SymId, u16>,
    // Sentinels: the machinery we REPLACE. If any of these resolve to a
    // different method than at seal time, someone patched Commissioner —
    // decline everything.
    trigger_responding: Rc<Method>,
    trigger_restricted: Rc<Method>,
    with_cop_error: Rc<Method>,
    /// `SendNode#method_name` — replaced by a direct `children[1]` read.
    send_method_name: Rc<Method>,
    /// `AST::Node#type` / `#children` — replaced by direct ivar reads.
    node_type_m: Rc<Method>,
    node_children_m: Rc<Method>,
    node_class: Rc<Class>,
    send_node_class: Rc<Class>,
    // Interned ids used per walk.
    sym_ivar_type: SymId,
    sym_ivar_children: SymId,
    sym_ivar_callbacks: SymId,
    sym_ivar_restricted: SymId,
    sym_type: SymId,
    sym_children: SymId,
    sym_method_name: SymId,
    /// `[on_send, on_csend, after_send, after_csend]`.
    restricted_keys: [SymId; 4],
    /// method_gen at which the sentinels last re-verified OK.
    verified_gen: Option<u32>,
    verified_ok: bool,
    /// Node classes whose `type`/`children` resolve to the stock
    /// accessors (keyed by `Rc::as_ptr`). Cleared on every mgen bump.
    node_class_ok: FxHashMap<usize, bool>,
}

// ---------------------------------------------------------------------------
// Walk state — one per active `Commissioner#walk`
// ---------------------------------------------------------------------------

/// Resolution of one (cop, callback) pair. `Missing`/`NonPublic` only
/// arise from a mid-walk re-resolve after a `method_gen` bump — the
/// invocation then synthesizes the NoMethodError `public_send` would
/// have raised (caught by the hook's rescue → recorded per contract).
#[derive(Clone)]
enum CbTarget {
    Resolved(Rc<Method>),
    Missing,
    NonPublic(Visibility),
}

struct CbEntry {
    cop: Value,
    cb_sym: SymId,
    target: CbTarget,
}

/// One trigger step of the flattened program: fire `lists[list]` (the
/// responding cops), then — for send/csend — the restricted cops for
/// this node's method name.
struct PItem {
    node: ObjId,
    /// Index into `WalkState::lists`; `u32::MAX` = no responding list.
    list: u32,
    /// 0 = none; 1..=4 → `restricted[rkind-1]`
    /// (`[on_send, on_csend, after_send, after_csend]`).
    rkind: u8,
    /// `children[1]` of the send/csend node (only read when rkind != 0).
    mname: SymId,
}

struct WalkState {
    nonce: u32,
    lists: Vec<Vec<CbEntry>>,
    /// method-name → `lists` index, per restricted kind.
    restricted: [FxHashMap<SymId, u32>; 4],
    program: Vec<PItem>,
    /// Resume cursor: program counter, phase (0 = responding list,
    /// 1 = restricted list), index within the current list.
    pc: u32,
    phase: u8,
    idx: u32,
    /// method_gen the lists were resolved against.
    mgen: u32,
    /// The (cop, node) whose callback raised — read back by the hook's
    /// error recorder while the interpreted rescue handles the exception.
    pending_cop: Value,
    pending_node: Value,
}

#[derive(Default)]
struct Commdrv {
    seal: Option<Seal>,
    slots: Vec<Option<Box<WalkState>>>,
    free: Vec<usize>,
    next_nonce: u32,
}

thread_local! {
    static COMMDRV: RefCell<Commdrv> = RefCell::new(Commdrv::default());
}

fn handle_of(slot: usize, nonce: u32) -> i64 {
    ((nonce as i64) << 32) | slot as i64
}

fn parse_handle(h: i64) -> (usize, u32) {
    ((h & 0xffff_ffff) as usize, (h >> 32) as u32)
}

// ---------------------------------------------------------------------------
// Seal construction + verification
// ---------------------------------------------------------------------------

fn resolve_on(vm: &Vm, cls: &Rc<Class>, sym: SymId) -> Option<Rc<Method>> {
    vm.lookup_method_uncached(cls, sym)
}

/// Classify `on_<type>`'s resolution: Ok(true) = Commissioner's generated
/// trigger method, Ok(false) = Traversal's bare visitor, Err = foreign
/// (patched) definition.
fn classify_on(m: &Rc<Method>, seal_comm: &Rc<Class>, traversal: *const Class) -> Result<bool, ()> {
    match m.defining_class.as_ref().and_then(std::rc::Weak::upgrade) {
        Some(dc) if Rc::ptr_eq(&dc, seal_comm) => Ok(true),
        Some(dc) if Rc::as_ptr(&dc) == traversal => Ok(false),
        _ => Err(()),
    }
}

fn build_seal(vm: &mut Vm, comm_cls: Rc<Class>, traversal: Rc<Class>, send_cls: Rc<Class>, node_cls: Rc<Class>) -> Option<Seal> {
    let traversal_ptr = Rc::as_ptr(&traversal);
    let mut types = Vec::with_capacity(TYPE_TABLE.len());
    let mut cb_index = FxHashMap::default();
    let mut type_index = FxHashMap::default();
    for (i, (name, _plan)) in TYPE_TABLE.iter().enumerate() {
        let type_sym = vm.interner.intern(name);
        let on_sym = vm.interner.intern(&format!("on_{name}"));
        let after_sym = vm.interner.intern(&format!("after_{name}"));
        let on_method = resolve_on(vm, &comm_cls, on_sym);
        let triggers = match &on_method {
            Some(m) => match classify_on(m, &comm_cls, traversal_ptr) {
                Ok(t) => t,
                Err(()) => return None, // pre-patched environment: unusable
            },
            None => false,
        };
        cb_index.insert(on_sym, (i as u16, 0));
        cb_index.insert(after_sym, (i as u16, 1));
        type_index.insert(type_sym, i as u16);
        types.push(SealType { on_sym, on_method, triggers });
    }
    let s = |vm: &mut Vm, n: &str| vm.interner.intern(n);
    let tr_sym = s(vm, "trigger_responding_cops");
    let tre_sym = s(vm, "trigger_restricted_cops");
    let wceh_sym = s(vm, "with_cop_error_handling");
    let mname_sym = s(vm, "method_name");
    let type_sym = s(vm, "type");
    let children_sym = s(vm, "children");
    let seal = Seal {
        trigger_responding: resolve_on(vm, &comm_cls, tr_sym)?,
        trigger_restricted: resolve_on(vm, &comm_cls, tre_sym)?,
        with_cop_error: resolve_on(vm, &comm_cls, wceh_sym)?,
        send_method_name: resolve_on(vm, &send_cls, mname_sym)?,
        node_type_m: resolve_on(vm, &node_cls, type_sym)?,
        node_children_m: resolve_on(vm, &node_cls, children_sym)?,
        commissioner_class: comm_cls,
        types,
        cb_index,
        type_index,
        node_class: node_cls,
        send_node_class: send_cls,
        sym_ivar_type: s(vm, "@type"),
        sym_ivar_children: s(vm, "@children"),
        sym_ivar_callbacks: s(vm, "@callbacks"),
        sym_ivar_restricted: s(vm, "@restricted_map"),
        sym_type: type_sym,
        sym_children: children_sym,
        sym_method_name: mname_sym,
        restricted_keys: [
            s(vm, "on_send"), s(vm, "on_csend"), s(vm, "after_send"), s(vm, "after_csend"),
        ],
        verified_gen: None,
        verified_ok: true,
        node_class_ok: FxHashMap::default(),
    };
    Some(seal)
}

/// Re-resolve every sealed sentinel and ptr-compare against the captured
/// methods. Runs only when `method_gen` moved since the last verification;
/// a mismatch means the walk machinery was patched → decline (re-checked
/// on the next mgen bump, so an un-patch would re-enable).
fn verify_seal(vm: &Vm, seal: &mut Seal) -> bool {
    let mgen = vm.method_gen;
    if seal.verified_gen == Some(mgen) {
        return seal.verified_ok;
    }
    seal.node_class_ok.clear();
    let ok = (|| {
        let same = |a: &Option<Rc<Method>>, b: &Rc<Method>| {
            a.as_ref().is_some_and(|m| Rc::ptr_eq(m, b))
        };
        let c = &seal.commissioner_class;
        if !same(&resolve_on(vm, c, vm.interner.get_id("trigger_responding_cops")?), &seal.trigger_responding) { return None; }
        if !same(&resolve_on(vm, c, vm.interner.get_id("trigger_restricted_cops")?), &seal.trigger_restricted) { return None; }
        if !same(&resolve_on(vm, c, vm.interner.get_id("with_cop_error_handling")?), &seal.with_cop_error) { return None; }
        if !same(&resolve_on(vm, &seal.send_node_class, seal.sym_method_name), &seal.send_method_name) { return None; }
        if !same(&resolve_on(vm, &seal.node_class, seal.sym_type), &seal.node_type_m) { return None; }
        if !same(&resolve_on(vm, &seal.node_class, seal.sym_children), &seal.node_children_m) { return None; }
        for st in &seal.types {
            let cur = resolve_on(vm, c, st.on_sym);
            match (&st.on_method, &cur) {
                (Some(a), Some(b)) if Rc::ptr_eq(a, b) => {}
                (None, None) => {}
                _ => return None,
            }
        }
        Some(())
    })()
    .is_some();
    seal.verified_gen = Some(mgen);
    seal.verified_ok = ok;
    ok
}

/// `type`/`children` on this node class must resolve to the stock
/// accessors captured at seal time (i.e. no node subclass overrides them —
/// the interpreted traversal calls the methods, we read the ivars).
fn node_class_verified(vm: &Vm, seal: &mut Seal, cls: &Rc<Class>) -> bool {
    let key = Rc::as_ptr(cls) as usize;
    if let Some(ok) = seal.node_class_ok.get(&key) {
        return *ok;
    }
    let ok = resolve_on(vm, cls, seal.sym_type).is_some_and(|m| Rc::ptr_eq(&m, &seal.node_type_m))
        && resolve_on(vm, cls, seal.sym_children).is_some_and(|m| Rc::ptr_eq(&m, &seal.node_children_m));
    seal.node_class_ok.insert(key, ok);
    ok
}

// ---------------------------------------------------------------------------
// Start: table build + prepass
// ---------------------------------------------------------------------------

/// Resolve one cop's callback like `public_send` would (public methods
/// only). At build time a non-`Resolved` target declines the whole walk;
/// after a mid-walk mgen bump it synthesizes the correct NoMethodError.
fn resolve_cb(vm: &Vm, cop: &Value, cb_sym: SymId) -> CbTarget {
    let Value::Object(oid) = cop else { return CbTarget::Missing };
    let cls = vm.heap.class_of(*oid);
    match vm.lookup_method_uncached(&cls, cb_sym) {
        Some(m) => match m.visibility.get() {
            Visibility::Public => CbTarget::Resolved(m),
            v => CbTarget::NonPublic(v),
        },
        None => CbTarget::Missing,
    }
}

/// Read the cops array behind one `@callbacks`/`@restricted_map` value
/// into resolved entries. Declines on non-Object cops or non-public
/// callbacks (interpreted path then reproduces the per-node raises).
fn build_list(vm: &Vm, arr: &Value, cb_sym: SymId) -> DRes<Vec<CbEntry>> {
    let Value::Array(aid) = arr else { return decline("callbacks value not an Array") };
    let cops: Vec<Value> = vm.heap.array(*aid).clone();
    let mut out = Vec::with_capacity(cops.len());
    for cop in cops {
        if !matches!(cop, Value::Object(_)) {
            return decline("cop is not an object instance");
        }
        let target = resolve_cb(vm, &cop, cb_sym);
        if !matches!(target, CbTarget::Resolved(_)) {
            return decline("cop callback missing or non-public");
        }
        out.push(CbEntry { cop, cb_sym, target });
    }
    Ok(out)
}

/// Per-walk trigger tables derived from `@callbacks` + `@restricted_map`.
struct Tables {
    lists: Vec<Vec<CbEntry>>,
    /// TYPE_TABLE-parallel: responding list index for (on, after).
    per_type: Vec<[u32; 2]>,
    restricted: [FxHashMap<SymId, u32>; 4],
    /// Precomputed PItem.rkind for send/csend × on/after (0 if that
    /// restricted map is empty).
    rkind: [[u8; 2]; 2],
}

fn build_tables(vm: &Vm, seal: &Seal, comm: ObjId) -> DRes<Tables> {
    let inst = vm.heap.instance(comm);
    let Some(&Value::Hash(cb_hid)) = inst.ivar_get(seal.sym_ivar_callbacks) else {
        return decline("@callbacks missing or not a Hash");
    };
    let Some(&Value::Hash(rm_hid)) = inst.ivar_get(seal.sym_ivar_restricted) else {
        return decline("@restricted_map missing or not a Hash");
    };

    let mut lists: Vec<Vec<CbEntry>> = Vec::new();
    let mut per_type = vec![[u32::MAX; 2]; TYPE_TABLE.len()];

    let cb_pairs: Vec<(Value, Value)> = vm.heap.hash(cb_hid).to_vec();
    for (k, v) in &cb_pairs {
        let Value::Sym(ks) = k else { return decline("@callbacks key not a Symbol") };
        // Keys that aren't `on_/after_<known type>` can only belong to
        // types the walk never dispatches (callbacks_needed guarantees the
        // on_/after_ prefix) — the interpreted path never fires them either.
        let Some(&(ti, phase)) = seal.cb_index.get(ks) else { continue };
        let entries = build_list(vm, v, *ks)?;
        per_type[ti as usize][phase as usize] = lists.len() as u32;
        lists.push(entries);
    }

    let mut restricted: [FxHashMap<SymId, u32>; 4] = Default::default();
    let rm_pairs: Vec<(Value, Value)> = vm.heap.hash(rm_hid).to_vec();
    for (k, v) in &rm_pairs {
        let Value::Sym(ks) = k else { return decline("@restricted_map key not a Symbol") };
        let Some(kind) = seal.restricted_keys.iter().position(|r| r == ks) else {
            return decline("@restricted_map has an unexpected key");
        };
        let Value::Hash(sub_hid) = v else { return decline("@restricted_map value not a Hash") };
        let sub_pairs: Vec<(Value, Value)> = vm.heap.hash(*sub_hid).to_vec();
        for (mk, mv) in &sub_pairs {
            let Value::Sym(ms) = mk else { return decline("restricted method name not a Symbol") };
            let entries = build_list(vm, mv, seal.restricted_keys[kind])?;
            restricted[kind].insert(*ms, lists.len() as u32);
            lists.push(entries);
        }
    }

    // send=0 / csend=1 × on=0 / after=1 → PItem.rkind (1-based, 0 = none).
    let rk = |k: usize| if restricted[k].is_empty() { 0u8 } else { (k + 1) as u8 };
    let rkind = [[rk(0), rk(2)], [rk(1), rk(3)]];

    Ok(Tables { lists, per_type, restricted, rkind })
}

/// One prepass work item: visit a node, or emit a node's after-trigger
/// once its subtree is done.
enum PP {
    Enter(ObjId, u32),
    After(ObjId, u16),
}

/// Flatten the traversal into trigger items, validating every node.
/// Runs BEFORE any callback fires, so every decline is safe.
fn prepass(vm: &Vm, seal: &mut Seal, t: &Tables, root: ObjId) -> DRes<Vec<PItem>> {
    let send_ti = *seal.type_index.get(&vm.interner.get_id("send").ok_or(Decline("send sym"))?).ok_or(Decline("send ti"))?;
    let csend_ti = *seal.type_index.get(&vm.interner.get_id("csend").ok_or(Decline("csend sym"))?).ok_or(Decline("csend ti"))?;

    let mut program: Vec<PItem> = Vec::with_capacity(1024);
    let mut stack: Vec<PP> = vec![PP::Enter(root, 0)];

    while let Some(item) = stack.pop() {
        match item {
            PP::After(node, ti) => {
                let list = t.per_type[ti as usize][1];
                let send_kind = if ti == send_ti { Some(0usize) } else if ti == csend_ti { Some(1) } else { None };
                let mut rkind = 0u8;
                let mut mname = SymId(0);
                if let Some(sk) = send_kind {
                    rkind = t.rkind[sk][1];
                    if rkind != 0 {
                        mname = send_mname(vm, seal, node)?;
                    }
                }
                let has_list = list != u32::MAX && !t.lists[list as usize].is_empty();
                if has_list || rkind != 0 {
                    program.push(PItem { node, list: if has_list { list } else { u32::MAX }, rkind, mname });
                }
            }
            PP::Enter(node, depth) => {
                if depth > MAX_DEPTH {
                    return decline("AST deeper than MAX_DEPTH");
                }
                let inst = match vm.heap.get(node) {
                    HeapObj::Instance(inst) => inst,
                    _ => return decline("node is not a plain instance"),
                };
                if inst.singleton_class.is_some() {
                    return decline("node has an eigenclass");
                }
                let cls = inst.class.clone();
                let Some(&Value::Sym(ts)) = inst.ivar_get(seal.sym_ivar_type) else {
                    return decline("node @type missing or not a Symbol");
                };
                let children_v = inst.ivar_get(seal.sym_ivar_children).cloned();
                if !node_class_verified(vm, seal, &cls) {
                    return decline("node class overrides type/children");
                }
                let Some(&ti) = seal.type_index.get(&ts) else {
                    return decline("unknown node type");
                };
                let st = &seal.types[ti as usize];
                if st.on_method.is_none() {
                    return decline("type without any on_ handler");
                }
                let plan = TYPE_TABLE[ti as usize].1;

                // ON trigger (only when Commissioner's generated method
                // would run — `triggers`).
                if st.triggers {
                    let list = t.per_type[ti as usize][0];
                    let send_kind = if ti == send_ti { Some(0usize) } else if ti == csend_ti { Some(1) } else { None };
                    let mut rkind = 0u8;
                    let mut mname = SymId(0);
                    if let Some(sk) = send_kind {
                        rkind = t.rkind[sk][0];
                        if rkind != 0 {
                            mname = send_mname(vm, seal, node)?;
                        }
                    }
                    let has_list = list != u32::MAX && !t.lists[list as usize].is_empty();
                    if has_list || rkind != 0 {
                        program.push(PItem { node, list: if has_list { list } else { u32::MAX }, rkind, mname });
                    }
                }

                if matches!(plan, Plan::NoChild) {
                    continue; // no super: no children, no after triggers
                }

                // After-trigger fires once the children are done — push it
                // below the children so it pops last.
                if st.triggers {
                    stack.push(PP::After(node, ti));
                }

                let Some(Value::Array(cid)) = children_v else {
                    return decline("node @children missing or not an Array");
                };
                let children = vm.heap.array(cid);
                // Push children in REVERSE so they pop in source order.
                let push_child = |stack: &mut Vec<PP>, c: &Value| -> DRes<()> {
                    match c {
                        Value::Nil => Ok(()), // nil slots skipped (see module doc)
                        Value::Object(id) => {
                            stack.push(PP::Enter(*id, depth + 1));
                            Ok(())
                        }
                        _ => decline("non-node child in a visited slot"),
                    }
                };
                match plan {
                    Plan::All => {
                        for c in children.iter().rev() {
                            push_child(&mut stack, c)?;
                        }
                    }
                    Plan::SendLike => {
                        for (i, c) in children.iter().enumerate().rev() {
                            if i == 1 {
                                continue; // method-name symbol
                            }
                            push_child(&mut stack, c)?;
                        }
                    }
                    Plan::Idx(idxs) => {
                        for &i in idxs.iter().rev() {
                            if let Some(c) = children.get(i as usize) {
                                push_child(&mut stack, c)?;
                            }
                        }
                    }
                    Plan::NoChild => unreachable!(),
                }
            }
        }
    }
    Ok(program)
}

/// `node.method_name` for send/csend — `children[1]`, guaranteed a Symbol
/// by the builder (anything else declines at prepass).
fn send_mname(vm: &Vm, seal: &Seal, node: ObjId) -> DRes<SymId> {
    let HeapObj::Instance(inst) = vm.heap.get(node) else { return decline("send node not an instance") };
    let Some(Value::Array(cid)) = inst.ivar_get(seal.sym_ivar_children) else {
        return decline("send node @children missing");
    };
    match vm.heap.array(*cid).get(1) {
        Some(Value::Sym(s)) => Ok(*s),
        _ => decline("send node method name is not a Symbol"),
    }
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

/// Invoke one cop callback through the ordinary VM method entry.
/// No pins needed: cop + node are rooted via the hook `walk` frame
/// (commissioner `self` → @callbacks; root-node arg → frozen AST tree).
fn invoke_cb(vm: &mut Vm, entry: &CbEntry, node: ObjId) -> Result<(), Trap> {
    let m = match &entry.target {
        CbTarget::Resolved(m) => m.clone(),
        CbTarget::Missing => {
            let name = vm.interner.resolve(entry.cb_sym).to_string();
            let recv = vm.recv_desc_for_error(&entry.cop);
            return Err(vm.trap(RubyError::NoMethodError {
                kind: NoMethodErrorKind::Missing,
                method: name,
                recv_type: std::borrow::Cow::Owned(recv),
            }));
        }
        CbTarget::NonPublic(v) => {
            let name = vm.interner.resolve(entry.cb_sym).to_string();
            let recv = vm.recv_desc_for_error(&entry.cop);
            let kind = match v {
                Visibility::Private => NoMethodErrorKind::Private,
                _ => NoMethodErrorKind::Protected,
            };
            return Err(vm.trap(RubyError::NoMethodError {
                kind,
                method: name,
                recv_type: std::borrow::Cow::Owned(recv),
            }));
        }
    };
    let pre = vm.frames.len();
    vm.invoke_method(m, entry.cop.clone(), vec![Value::Object(node)])?;
    vm.dispatch_until(pre)?;
    if vm.frames.len() != pre {
        // A suspension (Fiber.yield) escaped the callback — the native
        // walk can't be resumed coherently. Surface loudly.
        return Err(vm.trap(RubyError::RuntimeError {
            msg: "commdrv: cop callback suspended mid-walk".to_string(),
        }));
    }
    vm.stack.pop();
    Ok(())
}

/// Re-resolve every list entry after a `method_gen` bump (a cop defined /
/// removed / re-visibilitied methods mid-walk — lazy requires inside
/// callbacks do this).
fn reresolve(vm: &Vm, st: &mut WalkState) {
    for list in &mut st.lists {
        for e in list.iter_mut() {
            e.target = resolve_cb(vm, &e.cop, e.cb_sym);
        }
    }
    st.mgen = vm.method_gen;
}

/// Fire one callback list from `st.idx` onward. On a raise: saves the
/// pending (cop, node), advances the cursor past the raising entry (the
/// interpreted `each` continues with the next cop after
/// `with_cop_error_handling` records), and re-emits the trap.
fn fire_list(vm: &mut Vm, st: &mut WalkState, list_idx: u32, node: ObjId) -> Result<(), Trap> {
    loop {
        if vm.method_gen != st.mgen {
            reresolve(vm, st);
        }
        let list = &st.lists[list_idx as usize];
        let Some(entry_ref) = list.get(st.idx as usize) else { return Ok(()) };
        // Clone the entry out of `st` so no borrow spans the (re-entrant)
        // invocation.
        let entry = CbEntry {
            cop: entry_ref.cop.clone(),
            cb_sym: entry_ref.cb_sym,
            target: entry_ref.target.clone(),
        };
        match invoke_cb(vm, &entry, node) {
            Ok(()) => st.idx += 1,
            Err(t) => {
                st.pending_cop = entry.cop;
                st.pending_node = Value::Object(node);
                st.idx += 1;
                return Err(t);
            }
        }
    }
}

/// Drive the program from the saved cursor. `Ok(true)` = walk complete.
/// `Err` = a callback raised; the cursor + pending (cop, node) are saved
/// and the trap re-emits to the hook's interpreted rescue.
fn run(vm: &mut Vm, st: &mut WalkState) -> Result<bool, Trap> {
    while (st.pc as usize) < st.program.len() {
        let (node, list_idx, rkind, mname) = {
            let it = &st.program[st.pc as usize];
            (it.node, it.list, it.rkind, it.mname)
        };
        if st.phase == 0 {
            if list_idx != u32::MAX {
                fire_list(vm, st, list_idx, node)?;
            }
            st.phase = 1;
            st.idx = 0;
        }
        // Restricted phase (send/csend only): the cops registered for
        // this node's method name.
        if rkind != 0
            && let Some(&rlist) = st.restricted[(rkind - 1) as usize].get(&mname)
        {
            fire_list(vm, st, rlist, node)?;
        }
        st.pc += 1;
        st.phase = 0;
        st.idx = 0;
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Host fns
// ---------------------------------------------------------------------------

fn vm_from_ptr<'a>() -> Result<&'a mut Vm, Trap> {
    let ptr = crate::vm::current_vm_ptr();
    if ptr.is_null() {
        return Err(Trap::new(RubyError::RuntimeError {
            msg: "commdrv: CURRENT_VM_PTR null — called outside host-fn scope".to_string(),
        }));
    }
    // SAFETY: installed by the dispatch site for this call's synchronous
    // duration (same pattern as prism_wq / json_native).
    Ok(unsafe { &mut *ptr })
}

pub fn register_host_fns(rt: &mut crate::Runtime) {
    // __rubyrs_commdrv_seal(Commissioner, Traversal, SendNode, Node) → bool
    rt.register_fn("__rubyrs_commdrv_seal", |args| {
        let [Value::Class(comm), Value::Class(trav), Value::Class(send), Value::Class(node)] = args else {
            return Ok(Value::Bool(false));
        };
        let vm = vm_from_ptr()?;
        let seal = build_seal(vm, comm.clone(), trav.clone(), send.clone(), node.clone());
        let ok = seal.is_some();
        if !ok && debug_on() {
            eprintln!("commdrv seal failed: pre-patched Commissioner");
        }
        COMMDRV.with(|c| c.borrow_mut().seal = seal);
        Ok(Value::Bool(ok))
    });

    // __rubyrs_commdrv_start(commissioner, root_node) →
    //   Int handle    — native walk engaged
    //   false         — thread-local seal missing or from another VM /
    //                   snapshot generation; the hook may reseal + retry
    //   nil           — hard decline (interpreted walk)
    rt.register_fn("__rubyrs_commdrv_start", |args| {
        let [comm_v, root_v] = args else {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: "__rubyrs_commdrv_start(commissioner, root)".to_string(),
            }));
        };
        let vm = vm_from_ptr()?;
        let r = COMMDRV.with(|c| -> DRes<Value> {
            let mut c = c.borrow_mut();
            let c = &mut *c;
            let Some(seal) = c.seal.as_mut() else { return Ok(Value::Bool(false)) };
            let Value::Object(comm) = comm_v else { return decline("commissioner not an object") };
            let Value::Object(root) = root_v else { return decline("root not an object") };
            // Exact-class check (class_of returns the eigenclass when one
            // exists, so a per-instance patch also lands here). A ptr
            // mismatch is EITHER a Commissioner subclass (hook won't
            // reseal — its instance_of? gate fails) OR a stale seal from
            // another Runtime / a snapshot restore (hook reseals + retries).
            let ccls = vm.heap.class_of(*comm);
            if !Rc::ptr_eq(&ccls, &seal.commissioner_class) {
                return Ok(Value::Bool(false));
            }
            if !verify_seal(vm, seal) {
                return decline("sentinel drift (Commissioner patched)");
            }
            let tables = build_tables(vm, seal, *comm)?;
            let program = prepass(vm, seal, &tables, *root)?;
            c.next_nonce = c.next_nonce.wrapping_add(1).max(1);
            let nonce = c.next_nonce;
            let st = Box::new(WalkState {
                nonce,
                lists: tables.lists,
                restricted: tables.restricted,
                program,
                pc: 0,
                phase: 0,
                idx: 0,
                mgen: vm.method_gen,
                pending_cop: Value::Nil,
                pending_node: Value::Nil,
            });
            let slot = match c.free.pop() {
                Some(s) => s,
                None => {
                    c.slots.push(None);
                    c.slots.len() - 1
                }
            };
            c.slots[slot] = Some(st);
            Ok(Value::Int(handle_of(slot, nonce)))
        });
        match r {
            Ok(v) => Ok(v),
            Err(d) => {
                if debug_on() {
                    eprintln!("commdrv decline: {}", d.0);
                }
                Ok(Value::Nil)
            }
        }
    });

    // __rubyrs_commdrv_run(handle) → true (done) | raises (pause/abandon)
    rt.register_fn("__rubyrs_commdrv_run", |args| {
        let [Value::Int(h)] = args else {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: "__rubyrs_commdrv_run(handle)".to_string(),
            }));
        };
        let vm = vm_from_ptr()?;
        let (slot, nonce) = parse_handle(*h);
        // Take the state out of the slab for the duration — nested walks
        // (a cop investigating a sub-source) allocate their own slots.
        let mut st = COMMDRV.with(|c| {
            let mut c = c.borrow_mut();
            match c.slots.get_mut(slot) {
                Some(s @ Some(_)) if s.as_ref().unwrap().nonce == nonce => s.take(),
                _ => None,
            }
        });
        let Some(state) = st.as_mut() else {
            return Err(Trap::new(RubyError::RuntimeError {
                msg: "commdrv: stale walk handle".to_string(),
            }));
        };
        let result = run(vm, state);
        COMMDRV.with(|c| {
            let mut c = c.borrow_mut();
            if let Some(s) = c.slots.get_mut(slot) {
                *s = st;
            }
        });
        match result {
            Ok(true) => Ok(Value::Bool(true)),
            Ok(false) => unreachable!("run only completes or raises"),
            Err(t) => Err(t),
        }
    });

    // __rubyrs_commdrv_pending(handle) → [cop, node]
    rt.register_fn("__rubyrs_commdrv_pending", |args| {
        let [Value::Int(h)] = args else {
            return Err(Trap::new(RubyError::ArgumentError {
                msg: "__rubyrs_commdrv_pending(handle)".to_string(),
            }));
        };
        let vm = vm_from_ptr()?;
        let (slot, nonce) = parse_handle(*h);
        let pair = COMMDRV.with(|c| {
            let c = c.borrow();
            match c.slots.get(slot) {
                Some(Some(st)) if st.nonce == nonce => {
                    Some((st.pending_cop.clone(), st.pending_node.clone()))
                }
                _ => None,
            }
        });
        let Some((cop, node)) = pair else {
            return Err(Trap::new(RubyError::RuntimeError {
                msg: "commdrv: stale walk handle".to_string(),
            }));
        };
        // cop/node stay rooted via @callbacks / the AST tree; the fresh
        // array is returned straight onto the operand stack.
        vm.maybe_gc();
        let id = vm.heap.alloc(HeapObj::Array(vec![cop, node].into()));
        Ok(Value::Array(id))
    });

    // __rubyrs_commdrv_free(handle) → nil (stale handles ignored — the
    // hook's ensure may fire after an abandoning unwind already ran it).
    rt.register_fn("__rubyrs_commdrv_free", |args| {
        if let [Value::Int(h)] = args {
            let (slot, nonce) = parse_handle(*h);
            COMMDRV.with(|c| {
                let mut c = c.borrow_mut();
                if let Some(s) = c.slots.get_mut(slot)
                    && s.as_ref().is_some_and(|st| st.nonce == nonce)
                {
                    *s = None;
                    c.free.push(slot);
                }
            });
        }
        Ok(Value::Nil)
    });
}
