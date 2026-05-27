//! Behavioural test for the `ic-stats` cargo feature.
//!
//! Without the feature, `IcStats` is a ZST and the counters
//! aren't populated — there's nothing meaningful to assert at
//! integration-test level. With the feature on, a hot loop with
//! a single `Value::Object` receiver class should drive the IC's
//! hit rate to essentially 1.0; this test pins that behaviour so
//! a future refactor that turns the hot path into a megamorphic
//! walk would visibly fail here.
//!
//! Receiver type matters: `Array#length` and other primitive
//! dispatches go through `collection_call` (no IC), so we use a
//! user-defined class to make sure the assertion is really
//! exercising `lookup_method_cached`.
//!
//! Run with: `cargo test -p rubyrs --features ic-stats --test ic_stats`

#![cfg(feature = "ic-stats")]

use rubyrs::Runtime;

#[test]
fn hot_loop_drives_ic_hit_rate_high() {
    // 1000 iterations of `f.ping` against a user-class instance —
    // every dispatch is `Value::Object` so the call routes
    // through `lookup_method_cached`. Single class shape means
    // the 4-way IC sees no eviction; after the first miss every
    // subsequent lookup should hit.
    let mut rt = Runtime::new();
    let src = r#"
        class Foo
            def ping
                42
            end
        end
        f = Foo.new
        total = 0
        i = 0
        while i < 1000
            total += f.ping
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
        "expected ≥1000 receiver lookups for a 1000-iter loop calling f.ping; got hits={} misses={}",
        stats.hits, stats.misses,
    );
    // After the first miss the IC should keep hitting. We allow
    // for a small fixed number of misses (the first dispatch at
    // each emitted call site + a couple of preamble dispatches);
    // a hit rate above 99% is the actual interesting signal —
    // 1000 single-class-shape calls should saturate the IC.
    let hit_rate = stats.hit_rate();
    assert!(
        hit_rate > 0.99,
        "hot mono-shape loop should have hit_rate > 0.99; got {hit_rate:.4} (hits={} misses={} toplevel_hits={} toplevel_misses={})",
        stats.hits, stats.misses, stats.toplevel_hits, stats.toplevel_misses,
    );
}

#[test]
fn default_icstats_is_zero() {
    // `IcStats::default()` must report zeros across the board —
    // any other code path that touches the counters (e.g. a
    // future telemetry hook) would need to opt in, not start
    // from a non-zero baseline. Asserted directly on the type
    // rather than via Runtime, since the runtime's preamble
    // eval intentionally drives a handful of IC misses before
    // user code runs.
    let z = rubyrs::IcStats::default();
    assert_eq!(z.hits, 0);
    assert_eq!(z.misses, 0);
    assert_eq!(z.toplevel_hits, 0);
    assert_eq!(z.toplevel_misses, 0);
    assert_eq!(z.hit_rate(), 0.0);
}
