// Several tests build synthetic Reports via `let mut r =
// Report::default(); r.field = ...;`. clippy's
// `field_reassign_with_default` lint would have us collapse to a
// struct-literal `..Default::default()` spread, but the test cases
// are clearer when each field's purpose is on its own line.
#![allow(clippy::field_reassign_with_default)]

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

/// Per-test unique temp directory. Created on construct, removed on
/// drop. Leaks on panic — acceptable: tests live under
/// `std::env::temp_dir()` and the OS cleans up.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rubyrs-gapscan-{}-{}-{n}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create tempdir");
        Self { path }
    }
    fn path(&self) -> &std::path::Path {
        &self.path
    }
    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

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
    // UnlessNode landed as Supported (syntax sugar for `if !`,
    // swapped branches inside ast.rs). Pinned here as a regression
    // guard.
    assert_eq!(classify("UnlessNode"), Classification::Supported);
    assert_eq!(classify("ArgumentsNode"), Classification::RidesAlong);
    assert_eq!(classify("RescueNode"), Classification::RidesAlong);
    assert_eq!(classify("BackReferenceReadNode"), Classification::Missing);
    assert_eq!(classify("ClassVariableWriteNode"), Classification::Missing);
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
    let file = report.files.first().expect("one file");
    assert_eq!(file.translatable_ratio(), 1.0);
    assert!(file.missing_classes.is_empty());
}

#[test]
fn fixture_exercises_exact_missing_class_set() {
    let report = scan_path(fixture("missing_features.rb"));
    let names: std::collections::BTreeSet<&str> = report
        .missing_sorted()
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    // Exact set, exact counts. ConstantPathWriteNode landed
    // (`Foo::Bar = expr`), so the fixture swapped it for
    // ClassVariableWriteNode (`@@count = 0`) — same shape of
    // tripwire, no Missing children, distinct from the other
    // two exemplars.
    let expected: std::collections::BTreeSet<&str> =
        ["BackReferenceReadNode", "AliasMethodNode", "ClassVariableWriteNode"]
            .into_iter()
            .collect();
    assert_eq!(names, expected, "Missing-class set drifted");
    for cls in ["BackReferenceReadNode", "AliasMethodNode", "ClassVariableWriteNode"] {
        let count = report.histogram.get(cls).map(|s| s.count).unwrap_or(0);
        assert_eq!(count, 1, "{cls} count");
    }
    // Sanity: at least one Supported node from the body (DefNode,
    // IntegerNode, etc.) — the test isn't trivially "everything missing".
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
    // Synthetic before/after: before has BackReferenceReadNode missing, after
    // does not — closed gap. After introduces ClassVariableWriteNode
    // — new gap.
    let mut before = Report::default();
    before.total_nodes = 10;
    before.histogram.insert(
        "BackReferenceReadNode".to_string(),
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
        "ClassVariableWriteNode".to_string(),
        rubyrs_gapscan::NodeStat {
            count: 2,
            ..Default::default()
        },
    );

    let d = diff(&before, &after);
    assert_eq!(d.supported_delta, 5);
    assert_eq!(d.missing_delta, -3);
    assert_eq!(d.closed_missing_classes, vec![("BackReferenceReadNode".to_string(), 5)]);
    assert_eq!(d.new_missing_classes, vec![("ClassVariableWriteNode".to_string(), 2)]);
}

#[test]
fn diff_honours_scan_time_classification_for_cross_version_runs() {
    // The UnlessNode-landing PR exposed this bug: diff() used to
    // re-classify both sides with today's classify(), so a feature
    // that moved a class from Missing → Supported between scans
    // looked like a no-op. Now NodeStat carries a frozen
    // `scan_time_classification`; this test pins the cross-version
    // semantics by constructing reports as if scanned by two
    // different gapscan binaries.
    use rubyrs_gapscan::NodeStat;
    let mut before = Report::default();
    before.total_nodes = 10;
    // `CallNode` is a stand-in here: imagine a node class that
    // counted as Missing at "before" scan time and was reclassified
    // Supported later (just like `UnlessNode` actually did between
    // PR #7 and master's unless landing). We force scan-time
    // Missing on the before side and Supported on the after side to
    // simulate that without depending on which classes are
    // currently Missing.
    before.histogram.insert(
        "CallNode".to_string(),
        NodeStat {
            count: 4,
            scan_time_classification: Some(Classification::Missing),
            ..Default::default()
        },
    );
    // The "after" scan is on the same source but a newer rubyrs:
    // same CallNode count, but now classified Supported.
    let mut after = Report::default();
    after.total_nodes = 10;
    after.histogram.insert(
        "CallNode".to_string(),
        NodeStat {
            count: 4,
            scan_time_classification: Some(Classification::Supported),
            ..Default::default()
        },
    );

    let d = diff(&before, &after);
    // Before: 4 missing, 0 supported.  After: 0 missing, 4 supported.
    assert_eq!(d.missing_delta, -4);
    assert_eq!(d.supported_delta, 4);
    assert_eq!(d.closed_missing_classes, vec![("CallNode".to_string(), 4)]);
    assert!(d.new_missing_classes.is_empty());
}

