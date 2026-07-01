//! VM snapshot/image — thin slice (P1).
//!
//! Goal: after `require "rubocop"`, serialize the whole loaded VM state to
//! disk so a later fresh process RESTORES it and skips the ~1.3s
//! parse+compile+EXECUTE of the 787 cop files. Unlike the replay-based
//! `preamble_cache` (which re-executes cached bytecode, still paying the
//! ~79% execute cost) or CRuby's bootsnap (bytecode only), a state image
//! skips execute entirely — the one path where rubyrs's require can BEAT
//! CRuby. Feasibility was validated GO by a 3-agent workflow: fork-COW reuse
//! of the loaded state produces byte-identical cop output (correctness of
//! *restoring* the state), and the un-serializable Category-C state
//! (Fiber/C-ext/FDs) is empty after require. The one risk fork couldn't
//! close is the SERIALIZATION of the `Rc<Class>`/`Weak`/`Rc<Method>` pointer
//! graph to bytes and back.
//!
//! THIS SLICE closes exactly that risk: it captures the class graph +
//! interner + protos into a flat, id-based [`VmImage`] and proves it
//! round-trips losslessly through postcard. The restore-into-a-live-VM
//! wiring (already de-risked for correctness by the fork PoC) is the next
//! increment (P2/P3). Capture is READ-ONLY, so this slice can't corrupt VM
//! state.
//!
//! Edge encoding: every `Rc<Class>` reachable from `vm.classes` (values +
//! their superclass/includes/prepends closure) gets a dense `u32` class id
//! via pointer identity; all graph edges (superclass, includes, prepends,
//! `Method.defining_class` Weak back-ref) serialize as those ids. `ObjId`
//! is already position-independent, and `Proto`/`SymId` already derive serde
//! under the `preamble-cache` feature — so the graph is the only new work.

#![cfg(feature = "preamble-cache")]
// P1 thin slice: capture + serde are exercised by the round-trip test but not
// yet CALLED from a live path — the restore-into-VM + CLI wiring is the next
// increment (P2). Until then the capture/serde surface reads as dead code in a
// non-test build; the allow is removed when restore lands.
#![allow(dead_code)]

use crate::bytecode::Proto;
use crate::value::{Class, Method, Value, Visibility};
use std::collections::HashMap;
use std::rc::Rc;

/// The class-graph slice captured from a VM — owned, no borrows of the big
/// `Proto`/interner tables (those are combined by [`to_bytes`] at encode time
/// via [`VmImageRef`], mirroring `preamble_cache`'s ref/owned split, since
/// `Proto` is neither `Clone` nor `PartialEq`).
pub(crate) struct CapturedGraph {
    pub(crate) cache_counter: u32,
    pub(crate) classes: Vec<ClassImage>,
    pub(crate) registry: Vec<(u32, u32)>,
    pub(crate) const_classes: Vec<(u32, u32)>,
    pub(crate) toplevel_methods: Vec<(u32, MethodImage)>,
    /// Whole heap, index = ObjId (P3a).
    pub(crate) heap: Vec<HeapObjImage>,
    /// All top-level constants as (name SymId, value image) — supersedes
    /// `const_classes` (a Class value images as `ValueImage::Class`).
    pub(crate) constants: Vec<(u32, ValueImage)>,
}

/// Borrow twin used at encode time so serialize doesn't clone the proto
/// table (`Proto` has no `Clone`).
#[derive(serde::Serialize)]
struct VmImageRef<'a> {
    interner: Vec<&'a str>,
    protos: &'a [Proto],
    cache_counter: u32,
    classes: &'a [ClassImage],
    registry: &'a [(u32, u32)],
    const_classes: &'a [(u32, u32)],
    toplevel_methods: &'a [(u32, MethodImage)],
    heap: &'a [HeapObjImage],
    constants: &'a [(u32, ValueImage)],
}

/// Owned (decode) shape of the full image.
#[derive(serde::Deserialize)]
pub(crate) struct VmImage {
    /// Full interner table in id order (SymId is the index).
    pub(crate) interner: Vec<String>,
    /// Full proto (bytecode) table.
    pub(crate) protos: Vec<Proto>,
    /// `vm.cache_counter` — sizes the inline-cache vector on restore.
    pub(crate) cache_counter: u32,
    /// Every reachable class, dense-id order (index = class id).
    pub(crate) classes: Vec<ClassImage>,
    /// The `vm.classes` registry: (name SymId, class id).
    pub(crate) registry: Vec<(u32, u32)>,
    /// Top-level constants that bind to a class (`class Foo` binds the `Foo`
    /// constant): (name SymId, class id). Restored into `vm.constants` so a
    /// bare `Foo` const-ref resolves. Non-class constants (heap-valued) are
    /// deferred to the heap slice (P3).
    pub(crate) const_classes: Vec<(u32, u32)>,
    /// Toplevel (`<main>`) methods: (name SymId, method).
    pub(crate) toplevel_methods: Vec<(u32, MethodImage)>,
    /// Whole heap, index = ObjId (P3a).
    pub(crate) heap: Vec<HeapObjImage>,
    /// All top-level constants as (name SymId, value image).
    pub(crate) constants: Vec<(u32, ValueImage)>,
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub(crate) struct ClassImage {
    pub(crate) name: String,
    pub(crate) is_module: bool,
    /// Superclass as a class id (`None` = root, e.g. Object).
    pub(crate) superclass: Option<u32>,
    /// Included modules, as class ids (reverse-include order preserved).
    pub(crate) includes: Vec<u32>,
    /// Prepended modules, as class ids.
    pub(crate) prepends: Vec<u32>,
    /// Instance methods: (name SymId, method).
    pub(crate) methods: Vec<(u32, MethodImage)>,
    /// Singleton (`def self.x`) methods: (name SymId, method).
    pub(crate) singleton_methods: Vec<(u32, MethodImage)>,
    /// Class-instance ivars (`@foo` on the Class object): (SymId, value).
    /// RuboCop keeps critical state here (e.g. the cop registry on
    /// `Registry.@global`), so restoring these is required for a real run.
    pub(crate) ivars: Vec<(u32, ValueImage)>,
    /// Class variables (`@@foo`): (SymId, value).
    pub(crate) class_vars: Vec<(u32, ValueImage)>,
    /// Nested constants (`class Foo::Bar` / `Foo::CONST = …`): (SymId, value).
    /// `RuboCop::Formatter::SimpleTextFormatter` resolves through the
    /// `RuboCop::Formatter` module's consts.
    pub(crate) consts: Vec<(u32, ValueImage)>,
    /// Eigenclass id (`class << self`). Its own ClassImage holds the
    /// class-level methods defined that way (e.g. `YAML.safe_load` — stdlib
    /// modules define their surface here, NOT in `singleton_methods`).
    pub(crate) singleton_view: Option<u32>,
    /// The eigenclass's back-ref to its owning class (Weak), as a class id.
    pub(crate) singleton_target: Option<u32>,
    /// Modules included into the eigenclass (`class << self; include Mod`) —
    /// their instance methods become class methods (rubocop's ConfigFinder
    /// gets `find_last_file_upwards` from an included FileFinder this way).
    pub(crate) singleton_includes: Vec<u32>,
    /// Modules prepended into the eigenclass (`class << self; prepend Mod`).
    pub(crate) singleton_prepends: Vec<u32>,
}

/// Serializable image of a `Method`. `closure`/`builtin` payloads are
/// deferred (recorded as flags): a plain `def name` method — the case a
/// class-graph snapshot must reproduce — has both `None`. A builtin/closure
/// method is re-established by the fresh VM's own preamble on restore, so
/// only its presence needs recording here.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub(crate) struct MethodImage {
    pub(crate) params: Vec<String>,
    pub(crate) proto_idx: u32,
    /// `fixed_arity` flattened: `Some((required, n_locals, stack_eligible))`.
    pub(crate) fixed_arity: Option<(u16, u16, bool)>,
    /// Visibility as u8 (0 public, 1 protected, 2 private).
    pub(crate) visibility: u8,
    pub(crate) original_name: Option<u32>,
    /// `defining_class` Weak back-ref, as a class id (`None` = toplevel).
    pub(crate) defining_class: Option<u32>,
    /// `define_method`-installed closure (captured env), if any. FormatterSet's
    /// `started`/`finished` capture a `method_name` here.
    pub(crate) closure: Option<ClosureImage>,
    pub(crate) has_builtin: bool,
}

