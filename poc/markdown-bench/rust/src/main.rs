//! Self-timed markdown render benchmark.
//! Usage: md-bench-rust <pulldown|comrak|rostdown> <file> <iters>
//! Emits: "<engine>\t<ns_per_op>\t<mb_per_s>\t<out_bytes>" on stdout.

use std::time::Instant;

// TURBO build only (`--features turbo`): rostdown's scoped bump
// allocator (the `arena` feature). rostdown's to_html self-scopes, so
// its row reflects the realized arena number; pulldown/comrak never open
// a scope, so their allocations forward to System (stock behavior).
// The DEFAULT build installs no global allocator — every engine uses the
// System allocator, so the rostdown row is the stock zero-dep path.
#[cfg(feature = "turbo")]
#[global_allocator]
static A: rostdown::ScopedAlloc = rostdown::ScopedAlloc;

fn render_pulldown(src: &str) -> usize {
    use pulldown_cmark::{html, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TASKLISTS);
    // Fair fight: rostdown does kramdown smart typography by default, and
    // pulldown can too — so turn it on here (verified equivalent output:
    // ---→—, "x"→“x”, ...→…). pulldown still has NO heading auto-id
    // feature, which rostdown does — that asymmetry is real and remains.
    opts.insert(Options::ENABLE_SMART_PUNCTUATION);
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(src, opts));
    out.len()
}

fn render_comrak(src: &str) -> usize {
    use comrak::{markdown_to_html, Options};
    let mut opts = Options::default();
    opts.extension.table = true;
    opts.extension.strikethrough = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    markdown_to_html(src, &opts).len()
}

fn render_rostdown(src: &str) -> usize {
    use rostdown::{to_html, NoHighlight, Options};
    // jekyll() = GFM + auto_ids, the profile the gem accelerates.
    to_html(src, &Options::jekyll(), &mut NoHighlight)
        .map(|h| h.len())
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let engine = args.get(1).map(String::as_str).unwrap_or("");
    let path = args.get(2).expect("usage: md-bench-rust <engine> <file> <iters>");
    let iters: u32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(200);
    let src = std::fs::read_to_string(path).expect("read corpus");

    // Diagnostic: rostdown parse-vs-convert phase split.
    if engine == "rostdown-phases" {
        let (p, c) = rostdown::profile_phases(&src, &rostdown::Options::jekyll(), iters);
        let total = p + c;
        println!(
            "rostdown-phases\tparse={:.0}ns ({:.0}%)\tconvert={:.0}ns ({:.0}%)\ttotal={:.0}ns",
            p,
            100.0 * p / total,
            c,
            100.0 * c / total,
            total
        );
        return;
    }

    let render: fn(&str) -> usize = match engine {
        "pulldown" => render_pulldown,
        "comrak" => render_comrak,
        "rostdown" => render_rostdown,
        other => panic!("unknown engine: {other}"),
    };

    // Warmup.
    let mut out_bytes = 0;
    for _ in 0..(iters / 5).max(3) {
        out_bytes = render(&src);
    }

    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..iters {
        sink = sink.wrapping_add(render(&src));
    }
    let elapsed = start.elapsed();
    std::hint::black_box(sink);

    let ns_per_op = elapsed.as_nanos() as f64 / iters as f64;
    let mb_per_s = (src.len() as f64 * iters as f64) / elapsed.as_secs_f64() / 1.0e6;
    println!("{engine}\t{ns_per_op:.0}\t{mb_per_s:.1}\t{out_bytes}");
}
