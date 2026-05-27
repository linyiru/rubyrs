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
/// The `TempDir` value is intentionally leaked via `.keep()` —
/// the fuzz process owns this directory for its entire lifetime;
/// the OS reclaims it when the process exits.
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
    });
}
