//! Micro-runner for rubyrs's `spec/` directory.
//!
//! Loads each `spec/ruby/*_spec.rb` file under a fresh `Runtime`
//! that has `spec_helper.rb` pre-loaded plus a small set of
//! `__spec_*` host functions for reporting. Collects per-example
//! outcomes and asserts every example passes — anything else
//! (failure, uncaught exception, skip) is treated as a regression.
//!
//! This deliberately is not the full MSpec runner. The full
//! MSpec depends on Kernel#load / RSpec-style `.should ==` /
//! anonymous `Class.new { ... }` / mock libraries — most of
//! which are outside rubyrs's subset. Instead `spec_helper.rb`
//! provides function-style matchers (`assert_eq`,
//! `assert_raises`) and each spec file uses only the subset
//! features we ship. See `crates/rubyrs/spec/README.md`.
//!
//! What lands in this file:
//!   - Discovery of spec files via plain filesystem walk
//!   - Shared `ExampleTracker` between Ruby and Rust via host fns
//!   - Per-spec-file isolation: each file gets its own Runtime,
//!     so class definitions in one don't leak into the next
//!   - A failure summary covering all files before panicking,
//!     so CI sees every regression at once rather than just the
//!     first

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rubyrs::{Runtime, Value};

fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spec")
}

/// What happened to a single `it` block. `examples_in_order` on
/// the tracker preserves declaration order so the CI report
/// reads top-to-bottom of the source.
#[derive(Debug, Clone)]
struct ExampleOutcome {
    describe: String,
    name: String,
    // Multiple matchers can fire in one `it` body; we collect
    // every pass + fail message rather than collapsing to a
    // boolean. An `it` is considered passing only when there's
    // ≥1 pass AND zero fails.
    passes: Vec<String>,
    fails: Vec<String>,
}

impl ExampleOutcome {
    fn ok(&self) -> bool {
        !self.passes.is_empty() && self.fails.is_empty()
    }
    fn label(&self) -> String {
        format!("{} :: {}", self.describe, self.name)
    }
}

#[derive(Default, Debug)]
struct ExampleTracker {
    current_describe: Option<String>,
    examples_in_order: Vec<ExampleOutcome>,
}

impl ExampleTracker {
    fn start_describe(&mut self, name: String) {
        self.current_describe = Some(name);
    }
    fn start_example(&mut self, name: String) {
        let describe = self.current_describe.clone()
            .unwrap_or_else(|| "<no describe>".to_string());
        self.examples_in_order.push(ExampleOutcome {
            describe, name,
            passes: vec![], fails: vec![],
        });
    }
    fn record_pass(&mut self, label: String) {
        if let Some(last) = self.examples_in_order.last_mut() {
            last.passes.push(label);
        }
    }
    fn record_fail(&mut self, message: String) {
        if let Some(last) = self.examples_in_order.last_mut() {
            last.fails.push(message);
        }
    }
}

/// Set up the runtime: pre-load `spec_helper.rb`, register the
/// `__spec_*` host fns that the helper calls, return both the
/// runtime and a shared handle to the tracker so the caller can
/// read results back after `eval` returns.
fn make_runtime() -> (Runtime, Rc<RefCell<ExampleTracker>>) {
    let mut rt = Runtime::new();
    let tracker = Rc::new(RefCell::new(ExampleTracker::default()));

    {
        let t = tracker.clone();
        rt.register_fn("__spec_describe", move |args| {
            let name = string_arg(args, 0).unwrap_or_else(|| "?".into());
            t.borrow_mut().start_describe(name);
            Ok(Value::Nil)
        });
    }
    {
        let t = tracker.clone();
        rt.register_fn("__spec_it", move |args| {
            let name = string_arg(args, 0).unwrap_or_else(|| "?".into());
            t.borrow_mut().start_example(name);
            Ok(Value::Nil)
        });
    }
    {
        let t = tracker.clone();
        rt.register_fn("__spec_pass", move |args| {
            let label = string_arg(args, 0).unwrap_or_else(|| "pass".into());
            t.borrow_mut().record_pass(label);
            Ok(Value::Nil)
        });
    }
    {
        let t = tracker.clone();
        rt.register_fn("__spec_fail", move |args| {
            let msg = string_arg(args, 0).unwrap_or_else(|| "fail".into());
            t.borrow_mut().record_fail(msg);
            Ok(Value::Nil)
        });
    }

    let helper_src = fs::read_to_string(spec_dir().join("spec_helper.rb"))
        .expect("spec_helper.rb should exist next to ruby/ specs");
    rt.eval(&helper_src, "spec_helper.rb")
        .expect("spec_helper.rb must compile in rubyrs's subset");

    (rt, tracker)
}

