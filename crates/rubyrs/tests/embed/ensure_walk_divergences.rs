//! Pinned goldens for the suspended-ensure-walk corner (the b4/b4c
//! family): the shapes that CANNOT live in tests/diff/ because rubyrs
//! DELIBERATELY diverges from modern CRuby there. Each test asserts
//! rubyrs's current behaviour and cites the CRuby output, so a future
//! change to this machinery either keeps the pin or makes a conscious
//! decision to move it. Every shape that matches CRuby >= 3.4.2
//! byte-for-byte lives in tests/diff/ensure_walk_break_return.rb.
//!
//! THE UPSTREAM STORY (re-verified 2026-07 on this machine against
//! CRuby 3.3.10, 3.4.1 prism + parse.y, 3.4.5, 3.4.8 prism +
//! parse.y): CRuby 3.4.0/3.4.1's Prism compiler had a bug window in
//! exactly this corner — a bogus `end_label` in EnsureNode
//! compilation made `break`/`next` inside an ensure body that a
//! suspended walk crosses behave differently from parse.y, 3.3.x and
//! prism >= 3.4.2. Fixed upstream by ruby/ruby commit 31905d9e
//! ("Allow escaping from ensures through next", PR #12513,
//! [Bug #21001] "unexpected nil result from proc with ensure and
//! next"), backported into 3.4.2. rubyrs's original 39-shape probe
//! matrix predated the fix and mimicked several bug-window
//! behaviours; ticket S1 removed the mimicry
//! (`WalkOrigin::LocalMethodReturn` + the `Op::BreakLoop` artifact
//! branch) and re-mainlined the whole break family — plus E1, J4,
//! K2 and K3 — into the diff fixture against the modern oracle.
//!
//! What remains pinned here is ONE family: "a suspended walk
//! survives a block-`next`" (D3/K1/K4). Modern CRuby DISCARDS the
//! pending walk when a block ensure does `next` — and for K4 (the
//! bytecode-yield variant, `while true; yield; end`) that discard
//! makes the yielder spin, so CRuby >= 3.4.2 / parse.y / 3.3.x HANG
//! FOREVER (verified; long-standing CRuby semantics restored by the
//! [Bug #21001] fix — 3.4.1-prism was the lone non-hanging outlier).
//! rubyrs keeps the walk alive (the abandoned-walk replay in
//! `Op::Return` + do_yield's pending_yield retention): an infinite
//! loop is never a behaviour worth mimicking, and D3/K1 keep the
//! same semantics so the family stays self-consistent. K4 hanging
//! CI's floating "3.4" oracle is what forced the original
//! extraction from the diff fixture.

use super::SharedBuf;
use rubyrs::Runtime;

fn run(src: &str) -> String {
    let mut rt = Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(src, "ensure_walk_divergences.rb").unwrap();
    buf.snapshot()
}

/// One pinned probe-matrix shape: rubyrs must print `rubyrs`;
/// `cruby_342_plus` documents (and reports on failure) what the
/// modern oracle does instead.
struct PinnedShape {
    name: &'static str,
    src: &'static str,
    rubyrs: &'static str,
    cruby_342_plus: &'static str,
}

fn check_shapes(family: &str, shapes: &[PinnedShape]) {
    for s in shapes {
        let out = run(s.src);
        assert_eq!(
            out, s.rubyrs,
            "{family} shape {}: rubyrs deliberately keeps the suspended \
             walk alive across a block-`next` (CRuby >= 3.4.2 / parse.y / \
             3.3.x: {:?} — see the module doc; the family is kept for K4, \
             where the CRuby behaviour is an infinite loop)",
            s.name, s.cruby_342_plus
        );
    }
}

/// The walk-survives-block-`next` family (D3/K1 in the probe matrix):
/// `next` in a block's ensure while a non-local-return walk (D3) or
/// the block's own break walk (K1) is suspended in it. Modern CRuby
/// discards the suspended walk; rubyrs resumes it (the abandoned-walk
/// replay / adopt-break-value branches in `Op::Return`). Kept so the
/// family is consistent with K4 below, where the CRuby "discard"
/// means hanging forever.
#[test]
fn walk_survives_block_next_family() {
    check_shapes(
        "next-family",
        &[
            PinnedShape {
                name: "D3 (next in block ensure, pending method return)",
                src: r#"
def d3
  acc = []
  [1, 2, 3].each do |x|
    begin
      return :ret if x == 2
    ensure
      acc << x
      next
    end
  end
  puts "D3 acc=#{acc.inspect}"
  :fell_through
end
puts "D3 => #{d3.inspect}"
"#,
                rubyrs: "D3 => :ret\n",
                cruby_342_plus: "D3 acc=[1, 2, 3]\nD3 => :fell_through\n",
            },
            PinnedShape {
                name: "K1 (next in block ensure during the block's own break walk)",
                src: r#"
r = [1, 2].each do |x|
  begin
    break :b
  ensure
    next
  end
end
puts "K1 r=#{r.inspect}"
"#,
                rubyrs: "K1 r=:b\n",
                cruby_342_plus: "K1 r=[1, 2]\n",
            },
        ],
    );
}

/// K4 in the probe matrix: `next` in a block ensure during a
/// non-local return walk through a Ruby yielding method whose body is
/// `while true; yield; end` (the bytecode-yield variant of D3).
///
/// Re-verified 2026-07-05 on this machine: CRuby 3.4.5, 3.4.8, and
/// 3.4.1 with `--parser=parse.y` all HANG FOREVER (the pending return
/// walk is discarded by the `next`, so the `while true; yield; end`
/// loop spins — long-standing CRuby semantics restored by the
/// [Bug #21001] fix in 3.4.2; only 3.4.1-prism printed `K4 => :ret`).
/// rubyrs keeps the D3-consistent behaviour and returns `:ret`
/// cleanly: an infinite loop is never the answer we want to mimic.
/// The shape used to live in tests/diff/ensure_walk_break_return.rb,
/// but CI's "3.4" oracle floats to the latest patch release, which
/// hung the oracle run ("CRuby itself failed" + a 683s suite) — hence
/// this pinned golden.
#[test]
fn next_in_block_ensure_during_return_walk_through_ruby_yielder() {
    let out = run(r#"
def k4_yielder
  while true
    yield
  end
end
def k4
  acc = []
  k4_yielder do
    begin
      return :ret
    ensure
      acc << 1
      next
    end
  end
  puts "K4 acc=#{acc.inspect}"
  :fell
end
puts "K4 => #{k4.inspect}"
"#);
    assert_eq!(
        out, "K4 => :ret\n",
        "a suspended non-local-return walk must survive a block-`next` \
         even through a bytecode yielder (CRuby >= 3.4.2 and parse.y \
         hang forever here; only the 3.4.0/3.4.1-prism bug window \
         returned :ret)"
    );
}
