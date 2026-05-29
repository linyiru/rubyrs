//! GC + pin-guard correctness — verifies that the
//! `PinGuard` / `Vm::pinned` machinery keeps Values rooted
//! across `maybe_gc()` calls, and that block frames /
//! splat-rest params / toplevel constants stay reachable
//! under `STRESS_GC=1`.
//!
//! Why a separate sub-module: these tests all set
//! `Config::stress_gc = true` and exercise specific code
//! paths known to historically leak heap roots
//! (pre-PinGuard iterator drivers were the canonical
//! footgun). Concentrating them here makes "GC stress
//! coverage" filterable: `cargo test --test embed gc`
//! runs only the load-bearing GC assertions, much faster
//! than re-running the full diff_cruby suite under
//! STRESS_GC for a localised change.
//!
//! Tests covered:
//!   - `pin_guard_balanced_when_block_raises_inside_iterator`
//!     — PinGuard's RAII drop pops exactly the values it
//!     pinned, even on the `?`-early-return path.
//!   - `blocks_are_gc_reclaimed_under_stress` — Block
//!     allocations don't leak when the enclosing scope
//!     drops without invoking the block.
//!   - `splat_rest_param_survives_stress_gc` /
//!     `splat_rest_inline_receiver_survives_stress_gc` —
//!     `def f(*args)` rest-array Value stays rooted across
//!     the prologue maybe_gc.
//!   - `top_level_constant_array_survives_stress_gc` —
//!     toplevel constants (esp. arrays) are global roots.
//!   - `int_upto_downto_pin_block_under_stress_gc` —
//!     `Integer#upto`/`#downto` block-yield path pins the
//!     yielded i64-promoted-Value across each iteration.

use rubyrs::{Config, Runtime};

use super::SharedBuf;

#[test]
fn pin_guard_balanced_when_block_raises_inside_iterator() {
    // P0-2 regression: when a block running inside Array#each / #map /
    // any of the iterator drivers raises, the surrounding native code
    // used to leak `pinned` entries because the manual
    // `self.pinned.pop()` came AFTER the `?` early-return.
    //
    // The debug_assert in `Runtime::eval` catches an imbalanced pinned
    // stack at the end of every script. We hammer the path 50 times
    // under stress-GC to make sure the assertion doesn't fire and that
    // GC doesn't end up dragging zombie roots around.
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    for _ in 0..50 {
        let _ = rt.eval(
            r#"
            begin
              [1, 2, 3].map { |x| raise "boom" if x == 2; x * 2 }
            rescue => _e
              # swallow the synthetic RuntimeError so the script returns
              # normally; the *invariant* we're checking is that the
              # native side cleaned up its pins on the way out, not the
              # script's behaviour.
            end
            "#,
            "leak.rb",
        );
    }
    // If we got here without the debug_assert in eval firing, the
    // PinGuard's Drop was wired up correctly for every iterator
    // exit path. The assertion fired in `rt.eval` is the real test;
    // reaching this line is the success signal.
}

#[test]
fn blocks_are_gc_reclaimed_under_stress() {
    // P2-13 regression: with BlockHandle now in the GC heap, a
    // tight loop that creates many block values must let the GC
    // reclaim each block once the iteration moves on. Before
    // P2-13 blocks were Rc-managed and a (then-theoretical)
    // self-capturing cycle would leak; now they're swept like
    // Array/Hash.
    //
    // We set a small heap cap so any leak surfaces as a
    // ResourceExhausted trap rather than a slow degradation.
    // 200 iterations × {1 Array + 1 Block per iter} = 400 allocs.
    // Steady-state live_count should be O(1), well under 50.
    let mut rt = Runtime::with_config(Config {
        stress_gc: true,
        max_heap_objects: Some(50),
        ..Default::default()
    });
    rt.eval(
        r#"
        i = 0
        while i < 200
          [1, 2, 3].each { |x| i = i + 1 }
        end
        puts i
        "#,
        "many_blocks.rb",
    ).unwrap();
}

