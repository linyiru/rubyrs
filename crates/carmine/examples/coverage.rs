//! carmine↔rouge coverage harness. For every rouge lexer, lex its demo
//! with carmine's extracted table and compare the token stream to rouge's
//! own (the golden). Inputs are produced by `tools/gen_coverage.rb` (run on
//! a Ruby with the rouge gem): per-tag `*.table.json` / `*.golden.json` /
//! `*.demo` + a `manifest.json`. Prints an aggregate + the divergence/error
//! lists so the drop-in-rouge work has a measurable, re-runnable baseline.
//!
//!   ruby crates/carmine/tools/gen_coverage.rb
//!   cargo run -p carmine --example coverage

use std::fs;

use carmine::{Error, Lexer, LexerTable, NoCallbacks};
use serde_json::Value as J;

// `CARMINE_COV_DIR` holds the self-contained `gen_coverage.rb` output:
// `manifest.json` + per-tag `*.table.json` / `*.golden.json` / `*.demo`.
fn cov_dir() -> String {
    std::env::var("CARMINE_COV_DIR").unwrap_or_else(|_| "/tmp/carmine_cov".into())
}

fn golden_pairs(v: &J) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap_or("").to_string(),
                p[1].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn main() {
    let manifest: J =
        serde_json::from_str(&fs::read_to_string(format!("{}/manifest.json", cov_dir())).unwrap()).unwrap();

    let (mut matched, mut diverge, mut callback, mut errored, mut no_table) = (0, 0, 0, 0, 0);
    let mut diverge_list: Vec<String> = vec![];
    let mut error_list: Vec<String> = vec![];
    let mut match_list: Vec<String> = vec![];

    for rec in manifest.as_array().unwrap() {
        let tag = rec["tag"].as_str().unwrap();
        let cov = cov_dir(); let table_path = format!("{cov}/{tag}.table.json");
        if !std::path::Path::new(&table_path).exists() {
            no_table += 1;
            continue;
        }
        let table = match LexerTable::from_json(&fs::read_to_string(&table_path).unwrap()) {
            Ok(t) => t,
            Err(e) => {
                errored += 1;
                error_list.push(format!("{tag} (table: {e})"));
                continue;
            }
        };
        let Ok(demo) = fs::read_to_string(format!("{cov}/{tag}.demo")) else {
            continue;
        };
        let golden = golden_pairs(
            &serde_json::from_str::<J>(&fs::read_to_string(format!("{cov}/{tag}.golden.json")).unwrap())
                .unwrap(),
        );
        let mut lexer = Lexer::new(&table);
        match lexer.lex(&demo, &mut NoCallbacks) {
            Err(Error::CallbackRequired { .. }) => callback += 1,
            Err(e) => {
                errored += 1;
                error_list.push(format!("{tag} (lex: {e})"));
            }
            Ok(toks) => {
                let got: Vec<(String, String)> = toks
                    .iter()
                    .map(|(t, v)| (table.token_name(*t).to_string(), v.clone()))
                    .collect();
                if got == golden {
                    matched += 1;
                    match_list.push(tag.to_string());
                } else {
                    diverge += 1;
                    // first mismatching index, for triage
                    let at = got
                        .iter()
                        .zip(golden.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(got.len().min(golden.len()));
                    diverge_list.push(format!(
                        "{tag} @{at} got={:?} exp={:?}",
                        got.get(at),
                        golden.get(at)
                    ));
                }
            }
        }
    }

    let total = matched + diverge + callback + errored + no_table;
    println!("=== carmine vs rouge coverage (over rouge demos) ===");
    println!(
        "total={total}  MATCH={matched}  DIVERGE={diverge}  callback-decline={callback}  error={errored}  no-table={no_table}"
    );
    println!("\n--- MATCH ({}) ---\n{}", match_list.len(), match_list.join(" "));
    println!("\n--- DIVERGE ({}) ---", diverge_list.len());
    for d in &diverge_list {
        println!("  {d}");
    }
    println!("\n--- ERROR ({}) ---", error_list.len());
    for e in &error_list {
        println!("  {e}");
    }
}
