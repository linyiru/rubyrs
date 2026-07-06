//! Pinned goldens for the suspended-ensure-walk corner (the b4/b4c
//! family): shapes that CANNOT live in tests/diff/ because their
//! CRuby output is not stable — either rubyrs deliberately diverges,
//! or CRuby itself diverges ACROSS 3.4.x PATCH VERSIONS. Each test
//! asserts rubyrs's CURRENT behaviour and cites the CRuby outputs it
//! was verified against, so a future change to this machinery either
//! keeps the pin or makes a conscious decision to move it. The shapes
//! that DO match every probed CRuby byte-for-byte live in
//! tests/diff/ensure_walk_break_return.rb.
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
//! next"), backported into 3.4.2. The original 39-shape matrix was
//! probed against 3.4.1-prism, so rubyrs's WalkOrigin model pins
//! several bug-window behaviours:
//!
//! - The "local-return inline artifact" (a `while`/`until` `break`
//!   in the ensure of a syntactically-local `return` makes the
//!   METHOD return the break value) is 3.4.0/3.4.1-prism ONLY. In
//!   3.3.x / parse.y / prism >= 3.4.2 the break lands at the loop
//!   join and cancels the walk — which is rubyrs's own structural
//!   default for every NON-local origin. `prism_bug_window_break_family`
//!   pins the artifact; a follow-up could drop
//!   `WalkOrigin::LocalMethodReturn`'s special case and re-mainline
//!   those shapes into the diff fixture against the modern oracle.
//!
//! - "A suspended walk survives a block-`next`" (D3/K1/K4) is also
//!   3.4.0/3.4.1-prism only: modern CRuby DISCARDS the pending walk
//!   on `next` — for K4 that means `while true; yield; end` spins
//!   and the program HANGS FOREVER (verified: 3.4.5, 3.4.8, and
//!   3.4.1 parse.y all hang; this is long-standing CRuby semantics,
//!   not a regression). K4 hanging CI's floating "3.4" oracle is
//!   what forced this extraction.
//!
//! The deliberate divergences (rubyrs disagrees with SOME CRuby on
//! purpose):
//!
//! - `double_ensure_break_runs_outer_ensure_once` (E1): CRuby
//!   3.4.1-prism runs the OUTER ensure body TWICE (inline-copy
//!   duplication) and returns :brk; CRuby >= 3.4.2 runs it once but
//!   the break cancels the walk (returns :after). rubyrs runs each
//!   ensure body exactly once AND returns :brk — the side-effect
//!   count matches modern CRuby, the value matches 3.4.1-prism.
//!
//! - `toplevel_return_with_ensure_break` (J4): rubyrs compiles
//!   toplevel `return` as a non-local return (the pre-existing
//!   documented toplevel-return gap in compiler.rs Expr::Return), so
//!   the break lands at the loop join and the script continues.
//!   CRuby 3.4.1-prism ended the script; CRuby >= 3.4.2 / parse.y /
//!   3.3.x print the same thing rubyrs does — the pin stays only
//!   because the output differs between 3.4.1 and newer patches.
//!
//! - `next_in_while_ensure_during_exception_unwind` (K3) /
//!   `next_in_block_ensure_during_exception_unwind` (K2): CRuby
//!   3.4.1-prism re-raises; CRuby >= 3.4.2 / parse.y / 3.3.x swallow
//!   the exception and continue the loop — exactly what rubyrs does.
//!   Same situation as J4: rubyrs matches modern CRuby; pinned here
//!   only for the 3.4.1-vs-newer instability.

use super::SharedBuf;
use rubyrs::Runtime;

fn run(src: &str) -> String {
    let mut rt = Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(src, "ensure_walk_divergences.rb").unwrap();
    buf.snapshot()
}

/// One extracted probe-matrix shape: rubyrs must print
/// `rubyrs_and_341_prism`; `cruby_342_plus` documents (and reports on
/// failure) what the post-bug-window oracle prints instead.
struct BugWindowShape {
    name: &'static str,
    src: &'static str,
    rubyrs_and_341_prism: &'static str,
    cruby_342_plus: &'static str,
}

