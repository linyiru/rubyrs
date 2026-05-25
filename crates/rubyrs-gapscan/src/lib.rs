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

/// Per-file translatability summary.
#[derive(Debug, Default, Clone)]
pub struct FileStat {
    pub path: PathBuf,
    pub total: u64,
    pub supported: u64,
    pub rides_along: u64,
    pub missing: u64,
    /// Distinct Missing class names that appear in this file.
    pub missing_classes: BTreeSet<String>,
}

impl FileStat {
    pub fn translatable(&self) -> u64 {
        self.supported + self.rides_along
    }
    /// Fraction of nodes in the file that are Supported or RidesAlong.
    /// Empty files (`total == 0`) are treated as 1.0 (vacuously fully
    /// translatable).
    pub fn translatable_ratio(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.translatable() as f64 / self.total as f64
        }
    }
}

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
    /// Per-file translatability — populated even when `total == 0`.
    pub files: Vec<FileStat>,
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

    /// Files with at least `min_nodes` AST nodes whose translatable
    /// ratio is ≥ `threshold`. Sorted by descending ratio, then by
    /// descending total (bigger files first within the same ratio).
    /// `min_nodes` filters out trivial files (constants-only, etc.)
    /// so fixture candidates are non-degenerate.
    pub fn files_at_least(&self, threshold: f64, min_nodes: u64) -> Vec<&FileStat> {
        let mut v: Vec<&FileStat> = self
            .files
            .iter()
            .filter(|f| f.total >= min_nodes && f.translatable_ratio() >= threshold)
            .collect();
        v.sort_by(|a, b| {
            b.translatable_ratio()
                .partial_cmp(&a.translatable_ratio())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.total.cmp(&a.total))
        });
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
        let mut file_stat = FileStat {
            path: rel.clone(),
            ..Default::default()
        };
        let mut visitor = Histogrammer {
            histogram: &mut report.histogram,
            total: &mut report.total_nodes,
            calls: &mut report.calls,
            file_stat: &mut file_stat,
            src: &src,
            file: &rel,
        };
        use ruby_prism::Visit;
        visitor.visit(&parsed.node());
        report.files.push(file_stat);
    }

    Ok(report)
}

struct Histogrammer<'a> {
    histogram: &'a mut BTreeMap<String, NodeStat>,
    total: &'a mut u64,
    calls: &'a mut BTreeMap<String, CallStat>,
    file_stat: &'a mut FileStat,
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
        self.file_stat.total += 1;
        match classify(class) {
            Classification::Supported => self.file_stat.supported += 1,
            Classification::RidesAlong => self.file_stat.rides_along += 1,
            Classification::Missing => {
                self.file_stat.missing += 1;
                self.file_stat.missing_classes.insert(class.to_string());
            }
        }
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

    // Per-file translatability — fixture-candidate buckets.
    // min_nodes=20 excludes degenerate `version.rb`-style files where
    // 100% just means "two constants and a module declaration".
    let full = report.files_at_least(1.0, 20);
    let near = report.files_at_least(0.95, 20);
    let nontrivial: u64 = report
        .files
        .iter()
        .filter(|f| f.total >= 20)
        .count() as u64;
    let _ = writeln!(
        s,
        "\n### Per-file translatability (≥20 nodes: {nontrivial} non-trivial files)"
    );
    let _ = writeln!(
        s,
        "  100% translatable: {} files",
        full.len()
    );
    let _ = writeln!(
        s,
        "  ≥95% translatable: {} files (good fixture candidates)",
        near.len()
    );
    let show = top_missing.min(near.len());
    if show > 0 {
        let _ = writeln!(s, "\n  Top {show} fixture candidates (ratio × nodes):");
        for f in near.iter().take(show) {
            let miss = if f.missing_classes.is_empty() {
                String::new()
            } else {
                format!(
                    " — needs: {}",
                    f.missing_classes
                        .iter()
                        .take(4)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let _ = writeln!(
                s,
                "    {:>6.2}%  {:>5} nodes  {}{}",
                100.0 * f.translatable_ratio(),
                f.total,
                f.path.display(),
                miss
            );
        }
    }
    s
}

// ---- JSON I/O ----

/// Serialise a [`Report`] to JSON. Hand-built with `serde_json::Value`
/// to keep the schema explicit and avoid serde_derive's compile cost.
/// Schema is stable; bump `schema_version` on breaking changes.
pub fn render_json(report: &Report) -> String {
    use serde_json::{json, Value};
    let totals = json!({
        "supported": report.supported_total(),
        "rides_along": report.rides_along_total(),
        "missing": report.missing_total(),
    });
    let histogram: Vec<Value> = report
        .histogram
        .iter()
        .map(|(cls, stat)| {
            json!({
                "class": cls,
                "count": stat.count,
                "classification": classification_str(classify(cls)),
                "first_example": stat.first_example,
                "first_file": stat.first_file.as_ref().map(|p| p.display().to_string()),
            })
        })
        .collect();
    let calls: Vec<Value> = report
        .calls
        .iter()
        .map(|(name, stat)| {
            json!({
                "name": name,
                "bareword": stat.bareword,
                "receiver": stat.receiver,
                "operator": stat.operator,
            })
        })
        .collect();
    let files: Vec<Value> = report
        .files
        .iter()
        .map(|f| {
            json!({
                "path": f.path.display().to_string(),
                "total": f.total,
                "supported": f.supported,
                "rides_along": f.rides_along,
                "missing": f.missing,
                "missing_classes": f.missing_classes.iter().collect::<Vec<_>>(),
            })
        })
        .collect();
    let parse_errors: Vec<String> = report
        .files_parse_errors
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let doc = json!({
        "schema_version": 1,
        "tool": "rubyrs-gapscan",
        "root": report.root.display().to_string(),
        "files_scanned": report.files_scanned,
        "files_with_parse_errors": parse_errors,
        "total_nodes": report.total_nodes,
        "totals": totals,
        "histogram": histogram,
        "calls": calls,
        "files": files,
    });
    serde_json::to_string_pretty(&doc).expect("json serialise")
}

fn classification_str(c: Classification) -> &'static str {
    match c {
        Classification::Supported => "Supported",
        Classification::RidesAlong => "RidesAlong",
        Classification::Missing => "Missing",
    }
}