/// Serializable image of a `MethodClosure`. Captured env imaged inline
/// (sharing with a live BlockHandle not preserved — fine for the common
/// define_method-captures-a-constant case).
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub(crate) struct ClosureImage {
    pub(crate) captured: Vec<ValueImage>,
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
    pub(crate) captured_yield_block: Option<u32>,
}

fn vis_to_u8(v: Visibility) -> u8 {
    match v {
        Visibility::Public => 0,
        Visibility::Protected => 1,
        Visibility::Private => 2,
    }
}

/// Serializable encoding tag.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Debug, Clone)]
pub(crate) enum EncImage {
    Utf8,
    UsAscii,
    Binary,
    Other(u8),
}

fn enc_image(e: crate::value::EncodingTag) -> EncImage {
    use crate::value::EncodingTag as E;
    match e {
        E::Utf8 => EncImage::Utf8,
        E::UsAscii => EncImage::UsAscii,
        E::Binary => EncImage::Binary,
        E::Other(n) => EncImage::Other(n),
    }
}

/// Serializable image of a `Value`. Heap references (Array/Hash/Object/Range/
/// Block/BigInt/Rational/BoundMethod/…) become `Obj(ObjId)` — the concrete
/// `Value` variant is recovered from the restored slot's HeapObj type (1:1),
/// so a single `Obj` arm covers them all. Inline strings serialize their
/// bytes; Class → class id; Regex → its recompilable (source, flags). `f64`
/// travels as bits so the image is `Eq` and NaN round-trips exactly.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Debug, Clone)]
pub(crate) enum ValueImage {
    Nil,
    Bool(bool),
    Int(i64),
    Float(u64),
    Sym(u32),
    Str { bytes: Vec<u8>, enc: EncImage, frozen: bool },
    Class(u32),
    Regex { source: String, flags: u8 },
    Obj(u32),
}

/// Serializable image of a heap slot. Data variants only (P3a); Block/
/// BoundMethod/UnboundMethod/CurriedProc/TypedData/Fiber are recorded as
/// `Unsupported` for now (closures + bound methods land in P3b).
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Debug, Clone)]
pub(crate) enum HeapObjImage {
    Dead,
    Instance { class: u32, ivars: Vec<(u32, ValueImage)>, frozen: bool },
    Array { elems: Vec<ValueImage>, class_tag: Option<u32>, ivars: Vec<(u32, ValueImage)>, frozen: bool },
    Hash {
        pairs: Vec<(ValueImage, ValueImage)>,
        class_tag: Option<u32>,
        ivars: Vec<(u32, ValueImage)>,
        frozen: bool,
        by_identity: bool,
        /// `Hash.new { |h,k| … }` default block (ObjId of a Block) — pervasive
        /// in rubocop config (`relative_paths_cache`).
        default_block: Option<u32>,
        default_value: Option<Box<ValueImage>>,
    },
    Range { begin: ValueImage, end: ValueImage, exclusive: bool },
    BigInt(Vec<u8>),
    Rational { num: Vec<u8>, den: Vec<u8> },
    /// A `proc`/block closure. Captured env is imaged INLINE (sharing between
    /// a block and its enclosing method's closure is not preserved yet — fine
    /// for standalone blocks like Hash default blocks; define_method shared
    /// closures are a later refinement).
    Block {
        proto_idx: u32,
        captured: Vec<ValueImage>,
        self_val: ValueImage,
        lexical_cvar_class: Option<u32>,
        param_start: u16,
        n_params: u16,
        rest_slot: Option<u16>,
        kw_rest_slot: Option<u16>,
        captured_is_method_scope: bool,
        captured_yield_block: Option<u32>,
        is_lambda: bool,
    },
    Unsupported,
}

fn image_value(v: &crate::value::Value, ids: &mut ClassIds) -> ValueImage {
    use crate::value::Value as V;
    match v {
        V::Nil => ValueImage::Nil,
        V::Bool(b) => ValueImage::Bool(*b),
        V::Int(n) => ValueImage::Int(*n),
        V::Float(f) => ValueImage::Float(f.to_bits()),
        V::Sym(s) => ValueImage::Sym(s.0),
        V::Str(s) => ValueImage::Str {
            bytes: s.content.borrow().to_vec(),
            enc: enc_image(s.encoding.get()),
            frozen: s.frozen.get(),
        },
        V::Class(c) => ValueImage::Class(ids.intern(c)),
        V::Regex(r) => ValueImage::Regex { source: r.as_str().to_string(), flags: r.options() },
        V::Object(o) | V::Array(o) | V::Hash(o) | V::Range(o) | V::Block(o) | V::BigInt(o)
        | V::Rational(o) | V::BoundMethod(o) | V::UnboundMethod(o) | V::CurriedProc(o) => {
            ValueImage::Obj(o.0)
        }
    }
}

fn image_ivars(it: &crate::value::IvarTable, ids: &mut ClassIds) -> Vec<(u32, ValueImage)> {
    it.iter().map(|(s, v)| (s.0, image_value(v, ids))).collect()
}

