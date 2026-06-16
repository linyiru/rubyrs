//! Golden-corpus harness over kramdown's own test suite
//! (`tests/corpus`, vendored from kramdown 2.5.2).
//!
//! The contract under test is *right-or-declined*: for every corpus
//! case, rostdown must either produce byte-identical HTML or return
//! `Error::Declined`. A successful render that MISMATCHES the expected
//! HTML is a hard failure — that is exactly the silent corruption the
//! decline design exists to prevent. The pass count is the coverage
//! dashboard; growing it is task-by-task work, never a correctness
//! gamble.
//!
//! kramdown's harness runs cases without an `.options` sidecar as
//! `{auto_ids: false, footnote_nr: 1}` (test/test_files.rb:58) — i.e.
//! kramdown defaults plus auto_ids off, which is `Options::core()`.
//! Cases WITH an `.options` sidecar are skipped (non-default options).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rostdown::{Error, NoHighlight, Options, to_html};

// With `--features arena`, install the scoped bump allocator as THIS
// test binary's global allocator so the corpus runs through the LIVE
// arena path (bump → scope reset → result copy-out) — the path Miri
// can't cover (it doesn't execute `#[global_allocator]`). The
// right-or-declined assertions below then gate the live arena for
// byte-identity, and `-Zsanitizer=address` in CI gates it for memory UB.
#[cfg(feature = "arena")]
#[global_allocator]
static GLOBAL: rostdown::ScopedAlloc = rostdown::ScopedAlloc;

#[derive(Default)]
struct Tally {
    pass: usize,
    declined: usize,
    skipped: usize,
    total: usize,
}

fn collect_text_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("corpus dir readable");
    for entry in entries {
        let path = entry.expect("corpus entry").path();
        if path.is_dir() {
            collect_text_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "text") {
            out.push(path);
        }
    }
    out.sort();
}

#[test]
fn corpus_right_or_declined() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files = Vec::new();
    for sub in ["block", "span"] {
        collect_text_files(&corpus.join(sub), &mut files);
    }
    assert!(
        !files.is_empty(),
        "corpus missing — vendored testcases not found"
    );

    let mut by_dir: BTreeMap<String, Tally> = BTreeMap::new();
    let mut mismatches: Vec<String> = Vec::new();
    let opts = Options::core();

    for text_path in &files {
        let dir = text_path
            .parent()
            .and_then(|p| p.strip_prefix(&corpus).ok())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let tally = by_dir.entry(dir).or_default();
        tally.total += 1;

        // Non-default options or no expected-HTML pair: out of scope.
        if text_path.with_extension("options").exists() {
            tally.skipped += 1;
            continue;
        }
        let html_path = text_path.with_extension("html");
        let Ok(expected) = std::fs::read_to_string(&html_path) else {
            tally.skipped += 1;
            continue;
        };
        let Ok(src) = std::fs::read_to_string(text_path) else {
            tally.skipped += 1;
            continue;
        };

        match to_html(&src, &opts, &mut NoHighlight) {
            Ok(actual) if actual == expected => tally.pass += 1,
            Ok(actual) => {
                let name = text_path.strip_prefix(&corpus).unwrap_or(text_path);
                mismatches.push(format!(
                    "{}:\n--- expected ---\n{expected}--- actual ---\n{actual}",
                    name.display()
                ));
            }
            Err(Error::Declined(_)) => tally.declined += 1,
        }
    }

    let mut dashboard = String::new();
    let mut totals = Tally::default();
    for (dir, t) in &by_dir {
        let _ = writeln!(
            dashboard,
            "{dir:<42} {:>3} pass / {:>3} declined / {:>3} skipped / {:>3}",
            t.pass, t.declined, t.skipped, t.total
        );
        totals.pass += t.pass;
        totals.declined += t.declined;
        totals.skipped += t.skipped;
        totals.total += t.total;
    }
    let _ = writeln!(
        dashboard,
        "{:<42} {:>3} pass / {:>3} declined / {:>3} skipped / {:>3}",
        "TOTAL", totals.pass, totals.declined, totals.skipped, totals.total
    );
    println!("{dashboard}");

    assert!(
        mismatches.is_empty(),
        "rostdown produced WRONG output (must be byte-identical or declined) for {} case(s):\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}
