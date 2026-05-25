//! rubyrs-gapscan — quantify how far a Ruby codebase sits from the
//! [`rubyrs`] supported subset.
//!
//! Walks every `.rb` file under a path with Prism, histograms node
//! classes, and classifies each class as Supported / RidesAlong /
//! Missing using the manifests rubyrs publishes
//! ([`rubyrs::SUPPORTED_PRISM_NODES`], [`rubyrs::RIDES_ALONG_PRISM_NODES`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/prism_codegen.rs"));

/// How a Prism node class relates to the rubyrs subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// rubyrs translates this directly via an `as_*_node` accessor.
    Supported,
    /// Consumed inline by a supported parent (e.g. `ArgumentsNode`
    /// under `CallNode`). Counts as translatable.
    RidesAlong,
    /// No code path in rubyrs handles this. Real gap.
    Missing,
}

/// Static lookup: how does `node_name` (a Prism class name) classify?
pub fn classify(node_name: &str) -> Classification {
    if rubyrs::SUPPORTED_PRISM_NODES.contains(&node_name) {
        Classification::Supported
    } else if rubyrs::RIDES_ALONG_PRISM_NODES.contains(&node_name) {
        Classification::RidesAlong
    } else {
        Classification::Missing
    }
}

/// Per-class histogram entry.
#[derive(Debug, Default, Clone)]
pub struct NodeStat {
    pub count: u64,
    /// Best-effort short source excerpt of the first occurrence.
    pub first_example: Option<String>,
    /// Path of the first file the class appeared in (relative).
    pub first_file: Option<PathBuf>,
}

/// Per-method call counts, split by call shape.
///
/// CallNode in Prism covers ordinary method invocation, operators
/// (`+`, `==`, `[]`), and bareword (implicit-self / top-level) calls
/// like `require "x"` or `attr_accessor :name`. The split matters
/// because *bareword* calls hide the gaps that look "supported" at
/// the syntactic level — `require`, `attr_*`, `include`, etc. all
/// parse fine and translate to CallNode, but rubyrs doesn't implement
/// any of them.
#[derive(Debug, Default, Clone, Copy)]
pub struct CallStat {
    /// Implicit-self / top-level call (no explicit receiver).
    pub bareword: u64,
    /// Explicit receiver: `foo.bar`, `Class.new`, `arr[0]`, etc.
    pub receiver: u64,
    /// Operator-like name (`+`, `==`, `<<`, `[]`, ...).
    pub operator: u64,
}

impl CallStat {
    pub fn total(&self) -> u64 {
        self.bareword + self.receiver + self.operator
    }
}

/// Method names treated as operators rather than plain methods. Used
/// to peel `1 + 2`-shaped calls off the bareword/receiver buckets.
const OPERATOR_NAMES: &[&str] = &[
    "+", "-", "*", "/", "%", "**", "==", "!=", "<", "<=", ">", ">=", "<=>",
    "<<", ">>", "&", "|", "^", "~", "!", "[]", "[]=", "===", "=~", "!~",
    "+@", "-@",
];

/// One scan result.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub root: PathBuf,
    pub files_scanned: u64,
    pub files_parse_errors: Vec<PathBuf>,
    pub total_nodes: u64,
    pub histogram: BTreeMap<String, NodeStat>,
    /// Per-method-name CallNode breakdown.
    pub calls: BTreeMap<String, CallStat>,
}

impl Report {
    pub fn supported_total(&self) -> u64 {
        self.histogram
            .iter()
            .filter(|(k, _)| classify(k) == Classification::Supported)
            .map(|(_, v)| v.count)
            .sum()
    }
    pub fn rides_along_total(&self) -> u64 {
        self.histogram
            .iter()
            .filter(|(k, _)| classify(k) == Classification::RidesAlong)
            .map(|(_, v)| v.count)
            .sum()
    }
    pub fn missing_total(&self) -> u64 {
        self.histogram
            .iter()
            .filter(|(k, _)| classify(k) == Classification::Missing)
            .map(|(_, v)| v.count)
            .sum()
    }
    /// Returns missing entries sorted by descending count.
    pub fn missing_sorted(&self) -> Vec<(&String, &NodeStat)> {
        let mut v: Vec<_> = self
            .histogram
            .iter()
            .filter(|(k, _)| classify(k) == Classification::Missing)
            .collect();
        v.sort_by(|(_, a), (_, b)| b.count.cmp(&a.count));
        v
    }
    /// Returns supported entries sorted by descending count.
    pub fn supported_sorted(&self) -> Vec<(&String, &NodeStat)> {
        let mut v: Vec<_> = self
            .histogram
            .iter()
            .filter(|(k, _)| classify(k) == Classification::Supported)
            .collect();
        v.sort_by(|(_, a), (_, b)| b.count.cmp(&a.count));
        v
    }

