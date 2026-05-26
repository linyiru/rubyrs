//! Auto-generates `docs/SPEC_STATUS.md` from the contents of
//! `crates/rubyrs/spec/ruby/*_spec.rb`.
//!
//! Why a test (not a binary): the same machinery already lives in
//! tests/ruby_spec.rs (filesystem walk + per-file scan), and CI
//! already runs `cargo test`. Bundling the check here gives us a
//! free freshness gate — if someone adds a spec file without
//! regenerating SPEC_STATUS.md, the test fails with a copy-paste
//! command to fix.
//!
//! Workflow:
//!   - Normal:  `cargo test -p rubyrs --test spec_status`
//!     → diffs in-memory generation against on-disk file.
//!   - Update:  `UPDATE_SPEC_STATUS=1 cargo test -p rubyrs --test spec_status`
//!     → rewrites docs/SPEC_STATUS.md.
//!
//! The generator only counts — it doesn't execute the specs. The
//! `it "..." do` line count IS the passing example count because
//! tests/ruby_spec.rs already gates that every example passes;
//! that test runs as part of the same `cargo test` invocation, so
//! by the time `cargo test` reports green, the count we emit here
//! is also the proven-passing count.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spec").join("ruby")
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/rubyrs/. Workspace root
    // is two levels up (crates/ → workspace).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root above crates/rubyrs/")
        .to_path_buf()
}

/// What we extract from a single spec file.
#[derive(Debug)]
struct SpecSummary {
    file: String,
    /// First `describe "..."` argument, if present. `None` for
    /// files that use a bare `describe Foo` (no string) or have
    /// no describe at all — those still count via filename
    /// grouping.
    describe: Option<String>,
    /// Upstream source path from the `# Adapted from ruby/spec
    /// <path>` provenance line, if present.
    upstream: Option<String>,
    /// Number of `it "..." do` blocks (= passing examples,
    /// since tests/ruby_spec.rs gates all-pass).
    examples: usize,
    /// `# skipped (<category>):` traces, bucketed by category.
    skipped: BTreeMap<String, usize>,
}

fn summarize_file(path: &Path) -> SpecSummary {
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let file = path.file_name().unwrap().to_string_lossy().into_owned();

    let mut describe = None;
    let mut upstream = None;
    let mut examples = 0;
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();

    for line in src.lines() {
        let trimmed = line.trim_start();

        // `it "..." do` — count of passing examples.
        // Match must be at statement start (not e.g. `# it ...`
        // inside a skip trace, which is the next branch).
        if trimmed.starts_with("it ") && trimmed.contains(" do") {
            // Cheap shape check: `it "x" do` or `it 'x' do`.
            // Anything weirder (it without a string, fit/xit)
            // isn't used in our corpus — flag if it ever appears.
            examples += 1;
            continue;
        }

        // `# skipped (<category>): ...`
        if let Some(rest) = trimmed.strip_prefix("# skipped (") {
            if let Some(end) = rest.find(')') {
                let cat = rest[..end].to_string();
                *skipped.entry(cat).or_insert(0) += 1;
            }
            continue;
        }

        // First `describe "Foo#bar" do`.
        if describe.is_none() && trimmed.starts_with("describe ") {
            // Extract the first quoted string on the line, if any.
            describe = first_quoted(trimmed);
            continue;
        }

        // First `# Adapted from ruby/spec <path>` line.
        if upstream.is_none()
            && let Some(rest) = trimmed.strip_prefix("# Adapted from ruby/spec ")
        {
            // Our headers continue with " at <date>" or " at"
            // on the next line. Strip those + a trailing period
            // or comma so the path comes out clean.
            let mut path_part = rest.trim();
            if let Some(idx) = path_part.find(" at ") {
                path_part = &path_part[..idx];
            }
            let path_part = path_part
                .trim_end_matches(" at")
                .trim_end_matches('.')
                .trim_end_matches(',')
                .trim();
            if !path_part.is_empty() {
                upstream = Some(path_part.to_string());
            }
        }
    }

    SpecSummary { file, describe, upstream, examples, skipped }
}

fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let q = bytes[i];
        if q == b'"' || q == b'\'' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != q {
                j += 1;
            }
            if j < bytes.len() {
                return Some(String::from_utf8_lossy(&bytes[start..j]).into_owned());
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Group label for a spec file. Prefers the describe string when
/// it cleanly names a class (`Foo#bar` / `Foo.bar` / `Foo::Bar`
/// with an uppercase first char); otherwise falls back to a
/// filename-prefix lookup table.
fn group_of(s: &SpecSummary) -> String {
    // Filename mapping wins over describe — describe strings
    // sometimes name a related class (e.g. unbound_method specs
    // describe `Class#instance_method`), and filenames in this
    // corpus are well-organised. Multi-token prefixes come first
    // so they beat their single-token shadows (e.g.
    // `unbound_method_` must beat the `method_` fallback).
    let stem = s.file.trim_end_matches("_spec.rb");
    let table: &[(&str, &str)] = &[
        ("unbound_method_", "UnboundMethod"),
        ("method_missing", "BasicObject"),
        ("singleton_method", "BasicObject"),
        ("instance_eval", "BasicObject"),
        ("class_eval", "Module"),
        ("define_method", "Module"),
        ("alias_method", "Module"),
        ("array_", "Array"),
        ("string_", "String"),
        ("integer_", "Integer"),
        ("hash_", "Hash"),
        ("method_", "Method"),
    ];
    for (prefix, label) in table {
        if stem.starts_with(prefix) || stem == prefix.trim_end_matches('_') {
            return (*label).to_string();
        }
    }
    // Filename didn't match the table — try the describe head if
    // it starts uppercase, otherwise capitalise the filename's
    // first underscore-token.
    if let Some(d) = &s.describe {
        let cutoff = d.find(['#', '.', ' ']);
        let head = match cutoff {
            Some(i) => &d[..i],
            None => d.as_str(),
        };
        let head = head.trim();
        if head.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return head.to_string();
        }
    }
    let prefix = stem.split('_').next().unwrap_or(stem);
    let mut chars = prefix.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => stem.to_string(),
    }
}

