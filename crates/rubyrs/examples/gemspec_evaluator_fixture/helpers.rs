//! Shared helpers for the gemspec-evaluator example and its
//! CI-gated test mirror (`tests/embed/rubund_validation.rs`).
//!
//! Both artifacts pull this module via `#[path = "..."] mod helpers;`.
//! Keeping the rubund-shape Config in one place means a future
//! tweak (tightening a knob, adding a 5th capability) can't drift
//! between the example narrative and the CI gate.
//!
//! Ruby fixture sources (`version.rb`, `fakegem.gemspec`) live in
//! sibling files in this directory and are pulled via `include_str!`.

use std::path::Path;

use rubyrs::{Config, Runtime};

/// Single source of truth for the rubund-shape Runtime config
/// (allow_filesystem_io + scoped allowed_paths + load_paths seed).
/// Used by Phase 1 (gemspec eval) and Phase 2 (out-of-scope read).
/// Phase 3 uses `Runtime::new()` instead — host-fn panic-catch is
/// a baseline Runtime feature independent of sandbox config.
pub fn make_rt(gem_root: &Path) -> Runtime {
    Runtime::with_config(Config {
        // Capability gate ON — the gemspec uses `require`, which
        // is a load-class FS op.
        allow_filesystem_io: true,
        // Scope: only the gem root tree. Any read outside
        // (Phase 2 tries `/etc/passwd`) traps with IOError
        // before the syscall.
        allowed_paths: Some(vec![gem_root.to_path_buf()]),
        // Seed $LOAD_PATH with the gem's lib/ so
        // `require "fakegem/version"` resolves the bundled file
        // declaratively. No synthetic `$LOAD_PATH.unshift` as
        // the first eval.
        load_paths: Some(vec![gem_root.join("lib")]),
        ..Default::default()
    })
}
