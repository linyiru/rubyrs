//! Shared helpers for the fuzz targets.
//!
//! Each `fuzz_targets/*.rs` file is its own libfuzzer binary, so
//! anything they have in common either gets duplicated or hoisted
//! here. Today this module holds:
//!
//!   - `ensure_sandbox_cwd` — `require` I/O sandbox setup.
//!   - `Caps` + `fuzz_init(caps)` + `run(data)` — the iteration
//!     body both targets reduce down to. `fuzz_init` seeds the
//!     cached `Runtime` once per process; `run` does the per-
//!     iter reset+eval. New targets that want different cap
//!     settings can add another `Caps` preset rather than
//!     reproducing the full UTF-8-gate + Config-build + eval shape.
//!
//! Future shared concerns (corpus prefilter, panic-hook wiring,
//! etc.) land here too.

use rubyrs::{Config, Runtime};
use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Duration;

thread_local! {
    /// Per-process cached Runtime. Each cargo-fuzz target compiles
    /// to its own binary, so each binary's process gets its own
    /// `FUZZ_RT`. `fuzz_init(caps)` seeds the slot once;
    /// subsequent `run(data)` calls take the Runtime out via
    /// `Option::take`, eval against it, and put it back — so the
    /// preamble (~3-6 ms) is paid once per process and every
    /// iter past the first reuses it.
    ///
    /// Lives in `thread_local!` because `Runtime` isn't `Send` /
    /// `Sync` (Rc<RefCell<...>> everywhere). libfuzzer is single-
    /// threaded so the `thread_local` is effectively process-wide.
    static FUZZ_RT: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

/// Resource caps a fuzz target applies to its `Runtime`. Named
/// presets (`Caps::tight`, `Caps::loose`) encode the parse-vs-eval
/// balance — tight gives parser + AST→IR more mutation surface per
/// CPU second, loose lets dispatch / GC / method lookup run for
/// longer per iteration.
///
/// `Copy` because the struct is three POD fields (~24 bytes) and
/// `fuzz_init` takes `Caps` by value (called once per process,
/// but every iter past the first ignores the arg cheaply since
/// `fuzz_init` is idempotent). The Copy lets fuzz_target! bodies
/// pass `Caps::tight()` / `Caps::loose()` without callers
/// reaching for `&` / `.clone()`.
#[derive(Copy, Clone)]
pub struct Caps {
    pub fuel: u64,
    pub max_frames: usize,
    pub max_heap_objects: usize,
}

impl Caps {
    /// Bias toward parser + AST→IR coverage. 50k ops covers
    /// preamble load (~30k as of 2026-05) + a few thousand ops of
    /// user code per iteration.
    pub const fn tight() -> Self {
        Caps {
            fuel: 50_000,
            max_frames: 64,
            max_heap_objects: 1024,
        }
    }

    /// Bias toward deeper VM dispatch / GC / method lookup
    /// coverage. 10× the tight budget; non-trivial user programs
    /// (small recursion, a few iterators) run to completion.
    pub const fn loose() -> Self {
        Caps {
            fuel: 500_000,
            max_frames: 128,
            max_heap_objects: 4096,
        }
    }
}

/// Build the Config used to construct the cached Runtime.
/// Called once per fuzz process inside `fuzz_init`.
/// `Config::fuel` is per-eval (re-anchored by `Runtime::eval`
/// from `Runtime::fuel_budget` on every call), so the harness
/// doesn't need to re-stamp the budget per iteration.
fn build_cfg(caps: &Caps) -> Config {
    Config {
        fuel: Some(caps.fuel),
        max_frames: Some(caps.max_frames),
        max_heap_objects: Some(caps.max_heap_objects),
        // Cross-target invariants — value / symbol / time bounds
        // that defend the fuzz process against runaway scripts.
        // Same numbers for both targets because they aren't what
        // parse-vs-eval is biasing on.
        max_value_bytes: Some(1 << 16),
        max_symbols: Some(1 << 14),
        deadline: Some(Duration::from_millis(500)),
        // `Config::default()` reads `STRESS_GC` on non-wasi
        // hosts. The fuzz process inherits the runner's env, and
        // STRESS_GC=1 is endemic in this repo's test culture
        // (CI runs every PR twice, once stressed). Pin it off so
        // the harness's throughput is environment-independent.
        stress_gc: false,
        ..Default::default()
    }
}

/// One-time per-process setup: construct the cached `Runtime`
/// seeded with `caps`, and ensure the filesystem sandbox is in
/// place. Idempotent — call from every iteration of
/// `fuzz_target!`; only the first call does the work, every
/// subsequent call early-returns. Splitting this from `run`
/// makes the once-per-process semantic explicit at the API
/// boundary: previous shape (`run_with_caps(data, caps)`)
/// silently ignored every call's `caps` after the first,
/// which is a footgun if a future target tries to vary caps
/// per call.
///
/// Pre-PR-#212: each iteration constructed a fresh `Runtime`,
/// paying ~3-6 ms of preamble parse + compile + execute. The
/// `Runtime::reset()` API added in PR #212 (benchmarked at
/// ~107× faster than a fresh Runtime on the headline workload)
/// lets the harness keep one Runtime and rewind between inputs.
/// Combined with PR #244's `ensure_sandbox_cwd` syscall removal,
/// the parse target now does ~10k iter/sec.
pub fn fuzz_init(caps: Caps) {
    ensure_sandbox_cwd();
    FUZZ_RT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(Runtime::with_config(build_cfg(&caps)));
        }
    });
}

