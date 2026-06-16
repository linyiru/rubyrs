//! Optional scoped bump allocator (behind the `arena` feature).
//!
//! A single `to_html` is a flood of short-lived allocations (the
//! `Block`/`Span` tree) freed all at once, so when an embedder installs
//! [`ScopedAlloc`] as its `#[global_allocator]`, every allocation inside
//! `to_html`'s [`Scope`] becomes a pointer bump from a per-thread arena,
//! and the arena resets when the call finishes. Outside a scope (or when
//! `ScopedAlloc` is not installed) allocations forward to the system
//! allocator, so the scope primitives are inert and harmless.
//!
//! Ported verbatim (native subset) from rust-sass `src/arena.rs`; that
//! version carries the full Miri/ASan safety story. This is the library's
//! one `unsafe` module and only compiles under `--features arena`.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---- process-global arena-region registry (for dealloc routing) ----
const MAX_ARENAS: usize = 128;

struct Region {
    base: AtomicUsize,
    end: AtomicUsize,
}
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_REGION: Region = Region {
    base: AtomicUsize::new(0),
    end: AtomicUsize::new(0),
};
static REGION_SLOTS: AtomicUsize = AtomicUsize::new(0);
static REGIONS: [Region; MAX_ARENAS] = [ZERO_REGION; MAX_ARENAS];

fn register_region(base: usize, end: usize) -> bool {
    let idx = REGION_SLOTS.fetch_add(1, Ordering::Relaxed);
    if idx >= MAX_ARENAS {
        return false;
    }
    REGIONS[idx].base.store(base, Ordering::Relaxed);
    REGIONS[idx].end.store(end, Ordering::Relaxed);
    true
}

#[inline]
fn in_any_arena(p: usize) -> bool {
    let n = REGION_SLOTS.load(Ordering::Relaxed).min(MAX_ARENAS);
    for r in &REGIONS[..n] {
        let base = r.base.load(Ordering::Relaxed);
        if base != 0 && p >= base && p < r.end.load(Ordering::Relaxed) {
            return true;
        }
    }
    false
}

/// Pure bump arithmetic: align `cur` up, add `size`, check it fits.
fn bump_compute(cur: usize, align: usize, size: usize, end: usize) -> Option<(usize, usize)> {
    let aligned = cur.checked_add(align - 1)? & !(align - 1);
    let next = aligned.checked_add(size)?;
    (next <= end).then_some((aligned, next))
}

const ARENA_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2 GiB virtual (native)

struct ThreadState {
    base: Cell<*mut u8>,
    end: Cell<usize>,
    cursor: Cell<usize>,
    depth: Cell<u32>,
    reserve_failed: Cell<bool>,
}

impl ThreadState {
    const fn new() -> ThreadState {
        ThreadState {
            base: Cell::new(std::ptr::null_mut()),
            end: Cell::new(0),
            cursor: Cell::new(0),
            depth: Cell::new(0),
            reserve_failed: Cell::new(false),
        }
    }

    #[cold]
    fn reserve(&self) -> bool {
        let Ok(layout) = Layout::from_size_align(ARENA_SIZE, 4096) else {
            return false;
        };
        // SAFETY: non-zero size, 4096 is a valid power-of-two alignment.
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            return false;
        }
        if !register_region(p as usize, p as usize + ARENA_SIZE) {
            // SAFETY: p came from System.alloc with this same layout.
            unsafe { System.dealloc(p, layout) };
            return false;
        }
        self.base.set(p);
        self.end.set(p as usize + ARENA_SIZE);
        self.cursor.set(p as usize);
        true
    }
}

thread_local! {
    // const init: no lazy alloc, POD (no Drop) so accessing it from
    // inside the global allocator can't re-enter it.
    static TL: ThreadState = const { ThreadState::new() };
}

/// A scoped bump global allocator. Install it in the embedding binary or
/// cdylib (only meaningful with the `arena` feature):
///
/// ```ignore
/// #[global_allocator]
/// static ALLOC: rostdown::ScopedAlloc = rostdown::ScopedAlloc;
/// ```
///
/// Safe to install even if `to_html` is never called: with no active
/// scope every request goes straight to the system allocator.
pub struct ScopedAlloc;

unsafe impl GlobalAlloc for ScopedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TL.with(|tl| {
            if tl.depth.get() == 0 {
                return unsafe { System.alloc(layout) };
            }
            if tl.base.get().is_null() {
                if tl.reserve_failed.get() {
                    return unsafe { System.alloc(layout) };
                }
                if !tl.reserve() {
                    tl.reserve_failed.set(true);
                    return unsafe { System.alloc(layout) };
                }
            }
            match bump_compute(tl.cursor.get(), layout.align(), layout.size(), tl.end.get()) {
                Some((aligned, next)) => {
                    tl.cursor.set(next);
                    let base = tl.base.get();
                    // SAFETY: bump_compute keeps base <= aligned and the
                    // end in-bounds, so the offset is valid.
                    unsafe { base.add(aligned - base as usize) }
                }
                None => unsafe { System.alloc(layout) },
            }
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !in_any_arena(ptr as usize) {
            // SAFETY: not from any arena → it came from System.
            unsafe { System.dealloc(ptr, layout) };
        }
        // in-arena: no-op (reclaimed wholesale on reset)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Tail-grow in place when `ptr` is the most recent arena bump.
        let resized = TL.with(|tl| {
            if tl.depth.get() == 0 {
                return false;
            }
            let base = tl.base.get();
            if base.is_null() {
                return false;
            }
            let addr = ptr as usize;
            if addr < base as usize || addr + layout.size() != tl.cursor.get() {
                return false;
            }
            match addr.checked_add(new_size) {
                Some(new_end) if new_end <= tl.end.get() => {
                    tl.cursor.set(new_end);
                    true
                }
                _ => false,
            }
        });
        if resized {
            return ptr;
        }
        // SAFETY: same contract as the default realloc.
        unsafe {
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            let new_ptr = self.alloc(new_layout);
            if !new_ptr.is_null() {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
            new_ptr
        }
    }
}

/// RAII scope marker. On `drop` (panic / early-return path) it leaves the
/// scope and, if outermost, resets the arena. `to_html`'s success path
/// finishes manually (copy the result out, then [`reset`]) and
/// `mem::forget`s the guard.
pub struct Scope;

impl Scope {
    pub fn enter() -> Scope {
        TL.with(|tl| tl.depth.set(tl.depth.get() + 1));
        Scope
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if leave_no_reset() {
            reset();
        }
    }
}

/// Leave the current scope WITHOUT resetting; returns whether this was the
/// outermost scope (the only one allowed to reset).
pub fn leave_no_reset() -> bool {
    TL.with(|tl| {
        let d = tl.depth.get().saturating_sub(1);
        tl.depth.set(d);
        d == 0
    })
}

/// Reset the arena to empty (only when no scope is active).
pub fn reset() {
    TL.with(|tl| {
        if tl.depth.get() == 0 {
            tl.cursor.set(tl.base.get() as usize);
        }
    });
}