/// Parse a JSON report previously produced by [`render_json`].
///
/// Tolerant: missing optional fields default; we don't validate
/// histogram classifications since `classify()` rederives them.
pub fn parse_json(text: &str) -> Result<Report, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut report = Report::default();
    report.root = PathBuf::from(v["root"].as_str().unwrap_or(""));
    report.files_scanned = v["files_scanned"].as_u64().unwrap_or(0);
    report.total_nodes = v["total_nodes"].as_u64().unwrap_or(0);
    if let Some(arr) = v["files_with_parse_errors"].as_array() {
        for p in arr {
            if let Some(s) = p.as_str() {
                report.files_parse_errors.push(PathBuf::from(s));
            }
        }
    }
    if let Some(arr) = v["histogram"].as_array() {
        for item in arr {
            let class = item["class"].as_str().unwrap_or("").to_string();
            if class.is_empty() {
                continue;
            }
            let stat = NodeStat {
                count: item["count"].as_u64().unwrap_or(0),
                first_example: item["first_example"].as_str().map(|s| s.to_string()),
                first_file: item["first_file"].as_str().map(PathBuf::from),
            };
            report.histogram.insert(class, stat);
        }
    }
    if let Some(arr) = v["calls"].as_array() {
        for item in arr {
            let name = item["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let stat = CallStat {
                bareword: item["bareword"].as_u64().unwrap_or(0),
                receiver: item["receiver"].as_u64().unwrap_or(0),
                operator: item["operator"].as_u64().unwrap_or(0),
            };
            report.calls.insert(name, stat);
        }
    }
    if let Some(arr) = v["files"].as_array() {
        for item in arr {
            let path = PathBuf::from(item["path"].as_str().unwrap_or(""));
            let mut fs = FileStat {
                path,
                total: item["total"].as_u64().unwrap_or(0),
                supported: item["supported"].as_u64().unwrap_or(0),
                rides_along: item["rides_along"].as_u64().unwrap_or(0),
                missing: item["missing"].as_u64().unwrap_or(0),
                ..Default::default()
            };
            if let Some(mc) = item["missing_classes"].as_array() {
                for c in mc {
                    if let Some(s) = c.as_str() {
                        fs.missing_classes.insert(s.to_string());
                    }
                }
            }
            report.files.push(fs);
        }
    }
    Ok(report)
}

// ---- diff ----

