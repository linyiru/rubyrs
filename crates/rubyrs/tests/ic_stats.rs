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
    // the IC sees no eviction regardless of `IC_WAYS`; after the
    // first miss every subsequent lookup should hit.
    //
    // Snapshot before/after the workload eval so the assertion
    // is on the DELTA, not on aggregate counters that also
    // include any preamble/setup dispatches Runtime did at
    // construct time. Keeps the test stable as the preamble
    // evolves.
    let mut rt = Runtime::new();
    let before = rt.ic_stats();
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
    let after = rt.ic_stats();

    let hits = after.hits - before.hits;
    let misses = after.misses - before.misses;
    let total = hits + misses;
    assert!(
        total >= 1000,
        "expected ≥1000 receiver lookups during the 1000-iter loop calling f.ping; got delta hits={hits} misses={misses}",
    );
    // After the first miss the IC should keep hitting. A hit
    // rate above 99% on the workload-delta alone is the
    // interesting signal — 1000 single-class-shape calls should
    // saturate the IC at any reasonable `IC_WAYS` after the
    // very first dispatch — single shape can't evict itself.
    let hit_rate = hits as f64 / total as f64;
    assert!(
        hit_rate > 0.99,
        "hot mono-shape loop delta should have hit_rate > 0.99; got {hit_rate:.4} (Δ hits={hits} misses={misses}; aggregate hits={} misses={} toplevel_hits={} toplevel_misses={})",
        after.hits, after.misses, after.toplevel_hits, after.toplevel_misses,
    );
}

#[test]
fn hot_toplevel_loop_counts_fast_path_hits() {
    // 1000 calls to a user-defined toplevel `def helper` go
    // through `do_call(no_recv)`'s `lookup_toplevel_method_cache_hit`
    // fast path — distinct from the receiver-IC path covered
    // above. Without explicit instrumentation on the fast path,
    // these hits would silently bypass the counter and
    // `toplevel_hits` would stay at 0. Pin this contract so a
    // future dispatch refactor that splits the fast path can't
    // re-introduce the under-reporting bug.
    //
    // Same delta-snapshot pattern as `hot_loop_drives_ic_hit_rate_high`
    // so the assertion isolates the workload from any
    // preamble-time toplevel lookups.
    let mut rt = Runtime::new();
    let before = rt.ic_stats();
    let src = r#"
        def helper
            42
        end
        total = 0
        i = 0
        while i < 1000
            total += helper
            i += 1
        end
        total
    "#;
    let val = rt.eval(src, "<hot_toplevel>").expect("eval should succeed");
    assert!(matches!(val, rubyrs::Value::Int(_)));
    let after = rt.ic_stats();

    let delta_hits = after.toplevel_hits - before.toplevel_hits;
    let delta_misses = after.toplevel_misses - before.toplevel_misses;
    assert!(
        delta_hits >= 999,
        "fast-path toplevel hits must be counted; got Δ toplevel_hits={delta_hits} Δ toplevel_misses={delta_misses}",
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