#[test]
fn scan_propagates_missing_root_path() {
    // PR #3 review #9: a missing root path used to silently produce
    // an empty report (exit 0) — both CLI and library callers had no
    // way to tell scanning didn't happen. Now it surfaces as an
    // io::Error from the entry point.
    let bogus = PathBuf::from("/this/path/almost/certainly/does/not/exist/rubyrs-gapscan-test");
    let err = scan(&bogus, &ScanOptions::default()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn parse_json_rejects_wrong_tool_tag() {
    let bad = r#"{"tool":"some-other-tool","schema_version":1}"#;
    let err = parse_json(bad).unwrap_err();
    assert!(err.contains("rubyrs-gapscan"), "{err}");
}

#[test]
fn parse_json_rejects_future_schema_version() {
    let body = format!(
        r#"{{"tool":"rubyrs-gapscan","schema_version":{}}}"#,
        rubyrs_gapscan::JSON_SCHEMA_VERSION + 1
    );
    let err = parse_json(&body).unwrap_err();
    assert!(err.contains("schema_version"), "{err}");
}

#[test]
fn parse_json_rejects_missing_envelope() {
    assert!(parse_json("{}").is_err());
    assert!(parse_json(r#"{"tool":"rubyrs-gapscan"}"#).is_err());
    assert!(parse_json(r#"{"schema_version":1}"#).is_err());
}

#[test]
fn scan_skips_spec_and_test_dirs_by_default() {
    // PR #3 round 3 review #19: the previous version of this test
    // only verified the flag could be flipped — it never invoked
    // scan(). Now actually build a tree with both kinds of skip-
    // candidate dirs and assert the file count differs.
    let td = TempDir::new("skip");
    td.write("app.rb", "puts 1\n");                // always counted
    td.write("spec/app_spec.rb", "puts 2\n");      // skipped by default
    td.write("test/app_test.rb", "puts 3\n");      // skipped by default
    td.write("lib/util.rb", "puts 4\n");           // always counted
    // Negative control: a dir whose name *contains* "test" but isn't
    // exactly `test` must NOT be skipped (the filter is exact-match).
    td.write("tester/aux.rb", "puts 5\n");

    let default_report = scan(td.path(), &ScanOptions::default()).unwrap();
    assert_eq!(default_report.files_scanned, 3, "expected app + lib/util + tester/aux");

    let mut all = ScanOptions::default();
    all.skip_tests = false;
    let full_report = scan(td.path(), &all).unwrap();
    assert_eq!(full_report.files_scanned, 5, "expected every .rb file");
}

#[test]
fn single_file_root_reports_filename_not_empty_path() {
    // PR #3 round 4 review #20: scanning a single .rb file as root
    // used to record an empty path in FileStat (because
    // strip_prefix(root, root) yields ""). Now falls back to the
    // bare filename so JSON / Markdown / diff output stays readable.
    let td = TempDir::new("single-file");
    let file = td.write("hello.rb", "puts 1\n");
    let report = scan(&file, &ScanOptions::default()).unwrap();
    assert_eq!(report.files.len(), 1);
    let path = &report.files[0].path;
    assert!(
        !path.as_os_str().is_empty(),
        "expected non-empty path, got {path:?}"
    );
    assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("hello.rb"));
}

#[test]
fn files_with_errors_uses_root_relative_paths() {
    // PR #3 round 3 review #18: previously stored absolute paths
    // while every other path field is root-relative. Mismatched
    // paths make JSON reports unstable across machines.
    let td = TempDir::new("rel-errs");
    // Write a syntactically broken file so Prism reports an error.
    td.write("bad.rb", "def\n");
    td.write("good.rb", "puts 1\n");
    let report = scan(td.path(), &ScanOptions::default()).unwrap();
    assert!(!report.files_with_errors.is_empty(), "expected parse error");
    for p in &report.files_with_errors {
        assert!(p.is_relative(), "expected relative, got {p:?}");
        assert!(
            !p.starts_with(td.path()),
            "still contains tempdir prefix: {p:?}"
        );
    }
}

// ---- symlink edge cases (PR #3 round 3 reviews #16 #17) ----
// Gated on `unix` because creating symlinks on Windows requires
// elevated privileges; the production code itself is portable.

#[cfg(unix)]
#[test]
fn scan_follows_symlink_to_root_rb_file() {
    // Review #16: a symlink whose target is an existing .rb file
    // used to fall through to `read_dir` and surface a misleading
    // error. Now `fs::metadata` follows the link.
    let td = TempDir::new("sym-file");
    let real = td.write("real.rb", "puts 1\n");
    let link = td.path().join("link.rb");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let report = scan(&link, &ScanOptions::default()).expect("scan via symlink");
    assert_eq!(report.files_scanned, 1);
    assert!(report.total_nodes > 0);
}

#[cfg(unix)]
#[test]
fn scan_dangling_symlink_root_still_errors() {
    // The flip side: switching to fs::metadata mustn't weaken the
    // loud-failure-on-missing-root guarantee that Review #9 added.
    let td = TempDir::new("sym-dangle");
    let link = td.path().join("dangling.rb");
    std::os::unix::fs::symlink(td.path().join("does-not-exist.rb"), &link).unwrap();
    let err = scan(&link, &ScanOptions::default()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn scan_does_not_follow_symlinked_directories() {
    // Review #17: a `vendor -> ..` cycle would have recursed
    // forever. We now skip symlinked dirs outright during walk.
    let td = TempDir::new("sym-cycle");
    td.write("a.rb", "puts 1\n");
    td.write("sub/b.rb", "puts 2\n");
    // Cycle: `loop` symlinks back to the root.
    std::os::unix::fs::symlink(td.path(), td.path().join("loop")).unwrap();
    // Should terminate quickly and only count the two real files.
    let report = scan(td.path(), &ScanOptions::default()).expect("scan completes");
    assert_eq!(
        report.files_scanned, 2,
        "expected 2 real files, got {} (files = {:?})",
        report.files_scanned,
        report.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}