fn check_shapes(family: &str, shapes: &[BugWindowShape]) {
    for s in shapes {
        let out = run(s.src);
        assert_eq!(
            out, s.rubyrs_and_341_prism,
            "{family} shape {}: rubyrs must keep the CRuby-3.4.1-prism \
             behaviour it was probed against (CRuby >= 3.4.2 / parse.y / \
             3.3.x prints {:?} — see the module doc; moving to that \
             behaviour is a conscious WalkOrigin redesign, not a drift)",
            s.name, s.cruby_342_plus
        );
    }
}

/// The local-return inline artifact family (B1/B2/B3/B5, C1/C2,
/// E2/E3, H1, I1/I2 in the probe matrix): `break` in an ensure body
/// crossed by a syntactically-LOCAL `return` walk, where the loop
/// lies OUTSIDE the ensure region. CRuby 3.4.0/3.4.1-prism makes the
/// method return the break value; CRuby >= 3.4.2 (and parse.y and
/// 3.3.x) lands the break at the loop join and cancels the walk.
/// rubyrs models the former via `WalkOrigin::LocalMethodReturn`.
#[test]
fn prism_bug_window_break_family() {
    check_shapes(
        "break-family",
        &[
            BugWindowShape {
                name: "B1 (while outside; ensure breaks with value)",
                src: r#"
def b1
  while true
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "B1 after-loop reached"
  :after
end
puts "B1 => #{b1.inspect}"
"#,
                rubyrs_and_341_prism: "B1 => :brk\n",
                cruby_342_plus: "B1 after-loop reached\nB1 => :after\n",
            },
            BugWindowShape {
                name: "B2 (break with NO value)",
                src: r#"
def b2
  while true
    begin
      return :ret
    ensure
      break
    end
  end
  puts "B2 after-loop reached"
  :after
end
puts "B2 => #{b2.inspect}"
"#,
                rubyrs_and_341_prism: "B2 => nil\n",
                cruby_342_plus: "B2 after-loop reached\nB2 => :after\n",
            },
            BugWindowShape {
                name: "B3 (loop-join value observed)",
                src: r#"
def b3
  r = while true
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "B3 loop-join r=#{r.inspect}"
  :after
end
puts "B3 => #{b3.inspect}"
"#,
                rubyrs_and_341_prism: "B3 => :brk\n",
                cruby_342_plus: "B3 loop-join r=:brk\nB3 => :after\n",
            },
            BugWindowShape {
                name: "B5 (until-loop variant)",
                src: r#"
def b5
  until false
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "B5 after-loop reached"
  :after
end
puts "B5 => #{b5.inspect}"
"#,
                rubyrs_and_341_prism: "B5 => :brk\n",
                cruby_342_plus: "B5 after-loop reached\nB5 => :after\n",
            },
            BugWindowShape {
                name: "C1 (nested loops; ensure breaks INNER)",
                src: r#"
def c1
  outer_iters = 0
  while true
    outer_iters += 1
    break :outer_done if outer_iters > 2
    r = while true
      begin
        return :ret
      ensure
        break :brk
      end
    end
    puts "C1 inner join r=#{r.inspect} iter=#{outer_iters}"
  end
  puts "C1 after outer"
  :after
end
puts "C1 => #{c1.inspect}"
"#,
                rubyrs_and_341_prism: "C1 => :brk\n",
                cruby_342_plus: "C1 inner join r=:brk iter=1\nC1 inner join r=:brk iter=2\n\
                                 C1 after outer\nC1 => :after\n",
            },
            BugWindowShape {
                name: "C2 (contained loop-break inside the ensure, then outer break)",
                src: r#"
def c2
  while true
    begin
      return :ret
    ensure
      r = while true
        break :inner_brk
      end
      puts "C2 contained join r=#{r.inspect}"
      break :outer_brk
    end
  end
  puts "C2 after-loop reached"
  :after
end
puts "C2 => #{c2.inspect}"
"#,
                rubyrs_and_341_prism: "C2 contained join r=:inner_brk\nC2 => :outer_brk\n",
                cruby_342_plus: "C2 contained join r=:inner_brk\nC2 after-loop reached\n\
                                 C2 => :after\n",
            },
            BugWindowShape {
                name: "E2 (OUTER ensure breaks; inner ensure observes)",
                src: r#"
def e2
  while true
    begin
      begin
        return :ret
      ensure
        puts "E2 inner ensure"
      end
    ensure
      puts "E2 outer ensure"
      break :brk
    end
  end
  puts "E2 after-loop reached"
  :after
end
puts "E2 => #{e2.inspect}"
"#,
                rubyrs_and_341_prism: "E2 inner ensure\nE2 outer ensure\nE2 => :brk\n",
                cruby_342_plus: "E2 inner ensure\nE2 outer ensure\n\
                                 E2 after-loop reached\nE2 => :after\n",
            },
            BugWindowShape {
                name: "E3 (method-level ensure AROUND the loop)",
                src: r#"
def e3
  while true
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "E3 after-loop"
  :after
ensure
  puts "E3 method ensure"
end
puts "E3 => #{e3.inspect}"
"#,
                rubyrs_and_341_prism: "E3 method ensure\nE3 => :brk\n",
                cruby_342_plus: "E3 after-loop\nE3 method ensure\nE3 => :after\n",
            },
            BugWindowShape {
                name: "H1 (contained retry in the ensure, then break)",
                src: r#"
def h1
  while true
    begin
      return :ret
    ensure
      attempts = 0
      begin
        attempts += 1
        raise "h1-x" if attempts < 2
      rescue
        retry
      end
      puts "H1 attempts=#{attempts}"
      break :brk
    end
  end
  puts "H1 after-loop"
  :after
end
puts "H1 => #{h1.inspect}"
"#,
                rubyrs_and_341_prism: "H1 attempts=2\nH1 => :brk\n",
                cruby_342_plus: "H1 attempts=2\nH1 after-loop\nH1 => :after\n",
            },
            BugWindowShape {
                name: "I1 (two sequential loops)",
                src: r#"
def i1
  while true
    begin
      return :ret1
    ensure
      break :brk1
    end
  end
  puts "I1 between loops"
  while true
    break :brk2
  end
  puts "I1 after second loop"
  :after
end
puts "I1 => #{i1.inspect}"
"#,
                rubyrs_and_341_prism: "I1 => :brk1\n",
                cruby_342_plus: "I1 between loops\nI1 after second loop\nI1 => :after\n",
            },
            BugWindowShape {
                name: "I2 (break wrapped by an innermost ensure)",
                src: r#"
def i2
  while true
    begin
      return :ret
    ensure
      begin
        break :brk
      ensure
        puts "I2 innermost ensure"
      end
    end
  end
  puts "I2 after-loop"
  :after
end
puts "I2 => #{i2.inspect}"
"#,
                rubyrs_and_341_prism: "I2 innermost ensure\nI2 => :brk\n",
                cruby_342_plus: "I2 innermost ensure\nI2 after-loop\nI2 => :after\n",
            },
        ],
    );
}