#[test]
fn splat_rest_param_survives_stress_gc() {
    // Regression: `invoke_method_with_block` allocates the
    // rest-Array via `heap.alloc(HeapObj::Array(rest_vec))` after
    // a `maybe_gc()`. Before this fix (master a24d7cb,
    // vm.rs:2615-2620), GC ran while `locals` and `rest_vec` were
    // bare Rust Vecs not in any root set — any Object / Array /
    // Hash / Range / Block referenced through them would be
    // swept under `STRESS_GC=1`, leaving dangling ObjIds inside
    // the freshly-built frame.
    //
    // Force the situation: pass heap-allocated values (Arrays) as
    // rest-args, do enough method-internal work that we'd notice
    // a sweep, then read the rest contents back. Without the pin
    // guards the inner Array elements would dangle and `.inspect`
    // would either panic or print garbage.
    let mut rt = Runtime::with_config(Config {
        // `stress_gc` triggers a collection at every alloc check,
        // matching the CI `STRESS_GC=1` mode.
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        def collect(*items)
          # A few extra allocations after the rest-Array is built,
          # so a hypothetical dangling slot has had time to be
          # reused by the time we inspect.
          tmp = []
          i = 0
          while i < 50
            tmp << [i, i + 1]
            i = i + 1
          end
          items
        end
        # Crucially: pass Array LITERALS inline, not via locals.
        # If the rest-args came from local-variable slots, those
        # slots would already be in `frames[0].locals` and the
        # GC would mark through them via the normal root walk —
        # the bug wouldn't reproduce. Inline literals are
        # constructed right before the call, pushed to the
        # operand stack, drained into `args: Vec<Value>`, and held
        # ONLY via that bare Rust Vec by the time `maybe_gc()`
        # runs inside the rest-collect branch.
        result = collect([1, 2], [3, 4], [5, 6])
        puts result.length
        puts result[0].inspect
        puts result[1].inspect
        puts result[2].inspect
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "3\n[1, 2]\n[3, 4]\n[5, 6]\n");
}

#[test]
fn splat_rest_inline_receiver_survives_stress_gc() {
    // Companion regression for the second half of the same
    // PinGuard window — beyond locals/rest_vec, the *receiver*
    // (`self_val`) is also unrooted during the rest-Array alloc.
    // Inline-allocated receivers like `Container.new.collect(...)`
    // hold the Object only as a Rust local; without pinning it,
    // STRESS_GC would sweep the instance mid-call and the method
    // body would see a dangling self.
    let mut rt = Runtime::with_config(Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r##"
        class Container
          def initialize
            @label = "live"
          end
          def collect(*items)
            tmp = []
            i = 0
            while i < 20
              tmp << [i, i + 1]
              i = i + 1
            end
            "#{@label}: #{items.length}"
          end
        end
        # Inline `.new` — the Container Object is held only as a
        # Rust local in do_call's recv slot, never bound to a Ruby
        # variable. Without `self_val` in the rest-alloc PinGuard,
        # STRESS_GC would sweep the instance during the rest-Array
        # alloc and `@label` would land on a dangling ObjId.
        puts Container.new.collect([1, 2], [3, 4], [5, 6])
    "##, "t.rb").unwrap();
    let out = buf.snapshot();
    // The body interpolates `@label` (= "live") and items.length
    // (= 3); both rely on self surviving the alloc window.
    assert_eq!(out, "live: 3\n");
}

#[test]
fn top_level_constant_array_survives_stress_gc() {
    // Regression: `Vm.constants` (the `FOO = expr` table) was added
    // without a corresponding entry in `maybe_gc`'s root walk, so
    // Array/Hash/Object values stored as constants could be swept
    // between the assignment and any later LoadConst. Under
    // STRESS_GC=1 the inner allocations below would trip a sweep
    // before the final `.length` read, and the dangling ObjId would
    // either panic or print garbage.
    let mut rt = Runtime::with_config(Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        LIST = [10, 20, 30]
        MAP = { a: 1, b: 2 }
        # Burn allocations so any unrooted ObjId held by LIST/MAP
        # would be reused by the time we read them back.
        i = 0
        while i < 50
          [i, i + 1]
          { k: i }
          i = i + 1
        end
        puts LIST.length
        puts LIST.first
        puts MAP[:a]
        puts MAP[:b]
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "3\n10\n1\n2\n");
}

#[test]
fn int_upto_downto_pin_block_under_stress_gc() {
    // Regression: `Int#upto` and `Int#downto` did not wrap their
    // loop in a PinGuard pinning `Value::Block(block)`. With
    // STRESS_GC every block-body allocation triggers GC, and the
    // block ObjId — no longer on the operand stack at this point
    // — was swept mid-loop. Next iteration's `invoke_block`
    // panicked at heap.rs:320 with "ICE: heap slot is not a
    // Block". Sibling `Int#times` already had the right pin
    // pattern + documenting comment; upto / downto were missing
    // it. Surfaced by Copilot review on PR #173 (step_block
    // migration made the pin contract explicit, exposing the
    // omission).
    //
    // This test triggers the regression by allocating inside
    // the block body — a fresh Array via `(1..50).to_a` is
    // enough to satisfy STRESS_GC's "GC at every alloc check"
    // policy, and the resulting sweep finds the block ObjId
    // unrooted unless the driver has pinned it.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # upto + downto under STRESS_GC. The body allocates,
        # forcing GC each iter. Without a Block pin, the second
        # iteration's invoke_block hits a dangling slot and
        # panics — diff_cruby couldn't catch this because the
        # ICE is a host-side panic, not a Ruby-level mismatch.
        1.upto(5) { |i| (1..50).to_a; puts "upto #{i}" }
        5.downto(1) { |i| (1..50).to_a; puts "downto #{i}" }
    "#, "upto_downto_pin.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    assert!(out.contains("upto 5"), "upto did not complete: {out}");
    assert!(out.contains("downto 1"), "downto did not complete: {out}");
}


#[test]
fn scan_captures_pin_gid_under_stress_gc() {
    // Regression: PR #178's migration of `String#scan` regex-
    // with-capture-groups branch to `step_block` preserved a
    // pre-existing pin omission. The freshly-allocated `gid`
    // Array (holding the group values) was passed to
    // `invoke_block` without being pinned. `invoke_block` may
    // run `maybe_gc()` before copying args into block locals
    // (specifically when the block has a rest parameter and
    // needs to build a rest Array), and at that point `gid` is
    // only referenced from a Rust-local `Vec<Value>` — STRESS_GC
    // sweeps it and the block then operates on freed memory
    // (typically a stack overflow ICE on the next GC).
    //
    // Surfaced by Copilot review on #178. Initial fix used
    // `g.pin(Value::Array(gid))` on the outer PinGuard; second
    // review round flagged that as O(matches) memory growth and
    // it was tightened to a scoped manual
    // `pinned.push(...); let step_result = step_block(...);
    // pinned.pop(); match step_result?` shape — pin scope ends
    // before the next iter starts, an Err from step_block still
    // runs the pop. See vm/iter.rs's regex-with-capture-groups
    // arm for the canonical pattern.
    //
    // The test forces both conditions: STRESS_GC + a block with
    // a rest parameter (`|*args|`) so invoke_block goes through
    // the rest-Array alloc path that triggers maybe_gc.
    #[cfg(feature = "regex")]
    {
        let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
            stress_gc: true,
            ..Default::default()
        });
        let buf = super::SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(r#"
            "abcabc".scan(/(a)/) { |*args| puts args.inspect }
        "#, "scan_captures_pin.rb").expect("eval should not ICE");
        let out = buf.snapshot();
        // Two matches, each yields a single-element capture Array.
        // Output is `[["a"]]\n[["a"]]\n` if pin worked; under the
        // bug the second `inspect` would see freed memory and the
        // process would have already overflowed the stack.
        assert!(
            out.lines().count() == 2,
            "expected 2 lines from 2 matches, got: {out}"
        );
    }
}

#[test]
fn hash_each_pin_pair_id_under_stress_gc() {
    // Regression: Hash#each / #each_pair allocs a fresh `[k, v]`
    // pair Array per iteration and yields it to the block. The
    // pair_id ObjId was only referenced from a Rust-local arg
    // Vec when passed to step_block. With a rest-param block
    // (`|*args|`), step_block's args→locals copy allocs a rest
    // Array — that hits maybe_gc, and under STRESS_GC the
    // unrooted pair_id gets swept before the block body reads
    // it. Surfaced by /code-review on PR #178 (same family as
    // the scan-captures gid fix, e3b90a5).
    //
    // Fix uses the scoped `pinned.push/pop` pattern around the
    // single step_block call (NOT outer PinGuard accumulation —
    // that would be O(entries) memory growth).
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        { a: 1, b: 2, c: 3 }.each { |*args| puts args.inspect }
    "#, "hash_each_pin.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // Content assertion (not just line count): catches a
    // silent-corruption variant where pair_id's slot is freed
    // and recycled to another same-shape Array — the line count
    // would still be 3 but the printed pairs would be wrong.
    assert_eq!(
        out, "[[:a, 1]]\n[[:b, 2]]\n[[:c, 3]]\n",
        "pair contents corrupted (likely pair_id swept mid-iter), got: {out}"
    );
}

#[test]
fn hash_each_with_index_pin_pair_id_under_stress_gc() {
    // Shape lock-in (not a regression test for the pre-fix
    // shape). Hash#each_with_index pinned pair_id via the outer
    // PinGuard (`g.pin(Value::Array(pair_id))`) on master, so
    // pair_id was already rooted — this test would have passed
    // against master too. What changed in PR #183 is the pin
    // *lifetime*: scoped `pinned.push/pop` releases per-iter
    // (O(1) memory) instead of accumulating in PinGuard's slot
    // list (O(entries)). The test still serves a purpose: it
    // locks in that the new scoped shape preserves the pin
    // contract, so a future refactor that drops the push/pop
    // entirely would be caught (block body would see freed
    // pair_id memory and crash / print garbage).
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        { a: 1, b: 2, c: 3 }.each_with_index { |*args| puts args.inspect }
    "#, "hash_ewi_pin.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // Content assertion: see hash_each_pin_pair_id_... for
    // rationale. Block receives `(pair, idx)` so the rest-Array
    // is `[[k, v], i]`.
    assert_eq!(
        out, "[[:a, 1], 0]\n[[:b, 2], 1]\n[[:c, 3], 2]\n",
        "pair or index corrupted, got: {out}"
    );
}

#[test]
fn array_chunk_pin_heap_keys_under_stress_gc() {
    // Regression: Array#chunk accumulates block-returned keys in
    // a Rust-local `Vec<(Value, Vec<Value>)>`. If the block
    // returns a GC-tracked heap-slot Value (Array / Hash /
    // Object / Range / Block / BoundMethod / UnboundMethod /
    // CurriedProc / BigInt), the previous iteration's key is
    // only reachable via this Rust-local Vec — not via
    // scan_roots. Next iteration's step_block can fire
    // maybe_gc, sweep the key, and the post-iter
    // `groups.last() / ruby_eq` then reads freed memory.
    // (`Value::Str` is `Rc<RStr>` — not a GC heap slot — so
    // string keys don't need pinning; immediates like
    // Int/Sym/Bool/Nil/Float likewise.) Surfaced by Copilot
    // review on PR #187.
    //
    // The fixture forces both conditions: STRESS_GC (alloc-time
    // sweeps) + block returning a fresh heap Array each call.
    // Without the `g.pin(key.clone())` fix, this would ICE on
    // the second iteration's `ruby_eq` reading a dead Array
    // slot.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # Block returns a fresh Array each call — heap-managed
        # key. With 4 distinct elements we get 4 distinct keys
        # accumulating in `groups`. Under STRESS_GC each key
        # alloc + each step_block call triggers a sweep; the
        # previously-stored keys must survive.
        puts [1, 2, 3, 4].chunk { |x| [x] }.inspect
    "#, "chunk_pin.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // rubyrs prints the chunk result directly (eager
    // implementation). In CRuby `Array#chunk` returns an
    // Enumerator and you'd need `chunk(...).to_a` to materialize
    // this same shape — once enumerated, both produce
    // `[[[1], [1]], [[2], [2]], [[3], [3]], [[4], [4]]]` (each
    // group has one element since each key is unique).
    assert_eq!(
        out, "[[[1], [1]], [[2], [2]], [[3], [3]], [[4], [4]]]\n",
        "chunk output corrupted (likely a key was swept), got: {out}"
    );
}

#[test]
fn array_chunk_pin_snapshot_under_receiver_mutation() {
    // Regression: Array#chunk clones the receiver into a Rust-
    // local `snapshot: Vec<Value>` and iterates it. If the block
    // mutates the receiver mid-iteration (`arr.clear` / `shift`
    // / `slice!`), the original elements are no longer reachable
    // through the pinned receiver Array — they live only in the
    // Rust-local `snapshot` / `groups` Vecs, which scan_roots
    // can't see. Next iteration's step_block can fire maybe_gc
    // and sweep those Value slots, leaving the block body
    // operating on freed memory. Same family as the sort
    // driver's `for v in &copy { g.pin(v.clone()); }` defensive
    // pin (iter.rs:1713). Surfaced by Copilot review on PR #187.
    //
    // CRuby disallows concurrent mutation entirely (raises
    // RuntimeError); rubyrs keeps the elements alive defensively
    // so the primitive completes without ICE'ing, matching the
    // sort precedent.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # `nested` holds three inner Arrays (heap slots). On
        # iteration 1, the block shifts all elements off the
        # receiver. After that the inner Arrays `[3, 4]` /
        # `[5, 6]` are reachable ONLY via the Rust-local
        # snapshot Vec inside the chunk primitive. Under
        # STRESS_GC, the next step_block's maybe_gc would sweep
        # those slots without the defensive `for v in &snapshot
        # { g.pin(v.clone()); }` line.
        nested = [[1, 2], [3, 4], [5, 6]]
        result = nested.chunk { |inner|
          # Drain receiver on first iteration, leaving snapshot
          # as the sole live reference to inner Arrays.
          nested.shift while nested.length > 0
          inner.first
        }
        puts result.inspect
    "#, "chunk_mut.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // Each element keyed by its first; all distinct → 3 groups.
    assert_eq!(
        out, "[[1, [[1, 2]]], [3, [[3, 4]]], [5, [[5, 6]]]]\n",
        "chunk corrupted (likely snapshot element swept after receiver mutation), got: {out}"
    );
}

#[test]
fn array_group_by_pin_heap_keys_under_stress_gc() {
    // Regression: Array#group_by holds the block-returned `key` as
    // a Rust local across `maybe_gc / check_alloc / heap.alloc` for
    // the bucket Array, then pushes `(key, ...)` into the result
    // Hash. Without pinning, the explicit `maybe_gc()` BEFORE the
    // bucket alloc sweeps heap-Value keys; the immediately-
    // following `heap.alloc` reuses the freed slot for the bucket
    // Array; the Hash then stores the dangling ObjId alongside the
    // newly-allocated bucket (which now occupies the same slot).
    // Same family as the chunk driver's GC pin.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # Block returns a fresh heap Array with content DIFFERENT
        # from the value: `["k#{x}"]` (key) vs `x` (element). This
        # distinction is crucial — an earlier draft used `[x]` as
        # both the key and the bucket-content shape, which masked
        # the bug: a swept key slot got reused by the
        # immediately-following bucket alloc and `inspect` still
        # printed the "expected" shape because key and bucket
        # happened to be aliased.
        #
        # With a distinct shape we get the real failure mode:
        # without the fix, the swept key slot is overwritten by
        # the bucket Array, so `inspect` produces e.g.
        # `{[10] => [10], ...}` instead of CRuby's
        # `{["k10"] => [10], ...}`.
        result = [10, 20, 30].group_by { |x| ["k#{x}"] }
        puts result.inspect
    "#, "group_by_pin.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    assert_eq!(
        out, "{[\"k10\"] => [10], [\"k20\"] => [20], [\"k30\"] => [30]}\n",
        "group_by output corrupted (key slot was reused by bucket alloc), got: {out}"
    );
}

#[test]
fn array_group_by_pin_snapshot_under_receiver_mutation() {
    // Regression: Array#group_by clones the receiver into a Rust-
    // local `snapshot: Vec<Value>` and iterates it. If the block
    // mutates the receiver mid-iteration (`arr.shift` / `slice!` /
    // etc.), the snapshot's original heap-Value elements (e.g.
    // inner Arrays) are no longer reachable through the pinned
    // receiver — they live only in the snapshot Vec, which
    // scan_roots can't see. The explicit `maybe_gc()` BEFORE the
    // bucket alloc sweeps the still-pending snapshot elements;
    // the subsequent `heap.alloc(HeapObj::Array(vec![v]))` just
    // reuses freed slots, but subsequent iterations then read
    // those swept slots and crash. Same family as the chunk
    // driver's defensive snapshot pin and the group_by key pin
    // earlier in this same file.
    //
    // Without the defensive `for v in &snapshot { ... }` pin
    // loop, the reproducer ICEs at `heap.rs:180` with
    // `use-after-free ObjId(N)`. With the fix it completes —
    // rubyrs's `group_by` (eager — returns a Hash directly when a
    // block is given) produces all three bucket entries, whereas
    // CRuby's `group_by` stops processing after the receiver
    // mutation and only produces the first entry. The CRuby
    // divergence is documented but not under test here; the focus
    // is on rubyrs surviving the GC pressure without ICE'ing.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # `nested` holds three inner Arrays (heap slots). On
        # iteration 1, the block shifts all elements off the
        # receiver. After that, iter 2/3's elements are reachable
        # ONLY via the Rust-local snapshot Vec inside group_by.
        # Without the defensive pin, STRESS_GC + the bucket alloc
        # sweeps them and the next read ICEs.
        nested = [[1, 2], [3, 4], [5, 6]]
        result = nested.group_by { |inner|
          nested.shift while nested.length > 0
          inner.first
        }
        puts result.inspect
    "#, "group_by_mut.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // Soft assertion focused on the GC-safety invariant rather
    // than the exact bucket count: the test passes as long as
    // (a) eval completes without an ICE / SIGABRT (the `expect`
    // above), and (b) the printed result starts with `{1 => [[1, 2]]`
    // (the first inner Array survived the snapshot pin and made
    // it into the output Hash).
    //
    // Deliberately NOT asserting exact equality with the eager
    // rubyrs output `{1 => [[1, 2]], 3 => [[3, 4]], 5 => [[5, 6]]}`
    // nor with CRuby's `{1 => [[1, 2]]}` — both are valid given
    // CRuby's "unspecified under concurrent mutation" stance, and
    // pinning the test to whichever rubyrs happens to do today
    // would cause spurious failures if the implementation is
    // later aligned with CRuby's stop-after-mutation behaviour.
    // The bug this test guards against is a USE-AFTER-FREE on a
    // swept snapshot element — that failure mode is `expect("eval
    // should not ICE")`, NOT the bucket layout.
    assert!(
        out.starts_with("{1 => [[1, 2]]"),
        "group_by output corrupted (first inner Array did not survive): {out}"
    );
}

#[test]
fn array_chunk_while_pin_snapshot_under_receiver_mutation() {
    // Regression: Array#chunk_while clones the receiver into a
    // Rust-local `snapshot: Vec<Value>` and iterates pairs of
    // adjacent elements. If the block mutates the receiver mid-
    // iteration (`arr.shift` / `slice!` / etc.), the snapshot's
    // original heap-Value elements lose their transitive root
    // through the pinned receiver — they live only in the snapshot
    // Vec (and later in `current_chunk`), which scan_roots can't
    // see. The explicit `maybe_gc()` at the chunk-flush boundary
    // (or inside step_block) sweeps them; subsequent reads ICE at
    // `heap.rs:180` with `use-after-free ObjId(N)`. Same family as
    // the chunk driver's defensive snapshot pin and the group_by
    // snapshot pin already in this file.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # `nested` holds four inner Arrays (heap slots). The block
        # drains the receiver on the first pair, then the snapshot
        # is the sole live reference to the inner Arrays. Returning
        # `true` keeps them in `current_chunk`, where the next
        # iteration's step_block-triggered maybe_gc would sweep
        # them without the defensive pin.
        nested = [[1, 2], [3, 4], [5, 6], [7, 8]]
        result = nested.chunk_while { |a, b|
          nested.shift while nested.length > 0
          true
        }
        puts result.inspect
    "#, "chunk_while_mut.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // Soft assertion focused on GC safety (same rationale as the
    // group_by mutation test): assert the eval didn't ICE and the
    // first inner Array survived. Do not lock in the exact output
    // — CRuby's behaviour under concurrent mutation is unspecified.
    assert!(
        out.starts_with("[[[1, 2]"),
        "chunk_while output corrupted (first inner Array did not survive): {out}"
    );
}

#[test]
fn hash_reduce_pin_heap_acc_under_stress_gc() {
    // Regression: Hash#reduce / #inject's per-iteration acc-pin
    // was positioned AFTER the loop-top `maybe_gc()` + pair-
    // Array alloc. After iter 1, `acc` is whatever the block
    // returned — when it's a heap-backed Array, the loop-top
    // maybe_gc swept it before the pin push could root it,
    // and the immediately-following pair_id alloc reused the
    // slot. Caught by code-review on PR #278 and reproduced
    // here as a use-after-free panic.
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # acc starts as `[]` (from-init form) and grows by 2
        # elements per iter — each `acc + [k, v]` allocates a
        # fresh Array that becomes the next iter's acc, held
        # only in the iter.rs Rust local. Without the hoisted
        # pin the next loop-top maybe_gc sweeps it.
        h = {a: 1, b: 2, c: 3, d: 4}
        r = h.reduce([]) { |acc, (k, v)| acc + [k, v] }
        puts r.inspect
    "#, "hash_reduce_acc.rb").expect("eval should not ICE on heap-backed reduce acc");
    let out = buf.snapshot();
    assert!(
        out.starts_with("[:a, 1, :b, 2, :c, 3, :d, 4]"),
        "reduce acc corrupted under STRESS_GC: {out}"
    );
}

#[test]
#[cfg(feature = "bignum")]
fn hash_sum_pin_bigint_acc_under_stress_gc() {
    // Regression: Hash#sum's acc-pin had the same position
    // bug as reduce. The all-Int hot path was safe (Int is
    // not a heap ref) but Bignum overflow promoted acc to a
    // freshly-allocated BigInt held only in the Rust local;
    // the next iter's loop-top maybe_gc swept it before the
    // pin push. Reproducible as `ICE: heap slot is not a
    // BigInt` under STRESS_GC=1.
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # 4_611_686_018_427_387_904 = 2^62. Two of those
        # overflow i64::MAX → apply_int_promote widens acc
        # to a BigInt on iter 2.
        h = {a: 1, b: 2, c: 3, d: 4, e: 5}
        r = h.sum(0) { |k, v| 4_611_686_018_427_387_904 }
        puts r
    "#, "hash_sum_bigint.rb").expect("eval should not ICE on BigInt sum acc");
    let out = buf.snapshot();
    // 5 * 2^62 = 23_058_430_092_136_939_520.
    assert!(
        out.starts_with("23058430092136939520"),
        "sum BigInt acc corrupted under STRESS_GC: {out}"
    );
}

#[test]
fn hash_first_min_max_pin_receiver_under_stress_gc() {
    // Regression: Hash#first / #min / #max in hash.rs went
    // through `maybe_gc` + `heap.alloc` without pinning the
    // receiver Hash (held only in the Rust local from do_call's
    // recv-pop) or the chosen k/v pair. Under STRESS_GC=1, a
    // sweep at the alloc boundary would corrupt the returned
    // pair Array.
    //
    // Reproducer is load-bearing only when the receiver is
    // an INLINE Hash literal — a local-bound Hash gets rooted
    // via the frame-locals walker. Inline literals are built
    // on the operand stack, popped into the do_call recv-pop
    // Rust local, then ONLY held there. Interpolation in the
    // key plus inner-Array initialization fires enough allocs
    // to make the sweep window observable. Verified to ICE
    // (`use-after-free`) on the unfixed code via stash test.
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        r = {"long-key-#{1+1}" => [Array.new(3) { |i| i*2 }, "nested"]}.first
        puts r.inspect
        puts({"b" => 2, "a" => 1, "c" => 3}.min.inspect)
        puts({"b" => 2, "a" => 1, "c" => 3}.max.inspect)
    "#, "hash_first_min_max.rb").expect("eval should not ICE under STRESS_GC");
    let out = buf.snapshot();
    assert!(
        out.contains(r#"["long-key-2", "#) &&
        out.contains(r#"["a", 1]"#) &&
        out.contains(r#"["c", 3]"#),
        "first/min/max output corrupted under STRESS_GC: {out}"
    );
}

#[test]
fn hash_uniq_pin_seen_keys_under_stress_gc() {
    // Regression: Hash#uniq's `seen` set was a Rust-local
    // Vec<Value> holding block return values across
    // iterations. When the block returned a heap-backed
    // Value (Array / Hash / String / BigInt), the next
    // iter's maybe_gc swept the slot — and the subsequent
    // ruby_eql scan read use-after-free heap slots,
    // SILENTLY returning false on every comparison. No
    // ICE; just wrong output (the uniq predicate failed
    // to deduplicate anything).
    //
    // Caught by Copilot review on PR #292. Fixed by
    // storing the seen-keys list in a heap-backed Array
    // that's pinned via PinGuard — its contents become
    // real GC roots through the heap walker.
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        # Block returns a freshly-allocated Array per iter
        # (heap-backed). Without the fix, the seen-keys
        # list dangles and the dedup silently fails.
        h = {a: 1, b: 1, c: 2, d: 1, e: 2, f: 3, g: 1, h: 2}
        r = h.uniq { |k, v| [v, "tag", v.to_s, [v, v, v]] }
        puts r.inspect
    "#, "uniq_stress.rb").expect("eval should not ICE");
    let out = buf.snapshot();
    // Correct dedup: 3 unique values (1, 2, 3); first-seen
    // wins → [:a, 1] / [:c, 2] / [:f, 3].
    assert!(
        out.starts_with("[[:a, 1], [:c, 2], [:f, 3]]"),
        "uniq block-form seen keys corrupted under STRESS_GC: {out}"
    );
}
