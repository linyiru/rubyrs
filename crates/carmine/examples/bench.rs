//! Throughput benchmark: lex each sample in `$CARMINE_COV_DIR` with carmine
//! and report ns/lex + MB/s. Only lexers carmine handles NATIVELY (no
//! callback decline) are timed — the fair apples-to-apples set vs rouge.
//! Writes the native-tag list to `$CARMINE_COV_DIR/native_tags.txt` so the
//! Ruby side benchmarks rouge on exactly the same inputs.

use std::fs;
use std::time::Instant;

use carmine::{Lexer, LexerTable, NoCallbacks};

fn main() {
    let dir = std::env::var("CARMINE_COV_DIR").unwrap_or_else(|_| "/tmp/carmine_cov".into());
    let mut tags: Vec<String> = fs::read_dir(&dir)
        .expect("cov dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .into_string()
                .ok()
                .and_then(|n| n.strip_suffix(".table.json").map(str::to_string))
        })
        .collect();
    tags.sort();

    let mut native = Vec::new();
    let mut total_bytes = 0u64;
    let mut total_ns = 0u128;
    println!("tag\tbytes\tns_per_lex\tMB_per_s");
    for tag in &tags {
        let (Ok(tj), Ok(demo)) = (
            fs::read_to_string(format!("{dir}/{tag}.table.json")),
            fs::read_to_string(format!("{dir}/{tag}.demo")),
        ) else {
            continue;
        };
        let Ok(table) = LexerTable::from_json(&tj) else {
            continue;
        };
        // Native only: a single lex must succeed without a callback decline.
        {
            let mut lx = Lexer::new(&table);
            if lx.lex(&demo, &mut NoCallbacks).is_err() {
                continue;
            }
        }
        // Adaptive timing: grow iterations until ≥50ms elapsed.
        let mut n = 1u32;
        let ns = loop {
            let start = Instant::now();
            for _ in 0..n {
                let mut lx = Lexer::new(&table);
                let _ = lx.lex(&demo, &mut NoCallbacks);
            }
            let el = start.elapsed();
            if el.as_millis() >= 50 || n >= 1 << 22 {
                break el.as_nanos() / n as u128;
            }
            n *= 2;
        };
        let bytes = demo.len() as u64;
        let mbs = bytes as f64 / ns as f64 * 1000.0;
        println!("{tag}\t{bytes}\t{ns}\t{mbs:.1}");
        native.push(tag.clone());
        total_bytes += bytes;
        total_ns += ns;
    }
    fs::write(format!("{dir}/native_tags.txt"), native.join("\n")).ok();
    let agg = total_bytes as f64 / total_ns as f64 * 1000.0;
    eprintln!(
        "# carmine: {} native lexers, {} bytes, {} ns total, {:.1} MB/s aggregate",
        native.len(),
        total_bytes,
        total_ns,
        agg
    );
}
