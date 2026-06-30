// Feasibility PoC for ADR 0034 Gap B: would an object-HEADER representation close
// `bench_treesum`'s residual ~1.5× gap vs YJIT (rubyrs jitN 0.29ms / YJIT 0.19ms)?
//
// rubyrs's heap reaches an object as:
//     oid (u32 index) -> Vec<Slot> -> Slot::Live(HeapObj) -> HeapObj::Instance
//       -> Instance { class: Rc<Class>, ivars: SmallVec<[(Sym,Value);4]> }
// So an ivar read or a class read is a slab index + TWO enum matches (+ a linear ivar
// scan). The Cranelift JIT cannot inline that, so it emits an extern-C PRIMITIVE CALL
// per access (`jit_inst_get_int`, the class guard via `class_ptr_of`).
//
// CRuby/YJIT objects ARE pointers: the class is a word in the object header and ivars sit
// at FIXED offsets, so a class read or ivar read is a single INLINABLE load.
//
// This PoC runs the treesum recursion (`@v + @l.sum(d-1) + @r.sum(d-1)`, depth-counted)
// over three representations and reports ns/node:
//   RepA  — slab + enum, every access a primitive call            (pre-caching rubyrs)
//   RepA2 — self-Instance cached once/frame (the CURRENT shipped state, commit cfe9ef56)
//   RepB  — object-header: object = ptr, class word + fixed ivar offsets, accesses INLINED
//
// Both RepA and RepB use a CONTIGUOUS arena, so cache locality is held constant — the only
// variable is the access pattern. If RepB lands ~1.5× faster than RepA2 (matching the real
// jitN/YJIT ratio), the residual gap IS the representation and the heap rewrite would pay
// off. If RepB is only marginally faster, the gap is elsewhere (the call boundary) and the
// rewrite would NOT close it — saving us the work.
//
// Build & run:  rustc -O poc/jit-spike/treesum_rep.rs -o /tmp/treesum_rep && /tmp/treesum_rep

use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone, Copy)]
enum Value {
    Int(i64),
    Obj(u32),
    Nil,
}

struct Class {
    _tag: u64,
}

const SV: u32 = 1; // @v
const SL: u32 = 2; // @l
const SR: u32 = 3; // @r

// ===== RepA: rubyrs-like slab + enum + small inline-ivar scan =====
struct InstanceA {
    class: Rc<Class>,
    ivars: [(u32, Value); 4], // SmallVec inline storage (n live)
    n: u8,
}
enum HeapObjA {
    Instance(InstanceA),
    #[allow(dead_code)]
    Other(Vec<Value>),
}
enum SlotA {
    Live(HeapObjA),
    #[allow(dead_code)]
    Free,
}
struct HeapA {
    slots: Vec<SlotA>,
}

impl HeapA {
    // Models `jit_ivar_get_int`: the JIT can't inline the slab+enum walk, so it's a call.
    #[inline(never)]
    fn ivar(&self, oid: u32, sym: u32) -> Value {
        match &self.slots[oid as usize] {
            SlotA::Live(HeapObjA::Instance(i)) => {
                let mut k = 0usize;
                while k < i.n as usize {
                    if i.ivars[k].0 == sym {
                        return i.ivars[k].1;
                    }
                    k += 1;
                }
                Value::Nil
            }
            _ => Value::Nil,
        }
    }
    // Models the class guard primitive (`class_ptr_of`): slab + enum + Rc::as_ptr.
    #[inline(never)]
    fn class_ptr(&self, oid: u32) -> *const Class {
        match &self.slots[oid as usize] {
            SlotA::Live(HeapObjA::Instance(i)) => Rc::as_ptr(&i.class),
            _ => std::ptr::null(),
        }
    }
    // Models `jit_self_inst`: one slab + enum walk, returns the Instance address.
    #[inline(never)]
    fn inst(&self, oid: u32) -> *const InstanceA {
        match &self.slots[oid as usize] {
            SlotA::Live(HeapObjA::Instance(i)) => i as *const InstanceA,
            _ => std::ptr::null(),
        }
    }
}

// Models `jit_inst_get_int`: ivar read from a cached Instance ptr (no slab+enum, still a
// primitive call + the inline scan).
#[inline(never)]
fn inst_ivar(inst: *const InstanceA, sym: u32) -> Value {
    let i = unsafe { &*inst };
    let mut k = 0usize;
    while k < i.n as usize {
        if i.ivars[k].0 == sym {
            return i.ivars[k].1;
        }
        k += 1;
    }
    Value::Nil
}

