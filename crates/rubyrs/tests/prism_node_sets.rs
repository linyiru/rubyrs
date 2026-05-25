//! Smoke tests for `SUPPORTED_PRISM_NODES` / `RIDES_ALONG_PRISM_NODES`.
//!
//! The serious correctness check — every entry corresponds to an
//! `as_*_node` call in ast.rs and vice versa — runs in build.rs and
//! fails the build, not the test suite. These tests assert the
//! lighter invariants the build script doesn't cover: shape, sort
//! order, and disjointness of the two re-exported slices.

use rubyrs::{RIDES_ALONG_PRISM_NODES, SUPPORTED_PRISM_NODES};

fn is_valid_prism_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.ends_with("Node")
        && name.chars().all(|c| c.is_ascii_alphanumeric())
}

#[test]
fn supported_set_is_nonempty_and_well_formed() {
    assert!(!SUPPORTED_PRISM_NODES.is_empty());
    for n in SUPPORTED_PRISM_NODES {
        assert!(is_valid_prism_name(n), "malformed entry: {n:?}");
    }
}

#[test]
fn rides_along_set_is_well_formed() {
    for n in RIDES_ALONG_PRISM_NODES {
        assert!(is_valid_prism_name(n), "malformed entry: {n:?}");
    }
}

#[test]
fn both_sets_are_sorted_and_deduped() {
    for slice in [SUPPORTED_PRISM_NODES, RIDES_ALONG_PRISM_NODES] {
        let sorted: Vec<&&str> = {
            let mut v: Vec<&&str> = slice.iter().collect();
            v.sort();
            v
        };
        let actual: Vec<&&str> = slice.iter().collect();
        assert_eq!(actual, sorted, "slice not sorted: {slice:?}");
        let unique: std::collections::BTreeSet<&&str> = slice.iter().collect();
        assert_eq!(unique.len(), slice.len(), "duplicate in slice: {slice:?}");
    }
}

#[test]
fn supported_and_rides_along_are_disjoint() {
    let s: std::collections::BTreeSet<&&str> = SUPPORTED_PRISM_NODES.iter().collect();
    let r: std::collections::BTreeSet<&&str> = RIDES_ALONG_PRISM_NODES.iter().collect();
    let overlap: Vec<_> = s.intersection(&r).collect();
    assert!(overlap.is_empty(), "overlap: {overlap:?}");
}
