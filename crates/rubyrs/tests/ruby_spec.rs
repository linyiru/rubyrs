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
    /// Stack so nested `describe` blocks restore the outer scope
    /// on exit. ruby/spec uses nested describes routinely
    /// (`describe Module#foo do ... describe "when X" do ...`);
    /// a flat single-value field would orphan every outer-level
    /// `it` after the inner describe closes.
    describe_stack: Vec<String>,
    examples_in_order: Vec<ExampleOutcome>,
}

impl ExampleTracker {
    fn push_describe(&mut self, name: String) {
        self.describe_stack.push(name);
    }
    fn pop_describe(&mut self) {
        // If pop fires with an empty stack, spec_helper.rb is
        // out of sync with the tracker — surface as a synthetic
        // failure rather than panic.
        if self.describe_stack.pop().is_none() {
            self.record_orphan_fail("describe-pop with empty stack (spec_helper bug)".into());
        }
    }
    /// "outer / inner" — joined by " / " for readable failure
    /// labels when nested describes are in play.
    fn describe_label(&self) -> String {
        if self.describe_stack.is_empty() {
            "<no describe>".to_string()
        } else {
            self.describe_stack.join(" / ")
        }
    }
    fn start_example(&mut self, name: String) {
        let describe = self.describe_label();
        self.examples_in_order.push(ExampleOutcome {
            describe, name,
            passes: vec![], fails: vec![],
        });
    }
    fn record_pass(&mut self, label: String) {
        if let Some(last) = self.examples_in_order.last_mut() {
            last.passes.push(label);
        } else {
            // Pass reported outside any `it` — e.g., somebody
            // called `assert_eq` in a describe body or at file
            // scope. Silently dropping it (the previous behaviour)
            // hides the misuse; surface as a synthetic failure
            // so CI sees it.
            self.record_orphan_fail(format!(
                "pass `{}` reported outside any it block", label
            ));
        }
    }
    fn record_fail(&mut self, message: String) {
        if let Some(last) = self.examples_in_order.last_mut() {
            last.fails.push(message);
        } else {
            // Same as `record_pass`'s out-of-scope arm, but the
            // payload was already a failure — surface verbatim
            // rather than dropping it on the floor.
            self.record_orphan_fail(format!(
                "fail outside any it block: {}", message
            ));
        }
    }
    /// Push a synthetic file-level example carrying a single
    /// failure. Reused for any state-machine misuse that
    /// shouldn't pollute a real example's outcome.
    fn record_orphan_fail(&mut self, message: String) {
        self.examples_in_order.push(ExampleOutcome {
            describe: self.describe_label(),
            name: "<orphan>".into(),
            passes: vec![],
            fails: vec![message],
        });
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
        rt.register_fn("__spec_describe_push", move |args| {
            let name = string_arg(args, 0).unwrap_or_else(|| "?".into());
            t.borrow_mut().push_describe(name);
            Ok(Value::Nil)
        });
    }
    {
        let t = tracker.clone();
        rt.register_fn("__spec_describe_pop", move |_args| {
            t.borrow_mut().pop_describe();
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
    // Feature-detection host fn for spec_helper's `bignum_enabled?`.
    // Spec files written for the bignum-on profile (e.g. those that
    // use `(10000**10).even?` to verify BigInt semantics) call
    // `bignum_it "..."` instead of `it "..."` — the helper drops the
    // body when bignum is off, so the same source compiles and runs
    // on both profiles without producing spurious failures from the
    // no-bignum `**` arm's `i64::saturating_pow` (which caps at
    // `i64::MAX`, making the saturated value happen to be odd and
    // breaking any "is this bignum literal even?" assertion).
    rt.register_fn("__spec_bignum_enabled", move |_args| {
        #[cfg(feature = "bignum")]
        { Ok(Value::Bool(true)) }
        #[cfg(not(feature = "bignum"))]
        { Ok(Value::Bool(false)) }
    });

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
        Some(Value::Str(s)) => Some(s.to_string_lossy()),
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
        t.push_describe("<file-level>".into());
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
                // Two real causes land here: either the `it`
                // body raised before the first matcher (and
                // `it`'s rescue would have logged that as a
                // fail — so if we still see zero, the body
                // truly never reached an assertion), or the
                // body completed without calling any matcher
                // at all (forgot to write the assert, empty
                // `do; end`, helper helper that doesn't
                // forward, …). Don't pretend to diagnose
                // which.
                eprintln!("       (no assertions ran — empty `it` body or pre-matcher uncaught error)");
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