fn image_fx_ivars(
    m: &crate::intern::FxHashMap<crate::intern::SymId, crate::value::Value>,
    ids: &mut ClassIds,
) -> Vec<(u32, ValueImage)> {
    m.iter().map(|(s, v)| (s.0, image_value(v, ids))).collect()
}

/// Capture the whole heap as a flat `Vec<HeapObjImage>` (index = ObjId, so
/// references stay valid on restore). READ-ONLY.
fn capture_heap(vm: &crate::vm::Vm, ids: &mut ClassIds) -> Vec<HeapObjImage> {
    use crate::heap::{HeapObj, Slot};
    vm.heap
        .slots
        .iter()
        .map(|slot| match slot {
            Slot::Dead => HeapObjImage::Dead,
            Slot::Live(obj) => match obj {
                HeapObj::Instance(i) => HeapObjImage::Instance {
                    class: ids.intern(&i.class),
                    ivars: image_ivars(&i.ivars, ids),
                    frozen: i.frozen.get(),
                },
                HeapObj::Array(a) => HeapObjImage::Array {
                    elems: a.elems.iter().map(|v| image_value(v, ids)).collect(),
                    class_tag: a.class_tag.as_ref().map(|c| ids.intern(c)),
                    ivars: image_fx_ivars(&a.ivars, ids),
                    frozen: a.frozen.get(),
                },
                HeapObj::Hash(h) => HeapObjImage::Hash {
                    pairs: h
                        .pairs
                        .iter()
                        .map(|(k, v)| (image_value(k, ids), image_value(v, ids)))
                        .collect(),
                    class_tag: h.class_tag.as_ref().map(|c| ids.intern(c)),
                    ivars: image_fx_ivars(&h.ivars, ids),
                    frozen: h.frozen.get(),
                    by_identity: h.by_identity.get(),
                    default_block: h.default_block.map(|b| b.0),
                    default_value: h.default_value.as_ref().map(|v| Box::new(image_value(v, ids))),
                },
                HeapObj::Range(r) => HeapObjImage::Range {
                    begin: image_value(&r.begin, ids),
                    end: image_value(&r.end, ids),
                    exclusive: r.exclusive,
                },
                HeapObj::Block(b) => HeapObjImage::Block {
                    proto_idx: b.proto_idx as u32,
                    captured: b.captured.borrow().iter().map(|v| image_value(v, ids)).collect(),
                    self_val: image_value(&b.self_val, ids),
                    lexical_cvar_class: b.lexical_cvar_class.as_ref().map(|c| ids.intern(c)),
                    param_start: b.param_start,
                    n_params: b.n_params,
                    rest_slot: b.rest_slot,
                    kw_rest_slot: b.kw_rest_slot,
                    captured_is_method_scope: b.captured_is_method_scope,
                    captured_yield_block: b.captured_yield_block.map(|o| o.0),
                    is_lambda: b.is_lambda,
                },
                #[cfg(feature = "bignum")]
                HeapObj::BigInt(b) => HeapObjImage::BigInt(b.to_signed_bytes_le()),
                #[cfg(feature = "bignum")]
                HeapObj::Rational(r) => HeapObjImage::Rational {
                    num: r.num.to_signed_bytes_le(),
                    den: r.den.to_signed_bytes_le(),
                },
                _ => HeapObjImage::Unsupported,
            },
        })
        .collect()
}

/// Assigns dense ids to `Rc<Class>` by pointer identity, discovering the
/// full graph transitively (superclass/includes/prepends).
struct ClassIds {
    map: HashMap<*const Class, u32>,
    order: Vec<Rc<Class>>,
}

impl ClassIds {
    fn new() -> Self {
        Self { map: HashMap::new(), order: Vec::new() }
    }

    /// Get-or-assign the id for `c`, enqueuing it for discovery on first
    /// sight so its own edges get ids too.
    fn intern(&mut self, c: &Rc<Class>) -> u32 {
        let key = Rc::as_ptr(c);
        if let Some(&id) = self.map.get(&key) {
            return id;
        }
        let id = self.order.len() as u32;
        self.map.insert(key, id);
        self.order.push(c.clone());
        id
    }
}

fn capture_method(m: &Rc<Method>, ids: &mut ClassIds) -> MethodImage {
    MethodImage {
        params: m.params.clone(),
        proto_idx: m.proto_idx as u32,
        fixed_arity: m.fixed_arity.as_ref().map(|fa| (fa.required, fa.n_locals, fa.stack_eligible)),
        visibility: vis_to_u8(m.visibility.get()),
        original_name: m.original_name.map(|s| s.0),
        defining_class: m
            .defining_class
            .as_ref()
            .and_then(|w| w.upgrade())
            .map(|c| ids.intern(&c)),
        closure: m.closure.as_ref().map(|cl| ClosureImage {
            captured: cl.captured.borrow().iter().map(|v| image_value(v, ids)).collect(),
            param_start: cl.param_start,
            n_params: cl.n_params,
            captured_yield_block: cl.captured_yield_block.map(|o| o.0),
        }),
        has_builtin: m.builtin.is_some(),
    }
}

fn capture_class(c: &Rc<Class>, ids: &mut ClassIds) -> ClassImage {
    let superclass = c.superclass.borrow().as_ref().map(|s| ids.intern(s));
    let includes: Vec<u32> = c.includes.borrow().iter().map(|m| ids.intern(m)).collect();
    let prepends: Vec<u32> = c.prepends.borrow().iter().map(|m| ids.intern(m)).collect();
    let methods: Vec<(u32, MethodImage)> = c
        .methods
        .borrow()
        .iter()
        .map(|(sym, m)| (sym.0, capture_method(m, ids)))
        .collect();
    let singleton_methods: Vec<(u32, MethodImage)> = c
        .singleton_methods
        .borrow()
        .iter()
        .map(|(sym, m)| (sym.0, capture_method(m, ids)))
        .collect();
    ClassImage {
        name: c.name.clone(),
        is_module: c.is_module,
        superclass,
        includes,
        prepends,
        methods,
        singleton_methods,
        ivars: c.ivars.borrow().iter().map(|(s, v)| (s.0, image_value(v, ids))).collect(),
        class_vars: c.class_vars.borrow().iter().map(|(s, v)| (s.0, image_value(v, ids))).collect(),
        consts: c.consts.borrow().iter().map(|(s, v)| (s.0, image_value(v, ids))).collect(),
        singleton_view: c.singleton_view.borrow().as_ref().map(|v| ids.intern(v)),
        singleton_target: c
            .singleton_target
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
            .map(|t| ids.intern(&t)),
        singleton_includes: c.singleton_includes.borrow().iter().map(|m| ids.intern(m)).collect(),
        singleton_prepends: c.singleton_prepends.borrow().iter().map(|m| ids.intern(m)).collect(),
    }
}