    /// Top-N bareword calls (no explicit receiver). Reveals
    /// semantically-unsupported builtins that AST-level analysis
    /// would mis-classify as Supported (require, attr_*, include).
    pub fn bareword_calls_sorted(&self) -> Vec<(&String, &CallStat)> {
        let mut v: Vec<_> = self
            .calls
            .iter()
            .filter(|(_, s)| s.bareword > 0)
            .collect();
        v.sort_by(|(_, a), (_, b)| b.bareword.cmp(&a.bareword));
        v
    }
    /// Top-N receiver calls. Useful for spotting heavy stdlib reliance
    /// (File.read, Regexp.quote, etc.).
    pub fn receiver_calls_sorted(&self) -> Vec<(&String, &CallStat)> {
        let mut v: Vec<_> = self
            .calls
            .iter()
            .filter(|(_, s)| s.receiver > 0)
            .collect();
        v.sort_by(|(_, a), (_, b)| b.receiver.cmp(&a.receiver));
        v
    }
}

/// Scan options.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Skip `spec/` and `test/` directories (default: true).
    pub skip_tests: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { skip_tests: true }
    }
}

/// Walk `root`, parse every reachable `.rb` file, return a [`Report`].
pub fn scan(root: &Path, opts: &ScanOptions) -> std::io::Result<Report> {
    let mut report = Report {
        root: root.to_path_buf(),
        ..Default::default()
    };
    let mut files = Vec::new();
    collect_ruby_files(root, opts, &mut files)?;
    files.sort();

    for file in files {
        report.files_scanned += 1;
        let src = match std::fs::read(&file) {
            Ok(b) => b,
            Err(_) => {
                report.files_parse_errors.push(file);
                continue;
            }
        };
        let parsed = ruby_prism::parse(&src);
        // ruby_prism::parse always returns a tree; syntax errors live
        // on parsed.errors(). For scan purposes we still walk the
        // partial tree but record the file if it had any errors.
        let had_errors = parsed.errors().count() > 0;
        if had_errors {
            report.files_parse_errors.push(file.clone());
        }
        let rel = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        let mut visitor = Histogrammer {
            histogram: &mut report.histogram,
            total: &mut report.total_nodes,
            calls: &mut report.calls,
            src: &src,
            file: &rel,
        };
        use ruby_prism::Visit;
        visitor.visit(&parsed.node());
    }

    Ok(report)
}

struct Histogrammer<'a> {
    histogram: &'a mut BTreeMap<String, NodeStat>,
    total: &'a mut u64,
    calls: &'a mut BTreeMap<String, CallStat>,
    src: &'a [u8],
    file: &'a Path,
}

impl<'a> Histogrammer<'a> {
    /// Extra hook the generated macro injects in the `visit_call_node`
    /// arm. Splits the call into bareword / receiver / operator and
    /// bumps the per-method counter.
    fn record_call(&mut self, node: &ruby_prism::CallNode<'_>) {
        let name_bytes = node.name().as_slice();
        let name = match std::str::from_utf8(name_bytes) {
            Ok(s) => s,
            Err(_) => return,
        };
        let stat = self.calls.entry(name.to_string()).or_default();
        let is_op = OPERATOR_NAMES.contains(&name);
        if is_op {
            stat.operator += 1;
        } else if node.receiver().is_none() {
            stat.bareword += 1;
        } else {
            stat.receiver += 1;
        }
    }

    /// Called by the generated `impl_full_visit_for!` macro for every
    /// Prism node visited. `class` is a static name (e.g. `"CallNode"`)
    /// so we never allocate on the hot path until inserting a new key.
    fn record(&mut self, class: &'static str, node: &ruby_prism::Node<'_>) {
        *self.total += 1;
        let entry = self.histogram.entry(class.to_string()).or_default();
        entry.count += 1;
        if entry.first_example.is_none() {
            let loc = node.location();
            let s = loc.start_offset();
            let e = loc.end_offset().min(self.src.len());
            let slice = &self.src[s..e];
            let excerpt: String = std::str::from_utf8(slice)
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(60)
                .collect();
            entry.first_example = Some(excerpt);
            entry.first_file = Some(self.file.to_path_buf());
        }
    }
}

