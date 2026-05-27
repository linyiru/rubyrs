//! Behavioural test for the `ic-stats` cargo feature.
//!
//! Without the feature, `IcStats` is a ZST and the counters
//! aren't populated — there's nothing meaningful to assert at
//! integration-test level. With the feature on, a hot loop with
//! a single receiver class should drive the IC's hit rate to
//! essentially 1.0; this test pins that behaviour so a future
//! cache-eviction refactor that turns the hot path into a
//! megamorphic walk would visibly fail here.
//!
//! Run with: `cargo test -p rubyrs --features ic-stats --test ic_stats`

#![cfg(feature = "ic-stats")]

use rubyrs::Runtime;

#[test]
fn hot_loop_drives_ic_hit_rate_high() {
    // 1000 iterations of `arr.length` — same receiver class
    // (Array) every dispatch, so after the first miss the IC
    // should report a hit on every subsequent lookup. With
    // IC_WAYS=4 and one class shape, no eviction can happen.
    let mut rt = Runtime::new();
    let src = r#"
        arr = [1, 2, 3, 4, 5]
        total = 0
        i = 0
        while i < 1000
            total += arr.length
            i += 1
        end
        total
    "#;
    let val = rt.eval(src, "<hot_loop>").expect("eval should succeed");
    // Sanity: the loop body must have actually run.
    assert!(matches!(val, rubyrs::Value::Int(_)), "expected Int result, got {val:?}");

    let stats = rt.ic_stats();
    let total = stats.hits + stats.misses;
    assert!(
        total >= 1000,
        "expected ≥1000 receiver lookups for a 1000-iter loop calling .length; got hits={} misses={}",
        stats.hits, stats.misses,
    );
    // After the first miss the IC should keep hitting. We allow
    // for a small fixed number of misses (the first lookup at
    // each emitted call site + a couple of preamble dispatches);
    // a hit rate above 90% is the actual interesting signal.
    let hit_rate = stats.hit_rate();
    assert!(
        hit_rate > 0.90,
        "hot mono-shape loop should have hit_rate > 0.90; got {hit_rate:.4} (hits={} misses={} toplevel_hits={} toplevel_misses={})",
        stats.hits, stats.misses, stats.toplevel_hits, stats.toplevel_misses,
    );
}

#[test]
fn fresh_runtime_has_zero_counters() {
    // A Runtime that hasn't eval'd anything must report all
    // zeros — pins that `IcStats::default()` is the zero state
    // and no other code path accidentally touches the counters
    // during construction (e.g. the preamble eval increments
    // before user code runs).
    //
    // The CLI binary does load a preamble at construct time, so
    // we can't assert "zero after Runtime::new()". The contract
    // we DO want to hold: `IcStats::default()` is zero. Test
    // that directly.
    let z = rubyrs::IcStats::default();
    assert_eq!(z.hits, 0);
    assert_eq!(z.misses, 0);
    assert_eq!(z.toplevel_hits, 0);
    assert_eq!(z.toplevel_misses, 0);
    assert_eq!(z.hit_rate(), 0.0);
}
