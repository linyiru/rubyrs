//! Shared helpers for the fuzz targets.
//!
//! Each `fuzz_targets/*.rs` file is its own libfuzzer binary, so
//! anything they have in common either gets duplicated or hoisted
//! here. Today this module holds:
//!
//!   - `ensure_sandbox_cwd` — `require` I/O sandbox setup.
//!   - `Caps` + `run_with_caps` — the iteration body both targets
//!     reduce down to. New targets that want different cap
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
    /// `FUZZ_RT`. `RefCell<Option<...>>` lazy-inits on first call
    /// to `run_with_caps`, then every subsequent iteration takes
    /// `&mut Runtime` and calls `reset()` instead of paying the
    /// ~3-6 ms preamble rebuild every iter.
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

/// Build the Config the cached Runtime is constructed with AND
/// re-applied on every iteration. Factored out so construction
/// and the per-iter refresh use byte-identical values — drift
/// would let `caps` go stale or silently grow incompatible.
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

/// The full iteration body both fuzz targets share: sandbox the
/// cwd, UTF-8-gate the input, get-or-init the per-process cached
/// `Runtime`, rewind any user state from the previous iteration
/// via `Runtime::reset`, **re-apply the Config so `fuel` (and
/// every other resource budget) refills to the per-iter
/// target**, then evaluate. Ignores the `Result` (script errors
/// are expected; only Rust panics fail the iteration).
///
/// The `apply_config` refresh is load-bearing: `Runtime::reset`
/// is explicitly documented as preserving resource caps across
/// resets (a host configuring a tight sandbox stays in that
/// sandbox across calls — see PR #212's doc-comment). But
/// `fuel` is consumed monotonically by `vm.check_fuel` and is
/// NOT per-eval — across many cached-Runtime iterations the
/// counter exhausts, and every subsequent iter immediately
/// traps with `ResourceExhausted: "out of fuel"` while
/// libfuzzer reports them as "iterations" that ran. Without
/// the refresh the harness's `iter/sec` number looks healthy
/// but most iters are no-ops doing zero VM coverage. Caught on
/// PR #222 by Copilot's first review.
///
/// Pre-PR-#212: each iteration constructed a fresh `Runtime`,
/// paying ~3-6 ms of preamble parse + compile + execute. The
/// `Runtime::reset()` API added in PR #212 (benchmarked at
/// ~107× faster than a fresh Runtime on the headline workload)
/// lets the harness keep one Runtime and rewind between inputs.
/// Net effect: cargo-fuzz's iter/sec on the parse target goes
/// from ~2k/s to ~3k/s after the constant-work overhead of
/// libfuzzer's coverage-instrumented + ASan iteration becomes
/// the dominant cost.
///
/// `caps` is read on every call: once to seed the Runtime on
/// first construction (the closure passed to
/// `get_or_insert_with`) and again for the per-iter
/// `apply_config` refresh that runs BEFORE eval. In practice
/// both targets always pass the same constant (`Caps::tight()`
/// for parse, `Caps::loose()` for eval), so the distinction is
/// invisible. A future target that varies caps per call would
/// see the cap change apply on the SAME iter (apply_config runs
/// pre-eval) — no one-iteration delay.
pub fn run_with_caps(data: &[u8], caps: Caps) {
    ensure_sandbox_cwd();
    let source = match std::str::from_utf8(data) {
        Ok(s) => s,
        // `Runtime::eval` takes `&str` (UTF-8); skip non-UTF-8
        // bytes here. Ruby files CAN declare other source
        // encodings via `# encoding: ...` magic comments, but
        // rubyrs's embed API doesn't expose that path — covering
        // it would need a separate fuzz target.
        Err(_) => return,
    };
    FUZZ_RT.with(|cell| {
        let mut slot = cell.borrow_mut();
        let rt = slot.get_or_insert_with(|| Runtime::with_config(build_cfg(&caps)));
        // Rewind user state from the previous iteration. The
        // Runtime keeps its preamble bytecode, class tables,
        // method tables, and the resource caps; only the
        // per-eval state from the last `eval` (heap allocs,
        // user-interned symbols, user classes/constants/methods,
        // globals, ...) gets wiped. See PR #212's
        // `embed/reset.rs` for the full contract.
        rt.reset();
        // Refill `fuel` (and re-stamp every other resource cap)
        // before each user eval. Without this, the cached
        // Runtime exhausts fuel after a few iters and the rest
        // of the soak runs as no-ops. See the doc-comment above.
        rt.apply_config(build_cfg(&caps));
        let _ = rt.eval(source, "fuzz.rb");
    });
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
    // Every iteration cheaply re-confirms cwd is still inside the
    // rubyrs-fuzz-* prefix. Catches the failure mode where a future
    // fuzz target file forgets to call `ensure_sandbox_cwd` AT
    // STARTUP but lands here from some later code path — the
    // assertion fires before the unsandboxed `eval` would read
    // host files. Pattern check only (no syscall) — sub-microsecond.
    debug_assert!(
        std::env::current_dir()
            .map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rubyrs-fuzz-"))
            })
            .unwrap_or(false),
        "fuzz sandbox cwd lost between init and call site"
    );
}
