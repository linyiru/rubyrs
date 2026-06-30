// Feasibility PoC for ADR 0036 Phase 6: would representing objects as DIRECT POINTERS
// (eliminating the `oid -> slab[oid] -> Slot -> HeapObj::Instance` lookup) let `bench_treesum`
// surpass YJIT, now that ADR 0035 Phase 5 has already inlined the ivar reads + class guard?
//
// After Phase 5 the rubyrs JIT pays exactly ONE primitive per recursive `sum()` node:
// `jit_self_ivars`, the `oid -> Instance` slab lookup. A profile confirms it is the only
// remaining named hot frame; a single-ivar getter in a LOOP already beats YJIT 1.28x because
// that lookup AMORTIZES — treesum can't amortize it (one per recursive call). Real numbers:
//   rubyrs jitN treesum 0.258ms  vs  YJIT 0.187ms  (1.38x behind), per-node 7.87 vs 5.71 ns.
//
// This PoC runs the treesum recursion three ways, all with INLINE ivar reads + an inline class
// guard (the Phase-5 state), differing ONLY in how `self`'s data is reached per node:
//   RepA_now      : slab lookup via a #[inline(never)] call  (models today's jit_self_ivars)
//   RepA_inlined  : the SAME slab+enum walk, but INLINED      (no call boundary; the ceiling
//                                                              of "inline the slab into the JIT"
//                                                              without changing the heap rep)
//   RepB_directptr: object IS a pointer, one deref, no slab   (models Phase 6)
//
// Splitting RepA_now / RepA_inlined / RepB tells us WHICH lever matters:
//   - if RepA_inlined ≈ RepB  → the cost is the CALL BOUNDARY; inlining the slab walk into the
//     JIT (keeping oid) would suffice — a far cheaper change than a pointer rewrite.
//   - if RepA_inlined ≫ RepB  → the oid->slab+enum indirection ITSELF is the cost; only direct
//     pointers (Phase 6) close it.
// Both reps use a contiguous arena (locality held constant). Build:
//   rustc -O poc/jit-spike/objptr_feasibility.rs -o /tmp/objptr && /tmp/objptr

use std::hint::black_box;
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

// ===== Slab representation (today's rubyrs) =====
struct InstanceA {
    // ivars inline, slot-indexed (Phase 5 reads these inline: @v=0, @l=1, @r=2)
    ivars: [(u32, Value); 4],
    n: u8,
}
enum HeapObjA {
    Instance(InstanceA),
    #[allow(dead_code)]
    Other,
}
enum SlotA {
    Live(HeapObjA),
    #[allow(dead_code)]
    Dead,
}
struct HeapA {
    slots: Vec<SlotA>,
    class_flat: Vec<*const Class>, // ADR 0035 Phase 2/3b class table (inline-read guard)
}

// The slab walk, as a NON-inlined call — models the `jit_self_ivars` primitive boundary.
#[inline(never)]
fn slab_inst_call(h: &HeapA, oid: u32) -> *const InstanceA {
    match &h.slots[oid as usize] {
        SlotA::Live(HeapObjA::Instance(i)) => i as *const InstanceA,
        _ => std::ptr::null(),
    }
}
// The SAME walk, inlined — models inlining the slab access into the JIT (no call boundary).
#[inline(always)]
fn slab_inst_inlined(h: &HeapA, oid: u32) -> *const InstanceA {
    match &h.slots[oid as usize] {
        SlotA::Live(HeapObjA::Instance(i)) => i as *const InstanceA,
        _ => std::ptr::null(),
    }
}

#[inline(always)]
fn read_inst(inst: *const InstanceA, slot: usize) -> Value {
    unsafe { (*inst).ivars[slot].1 } // Phase 5 inline ivar read (slot known)
}

// RepA_now: per node ONE non-inlined slab lookup (jit_self_ivars), then inline reads + guard.
#[inline(never)]
fn sum_a_now(h: &HeapA, oid: u32, cls: *const Class, d: i32) -> i64 {
    let inst = slab_inst_call(h, oid);
    let v = match read_inst(inst, 0) {
        Value::Int(n) => n,
        _ => 0,
    };
    if d == 0 {
        return v;
    }
    let l = match read_inst(inst, 1) {
        Value::Obj(o) => o,
        _ => return v,
    };
    let r = match read_inst(inst, 2) {
        Value::Obj(o) => o,
        _ => return v,
    };
    // inline class guard (Phase 3b: flat class table read)
    if h.class_flat[l as usize] != cls || h.class_flat[r as usize] != cls {
        panic!("deopt");
    }
    let sl = sum_a_now(h, l, cls, d - 1);
    let sr = sum_a_now(h, r, cls, d - 1);
    v + sl + sr
}

