// Regression repro for reset()-vs-stale-state ICEs: run the whole
// diff corpus through ONE Runtime with reset() between inputs (the
// fuzz harness shape). Catch #1: refinements tables (63a1a66f).
// Catch #2: const_cache_flat holding ENV's freed ObjId across the
// heap truncation. Pre-fix either panicked mid-corpus.
use rubyrs::{Config, Runtime};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static PANICKED: AtomicBool = AtomicBool::new(false);

fn main() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICKED.store(true, Ordering::SeqCst);
        prev(info);
    }));
    let cfg = Config {
        fuel: Some(500_000),
        max_frames: Some(128),
        max_heap_objects: Some(4096),
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
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = rt.eval(&src, "fuzz.rb");
        }));
        if PANICKED.load(Ordering::SeqCst) {
            eprintln!("PANIC on {}", f.display());
            std::process::exit(1);
        }
        n += 1;
    }
    eprintln!("all {n} fixtures survived reset+eval cycling");
}
