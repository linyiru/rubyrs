//! Allocation profile of one `to_html` render (default features = no
//! arena, so every allocation hits the counting allocator). Buckets by
//! size to separate small text `String`s from larger node `Vec`s — this
//! sizes the win available to "flat-index nodes" (Stage 1) vs "borrow
//! text" (Stage 2) of the zero-copy plan.
//!
//! Run: cargo run -p rostdown --release --example allocs -- <corpus.md>

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
// Size buckets (bytes): text runs are small; Vec<Span>/Vec<Block> are
// cap*size_of(node) — larger.
static LE16: AtomicU64 = AtomicU64::new(0);
static LE48: AtomicU64 = AtomicU64::new(0);
static LE128: AtomicU64 = AtomicU64::new(0);
static GT128: AtomicU64 = AtomicU64::new(0);
static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ON.load(Ordering::Relaxed) {
            N.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
            let s = l.size();
            if s <= 16 {
                &LE16
            } else if s <= 48 {
                &LE48
            } else if s <= 128 {
                &LE128
            } else {
                &GT128
            }
            .fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn main() {
    use rostdown::{NoHighlight, Options};
    let path = std::env::args().nth(1).expect("usage: allocs <corpus.md>");
    let src = std::fs::read_to_string(&path).expect("read corpus");

    // Warm (one-time inits) then measure exactly one render.
    let _ = rostdown::to_html(&src, &Options::jekyll(), &mut NoHighlight);
    ON.store(true, Ordering::Relaxed);
    let html = rostdown::to_html(&src, &Options::jekyll(), &mut NoHighlight).unwrap();
    ON.store(false, Ordering::Relaxed);
    std::hint::black_box(&html);

    let n = N.load(Ordering::Relaxed);
    let small = LE16.load(Ordering::Relaxed) + LE48.load(Ordering::Relaxed);
    let large = LE128.load(Ordering::Relaxed) + GT128.load(Ordering::Relaxed);
    println!("total allocs : {n}   ({} KiB)", BYTES.load(Ordering::Relaxed) / 1024);
    println!("  <=16  : {}", LE16.load(Ordering::Relaxed));
    println!("  17-48 : {}", LE48.load(Ordering::Relaxed));
    println!("  49-128: {}", LE128.load(Ordering::Relaxed));
    println!("  >128  : {}", GT128.load(Ordering::Relaxed));
    println!(
        "small(<=48, ~text strings) : {small}  ({:.0}%)",
        100.0 * small as f64 / n as f64
    );
    println!(
        "large(>48,  ~node vecs)    : {large}  ({:.0}%)",
        100.0 * large as f64 / n as f64
    );
}

// Placeholder so the println compiles without exposing Span; size is
// printed via the buckets instead.