/// Per-iteration body: UTF-8-gate the input, take the cached
/// `Runtime` out of `FUZZ_RT`, reset + eval, put it back.
/// Ignores the `Result` (script errors are expected; only Rust
/// panics fail the iteration).
///
/// `Option::take`-ing the Runtime out of the `RefCell` during
/// eval drops the outer `borrow_mut` before `Runtime::eval`
/// starts. A future `host_fn` registered on the cached Runtime
/// that captures `FUZZ_RT` and reaches for it from inside
/// script-callable Rust code can now `borrow_mut` the cell
/// successfully — but will find the slot **empty** (`None`),
/// because the active Runtime is held in this stack frame's
/// local for the duration of eval. The host_fn callback must
/// handle that — e.g. early-return — rather than expecting a
/// Runtime to be present.
///
/// Pre-PR the outer `borrow_mut` was held for the whole eval
/// scope, so any such re-entrancy would panic with
/// `already borrowed`. The new shape trades that panic for an
/// explicit "slot is empty mid-eval" contract, which is
/// recoverable by the caller.
///
/// Fuel handling note: `Config::fuel` is per-eval — every
/// `Runtime::eval` re-anchors `vm.fuel` from the host's
/// configured ceiling at entry. The harness's `caps.fuel`
/// therefore applies fresh to every iteration's eval without
/// any explicit refill step. (Pre-PR-#236 fuel was lifetime-
/// cumulative on the cached Runtime; the per-eval refactor on
/// PR #236 closed the leak at the source.)
///
/// Panics if called before `fuzz_init`; that's a structural
/// bug, not a runtime concern.
pub fn run(data: &[u8]) {
    let source = match std::str::from_utf8(data) {
        Ok(s) => s,
        // `Runtime::eval` takes `&str` (UTF-8); skip non-UTF-8
        // bytes here. Ruby files CAN declare other source
        // encodings via `# encoding: ...` magic comments, but
        // rubyrs's embed API doesn't expose that path — covering
        // it would need a separate fuzz target.
        Err(_) => return,
    };
    // `Option::take` the Runtime out of FUZZ_RT for the duration
    // of eval. Any re-entrant access to FUZZ_RT (today: none, no
    // host_fn is registered; future-proof: a host_fn capturing
    // FUZZ_RT could borrow_mut the cell while we hold rt as a
    // local) sees an EMPTY SLOT, not an already-borrowed RefCell
    // — see the run() doc for the resulting contract.
    let mut rt = FUZZ_RT.with(|cell| {
        cell.borrow_mut()
            .take()
            .expect("rubyrs_fuzz::run called before fuzz_init")
    });
    // Rewind user state from the previous iteration. The Runtime
    // keeps its preamble bytecode, class tables, method tables,
    // host_fns, and the resource caps' configured ceiling values;
    // only the per-eval state from the last `eval` (heap allocs,
    // user-interned symbols, user classes/constants/methods,
    // globals, ...) gets wiped. See PR #212's `embed/reset.rs`
    // for the full contract.
    rt.reset();
    let _ = rt.eval(source, "fuzz.rb");
    // Put the Runtime back so the next iter can reuse it. If
    // `eval` Rust-panicked above, this line never runs and the
    // Runtime is dropped during unwinding — fine, because
    // libfuzzer treats panics as crash findings and exits the
    // process; there is no next iter on this process.
    FUZZ_RT.with(|cell| *cell.borrow_mut() = Some(rt));
}