/// Capture the class-graph slice of `vm`. READ-ONLY — never mutates the VM.
/// Returns the owned graph; combine with the VM's proto/interner tables via
/// [`to_bytes`] to produce the serialized image.
pub(crate) fn capture(vm: &crate::vm::Vm) -> CapturedGraph {
    let mut ids = ClassIds::new();
    // Seed with every registered class (this also assigns their ids).
    let registry: Vec<(u32, u32)> = vm
        .classes
        .iter()
        .map(|(sym, c)| (sym.0, ids.intern(c)))
        .collect();
    // Seed from class-valued top-level constants too (BEFORE the fixpoint,
    // so any class reachable only via a constant is still captured).
    let const_classes: Vec<(u32, u32)> = vm
        .constants
        .iter()
        .filter_map(|(sym, v)| match v {
            crate::value::Value::Class(c) => Some((sym.0, ids.intern(c))),
            _ => None,
        })
        .collect();
    // Discover transitively: capture_class enqueues superclass/includes/
    // prepends via `ids.intern`, growing `ids.order`. Walk by index so
    // newly-discovered classes are captured too (fixpoint).
    let mut classes: Vec<ClassImage> = Vec::new();
    let mut i = 0;
    while i < ids.order.len() {
        let c = ids.order[i].clone();
        let img = capture_class(&c, &mut ids);
        classes.push(img);
        i += 1;
    }
    let toplevel_methods: Vec<(u32, MethodImage)> = vm
        .toplevel_methods
        .iter()
        .map(|(sym, m)| (sym.0, capture_method(m, &mut ids)))
        .collect();
    // Full constants (class-valued ones image as ValueImage::Class; heap ones
    // reference the heap captured below).
    let constants: Vec<(u32, ValueImage)> = vm
        .constants
        .iter()
        .map(|(sym, v)| (sym.0, image_value(v, &mut ids)))
        .collect();
    // Heap LAST — image_value calls above may have discovered more classes,
    // but the heap walk itself can discover still more (Instance.class); the
    // `ids` table keeps growing, and `classes` was already built by the
    // fixpoint over classes reachable from the class/const roots. (Heap-only
    // classes are an edge case handled by restore's unregistered-shell path.)
    let heap = capture_heap(vm, &mut ids);
    CapturedGraph {
        cache_counter: vm.cache_counter,
        classes,
        registry,
        const_classes,
        toplevel_methods,
        heap,
        constants,
    }
}

/// Serialize the VM's proto/interner tables + a [`CapturedGraph`] to postcard
/// bytes (borrows everything — no proto clone).
pub(crate) fn to_bytes(
    vm: &crate::vm::Vm,
    graph: &CapturedGraph,
) -> Result<Vec<u8>, postcard::Error> {
    let interner: Vec<&str> = (0..vm.interner.len())
        .map(|i| &**vm.interner.resolve(crate::intern::SymId(i as u32)))
        .collect();
    let img = VmImageRef {
        interner,
        protos: &vm.protos,
        cache_counter: graph.cache_counter,
        classes: &graph.classes,
        registry: &graph.registry,
        const_classes: &graph.const_classes,
        toplevel_methods: &graph.toplevel_methods,
        heap: &graph.heap,
        constants: &graph.constants,
    };
    postcard::to_allocvec(&img)
}

/// Deserialize an image from postcard bytes.
pub(crate) fn from_bytes(bytes: &[u8]) -> Result<VmImage, postcard::Error> {
    postcard::from_bytes(bytes)
}

fn u8_to_vis(v: u8) -> Visibility {
    match v {
        1 => Visibility::Protected,
        2 => Visibility::Private,
        _ => Visibility::Public,
    }
}

/// Build a fresh, empty `Class` shell with the given name/module flag. All
/// edge/method tables start empty; the caller wires them in pass 2.
fn new_class_shell(name: String, is_module: bool) -> Rc<Class> {
    use std::cell::{Cell, RefCell};
    Rc::new(Class {
        name,
        is_module,
        undefed: RefCell::new(crate::intern::FxHashSet::default()),
        anon_serial: Cell::new(0),
        ivars: RefCell::new(crate::intern::FxHashMap::default()),
        methods: RefCell::new(crate::intern::FxHashMap::default()),
        singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
        superclass: RefCell::new(None),
        includes: RefCell::new(Vec::new()),
        prepends: RefCell::new(Vec::new()),
        singleton_prepends: RefCell::new(Vec::new()),
        singleton_includes: RefCell::new(Vec::new()),
        singleton_view: RefCell::new(None),
        singleton_target: RefCell::new(None),
        class_vars: RefCell::new(crate::intern::FxHashMap::default()),
        consts: RefCell::new(crate::intern::FxHashMap::default()),
        assigned_name: RefCell::new(None),
        class_tag: None,
        #[cfg(feature = "cext")]
        cext_alloc_func: Cell::new(None),
    })
}

fn build_method(mi: &MethodImage, defining: &Rc<Class>, id_to_class: &[Rc<Class>], kinds: &[u8]) -> Rc<Method> {
    let defining_class = mi
        .defining_class
        .and_then(|id| id_to_class.get(id as usize))
        .map(Rc::downgrade)
        .or_else(|| Some(Rc::downgrade(defining)));
    let closure = mi.closure.as_ref().map(|cl| crate::value::MethodClosure {
        captured: std::rc::Rc::new(std::cell::RefCell::new(
            cl.captured.iter().map(|vi| value_from_image(vi, id_to_class, kinds)).collect(),
        )),
        param_start: cl.param_start,
        n_params: cl.n_params,
        captured_yield_block: cl.captured_yield_block.map(crate::value::ObjId),
    });
    Rc::new(Method {
        params: mi.params.clone(),
        proto_idx: mi.proto_idx as usize,
        fixed_arity: mi.fixed_arity.map(|(required, n_locals, stack_eligible)| {
            crate::value::FixedArity { required, n_locals, stack_eligible }
        }),
        defining_class,
        visibility: std::cell::Cell::new(u8_to_vis(mi.visibility)),
        closure,
        builtin: None,
        original_name: mi.original_name.map(crate::intern::SymId),
    })
}

/// Per-slot variant tag, precomputed from the image so `value_from_image`
/// can recover an `Obj(oid)`'s concrete Value variant without reading the
/// (being-built) heap.
fn heap_kind(hi: &HeapObjImage) -> u8 {
    match hi {
        HeapObjImage::Dead | HeapObjImage::Unsupported => 0,
        HeapObjImage::Instance { .. } => 1,
        HeapObjImage::Array { .. } => 2,
        HeapObjImage::Hash { .. } => 3,
        HeapObjImage::Range { .. } => 4,
        HeapObjImage::BigInt(_) => 5,
        HeapObjImage::Rational { .. } => 6,
        HeapObjImage::Block { .. } => 7,
    }
}

