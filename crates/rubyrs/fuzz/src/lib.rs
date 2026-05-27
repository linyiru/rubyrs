//! Shared helpers for the fuzz targets.
//!
//! Each `fuzz_targets/*.rs` file is its own libfuzzer binary, so
//! anything they have in common either gets duplicated or hoisted
//! here. Today this module holds the sandbox-cwd helper; future
//! shared concerns (corpus prefilter, panic-hook wiring, etc.)
//! land here too.

use std::sync::OnceLock;

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
