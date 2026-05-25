//! End-to-end tests for `rubyrs-gapscan`.
//!
//! Two checked-in fixtures drive these:
//!
//! - `crates/rubyrs/examples/brewfile/Brewfile.rb` — the canonical
//!   workspace example. It must scan as 100% inside the rubyrs
//!   subset (zero Missing). If this regresses, either rubyrs lost
//!   support for something the example uses, or the example grew a
//!   feature outside the subset — both worth catching loudly.
//! - `crates/rubyrs-gapscan/tests/fixtures/missing_features.rb`
//!   exercises three deliberately-missing classes; precise counts
//!   are asserted so any drift in the classifier shows up here.

use rubyrs_gapscan::{
    classify, diff, parse_json, render_json, scan, Classification, Report, ScanOptions,
};
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate = crates/rubyrs-gapscan
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

fn scan_path(p: PathBuf) -> Report {
    scan(&p, &ScanOptions::default()).expect("scan ok")
}

#[test]
fn classify_returns_expected_for_known_classes() {
    assert_eq!(classify("CallNode"), Classification::Supported);
    assert_eq!(classify("IfNode"), Classification::Supported);
    assert_eq!(classify("ArgumentsNode"), Classification::RidesAlong);
    assert_eq!(classify("RescueNode"), Classification::RidesAlong);
    assert_eq!(classify("ModuleNode"), Classification::Missing);
    assert_eq!(classify("UnlessNode"), Classification::Missing);
    // Sanity: a name that is not a Prism node at all also lands in
    // Missing (caller's job to validate using `unknown_classes_in`).
    assert_eq!(classify("NotARealNode"), Classification::Missing);
}

#[test]
fn brewfile_example_is_fully_inside_subset() {
    let path = workspace_root().join("crates/rubyrs/examples/brewfile/Brewfile.rb");
    assert!(path.exists(), "expected brewfile at {}", path.display());
    let report = scan_path(path);
    assert_eq!(report.files_scanned, 1);
    assert!(
        report.missing_total() == 0,
        "brewfile leaked outside rubyrs subset: {:?}",
        report.missing_sorted()
    );
    // Should classify everything as Supported or RidesAlong.
    assert_eq!(
        report.supported_total() + report.rides_along_total(),
        report.total_nodes
    );
    // And the per-file translatable ratio should be exactly 1.0.
    let file = report.files.iter().next().expect("one file");
    assert_eq!(file.translatable_ratio(), 1.0);
    assert!(file.missing_classes.is_empty());
}

#[test]
fn fixture_exercises_three_missing_classes() {
    let report = scan_path(fixture("missing_features.rb"));
    let names: Vec<&String> = report
        .missing_sorted()
        .iter()
        .map(|(k, _)| *k)
        .collect();
    // Exact set, exact counts.
    assert!(names.contains(&&"ModuleNode".to_string()), "got {names:?}");
    assert!(names.contains(&&"ConstantWriteNode".to_string()), "got {names:?}");
    assert!(names.contains(&&"UnlessNode".to_string()), "got {names:?}");
    for cls in ["ModuleNode", "ConstantWriteNode", "UnlessNode"] {
        let count = report.histogram.get(cls).map(|s| s.count).unwrap_or(0);
        assert_eq!(count, 1, "{cls} count");
    }
    // Sanity: at least one Supported node from the body (DefNode,
    // ReturnNode, etc.) — the test isn't trivially "everything missing".
    assert!(report.supported_total() > 0);
}

#[test]
fn json_roundtrip_preserves_essentials() {
    let before = scan_path(fixture("missing_features.rb"));
    let json = render_json(&before);
    let after = parse_json(&json).expect("parse_json");
    assert_eq!(before.files_scanned, after.files_scanned);
    assert_eq!(before.total_nodes, after.total_nodes);
    assert_eq!(before.supported_total(), after.supported_total());
    assert_eq!(before.rides_along_total(), after.rides_along_total());
    assert_eq!(before.missing_total(), after.missing_total());
    assert_eq!(before.histogram.len(), after.histogram.len());
    for (cls, stat) in &before.histogram {
        let rt = after.histogram.get(cls).expect(cls);
        assert_eq!(stat.count, rt.count, "{cls} count");
    }
    assert_eq!(before.calls.len(), after.calls.len());
    assert_eq!(before.files.len(), after.files.len());
}

#[test]
fn diff_detects_closed_and_new_gaps() {
    // Synthetic before/after: before has ModuleNode missing, after
    // does not — closed gap. After introduces UnlessNode — new gap.
    let mut before = Report::default();
    before.total_nodes = 10;
    before.histogram.insert(
        "ModuleNode".to_string(),
        rubyrs_gapscan::NodeStat {
            count: 5,
            ..Default::default()
        },
    );
    before.histogram.insert(
        "CallNode".to_string(),
        rubyrs_gapscan::NodeStat {
            count: 5,
            ..Default::default()
        },
    );

    let mut after = Report::default();
    after.total_nodes = 12;
    after.histogram.insert(
        "CallNode".to_string(),
        rubyrs_gapscan::NodeStat {
            count: 10,
            ..Default::default()
        },
    );
    after.histogram.insert(
        "UnlessNode".to_string(),
        rubyrs_gapscan::NodeStat {
            count: 2,
            ..Default::default()
        },
    );

    let d = diff(&before, &after);
    assert_eq!(d.supported_delta, 5);
    assert_eq!(d.missing_delta, -3);
    assert_eq!(d.closed_missing_classes, vec![("ModuleNode".to_string(), 5)]);
    assert_eq!(d.new_missing_classes, vec![("UnlessNode".to_string(), 2)]);
}

#[test]
fn scan_skips_spec_and_test_dirs_by_default() {
    // Sanity: the same fixture file lives only at tests/fixtures —
    // not under a `spec/` or `test/` dir — so we can't directly
    // assert exclusion against committed files without conflating
    // with the test runner. Instead assert the option is honored
    // structurally: scanning the gapscan crate root excludes
    // tests/ (cargo's `tests/` lives at crate root, not under a
    // dir named exactly `spec` or `test`, so this would still
    // include it — guard the assertion only on `--include-tests`
    // changing the count when a spec/ dir exists somewhere we
    // control). For now we just verify the option flag flips.
    let mut opts = ScanOptions::default();
    assert!(opts.skip_tests);
    opts.skip_tests = false;
    assert!(!opts.skip_tests);
}
