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
use crate::value::{Class, Method, Visibility};
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
    /// Count of class-instance ivars (payload deferred — a Value serde that
    /// preserves heap ObjIds lands in a later slice; recorded so a
    /// round-trip can assert no ivar-bearing class was silently dropped).
    pub(crate) ivar_count: u32,
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
    pub(crate) has_closure: bool,
    pub(crate) has_builtin: bool,
}

fn vis_to_u8(v: Visibility) -> u8 {
    match v {
        Visibility::Public => 0,
        Visibility::Protected => 1,
        Visibility::Private => 2,
    }
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
        has_closure: m.closure.is_some(),
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
        ivar_count: c.ivars.borrow().len() as u32,
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
    CapturedGraph { cache_counter: vm.cache_counter, classes, registry, const_classes, toplevel_methods }
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

fn build_method(mi: &MethodImage, defining: &Rc<Class>, id_to_class: &[Rc<Class>]) -> Rc<Method> {
    let defining_class = mi
        .defining_class
        .and_then(|id| id_to_class.get(id as usize))
        .map(Rc::downgrade)
        .or_else(|| Some(Rc::downgrade(defining)));
    Rc::new(Method {
        params: mi.params.clone(),
        proto_idx: mi.proto_idx as usize,
        fixed_arity: mi.fixed_arity.map(|(required, n_locals, stack_eligible)| {
            crate::value::FixedArity { required, n_locals, stack_eligible }
        }),
        defining_class,
        visibility: std::cell::Cell::new(u8_to_vis(mi.visibility)),
        closure: None,
        builtin: None,
        original_name: mi.original_name.map(crate::intern::SymId),
    })
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

    // 4. Wire edges + install methods for NEWLY-created classes only (reused
    //    builtins already carry correct state in the fresh VM).
    for i in 0..img.classes.len() {
        if !is_new[i] {
            continue;
        }
        let ci = &img.classes[i];
        let cls = &resolved[i];
        if let Some(sid) = ci.superclass {
            *cls.superclass.borrow_mut() = Some(resolved[sid as usize].clone());
        }
        *cls.includes.borrow_mut() = ci.includes.iter().map(|&id| resolved[id as usize].clone()).collect();
        *cls.prepends.borrow_mut() = ci.prepends.iter().map(|&id| resolved[id as usize].clone()).collect();
        for (name_sym, mi) in &ci.methods {
            let m = build_method(mi, cls, &resolved);
            cls.methods.borrow_mut().insert(SymId(*name_sym), m);
        }
        for (name_sym, mi) in &ci.singleton_methods {
            let m = build_method(mi, cls, &resolved);
            cls.singleton_methods.borrow_mut().insert(SymId(*name_sym), m);
        }
    }

    // 4b. Class-name constants so a bare `Foo` const-ref resolves.
    for &(name_sym, class_id) in &img.const_classes {
        vm.constants.insert(
            SymId(name_sym),
            crate::value::Value::Class(resolved[class_id as usize].clone()),
        );
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
}
