// Regression repro for the reset()-vs-refinements ICE: run the whole
// diff corpus through ONE Runtime with reset() between inputs (the
// fuzz harness shape). Pre-fix this panicked with proto_idx out of
// bounds the first time a refined method name was dispatched after
// refinements.rb + reset.
use rubyrs::{Config, Runtime};
use std::time::Duration;

fn main() {
    let cfg = Config {
        fuel: Some(50_000),
        max_frames: Some(64),
        max_heap_objects: Some(1024),
        max_value_bytes: Some(1 << 16),
        max_symbols: Some(1 << 14),
        deadline: Some(Duration::from_millis(500)),
        stress_gc: false,
        ..Default::default()
    };
    let mut rt = Runtime::with_config(cfg);
    let mut files: Vec<_> = std::fs::read_dir("crates/rubyrs/tests/diff").unwrap()
        .filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "rb").unwrap_or(false)).collect();
    files.sort();
    let mut n = 0;
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        rt.reset();
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = rt.eval(&src, "fuzz.rb");
        })).is_ok();
        if !ok {
            eprintln!("PANIC on {}", f.display());
            std::process::exit(1);
        }
        n += 1;
    }
    eprintln!("all {n} fixtures survived reset+eval cycling");
}
