//! `require 'X'` search-path leniency — caller-dir + caller-
//! parent-dir fallback for cross-package lookups in
//! co-located source trees.
//!
//! Why not a `diff_cruby` fixture: rubyrs's lookup walks
//! caller-source-file's directory + parent directly, while
//! CRuby's `require` walks `$LOAD_PATH` which rubyrs doesn't
//! currently expose as an Array. CRuby would need
//! `$LOAD_PATH.unshift __dir__` to reach the same files;
//! rubyrs would just no-op on that line because `$LOAD_PATH`
//! is `nil`. The asymmetric setup is the wrong shape for
//! diff_cruby (which compares stdout byte-for-byte after
//! identical script invocations). Rust integration test
//! instead — exercises the load path directly, asserts the
//! expected output the loader script produces.
//!
//! Layout under `tests/diff/require_xpkg/`:
//!   - sinatra/loader.rb     (entry — runs the requires + prints)
//!   - sinatra/helpers.rb    (sibling — `require 'helpers'`)
//!   - rack/show_exceptions.rb  (cross-package — `require 'rack/show_exceptions'`)
//!   - rack/utils.rb         (cross-package — `require 'rack/utils'`)
//!   - common/log.rb         (cross-package — `require 'common/log'`)

use std::path::PathBuf;
use std::process::Command;

#[test]
fn require_resolves_sibling_and_cross_package() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let loader = crate_dir.join("tests/diff/require_xpkg/sinatra/loader.rb");
    assert!(
        loader.exists(),
        "loader fixture missing at {}",
        loader.display()
    );

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&loader)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs failed:\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );

    let expected = "\
hello from sinatra/helpers
from rack
escaped(a b)
logged
";
    assert_eq!(stdout, expected, "stdout mismatch:\n{}", stdout);
}