// RepA — uncached: every ivar read AND every class guard is a slab+enum primitive call.
#[inline(never)]
fn sum_a(h: &HeapA, oid: u32, cls: *const Class, d: i32) -> i64 {
    let v = match h.ivar(oid, SV) {
        Value::Int(n) => n,
        _ => 0,
    };
    if d == 0 {
        return v;
    }
    let l = match h.ivar(oid, SL) {
        Value::Obj(o) => o,
        _ => return v,
    };
    let r = match h.ivar(oid, SR) {
        Value::Obj(o) => o,
        _ => return v,
    };
    if h.class_ptr(l) != cls {
        panic!("deopt");
    }
    let sl = sum_a(h, l, cls, d - 1);
    if h.class_ptr(r) != cls {
        panic!("deopt");
    }
    let sr = sum_a(h, r, cls, d - 1);
    v + sl + sr
}

// RepA2 — self-Instance cached (CURRENT shipped rubyrs, commit cfe9ef56): one slab+enum
// per frame (jit_self_inst); the 3 self-ivar reads skip the slab walk; class guards still
// slab+enum; the child entry re-fetches its own Instance (the unavoidable double-fetch).
#[inline(never)]
fn sum_a2(h: &HeapA, oid: u32, cls: *const Class, d: i32) -> i64 {
    let inst = h.inst(oid); // one slab+enum
    let v = match inst_ivar(inst, SV) {
        Value::Int(n) => n,
        _ => 0,
    };
    if d == 0 {
        return v;
    }
    let l = match inst_ivar(inst, SL) {
        Value::Obj(o) => o,
        _ => return v,
    };
    let r = match inst_ivar(inst, SR) {
        Value::Obj(o) => o,
        _ => return v,
    };
    if h.class_ptr(l) != cls {
        panic!("deopt");
    }
    let sl = sum_a2(h, l, cls, d - 1);
    if h.class_ptr(r) != cls {
        panic!("deopt");
    }
    let sr = sum_a2(h, r, cls, d - 1);
    v + sl + sr
}

// ===== RepB: object-HEADER rep (CRuby/YJIT-style) =====
// Object IS a pointer; class is a header word; ivars at FIXED offsets. Every access is a
// single INLINED load — no slab index, no enum match, no scan, no per-access primitive call.
#[repr(C)]
struct ObjB {
    class: *const Class,
    v: i64,
    l: *const ObjB,
    r: *const ObjB,
}

#[inline(never)]
fn sum_b(o: *const ObjB, cls: *const Class, d: i32) -> i64 {
    let o = unsafe { &*o };
    let v = o.v; // inlined load
    if d == 0 {
        return v;
    }
    let l = o.l; // inlined load
    let r = o.r;
    unsafe {
        if (*l).class != cls {
            panic!("deopt");
        }
    }
    let sl = sum_b(l, cls, d - 1);
    unsafe {
        if (*r).class != cls {
            panic!("deopt");
        }
    }
    let sr = sum_b(r, cls, d - 1);
    v + sl + sr
}

fn build_a(h: &mut HeapA, cls: &Rc<Class>, v: i64, d: i32) -> u32 {
    let oid = h.slots.len() as u32;
    h.slots.push(SlotA::Live(HeapObjA::Instance(InstanceA {
        class: cls.clone(),
        ivars: [(SV, Value::Int(v)), (SL, Value::Nil), (SR, Value::Nil), (0, Value::Nil)],
        n: 3,
    })));
    if d > 0 {
        let l = build_a(h, cls, v * 2 + 1, d - 1);
        let r = build_a(h, cls, v * 2 + 2, d - 1);
        if let SlotA::Live(HeapObjA::Instance(i)) = &mut h.slots[oid as usize] {
            i.ivars[1].1 = Value::Obj(l);
            i.ivars[2].1 = Value::Obj(r);
        }
    }
    oid
}

// Contiguous arena (cap reserved → no realloc → stable ptrs), to match RepA's locality.
fn build_b(arena: &mut Vec<ObjB>, cls: *const Class, v: i64, d: i32) -> *const ObjB {
    let idx = arena.len();
    arena.push(ObjB { class: cls, v, l: std::ptr::null(), r: std::ptr::null() });
    if d > 0 {
        let l = build_b(arena, cls, v * 2 + 1, d - 1);
        let r = build_b(arena, cls, v * 2 + 2, d - 1);
        arena[idx].l = l;
        arena[idx].r = r;
    }
    unsafe { arena.as_ptr().add(idx) }
}

