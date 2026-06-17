//! Real-content decline analysis: render every file passed on argv
//! through rostdown's jekyll profile and tally acceptance + the
//! decline-reason histogram. Usage: decline_scan <file>...
use rostdown::{to_html, Error, NoHighlight, Options};
use std::collections::BTreeMap;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let opts = Options::jekyll();

    let mut accepted = 0usize;
    let mut read_err = 0usize;
    let mut reasons: BTreeMap<&'static str, (usize, String)> = BTreeMap::new(); // reason -> (count, first example)
    let mut total = 0usize;

    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => {
                read_err += 1;
                continue;
            }
        };
        total += 1;
        match to_html(&src, &opts, &mut NoHighlight) {
            Ok(_) => accepted += 1,
            Err(Error::Declined(reason)) => {
                let e = reasons.entry(reason).or_insert((0, f.clone()));
                e.0 += 1;
            }
        }
    }

    let declined: usize = reasons.values().map(|(c, _)| c).sum();
    println!("files read:   {total}  (skipped unreadable: {read_err})");
    println!(
        "accepted:     {accepted}  ({:.1}%)",
        100.0 * accepted as f64 / total.max(1) as f64
    );
    println!(
        "declined:     {declined}  ({:.1}%)",
        100.0 * declined as f64 / total.max(1) as f64
    );
    println!("\ndecline reasons (most frequent first):");

    // sort by count desc
    let mut v: Vec<(&&str, &(usize, String))> = reasons.iter().collect();
    v.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    for (reason, (count, example)) in v {
        println!("  {count:>4}  {reason:<28} e.g. {example}");
    }
}