fn render_markdown(summaries: &[SpecSummary]) -> String {
    let total_examples: usize = summaries.iter().map(|s| s.examples).sum();
    let total_files = summaries.len();
    let total_skipped: usize = summaries
        .iter()
        .flat_map(|s| s.skipped.values())
        .sum();

    // Aggregate skipped by category across all files.
    let mut skipped_by_cat: BTreeMap<String, usize> = BTreeMap::new();
    for s in summaries {
        for (cat, n) in &s.skipped {
            *skipped_by_cat.entry(cat.clone()).or_insert(0) += n;
        }
    }

    // Group files by class label.
    let mut by_group: BTreeMap<String, Vec<&SpecSummary>> = BTreeMap::new();
    for s in summaries {
        by_group.entry(group_of(s)).or_default().push(s);
    }

    let mut out = String::new();
    out.push_str("# Spec status\n\n");
    out.push_str("Auto-generated by `cargo test -p rubyrs --test spec_status`.\n");
    out.push_str("Regenerate with `UPDATE_SPEC_STATUS=1 cargo test -p rubyrs --test spec_status`.\n\n");
    out.push_str("All examples below are gated by `tests/ruby_spec.rs` to pass on every\n");
    out.push_str("`cargo test` run — the example count IS the passing count.\n\n");

    out.push_str("## Summary\n\n");
    out.push_str("| Metric | Count |\n");
    out.push_str("|---|---|\n");
    out.push_str(&format!("| Files | {} |\n", total_files));
    out.push_str(&format!("| Passing examples | {} |\n", total_examples));
    out.push_str(&format!("| Skipped `it` traces | {} |\n", total_skipped));
    out.push('\n');

    if !skipped_by_cat.is_empty() {
        out.push_str("### Skipped traces by category\n\n");
        out.push_str("| Category | Count |\n");
        out.push_str("|---|---|\n");
        for (cat, n) in &skipped_by_cat {
            out.push_str(&format!("| `{}` | {} |\n", cat, n));
        }
        out.push('\n');
        out.push_str("Categories come from `crates/rubyrs-spec-extract/scripts/polish.py`'s\n");
        out.push_str("`DROP_PATTERNS`. Find blocks unlocked by a future feature with e.g.\n");
        out.push_str("`git grep \"# skipped (method-not-implemented)\"`.\n\n");
    }

    out.push_str("## By class\n\n");
    out.push_str("| Class | Files | Examples | Skipped |\n");
    out.push_str("|---|---|---|---|\n");
    for (group, files) in &by_group {
        let ex: usize = files.iter().map(|f| f.examples).sum();
        let sk: usize = files.iter().flat_map(|f| f.skipped.values()).sum();
        out.push_str(&format!("| {} | {} | {} | {} |\n", group, files.len(), ex, sk));
    }
    out.push('\n');

    out.push_str("## Files\n\n");
    out.push_str("| File | Describe | Upstream | Examples | Skipped |\n");
    out.push_str("|---|---|---|---|---|\n");
    for s in summaries {
        let desc = s.describe.as_deref().unwrap_or("");
        let up = s.upstream.as_deref().unwrap_or("");
        let sk: usize = s.skipped.values().sum();
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            s.file,
            desc,
            if up.is_empty() { String::new() } else { format!("`{}`", up) },
            s.examples,
            sk,
        ));
    }
    out.push('\n');

    out
}

fn collect_summaries() -> Vec<SpecSummary> {
    let dir = spec_dir();
    let mut paths: Vec<PathBuf> = Vec::new();
    let reader = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {}", dir.display(), e));
    for entry in reader {
        let entry = entry.unwrap_or_else(|e| {
            panic!("read_dir entry in {} failed: {}", dir.display(), e)
        });
        let path = entry.path();
        let is_spec = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("_spec.rb"))
            .unwrap_or(false);
        if is_spec {
            paths.push(path);
        }
    }
    paths.sort();
    paths.iter().map(|p| summarize_file(p)).collect()
}

#[test]
fn spec_status_is_up_to_date() {
    let summaries = collect_summaries();
    assert!(!summaries.is_empty(), "no spec files found");
    let generated = render_markdown(&summaries);
    let path = workspace_root().join("docs").join("SPEC_STATUS.md");

    if std::env::var_os("UPDATE_SPEC_STATUS").is_some() {
        fs::write(&path, &generated)
            .unwrap_or_else(|e| panic!("write {}: {}", path.display(), e));
        eprintln!("wrote {}", path.display());
        return;
    }

    let on_disk = fs::read_to_string(&path).unwrap_or_else(|_| String::new());
    if on_disk != generated {
        // Show a small head-diff hint, not the full file.
        let on_disk_head: String = on_disk.lines().take(10).collect::<Vec<_>>().join("\n");
        let gen_head: String = generated.lines().take(10).collect::<Vec<_>>().join("\n");
        panic!(
            "docs/SPEC_STATUS.md is stale.\n\
             Regenerate with:\n    \
             UPDATE_SPEC_STATUS=1 cargo test -p rubyrs --test spec_status\n\n\
             --- on disk (first 10 lines) ---\n{}\n\
             --- expected (first 10 lines) ---\n{}\n",
            on_disk_head, gen_head
        );
    }
}