// Generates a 151-arm `impl Visit` for Histogrammer — see build.rs
// rationale (the standard `visit_branch_node_enter` hook silently
// skips wrapper nodes like ArgumentsNode/RescueNode).
impl_full_visit_for!(Histogrammer<'_>);

fn collect_ruby_files(
    dir: &Path,
    opts: &ScanOptions,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    if dir.is_file() {
        if dir.extension().is_some_and(|e| e == "rb") {
            out.push(dir.to_path_buf());
        }
        return Ok(());
    }
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if opts.skip_tests && (name == "spec" || name == "test") {
                continue;
            }
            collect_ruby_files(&path, opts, out)?;
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path);
        }
    }
    Ok(())
}

// ---- text output ----

/// Render `report` as a human-readable plain-text summary.
pub fn render_text(report: &Report, top_missing: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "## Inventory: {}", report.root.display());
    let _ = writeln!(s, "Files scanned: {}", report.files_scanned);
    let _ = writeln!(s, "Files with parse errors: {}", report.files_parse_errors.len());
    let _ = writeln!(s, "Total AST nodes: {}", report.total_nodes);
    let _ = writeln!(s, "Unique node classes: {}", report.histogram.len());
    let total = report.total_nodes.max(1) as f64;
    let sup = report.supported_total();
    let ride = report.rides_along_total();
    let miss = report.missing_total();
    let _ = writeln!(
        s,
        "  Supported:   {sup:>8} ({:.2}%)",
        100.0 * sup as f64 / total
    );
    let _ = writeln!(
        s,
        "  RidesAlong:  {ride:>8} ({:.2}%)",
        100.0 * ride as f64 / total
    );
    let _ = writeln!(
        s,
        "  Missing:     {miss:>8} ({:.2}%)",
        100.0 * miss as f64 / total
    );
    let missing = report.missing_sorted();
    let _ = writeln!(s, "\n### Missing node classes ({} unique)", missing.len());
    let _ = writeln!(s, "{:<40} {:>10}   first example", "class", "count");
    let _ = writeln!(s, "{}", "-".repeat(96));
    for (cls, stat) in missing.iter().take(top_missing) {
        let ex = stat.first_example.as_deref().unwrap_or("");
        let _ = writeln!(s, "{cls:<40} {:>10}   {ex}", stat.count);
    }
    if missing.len() > top_missing {
        let _ = writeln!(s, "  ... {} more (use --top to widen)", missing.len() - top_missing);
    }

    // Method-call dimension: bareword calls are the eye-opener — many
    // of them are runtime gaps masquerading as "supported" CallNodes.
    let bareword = report.bareword_calls_sorted();
    let receiver = report.receiver_calls_sorted();
    let _ = writeln!(
        s,
        "\n### Top bareword (implicit-self) calls — semantic gaps hide here"
    );
    let _ = writeln!(s, "{:<40} {:>10}", "method", "count");
    let _ = writeln!(s, "{}", "-".repeat(56));
    for (name, stat) in bareword.iter().take(top_missing) {
        let _ = writeln!(s, "{name:<40} {:>10}", stat.bareword);
    }
    let _ = writeln!(s, "\n### Top receiver method calls");
    let _ = writeln!(s, "{:<40} {:>10}", "method", "count");
    let _ = writeln!(s, "{}", "-".repeat(56));
    for (name, stat) in receiver.iter().take(top_missing) {
        let _ = writeln!(s, "{name:<40} {:>10}", stat.receiver);
    }
    s
}

// ---- self-check exposed for tests / CLI --strict ----

/// Sanity: every class name we ever emit must be in `ALL_PRISM_NODES`.
/// Catches the case where Prism upgrades add a new node and our
/// data file is stale.
pub fn unknown_classes_in(report: &Report) -> Vec<String> {
    let known: BTreeSet<&str> = ALL_PRISM_NODES.iter().copied().collect();
    report
        .histogram
        .keys()
        .filter(|k| !known.contains(k.as_str()))
        .cloned()
        .collect()
}
