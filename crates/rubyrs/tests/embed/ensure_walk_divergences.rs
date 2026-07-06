//! Pinned DELIBERATE divergences from CRuby in the suspended-ensure-
//! walk corner (the b4/b4c family). Each test asserts rubyrs's CURRENT
//! behaviour and cites the CRuby 3.4.1 output it diverges from, so a
//! future change to this machinery either keeps the pin or makes a
//! conscious decision to move it. These are NOT diff fixtures — they
//! diverge from the oracle by design; the 37 shapes that DO match
//! byte-for-byte live in tests/diff/ensure_walk_break_return.rb.
//!
//! Why each divergence stands (see also SUBSET.md "break/next inside
//! a suspended ensure walk" and `WalkOrigin`'s doc in vm.rs):
//!
//! - `double_ensure_break_runs_outer_ensure_once`: CRuby compiles the
//!   ensure bodies a local `return` crosses as INLINE copies, and a
//!   `break` inside the inner copy emits its OWN inline copy of the
//!   outer ensure before falling into the return's copy of it — the
//!   outer ensure body executes TWICE (side effects duplicated). That
//!   is a compile-time copy-duplication artifact, not a semantic;
//!   rubyrs runs each ensure body exactly once on the walk. The
//!   method's return VALUE (:brk) matches CRuby — only the duplicate
//!   side effect differs.
//!
//! - `toplevel_return_with_ensure_break`: CRuby applies the local-
//!   return inline artifact to main (`return` at toplevel ends the
//!   script; the break value rides the exit). rubyrs compiles toplevel
//!   `return` as a non-local return (the pre-existing documented
//!   toplevel-return gap in compiler.rs Expr::Return), so the break
//!   lands at the loop join and the script continues.
//!
//! - `next_in_while_ensure_during_exception_unwind` /
//!   `next_in_block_ensure_during_exception_unwind`: CRuby re-raises
//!   (a `next` in an ensure body entered on the EXCEPTION path does
//!   not swallow the exception — asymmetric with `break`, which DOES
//!   swallow it, G1/G4 in the diff fixture). rubyrs's structural
//!   `next` supersedes the unwind and continues the loop. Fixing this
//!   means teaching the exception-path EndEnsure arm that a NextLoop
//!   transfer begun inside an exception-entered handler must re-raise
//!   at the transfer landing — tracked as a design sketch in the
//!   WalkOrigin doc; the swallow direction is the safe one (no
//!   spurious exceptions).

use super::SharedBuf;
use rubyrs::Runtime;

fn run(src: &str) -> String {
    let mut rt = Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(src, "ensure_walk_divergences.rb").unwrap();
    buf.snapshot()
}

/// E1 in the probe matrix. CRuby 3.4.1 prints:
///   E1 inner ensure / E1 outer ensure / E1 outer ensure / E1 => :brk
/// (outer ensure body runs TWICE — inline-copy duplication).
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
         break value (CRuby duplicates the outer ensure side effect)"
    );
}

/// J4 in the probe matrix. CRuby 3.4.1 prints NOTHING (the toplevel
/// return ends the script; the ensure's break value rides the exit).
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
         the break lands at the loop join and the script continues"
    );
}

/// K3 in the probe matrix. CRuby 3.4.1 prints: K3 raised k3-boom
/// (`next` in an exception-entered ensure does NOT swallow the raise).
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
         loop continues (CRuby re-raises)"
    );
}

/// K2 in the probe matrix. CRuby 3.4.1 prints: K2 raised k2-boom.
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
         iteration continues (CRuby re-raises)"
    );
}