/// Pull `args[idx]` as a Ruby `String` if possible. The helpers
/// always pass strings, but we degrade gracefully rather than
/// panic if a future spec author calls one with an unexpected
/// shape.
fn string_arg(args: &[Value], idx: usize) -> Option<String> {
    // RStr derefs to RefCell<String>; clone out the inner String
    // so we own it past the borrow.
    match args.get(idx) {
        Some(Value::Str(s)) => Some(s.borrow().clone()),
        _ => None,
    }
}

fn run_spec_file(path: &Path) -> Vec<ExampleOutcome> {
    let (mut rt, tracker) = make_runtime();
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let label = path.file_name().unwrap().to_string_lossy().into_owned();
    if let Err(trap) = rt.eval(&src, &label) {
        // Spec-file-level trap (syntax error, exception thrown
        // outside any `it` block, etc.). Surface as a synthetic
        // failing example so it shows up in the summary instead
        // of vanishing — otherwise an early parse error would
        // produce zero examples and look like the file was
        // empty.
        let mut t = tracker.borrow_mut();
        t.start_describe("<file-level>".into());
        t.start_example(label.clone());
        t.record_fail(format!("file-level trap: {}", rt.format_trap(&trap)));
    }
    tracker.borrow().examples_in_order.clone()
}

#[test]
fn ruby_spec_microrunner_all_examples_pass() {
    let dir = spec_dir().join("ruby");
    // Surface read_dir entry errors instead of swallowing via
    // `.filter_map(|e| e.ok())`. A permission-denied / I/O error
    // on a single entry would otherwise let the runner pass with
    // that file silently missing — same class of silent-skip as
    // PR #17's SETUP-row gap.
    let mut entries: Vec<PathBuf> = Vec::new();
    let reader = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {}", dir.display(), e));
    for entry in reader {
        let entry = entry.unwrap_or_else(|e| {
            panic!("read_dir entry in {} failed: {}", dir.display(), e)
        });
        let path = entry.path();
        // README + module docstring promise `*_spec.rb` only.
        // Accepting any `.rb` would silently execute future
        // sibling files (shared helpers, fixtures, work-in-
        // progress drafts) as specs the first time someone
        // adds one. Match the documented suffix exactly.
        let is_spec = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("_spec.rb"))
            .unwrap_or(false);
        if is_spec {
            entries.push(path);
        }
    }
    assert!(!entries.is_empty(), "no spec files found in {}", dir.display());

    // Stable order so failures are easy to scan in CI output.
    entries.sort();

    let mut total = 0;
    let mut failures: Vec<(PathBuf, ExampleOutcome)> = Vec::new();
    for path in &entries {
        let outcomes = run_spec_file(path);
        // A file that compiles cleanly but registers zero `it`
        // blocks is almost always a mistake (forgot to wrap in
        // `describe { }`, accidentally deleted everything,
        // copy-paste boilerplate without bodies). Treat as a
        // synthetic failure so it shows up in the per-file
        // report rather than silently inflating the file count
        // without contributing examples.
        if outcomes.is_empty() {
            failures.push((path.clone(), ExampleOutcome {
                describe: "<file-level>".into(),
                name: "spec file registered zero examples".into(),
                passes: vec![],
                fails: vec!["expected at least one `it` block; found none".into()],
            }));
            continue;
        }
        total += outcomes.len();
        for o in outcomes {
            if !o.ok() {
                failures.push((path.clone(), o));
            }
        }
    }

    if !failures.is_empty() {
        eprintln!("\nruby_spec micro-runner: {} of {} example(s) failed.\n",
            failures.len(), total);
        for (path, o) in &failures {
            let fname = path.file_name().unwrap().to_string_lossy();
            eprintln!("FAIL {}", fname);
            eprintln!("     {}", o.label());
            if o.passes.is_empty() && o.fails.is_empty() {
                eprintln!("       (no assertions ran — likely an uncaught error before any matcher)");
            }
            for msg in &o.fails {
                eprintln!("       fail: {}", msg);
            }
            eprintln!();
        }
        panic!("{} ruby/spec example(s) failed (see above)", failures.len());
    }

    println!("ruby_spec micro-runner: {} example(s) passed across {} file(s).",
        total, entries.len());
}