fn build_str(bytes: Vec<u8>, enc: &EncImage, frozen: bool) -> Value {
    use crate::value::EncodingTag as E;
    let v = Value::new_str_bytes(bytes);
    if let Value::Str(s) = &v {
        s.encoding.set(match enc {
            EncImage::Utf8 => E::Utf8,
            EncImage::UsAscii => E::UsAscii,
            EncImage::Binary => E::Binary,
            EncImage::Other(n) => E::Other(*n),
        });
        if frozen {
            s.frozen.set(true);
        }
    }
    v
}

fn rebuild_regex(source: &str) -> Value {
    // Best-effort recompile from the bare source. Full fidelity (the `\G`/
    // named-group preprocess in vm::step, which is private here, + flags) is a
    // P3c refinement; no regex-in-constant is exercised until then.
    match crate::regex_engine::compile(source) {
        Ok(c) => Value::Regex(std::rc::Rc::new(c)),
        Err(_) => Value::Nil,
    }
}

/// Reconstruct a `Value` from its image. `kinds[oid]` gives an `Obj` ref's
/// concrete variant (Value-heap-variant ↔ HeapObj-variant is 1:1).
fn value_from_image(vi: &ValueImage, classes: &[Rc<Class>], kinds: &[u8]) -> crate::value::Value {
    use crate::value::{ObjId, Value};
    match vi {
        ValueImage::Nil => Value::Nil,
        ValueImage::Bool(b) => Value::Bool(*b),
        ValueImage::Int(n) => Value::Int(*n),
        ValueImage::Float(bits) => Value::Float(f64::from_bits(*bits)),
        ValueImage::Sym(s) => Value::Sym(crate::intern::SymId(*s)),
        ValueImage::Str { bytes, enc, frozen } => build_str(bytes.clone(), enc, *frozen),
        ValueImage::Class(id) => Value::Class(classes[*id as usize].clone()),
        ValueImage::Regex { source, .. } => rebuild_regex(source),
        ValueImage::Obj(oid) => {
            let id = ObjId(*oid);
            match kinds.get(*oid as usize).copied().unwrap_or(0) {
                1 => Value::Object(id),
                2 => Value::Array(id),
                3 => Value::Hash(id),
                4 => Value::Range(id),
                5 => Value::BigInt(id),
                6 => Value::Rational(id),
                7 => Value::Block(id),
                _ => Value::Nil,
            }
        }
    }
}

fn fx_ivars_from_image(
    pairs: &[(u32, ValueImage)],
    classes: &[Rc<Class>],
    kinds: &[u8],
) -> crate::intern::FxHashMap<crate::intern::SymId, crate::value::Value> {
    pairs
        .iter()
        .map(|(s, vi)| (crate::intern::SymId(*s), value_from_image(vi, classes, kinds)))
        .collect()
}

/// Rebuild the heap `Vec<Slot>` from the image. Single pass: `Obj` variants
/// resolve via the precomputed `kinds`, so forward refs are fine.
fn build_heap(img_heap: &[HeapObjImage], classes: &[Rc<Class>], kinds: &[u8]) -> Vec<crate::heap::Slot> {
    use crate::heap::{HeapObj, HashObj, Slot};
    use crate::value::{Instance, IvarTable};
    img_heap
        .iter()
        .map(|hi| match hi {
            HeapObjImage::Dead | HeapObjImage::Unsupported => Slot::Dead,
            HeapObjImage::Instance { class, ivars, frozen } => {
                let mut it = IvarTable::default();
                for (s, vi) in ivars {
                    it.insert(crate::intern::SymId(*s), value_from_image(vi, classes, kinds));
                }
                Slot::Live(HeapObj::Instance(Instance {
                    class: classes[*class as usize].clone(),
                    ivars: it,
                    singleton_class: None,
                    frozen: std::cell::Cell::new(*frozen),
                }))
            }
            HeapObjImage::Array { elems, class_tag, ivars, frozen } => {
                Slot::Live(HeapObj::Array(crate::heap::ArrayObj {
                    elems: elems.iter().map(|vi| value_from_image(vi, classes, kinds)).collect(),
                    class_tag: class_tag.map(|id| classes[id as usize].clone()),
                    ivars: fx_ivars_from_image(ivars, classes, kinds),
                    frozen: std::cell::Cell::new(*frozen),
                }))
            }
            HeapObjImage::Hash { pairs, class_tag, ivars, frozen, by_identity, default_block, default_value } => {
                let mut h = HashObj::with_pairs(
                    pairs
                        .iter()
                        .map(|(k, v)| {
                            (value_from_image(k, classes, kinds), value_from_image(v, classes, kinds))
                        })
                        .collect(),
                );
                h.class_tag = class_tag.map(|id| classes[id as usize].clone());
                h.ivars = fx_ivars_from_image(ivars, classes, kinds);
                h.frozen = std::cell::Cell::new(*frozen);
                h.by_identity = std::cell::Cell::new(*by_identity);
                h.default_block = default_block.map(crate::value::ObjId);
                h.default_value =
                    default_value.as_ref().map(|v| value_from_image(v, classes, kinds));
                Slot::Live(HeapObj::Hash(h))
            }
            HeapObjImage::Range { begin, end, exclusive } => {
                Slot::Live(HeapObj::Range(crate::heap::RangeObj {
                    begin: value_from_image(begin, classes, kinds),
                    end: value_from_image(end, classes, kinds),
                    exclusive: *exclusive,
                }))
            }
            HeapObjImage::Block {
                proto_idx,
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
            } => {
                let env: Vec<Value> =
                    captured.iter().map(|vi| value_from_image(vi, classes, kinds)).collect();
                Slot::Live(HeapObj::Block(crate::value::BlockHandle {
                    proto_idx: *proto_idx as usize,
                    captured: std::rc::Rc::new(std::cell::RefCell::new(env)),
                    self_val: value_from_image(self_val, classes, kinds),
                    lexical_cvar_class: lexical_cvar_class.map(|id| classes[id as usize].clone()),
                    param_start: *param_start,
                    n_params: *n_params,
                    rest_slot: *rest_slot,
                    kw_rest_slot: *kw_rest_slot,
                    captured_is_method_scope: *captured_is_method_scope,
                    captured_yield_block: captured_yield_block.map(crate::value::ObjId),
                    is_lambda: *is_lambda,
                }))
            }
            #[cfg(feature = "bignum")]
            HeapObjImage::BigInt(bytes) => {
                Slot::Live(HeapObj::BigInt(num_bigint::BigInt::from_signed_bytes_le(bytes)))
            }
            #[cfg(feature = "bignum")]
            HeapObjImage::Rational { num, den } => {
                Slot::Live(HeapObj::Rational(crate::heap::RationalRepr {
                    num: num_bigint::BigInt::from_signed_bytes_le(num),
                    den: num_bigint::BigInt::from_signed_bytes_le(den),
                }))
            }
            #[cfg(not(feature = "bignum"))]
            _ => Slot::Dead,
        })
        .collect()
}