/// Move the fuzz process cwd into a fresh, unpredictable tempdir
/// once at startup so that any `require '<relative>'` /
/// `require_relative '...'` the script tries to invoke can't
/// reach into the runner's filesystem. Without this, the script
/// could read arbitrary host files via
/// `std::fs::read_to_string`, which (a) bypasses the
/// fuel / deadline / max_value_bytes accounting since file I/O
/// happens before any ops dispatch, (b) makes iterations
/// non-deterministic because the result depends on host FS
/// state, and (c) introduces I/O latency that crowds out exec/s.
///
/// Uses `tempfile::TempDir` so the path is unpredictable and
/// created with O_EXCL semantics — closes the symlink-pre-creation
/// TOCTOU window a `temp_dir().join(format!("rubyrs-fuzz-{pid}"))`
/// + `create_dir_all` shape would leave open on shared
/// multi-user systems. Not a realistic threat on GitHub's
/// single-user ephemeral runners, but the safer construction is
/// one line longer so there's no reason not to pay it.
///
/// The `TempDir` value is intentionally leaked via `.keep()` so
/// the fuzz process owns the directory for its entire lifetime —
/// libfuzzer often exits via `abort()` (a real crash, a SIGABRT
/// from an ASan finding, an OOM), and a held `TempDir`'s `Drop`
/// would not run on those paths anyway. The trade-off: the
/// `rubyrs-fuzz-*` directory persists on disk after the fuzz
/// process ends. On CI's ephemeral runner that's a non-issue
/// (the whole filesystem is discarded with the runner). Locally,
/// `/tmp` is reaped by age — systemd-tmpfiles on Linux (default
/// 10-day floor for `/tmp`) and the periodic launchd job on
/// macOS — not by name prefix; the leftover dirs sit until that
/// age threshold hits. Each fuzz process gets a fresh random
/// suffix from `tempfile`, so leftover dirs don't conflict across
/// runs, but a long-running developer machine will accumulate
/// them between reboots.
pub fn ensure_sandbox_cwd() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("rubyrs-fuzz-")
            .tempdir()
            .expect("ICE: fuzz sandbox tempdir creation failed");
        let path = dir.keep();
        std::env::set_current_dir(&path)
            .expect("ICE: fuzz sandbox set_current_dir failed");
        // Belt-and-braces: confirm the cwd actually moved.
        // `set_current_dir` returning Ok doesn't fully guarantee
        // it on every kernel/FS combo (NFS edge cases, sandbox
        // policies on macOS) and we'd rather fail fuzzing loudly
        // than run the rest of the corpus against the runner FS.
        let cwd = std::env::current_dir()
            .expect("ICE: current_dir lookup failed after sandbox set");
        assert_eq!(
            cwd.canonicalize().ok().as_deref(),
            path.canonicalize().ok().as_deref(),
            "ICE: fuzz sandbox cwd did not stick — expected {path:?}, got {cwd:?}"
        );
    });
    // No per-call drift check: the OnceLock-time assertion above
    // catches the realistic failure mode (cwd setup never stuck),
    // and rubyrs exposes no script-reachable `Dir.chdir` mutation
    // path that would let a fuzz input move cwd post-init.
}