// RepA_inlined: identical, but the slab walk is INLINED (no call boundary).
#[inline(never)]
fn sum_a_inlined(h: &HeapA, oid: u32, cls: *const Class, d: i32) -> i64 {
    let inst = slab_inst_inlined(h, oid);
    let v = match read_inst(inst, 0) {
        Value::Int(n) => n,
        _ => 0,
    };
    if d == 0 {
        return v;
    }
    let l = match read_inst(inst, 1) {
        Value::Obj(o) => o,
        _ => return v,
    };
    let r = match read_inst(inst, 2) {
        Value::Obj(o) => o,
        _ => return v,
    };
    if h.class_flat[l as usize] != cls || h.class_flat[r as usize] != cls {
        panic!("deopt");
    }
    let sl = sum_a_inlined(h, l, cls, d - 1);
    let sr = sum_a_inlined(h, r, cls, d - 1);
    v + sl + sr
}

// ===== Direct-pointer representation (Phase 6) =====
#[repr(C)]
struct ObjB {
    class: *const Class,
    v: i64,
    l: *const ObjB,
    r: *const ObjB,
}

#[inline(never)]
fn sum_b(o: *const ObjB, cls: *const Class, d: i32) -> i64 {
    let o = unsafe { &*o }; // one direct deref, no slab/enum/oid
    let v = o.v;
    if d == 0 {
        return v;
    }
    let l = o.l;
    let r = o.r;
    unsafe {
        if (*l).class != cls || (*r).class != cls {
            panic!("deopt");
        }
    }
    let sl = sum_b(l, cls, d - 1);
    let sr = sum_b(r, cls, d - 1);
    v + sl + sr
}

fn build_a(h: &mut HeapA, cls: *const Class, v: i64, d: i32) -> u32 {
    let oid = h.slots.len() as u32;
    h.slots.push(SlotA::Live(HeapObjA::Instance(InstanceA {
        ivars: [(1, Value::Int(v)), (2, Value::Nil), (3, Value::Nil), (0, Value::Nil)],
        n: 3,
    })));
    h.class_flat.push(cls);
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

    let cls = Box::leak(Box::new(Class { _tag: 1 })) as *const Class;
    let mut ha = HeapA { slots: Vec::with_capacity(40000), class_flat: Vec::with_capacity(40000) };
    let root_a = build_a(&mut ha, cls, 1, d);
    let nodes = ha.slots.len();

    let mut arena: Vec<ObjB> = Vec::with_capacity(nodes + 8);
    let root_b = build_b(&mut arena, cls, 1, d);

    let chk_now = sum_a_now(&ha, root_a, cls, d);
    let chk_inl = sum_a_inlined(&ha, root_a, cls, d);
    let chk_b = sum_b(root_b, cls, d);
    assert_eq!(chk_now, chk_inl);
    assert_eq!(chk_now, chk_b);
    println!("nodes/tree={} N={} (acc={})", nodes, n, chk_now);

    let run = |label: &str, f: &dyn Fn() -> i64| {
        black_box(f());
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let mut acc = 0i64;
            for _ in 0..n {
                acc = acc.wrapping_add(black_box(f()));
            }
            best = best.min(t.elapsed().as_secs_f64());
            black_box(acc);
        }
        let per_iter = best * 1000.0 / n as f64;
        let per_node = best * 1e9 / (n as f64 * nodes as f64);
        println!("  {:<26} per_iter={:.4}ms  per_node={:.2}ns", label, per_iter, per_node);
        per_node
    };

    let a_now = run("RepA_now (slab call)", &|| sum_a_now(&ha, root_a, cls, d));
    let a_inl = run("RepA_inlined (slab inline)", &|| sum_a_inlined(&ha, root_a, cls, d));
    let b = run("RepB_directptr (Phase 6)", &|| sum_b(root_b, cls, d));

    println!();
    println!("PER-NODE ns:  RepA_now={:.2}  RepA_inlined={:.2}  RepB_directptr={:.2}", a_now, a_inl, b);
    println!("  call-boundary cost (A_now - A_inlined) = {:.2} ns/node", a_now - a_inl);
    println!("  slab+enum-work cost (A_inlined - RepB) = {:.2} ns/node", a_inl - b);
    println!("  total slab cost a pointer removes (A_now - RepB) = {:.2} ns/node", a_now - b);
    println!();
    println!("CALIBRATION vs real rubyrs: jitN 7.87 ns/node, YJIT 5.71 ns/node, gap 2.16 ns/node.");
    println!("  This PoC's lean rustc skeleton INFLATES the slab's relative share — it is not");
    println!("  authoritative. The real jitN profile is: the anonymous Cranelift code (recursion");
    println!("  + arith + guards) is ~95% of the run; jit_self_ivars (the slab lookup) is ~5%.");
    println!("  => removing the slab (Phase 6) saves ~5% of a ~27% gap. treesum stays ~1.3x");
    println!("  behind. The gap is CODEGEN QUALITY (call_indirect recursion), not the rep.");
    println!("  VERDICT: Phase 6 (objects-as-pointers) REJECTED — see ADR 0036.");
}