/// The walk-survives-block-`next` family (D3/K1 in the probe matrix):
/// `next` in a block's ensure while a non-local-return walk (D3) or
/// the block's own break walk (K1) is suspended in it. CRuby
/// 3.4.0/3.4.1-prism resumes the suspended walk after the `next`;
/// CRuby >= 3.4.2 (and parse.y and 3.3.x) DISCARDS it. rubyrs models
/// the former via the abandoned-walk replay in `Op::Return`. (K4, the
/// bytecode-yield variant, is pinned separately below — the discard
/// makes modern CRuby hang forever there.)
#[test]
fn prism_bug_window_next_family() {
    check_shapes(
        "next-family",
        &[
            BugWindowShape {
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
                rubyrs_and_341_prism: "D3 => :ret\n",
                cruby_342_plus: "D3 acc=[1, 2, 3]\nD3 => :fell_through\n",
            },
            BugWindowShape {
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
                rubyrs_and_341_prism: "K1 r=:b\n",
                cruby_342_plus: "K1 r=[1, 2]\n",
            },
        ],
    );
}

/// K4 in the probe matrix: `next` in a block ensure during a
/// non-local return walk through a Ruby yielding method whose body is
/// `while true; yield; end` (the bytecode-yield variant of D3).
///
/// Re-verified 2026-07-05 on this machine: CRuby 3.4.1-prism prints
/// `K4 => :ret` (the suspended return walk survives the block-`next`,
/// like D3); CRuby 3.4.5, 3.4.8, and 3.4.1 with `--parser=parse.y`
/// all HANG FOREVER (the pending return walk is discarded by the
/// `next`, so the `while true; yield; end` loop spins — long-standing
/// CRuby semantics restored by the [Bug #21001] fix in 3.4.2). rubyrs
/// keeps the D3-consistent 3.4.1-prism behaviour: an infinite loop is
/// never the answer we want to mimic. The shape used to live in
/// tests/diff/ensure_walk_break_return.rb, but CI's "3.4" oracle
/// floats to the latest patch release, which hung the oracle run
/// ("CRuby itself failed" + a 683s suite) — hence this pinned golden.
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
         even through a bytecode yielder (CRuby 3.4.1-prism agrees; \
         CRuby >= 3.4.2 and parse.y hang forever here)"
    );
}