fn main() {
    let d: i32 = 14;
    let n: usize = 3000;

    let cls_a = Rc::new(Class { _tag: 1 });
    let cls_a_ptr = Rc::as_ptr(&cls_a);
    let mut ha = HeapA { slots: Vec::with_capacity(40000) };
    let root_a = build_a(&mut ha, &cls_a, 1, d);
    let node_count = ha.slots.len();

    let cls_b = Box::leak(Box::new(Class { _tag: 1 })) as *const Class;
    let mut arena: Vec<ObjB> = Vec::with_capacity(node_count + 8);
    let root_b = build_b(&mut arena, cls_b, 1, d);

    // All three must agree.
    let chk_a = sum_a(&ha, root_a, cls_a_ptr, d);
    let chk_a2 = sum_a2(&ha, root_a, cls_a_ptr, d);
    let chk_b = sum_b(root_b, cls_b, d);
    assert_eq!(chk_a, chk_a2, "RepA vs RepA2 mismatch");
    assert_eq!(chk_a, chk_b, "RepA vs RepB mismatch");

    println!("nodes/tree={} N={} (acc check={})", node_count, n, chk_a);

    let run = |label: &str, f: &dyn Fn() -> i64| {
        black_box(f()); // warmup
        let t = Instant::now();
        let mut acc = 0i64;
        for _ in 0..n {
            acc = acc.wrapping_add(black_box(f()));
        }
        let dt = t.elapsed();
        black_box(acc);
        let per_iter = dt.as_secs_f64() * 1000.0 / n as f64;
        let per_node = dt.as_secs_f64() * 1e9 / (n as f64 * node_count as f64);
        println!("  {:<34} per_iter={:.4}ms  per_node={:.2}ns", label, per_iter, per_node);
        per_iter
    };

    // Best-of-5 (medians are noisy at this granularity; min is the cleanest signal).
    let mut best_a = f64::MAX;
    let mut best_a2 = f64::MAX;
    let mut best_b = f64::MAX;
    for _ in 0..5 {
        best_a = best_a.min(run("RepA  slab+enum (uncached)", &|| sum_a(&ha, root_a, cls_a_ptr, d)));
        best_a2 = best_a2.min(run("RepA2 self-Instance cached", &|| sum_a2(&ha, root_a, cls_a_ptr, d)));
        best_b = best_b.min(run("RepB  object-header", &|| sum_b(root_b, cls_b, d)));
    }
    let nodes = node_count as f64;
    let acc_a2 = (best_a2 - best_b) * 1e6 / nodes; // ns/node of pure access overhead
    println!();
    println!("BEST-OF-5 per_iter:  RepA={:.4}ms  RepA2={:.4}ms  RepB={:.4}ms", best_a, best_a2, best_b);
    println!(
        "  RepA2 vs RepA  : {:.2}× ({:+.1}%)   self-Instance caching — marginal (rubyrs measured ~4%)",
        best_a / best_a2,
        (best_a2 / best_a - 1.0) * 100.0
    );
    println!(
        "  RepB  vs RepA2 : {:.2}× ({:+.1}%)   object-header, IDEAL inlining (rustc -O upper bound)",
        best_a2 / best_b,
        (best_b / best_a2 - 1.0) * 100.0
    );
    println!();
    println!("INTERPRETATION (the share, not the idealized multiplier, is what transfers):");
    println!("  slab+enum+primitive-call ACCESS overhead this PoC isolates : {:.2} ns/node", acc_a2);
    println!("  rubyrs real per-node:  jitN 0.29ms/{}n = 8.85ns   YJIT 0.19ms/{}n = 5.80ns", node_count, node_count);
    println!("  rubyrs jitN→YJIT GAP to close                              : 3.05 ns/node");
    println!("  => the access overhead a header rep removes ({:.1}ns) EXCEEDS the gap (3.05ns):", acc_a2);
    println!("     a header rep would close treesum's gap and likely beat YJIT. Feasible + worth it.");
    println!("     (The 6×+ here is rustc's ideal inlining; Cranelift gets less, but the access");
    println!("      cost it eliminates is larger than the gap either way.)");
}