/// Difference between two [`Report`]s. All numeric fields are
/// `after - before` (positive = "got worse" for missing/nodes,
/// "got better" for supported).
#[derive(Debug, Clone, Default)]
pub struct ReportDiff {
    pub before_root: PathBuf,
    pub after_root: PathBuf,
    pub total_nodes_delta: i64,
    pub supported_delta: i64,
    pub rides_along_delta: i64,
    pub missing_delta: i64,
    /// Classes that appear with count > 0 in `after` but were absent
    /// (or zero) in `before`. New gaps surfaced.
    pub new_missing_classes: Vec<(String, u64)>,
    /// Classes that were Missing in `before` and have count 0 in
    /// `after`. Closed gaps.
    pub closed_missing_classes: Vec<(String, u64)>,
    /// Bareword call deltas: (name, before, after, delta), sorted
    /// by absolute delta descending.
    pub bareword_call_changes: Vec<(String, u64, u64, i64)>,
}

pub fn diff(before: &Report, after: &Report) -> ReportDiff {
    let mut d = ReportDiff {
        before_root: before.root.clone(),
        after_root: after.root.clone(),
        total_nodes_delta: after.total_nodes as i64 - before.total_nodes as i64,
        supported_delta: after.supported_total() as i64 - before.supported_total() as i64,
        rides_along_delta: after.rides_along_total() as i64 - before.rides_along_total() as i64,
        missing_delta: after.missing_total() as i64 - before.missing_total() as i64,
        ..Default::default()
    };
    // Missing-class movements.
    let before_missing: BTreeMap<&String, u64> = before
        .histogram
        .iter()
        .filter(|(k, _)| classify(k) == Classification::Missing)
        .map(|(k, v)| (k, v.count))
        .collect();
    let after_missing: BTreeMap<&String, u64> = after
        .histogram
        .iter()
        .filter(|(k, _)| classify(k) == Classification::Missing)
        .map(|(k, v)| (k, v.count))
        .collect();
    for (k, &v) in &after_missing {
        if before_missing.get(k).copied().unwrap_or(0) == 0 {
            d.new_missing_classes.push((k.to_string(), v));
        }
    }
    for (k, &v) in &before_missing {
        if after_missing.get(k).copied().unwrap_or(0) == 0 {
            d.closed_missing_classes.push((k.to_string(), v));
        }
    }
    d.new_missing_classes.sort_by(|a, b| b.1.cmp(&a.1));
    d.closed_missing_classes.sort_by(|a, b| b.1.cmp(&a.1));

    // Bareword call deltas.
    let mut names: BTreeSet<&String> = BTreeSet::new();
    names.extend(before.calls.keys());
    names.extend(after.calls.keys());
    for name in names {
        let b = before.calls.get(name).map(|s| s.bareword).unwrap_or(0);
        let a = after.calls.get(name).map(|s| s.bareword).unwrap_or(0);
        if a != b {
            d.bareword_call_changes
                .push((name.clone(), b, a, a as i64 - b as i64));
        }
    }
    d.bareword_call_changes
        .sort_by(|x, y| y.3.abs().cmp(&x.3.abs()));
    d
}

/// Render a [`ReportDiff`] as text.
pub fn render_text_diff(d: &ReportDiff, top: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "## Diff");
    let _ = writeln!(s, "before: {}", d.before_root.display());
    let _ = writeln!(s, "after:  {}", d.after_root.display());
    let _ = writeln!(s, "\nTotals (after − before):");
    let _ = writeln!(s, "  total_nodes:   {:+}", d.total_nodes_delta);
    let _ = writeln!(s, "  supported:     {:+}", d.supported_delta);
    let _ = writeln!(s, "  rides_along:   {:+}", d.rides_along_delta);
    let _ = writeln!(s, "  missing:       {:+}", d.missing_delta);
    if !d.closed_missing_classes.is_empty() {
        let _ = writeln!(s, "\n### Closed gaps (now zero in after)");
        for (cls, was) in d.closed_missing_classes.iter().take(top) {
            let _ = writeln!(s, "  -{was:>6}  {cls}");
        }
    }
    if !d.new_missing_classes.is_empty() {
        let _ = writeln!(s, "\n### Newly-appearing missing classes");
        for (cls, is) in d.new_missing_classes.iter().take(top) {
            let _ = writeln!(s, "  +{is:>6}  {cls}");
        }
    }
    if !d.bareword_call_changes.is_empty() {
        let _ = writeln!(s, "\n### Bareword-call changes (top {top} by |delta|)");
        let _ = writeln!(s, "  {:<40} {:>8} {:>8} {:>8}", "method", "before", "after", "delta");
        for (n, b, a, dl) in d.bareword_call_changes.iter().take(top) {
            let _ = writeln!(s, "  {n:<40} {b:>8} {a:>8} {dl:>+8}");
        }
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