/// E1 in the probe matrix. CRuby 3.4.1-prism prints:
///   E1 inner ensure / E1 outer ensure / E1 outer ensure / E1 => :brk
/// (outer ensure body runs TWICE — inline-copy duplication).
/// CRuby >= 3.4.2 / parse.y / 3.3.x print:
///   E1 inner ensure / E1 outer ensure / E1 after-loop reached / E1 => :after
/// (single run, break cancels the walk). rubyrs runs each body once
/// (matching modern CRuby) and returns :brk (matching 3.4.1-prism).
#[test]
fn double_ensure_break_runs_outer_ensure_once() {
    let out = run(r#"
def e1
  while true
    begin
      begin
        return :ret
      ensure
        puts "E1 inner ensure"
        break :brk
      end
    ensure
      puts "E1 outer ensure"
    end
  end
  puts "E1 after-loop reached"
  :after
end
puts "E1 => #{e1.inspect}"
"#);
    assert_eq!(
        out,
        "E1 inner ensure\nE1 outer ensure\nE1 => :brk\n",
        "walk must run each ensure body exactly once and return the \
         break value (CRuby 3.4.1-prism duplicates the outer ensure \
         side effect; >= 3.4.2 cancels the walk and returns :after)"
    );
}

/// J4 in the probe matrix. CRuby 3.4.1-prism prints NOTHING (the
/// toplevel return ends the script; the ensure's break value rides
/// the exit). CRuby >= 3.4.2 / parse.y / 3.3.x print exactly what
/// rubyrs prints — the pin stays only because the two patch-version
/// outputs differ from each other.
#[test]
fn toplevel_return_with_ensure_break() {
    let out = run(r#"
while true
  begin
    return
  ensure
    break :brk
  end
end
puts "J4 toplevel after"
"#);
    assert_eq!(
        out, "J4 toplevel after\n",
        "rubyrs's toplevel return is non-local (documented gap), so \
         the break lands at the loop join and the script continues \
         (CRuby >= 3.4.2 agrees; 3.4.1-prism ended the script)"
    );
}

/// K3 in the probe matrix. CRuby 3.4.1-prism prints: K3 raised k3-boom
/// (it re-raises). CRuby >= 3.4.2 / parse.y / 3.3.x swallow the
/// exception and print exactly what rubyrs prints.
#[test]
fn next_in_while_ensure_during_exception_unwind() {
    let out = run(r#"
def k3
  i = 0
  while i < 2
    i += 1
    begin
      raise "k3-boom" if i == 1
    ensure
      next
    end
  end
  puts "K3 i=#{i}"
  :done
end
begin
  puts "K3 => #{k3.inspect}"
rescue => e
  puts "K3 raised #{e.message}"
end
"#);
    assert_eq!(
        out, "K3 i=2\nK3 => :done\n",
        "rubyrs's next supersedes the in-flight exception and the \
         loop continues (CRuby >= 3.4.2 agrees; 3.4.1-prism re-raised)"
    );
}

/// K2 in the probe matrix. CRuby 3.4.1-prism prints: K2 raised k2-boom.
/// CRuby >= 3.4.2 / parse.y / 3.3.x print exactly what rubyrs prints.
#[test]
fn next_in_block_ensure_during_exception_unwind() {
    let out = run(r#"
def k2
  acc = []
  [1, 2].each do |x|
    begin
      raise "k2-boom" if x == 1
    ensure
      acc << x
      next
    end
  end
  puts "K2 acc=#{acc.inspect}"
  :done
end
begin
  puts "K2 => #{k2.inspect}"
rescue => e
  puts "K2 raised #{e.message}"
end
"#);
    assert_eq!(
        out, "K2 acc=[1, 2]\nK2 => :done\n",
        "rubyrs's block-next supersedes the in-flight exception and \
         iteration continues (CRuby >= 3.4.2 agrees; 3.4.1-prism \
         re-raised)"
    );
}
