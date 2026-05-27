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
//! The generator only counts — it doesn't execute the specs. So
//! the rendered example count is "examples in the corpus", not a
//! freshness-checked passing count. Passing is gated separately
//! by `tests/ruby_spec.rs`, which runs alongside this test in the
//! full `cargo test -p rubyrs` invocation (and in CI); when both
//! tests are green together, the two numbers coincide. Running
//! `cargo test -p rubyrs --test spec_status` in isolation only
//! checks the markdown is up-to-date with the corpus.

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
    /// Number of `it "..." do` blocks present in the file.
    /// Examples are gated to all-pass separately by
    /// `tests/ruby_spec.rs`; this count is "examples in the
    /// corpus", and matches the passing count only when that
    /// test is also green (full `cargo test -p rubyrs` / CI).
    examples: usize,
    /// `# skipped (<category>):` traces, bucketed by category.
    skipped: BTreeMap<String, usize>,
}

fn summarize_file(path: &Path) -> SpecSummary {
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let file = path.file_name().unwrap().to_string_lossy().into_owned();
    summarize_src(file, &src)
}

fn summarize_src(file: String, src: &str) -> SpecSummary {
    let lines: Vec<&str> = src.lines().collect();
    let mut describe = None;
    let mut upstream = None;
    let mut examples = 0;
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();

        // `it "x" do` / `it 'x' do`. The corpus uses exactly this
        // shape (string + ` do`); `it` without a string, `fit`,
        // `xit`, etc. don't appear. We don't try to validate that
        // here — if a stray form ever lands, the `tests/ruby_spec.rs`
        // runner already counts in lockstep with this counter
        // (both follow the `it "..." do` convention) and would
        // diverge first.
        if trimmed.starts_with("it ") && trimmed.contains(" do") {
            examples += 1;
            i += 1;
            continue;
        }

        // `# skipped (<category>): ...`. The trailing `:` is
        // load-bearing — `polish.py` emits exactly that shape,
        // and we require it here so stray prose comments like
        // `# skipped (mock) for now, revisit later` don't slip
        // into the skipped-by-category table.
        if let Some(rest) = trimmed.strip_prefix("# skipped (")
            && let Some(end) = rest.find(')')
            && rest[end + 1..].starts_with(':')
        {
            let cat = rest[..end].to_string();
            *skipped.entry(cat).or_insert(0) += 1;
            i += 1;
            continue;
        }

        // First `describe "Foo#bar" do`.
        if describe.is_none() && trimmed.starts_with("describe ") {
            describe = first_quoted(trimmed);
            i += 1;
            continue;
        }

        // First `# Adapted from ruby/spec <path>` line, possibly
        // continued onto subsequent comment lines. Two continuation
        // shapes appear in the corpus:
        //   `# Adapted from ruby/spec a.rb +\n# shared/b.rb at ...`
        //   `# Adapted from ruby/spec a.rb\n# + core/x/b.rb at ...`
        // Greedily fold comment lines into one buffer until we hit
        // a non-comment line or a ` at ` terminator.
        if upstream.is_none()
            && let Some(rest) = trimmed.strip_prefix("# Adapted from ruby/spec ")
        {
            let mut buf = rest.trim().to_string();
            let mut j = i + 1;
            while !buf.contains(" at ") && j < lines.len() {
                let next = lines[j].trim_start();
                if let Some(more) = next.strip_prefix("# ") {
                    let more = more.trim();
                    if more.is_empty() {
                        break;
                    }
                    buf.push(' ');
                    buf.push_str(more);
                    j += 1;
                } else {
                    break;
                }
            }
            // Cut at the ` at ` terminator (date / version note).
            if let Some(idx) = buf.find(" at ") {
                buf.truncate(idx);
            }
            let cleaned = buf
                .trim_end_matches(" at")
                .trim_end_matches('.')
                .trim_end_matches(',')
                .trim_end_matches('+')
                .trim()
                .to_string();
            if !cleaned.is_empty() {
                upstream = Some(cleaned);
            }
        }
        i += 1;
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

/// Group label for a spec file. Tries the filename-prefix
/// lookup table first (deterministic, and our corpus filenames
/// are well-organised), then falls back to the describe string
/// head when it starts uppercase, then finally to a capitalised
/// filename prefix.
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
    out.push_str("The example count below is \"examples in the corpus\". Passing is\n");
    out.push_str("gated separately by `tests/ruby_spec.rs` (full `cargo test -p rubyrs`\n");
    out.push_str("/ CI); when both this test and `ruby_spec` are green together, the\n");
    out.push_str("count below is also the passing count.\n\n");

    out.push_str("## Summary\n\n");
    out.push_str("| Metric | Count |\n");
    out.push_str("|---|---|\n");
    out.push_str(&format!("| Files | {} |\n", total_files));
    out.push_str(&format!("| Examples in corpus | {} |\n", total_examples));
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

#[test]
fn skipped_counter_requires_colon_after_category() {
    // polish.py's documented shape is `# skipped (<cat>): <it line>`.
    // A stray comment that drops the `:` (e.g. a TODO-style note)
    // must NOT increment the skipped-by-category counter, or the
    // SPEC_STATUS.md numbers would drift from the polish convention.
    let src = "\
describe \"Foo\" do
  # skipped (mock): it \"calls observer\" do
  # skipped (mock) for now, revisit later
  # skipped (fixture): it \"uses MyArray\" do
  it \"works\" do
    assert_eq(1, 1)
  end
end
";
    let s = summarize_src("synthetic_spec.rb".into(), src);
    assert_eq!(s.examples, 1, "one `it` block");
    assert_eq!(s.skipped.get("mock").copied(), Some(1), "only the `:`-form mock counts");
    assert_eq!(s.skipped.get("fixture").copied(), Some(1));
}

#[test]
fn upstream_folds_multi_line_continuations() {
    // Trailing `+` on first line, then continuation on next.
    let src = "\
# Adapted from ruby/spec core/method/equal_value_spec.rb +
# shared/eql.rb at 2026-05 (subset).
describe \"Method#==\" do
end
";
    let s = summarize_src("method_equal_spec.rb".into(), src);
    assert_eq!(
        s.upstream.as_deref(),
        Some("core/method/equal_value_spec.rb + shared/eql.rb"),
    );

    // Leading `+ ` on the continuation line.
    let src2 = "\
# Adapted from ruby/spec core/basicobject/singleton_method_spec.rb
# + core/kernel/define_singleton_method_spec.rb at 2026-05.
describe \"def obj.name\" do
end
";
    let s2 = summarize_src("singleton_method_spec.rb".into(), src2);
    assert_eq!(
        s2.upstream.as_deref(),
        Some("core/basicobject/singleton_method_spec.rb + core/kernel/define_singleton_method_spec.rb"),
    );
}