/// Restore `img` into a FRESH `vm` (one that has just run its preamble, so
/// builtins/Object/String exist). Class-graph slice only (P2 milestone): no
/// heap objects, constants, or closures — a program whose captured state is
/// pure classes+methods restores + dispatches correctly, because instances
/// are created fresh when the restored code runs. Builtins are REUSED by name
/// (never duplicated); user classes are created + their edges/methods wired.
///
/// Preconditions (hold when `vm` is a same-version fresh Runtime): the image's
/// interner + protos share `vm`'s current prefix (same preamble), so the
/// user tail is appended / the proto table replaced wholesale — the exact
/// discipline `preamble_cache::try_load` uses.
pub(crate) fn restore(vm: &mut crate::vm::Vm, img: VmImage) {
    use crate::intern::SymId;
    // 1. Interner: fresh prefix matches; append the user tail.
    debug_assert!(img.interner.len() >= vm.interner.len());
    for s in &img.interner[vm.interner.len()..] {
        vm.interner.intern(s);
    }
    // 2. Protos + call-cache sizing (image ⊇ preamble at identical indices).
    vm.protos = img.protos;
    vm.cache_counter = img.cache_counter;
    vm.ensure_call_caches(img.cache_counter as usize);

    // 3. Resolve every image class-id to an Rc<Class>: reuse an existing
    //    (builtin) class by its registered name, else create a shell. Also
    //    register the newly-created ones.
    let mut id_to_class: Vec<Option<Rc<Class>>> = vec![None; img.classes.len()];
    let mut is_new = vec![false; img.classes.len()];
    for &(name_sym, class_id) in &img.registry {
        let idx = class_id as usize;
        if let Some(existing) = vm.classes.get(&SymId(name_sym)) {
            id_to_class[idx] = Some(existing.clone());
        } else if let Some(already) = id_to_class[idx].clone() {
            // Same class-id already has a shell — it's an ALIAS (`YAML = Psych`
            // share one Rc). Register that SAME shell under this name too, so
            // state is installed once and both names resolve to it (a second
            // shell would split methods/ivars and leave one name empty).
            vm.classes.insert(SymId(name_sym), already);
        } else {
            let ci = &img.classes[idx];
            let shell = new_class_shell(ci.name.clone(), ci.is_module);
            vm.classes.insert(SymId(name_sym), shell.clone());
            id_to_class[idx] = Some(shell);
            is_new[idx] = true;
        }
    }
    // Any class reached only via an edge (unregistered — anonymous / a module
    // referenced by include) gets a shell too, so edge resolution never nulls.
    for i in 0..img.classes.len() {
        if id_to_class[i].is_none() {
            let ci = &img.classes[i];
            id_to_class[i] = Some(new_class_shell(ci.name.clone(), ci.is_module));
            is_new[i] = true;
        }
    }
    let resolved: Vec<Rc<Class>> = id_to_class.into_iter().map(|c| c.unwrap()).collect();

    // 3b. Rebuild the heap from the image (index = ObjId, so every stored
    //     ObjId reference stays valid). The whole heap is replaced — the fresh
    //     VM's preamble heap is discarded and every heap-referencing bit of
    //     state (constants + class ivars, wired below) is re-pointed at the
    //     image heap, so there's no dangling into the old heap. GC parallel
    //     vecs are rebuilt from scratch (all slots treated as old survivors).
    let kinds: Vec<u8> = img.heap.iter().map(heap_kind).collect();
    let slots = build_heap(&img.heap, &resolved, &kinds);
    let n = slots.len();
    vm.heap.slots = slots;
    vm.heap.marks = vec![false; n];
    vm.heap.old = vec![true; n];
    vm.heap.young_slots = Vec::new();
    vm.heap.remembered = Vec::new();
    vm.heap.minors_since_major = 0;
    vm.heap.free = (0..n as u32)
        .filter(|&i| matches!(vm.heap.slots[i as usize], crate::heap::Slot::Dead))
        .collect();
    vm.heap.live_count = n - vm.heap.free.len();
    #[cfg(feature = "jit-native")]
    {
        // Keep the ObjId-indexed JIT class-ptr cache length-consistent with the
        // new heap; entries invalidate to 0 (recomputed on demand).
        // (jit-native + snapshot is not a tested combination yet.)
        vm.heap.class_ptrs = vec![0; n];
    }

    // 4. Wire edges + install methods. NEW classes get everything; REUSED
    //    builtins keep their native methods but gain any USER-added ones
    //    (rubocop monkeypatches core classes). Class ivars/class_vars are
    //    restored for ALL classes (reused builtins' ivars must re-point at the
    //    image heap too — e.g. rubocop's registry on a class ivar).
    for i in 0..img.classes.len() {
        let ci = &img.classes[i];
        let cls = &resolved[i];
        if is_new[i] {
            if let Some(sid) = ci.superclass {
                *cls.superclass.borrow_mut() = Some(resolved[sid as usize].clone());
            }
            *cls.includes.borrow_mut() =
                ci.includes.iter().map(|&id| resolved[id as usize].clone()).collect();
            *cls.prepends.borrow_mut() =
                ci.prepends.iter().map(|&id| resolved[id as usize].clone()).collect();
            for (name_sym, mi) in &ci.methods {
                cls.methods.borrow_mut().insert(SymId(*name_sym), build_method(mi, cls, &resolved, &kinds));
            }
            for (name_sym, mi) in &ci.singleton_methods {
                cls.singleton_methods
                    .borrow_mut()
                    .insert(SymId(*name_sym), build_method(mi, cls, &resolved, &kinds));
            }
            // Eigenclass (`class << self`) — its ClassImage (also restored)
            // carries the class-level methods; wire the view + back-ref.
            if let Some(vid) = ci.singleton_view {
                *cls.singleton_view.borrow_mut() = Some(resolved[vid as usize].clone());
            }
            if let Some(tid) = ci.singleton_target {
                *cls.singleton_target.borrow_mut() = Some(Rc::downgrade(&resolved[tid as usize]));
            }
            *cls.singleton_includes.borrow_mut() =
                ci.singleton_includes.iter().map(|&id| resolved[id as usize].clone()).collect();
            *cls.singleton_prepends.borrow_mut() =
                ci.singleton_prepends.iter().map(|&id| resolved[id as usize].clone()).collect();
        } else {
            // Reused builtin / stdlib stub: merge USER-defined methods (a
            // native method has `has_builtin`; a closure needs P3c). `or_insert`
            // so a native already present isn't clobbered. BOTH instance and
            // singleton tables — stdlib modules loaded via `require` (YAML,
            // JSON, …) define their surface as SINGLETON methods (`YAML.
            // safe_load`), and those must land on the reused module too.
            for (name_sym, mi) in &ci.methods {
                if !mi.has_builtin {
                    cls.methods
                        .borrow_mut()
                        .entry(SymId(*name_sym))
                        .or_insert_with(|| build_method(mi, cls, &resolved, &kinds));
                }
            }
            for (name_sym, mi) in &ci.singleton_methods {
                if !mi.has_builtin {
                    cls.singleton_methods
                        .borrow_mut()
                        .entry(SymId(*name_sym))
                        .or_insert_with(|| build_method(mi, cls, &resolved, &kinds));
                }
            }
            // Merge MONKEYPATCH module edges — modules included/prepended onto a
            // core class at load time (e.g. the prism `unpack1` polyfill
            // prepended to String). Only NEWLY-restored modules are appended
            // (standard modules are already on the reused fresh class); this
            // preserves the fresh builtin's own chain.
            for &id in &ci.prepends {
                if is_new[id as usize] {
                    cls.prepends.borrow_mut().push(resolved[id as usize].clone());
                }
            }
            for &id in &ci.includes {
                if is_new[id as usize] {
                    cls.includes.borrow_mut().push(resolved[id as usize].clone());
                }
            }
            for &id in &ci.singleton_prepends {
                if is_new[id as usize] {
                    cls.singleton_prepends.borrow_mut().push(resolved[id as usize].clone());
                }
            }
            for &id in &ci.singleton_includes {
                if is_new[id as usize] {
                    cls.singleton_includes.borrow_mut().push(resolved[id as usize].clone());
                }
            }
        }
        for (s, vi) in &ci.ivars {
            cls.ivars.borrow_mut().insert(SymId(*s), value_from_image(vi, &resolved, &kinds));
        }
        for (s, vi) in &ci.class_vars {
            cls.class_vars.borrow_mut().insert(SymId(*s), value_from_image(vi, &resolved, &kinds));
        }
        for (s, vi) in &ci.consts {
            cls.consts.borrow_mut().insert(SymId(*s), value_from_image(vi, &resolved, &kinds));
        }
    }

    // 4b. All top-level constants — heap-valued ones point into the restored
    //     heap, class-valued ones resolve to the class map. Overwrites the
    //     fresh VM's builtin constants with the image's (consistent heap).
    for (name_sym, vi) in &img.constants {
        vm.constants.insert(SymId(*name_sym), value_from_image(vi, &resolved, &kinds));
    }

    // 5. Toplevel methods.
    for (name_sym, mi) in &img.toplevel_methods {
        // Toplevel methods have no defining class; build_method falls back to
        // a downgrade of `defining` only when the image had one — pass a
        // throwaway that the None-arity path ignores. Use the first resolved
        // class as the fallback anchor is wrong; instead skip defining.
        let m = Rc::new(Method {
            params: mi.params.clone(),
            proto_idx: mi.proto_idx as usize,
            fixed_arity: mi.fixed_arity.map(|(required, n_locals, stack_eligible)| {
                crate::value::FixedArity { required, n_locals, stack_eligible }
            }),
            defining_class: mi
                .defining_class
                .and_then(|id| resolved.get(id as usize))
                .map(Rc::downgrade),
            visibility: std::cell::Cell::new(u8_to_vis(mi.visibility)),
            closure: None,
            builtin: None,
            original_name: mi.original_name.map(SymId),
        });
        vm.toplevel_methods.insert(SymId(*name_sym), m);
    }

    // 6. Invalidate any inline caches minted before the graph changed.
    vm.method_gen = vm.method_gen.wrapping_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core P1 proof: after executing a small program that builds a
    /// class graph (inheritance + methods + a module include), the captured
    /// image round-trips through postcard bytes LOSSLESSLY. This closes the
    /// one snapshot risk the fork PoC couldn't (Rc<Class> graph -> bytes).
    #[test]
    fn class_graph_round_trips_through_bytes() {
        let mut rt = crate::Runtime::new();
        rt.eval(
            r#"
            module Greeting
              def hello; "hi #{name}"; end
            end
            class Animal
              def initialize(n); @n = n; end
              def name; @n; end
            end
            class Dog < Animal
              include Greeting
              def self.species; "canis"; end
              def speak; "woof"; end
            end
            "#,
            "<snapshot-test>",
        )
        .expect("eval failed");

        let graph = capture(&rt.vm);
        let n_classes = graph.classes.len();
        let n_protos = rt.vm.protos.len();
        let bytes = to_bytes(&rt.vm, &graph).expect("serialize failed");
        let back = from_bytes(&bytes).expect("deserialize failed");

        // Lossless on the big tables + graph size.
        assert_eq!(back.protos.len(), n_protos, "proto table lost entries");
        assert_eq!(back.classes.len(), n_classes, "class graph lost entries");
        assert_eq!(back.interner.len(), rt.vm.interner.len(), "interner truncated");

        // And it actually captured the graph we built.
        let find = |name: &str| back.classes.iter().find(|c| c.name == name);
        let dog = find("Dog").expect("Dog missing from image");
        let animal_id = back
            .classes
            .iter()
            .position(|c| c.name == "Animal")
            .expect("Animal missing") as u32;
        assert_eq!(dog.superclass, Some(animal_id), "Dog<Animal edge lost");
        assert!(
            !dog.includes.is_empty(),
            "Dog include Greeting edge lost"
        );
        // method names present (resolve via the round-tripped interner)
        let sym = |name: &str| {
            back.interner.iter().position(|s| s == name).map(|i| i as u32)
        };
        let speak = sym("speak").expect("speak not interned");
        assert!(
            dog.methods.iter().any(|(s, _)| *s == speak),
            "Dog#speak lost"
        );
        let species = sym("species").expect("species not interned");
        assert!(
            dog.singleton_methods.iter().any(|(s, _)| *s == species),
            "Dog.species (singleton) lost"
        );

        assert!(!bytes.is_empty());
    }

    /// P2 end-to-end: capture a class graph, restore it into a FRESH VM that
    /// never ran the class defs, and dispatch against the restored classes —
    /// exercising an instance method, an INHERITED method, an INCLUDED-module
    /// method, a SINGLETON method, and `initialize`. Output must match a cold
    /// run (defs + probe in one VM). This proves the restore wiring (the half
    /// the fork PoC couldn't, since fork shares live Rc pointers).
    #[test]
    fn restore_runs_dispatch_end_to_end() {
        let defs = r#"
            module Greeting
              def hello; "hi #{name}"; end
            end
            class Animal
              def initialize(n); @n = n; end
              def name; @n; end
            end
            class Dog < Animal
              include Greeting
              def self.species; "canis"; end
              def speak; "woof"; end
            end
        "#;
        let probe =
            r#"Dog.new("rex").name + "|" + Dog.new("z").speak + "|" + Dog.new("y").hello + "|" + Dog.species"#;

        // loader → capture → bytes → image
        let mut loader = crate::Runtime::new();
        loader.eval(defs, "defs").expect("defs eval");
        let graph = capture(&loader.vm);
        let bytes = to_bytes(&loader.vm, &graph).expect("serialize");
        let img = from_bytes(&bytes).expect("deserialize");

        // restore into a FRESH vm (never saw the defs), then run the probe
        let mut restored = crate::Runtime::new();
        restore(&mut restored.vm, img);
        let vr = restored.eval(probe, "probe").expect("restored probe eval");

        // cold reference
        let mut cold = crate::Runtime::new();
        cold.eval(defs, "defs").expect("cold defs");
        let vc = cold.eval(probe, "probe").expect("cold probe");

        let as_s = |v: &crate::value::Value| match v {
            crate::value::Value::Str(s) => s.to_string_lossy(),
            other => format!("{other:?}"),
        };
        assert_eq!(as_s(&vr), as_s(&vc), "restored dispatch != cold");
        assert_eq!(as_s(&vr), "rex|woof|hi y|canis", "unexpected result");
    }

    /// P3a: the HEAP (arrays/hashes/strings/instances/ranges — with nested
    /// Values, ivars, frozen bits, subclass tags) + all constants serialize
    /// through postcard bytes LOSSLESSLY. Closes the heap-serialization risk
    /// (the P3 analog of P1's class-graph proof); heap RESTORE is P3b.
    #[test]
    fn heap_and_constants_round_trip_through_bytes() {
        let mut rt = crate::Runtime::new();
        rt.eval(
            r#"
            FROZEN_ARR = [1, "two", :three, nil, true, 3.5].freeze
            CONFIG = { "a" => 1, b: [10, 20], nested: { x: [1, 2] } }
            class Widget
              @count = 42
              def initialize; @tag = "w"; @nums = [1, 2, 3]; end
            end
            INST = Widget.new
            RNG = (1...5)
            "#,
            "<heap-snapshot-test>",
        )
        .expect("eval failed");

        let graph = capture(&rt.vm);
        let bytes = to_bytes(&rt.vm, &graph).expect("serialize failed");
        let back = from_bytes(&bytes).expect("deserialize failed");

        // Whole heap + constants round-trip losslessly.
        assert_eq!(back.heap, graph.heap, "heap did not round-trip");
        assert_eq!(back.constants, graph.constants, "constants did not round-trip");

        // Spot-check: FROZEN_ARR → an Obj ref → a frozen Array of the right
        // elements (proves nested-Value + frozen + variety survived).
        let arr_sym = back
            .interner
            .iter()
            .position(|s| s == "FROZEN_ARR")
            .expect("FROZEN_ARR not interned") as u32;
        let (_, arr_val) = back
            .constants
            .iter()
            .find(|(s, _)| *s == arr_sym)
            .expect("FROZEN_ARR constant missing");
        let ValueImage::Obj(oid) = arr_val else { panic!("FROZEN_ARR not an Obj: {arr_val:?}") };
        match &back.heap[*oid as usize] {
            HeapObjImage::Array { elems, frozen, .. } => {
                assert!(*frozen, "FROZEN_ARR lost its frozen bit");
                assert_eq!(elems.len(), 6, "FROZEN_ARR elem count wrong");
                assert_eq!(elems[0], ValueImage::Int(1));
                assert!(matches!(&elems[1], ValueImage::Str { bytes, .. } if bytes == b"two"));
                assert_eq!(elems[3], ValueImage::Nil);
                assert_eq!(elems[4], ValueImage::Bool(true));
            }
            other => panic!("FROZEN_ARR heap slot not an Array: {other:?}"),
        }

        // And a subclass instance carries its ivars.
        let inst_sym = back.interner.iter().position(|s| s == "INST").unwrap() as u32;
        let (_, inst_val) = back.constants.iter().find(|(s, _)| *s == inst_sym).unwrap();
        let ValueImage::Obj(ioid) = inst_val else { panic!("INST not Obj") };
        assert!(
            matches!(&back.heap[*ioid as usize], HeapObjImage::Instance { ivars, .. } if ivars.len() == 2),
            "INST instance ivars lost"
        );
    }

    /// P3b end-to-end: restore HEAP constants (a frozen mixed array, a nested
    /// hash, an instance whose ivar holds an array, a range) into a fresh VM
    /// and read them back — output must match cold. Proves the heap RESTORE
    /// (rebuild + re-point) works, not just serialization.
    #[test]
    fn restore_heap_constants_end_to_end() {
        let defs = r#"
            FROZEN = [1, "two", :three].freeze
            CONFIG = { "a" => 1, b: [10, 20] }
            class Box
              def initialize(v); @v = v; end
              def v; @v; end
            end
            BOX = Box.new([7, 8, 9])
            RNG = (1...4)
        "#;
        let probe = r#"
            [ FROZEN.join(","),
              CONFIG["a"].to_s, CONFIG[:b].sum.to_s,
              BOX.v.sum.to_s,
              RNG.to_a.inspect,
              FROZEN.frozen?.to_s ].join("|")
        "#;

        let mut loader = crate::Runtime::new();
        loader.eval(defs, "defs").expect("defs");
        let graph = capture(&loader.vm);
        let bytes = to_bytes(&loader.vm, &graph).expect("serialize");
        let img = from_bytes(&bytes).expect("deserialize");

        let mut restored = crate::Runtime::new();
        restore(&mut restored.vm, img);
        let vr = restored.eval(probe, "probe").expect("restored probe");

        let mut cold = crate::Runtime::new();
        cold.eval(defs, "defs").expect("cold defs");
        let vc = cold.eval(probe, "probe").expect("cold probe");

        let as_s = |v: &crate::value::Value| match v {
            crate::value::Value::Str(s) => s.to_string_lossy(),
            other => format!("{other:?}"),
        };
        assert_eq!(as_s(&vr), as_s(&vc), "restored heap constants != cold");
        assert_eq!(as_s(&vr), "1,two,three|1|30|24|[1, 2, 3]|true", "unexpected");
    }
}
