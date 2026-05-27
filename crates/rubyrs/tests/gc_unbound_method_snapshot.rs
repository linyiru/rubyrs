//! GC root-walker coverage for `HeapObj::UnboundMethod.method`'s
//! snapshot.
//!
//! The snapshot field added in the bind_call PR holds an
//! `Rc<Method>` that may carry a `MethodClosure.captured` Vec of
//! heap-managed Values. Once a `remove_method` strips the
//! captured method from its class's methods table, the snapshot
//! becomes the SOLE holder of those captured Values — the
//! regular `Vm.maybe_gc` walker only iterates `Vm.classes`'s
//! method tables and won't reach the snapshot.
//!
//! Without an explicit mark traversal of
//! `UnboundMethod.method.closure.captured`, the captured Vec
//! gets swept while the UnboundMethod (and thus the snapshot) is
//! still reachable. The subsequent `bind_call` would observe
//! ObjId pointing at Slot::Dead / a re-used slot — surfacing as
//! either a panic ("ICE: heap slot is not an Array") or wrong
//! data.
//!
//! Under `STRESS_GC=1` every allocation triggers a sweep, so any
//! gap in the new walker would surface immediately. The
//! assertion locks the captured array round-trip; a regression
//! would either crash or print a non-`[1, 2, 3, 4, 5]` line.

use std::process::Command;

const SCRIPT: &str = r#"
class Capturer
  outer = [1, 2, 3, 4, 5]
  define_method(:get) { outer }
end
um = Capturer.instance_method(:get)
Capturer.class_eval { remove_method(:get) }
# Burn allocations to provoke sweeps under STRESS_GC=1.
30.times { _x = (1..50).to_a; _y = { a: 1, b: 2 } }
puts um.bind_call(Capturer.new).inspect
"#;

#[test]
fn unbound_method_snapshot_survives_stress_gc() {
    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let tmpdir = env!("CARGO_TARGET_TMPDIR");
    let driver = std::path::Path::new(tmpdir).join("gc_unbound_method_snapshot.rb");
    std::fs::write(&driver, SCRIPT).expect("write driver");
    let out = Command::new(rubyrs_bin)
        .env("STRESS_GC", "1")
        .arg(&driver)
        .output()
        .expect("spawn rubyrs");
    assert!(
        out.status.success(),
        "rubyrs failed under STRESS_GC=1:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "[1, 2, 3, 4, 5]",
        "captured closure locals went stale under STRESS_GC — snapshot field's MethodClosure.captured isn't being walked"
    );
}
