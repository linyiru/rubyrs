//! Build script for `rubyrs-wasm-embed`.
//!
//! Inputs:
//!   - `RUBYRS_WIZER_WASM` env var (optional): path to a wizer'd
//!     `rubyrs.wasm`. When set, this script reads it and asks the
//!     wasmtime build-dep's `Engine::precompile_module` to produce
//!     a host-arch cwasm, written to `$OUT_DIR/rubyrs.cwasm`.
//!
//!   - If unset OR the file is missing: writes a zero-byte stub at
//!     the same path. `main.rs` checks for the empty case at
//!     runtime and prints an actionable error pointing at
//!     `perf/build_embedder.sh`. This keeps `cargo build -p
//!     rubyrs-wasm-embed` working in CI / clean checkouts (no
//!     external wasm pipeline required just to compile-check the
//!     embedder source) while still failing loudly the first time
//!     someone tries to actually RUN it without the wasm.
//!
//! Output: `$OUT_DIR/rubyrs.cwasm` — either the AOT-compiled cwasm
//! or a zero-byte placeholder. `main.rs` unconditionally
//! `include_bytes!`s it.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=RUBYRS_WIZER_WASM");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let cwasm_out = out_dir.join("rubyrs.cwasm");

    // No input wasm? Write a zero-byte stub and return early. The
    // runtime side surfaces an actionable error when it sees an
    // empty embedded cwasm AND no `RUBYRS_CWASM` override.
    let wasm_path = match env::var_os("RUBYRS_WIZER_WASM") {
        Some(p) => PathBuf::from(p),
        None => {
            stub(&cwasm_out, "RUBYRS_WIZER_WASM not set");
            return;
        }
    };
    println!("cargo:rerun-if-changed={}", wasm_path.display());
    if !wasm_path.exists() {
        stub(
            &cwasm_out,
            &format!("RUBYRS_WIZER_WASM={} does not exist", wasm_path.display()),
        );
        return;
    }

    // If we got this far the user EXPLICITLY pointed us at a
    // file that exists — a read failure (EACCES, IO error,
    // mid-build deletion) is almost certainly a mistake they
    // want to know about loudly, not silently demoted to "no
    // cwasm available" via a cargo:warning that's easy to miss
    // in piped release-mode build output. Asymmetric vs the
    // "env var unset" / "file doesn't exist" branches above,
    // which legitimately mean "no opinion expressed yet" and
    // route to the stub. Reviewer feedback PR #125: the previous
    // shape made debuggability inconsistent — stub-write
    // failures panic, stub-read failures stubbed silently,
    // both representing the same class of "I tried but
    // couldn't" fault.
    let wasm_bytes = match fs::read(&wasm_path) {
        Ok(b) => b,
        Err(e) => {
            panic!("failed to read RUBYRS_WIZER_WASM={}: {e}", wasm_path.display());
        }
    };

    // AOT-compile with wasmtime's default Config. Using the same
    // wasmtime crate version as [dependencies] is what makes the
    // resulting cwasm deserialize-able by the runtime Engine.
    let engine = wasmtime::Engine::default();
    let cwasm_bytes = match engine.precompile_module(&wasm_bytes) {
        Ok(b) => b,
        Err(e) => {
            // `precompile_module` failing is exotic (malformed
            // wasm, unsupported features) — still write a stub
            // rather than abort the whole build so the surface
            // error stays at runtime where it's actionable.
            stub(
                &cwasm_out,
                &format!("precompile_module({}) failed: {e}", wasm_path.display()),
            );
            return;
        }
    };

    if let Err(e) = fs::write(&cwasm_out, &cwasm_bytes) {
        // Now this IS a hard fail — we successfully compiled but
        // can't write the artifact. Surfacing as build failure
        // matches "cargo build can't produce its outputs".
        panic!("failed to write {}: {e}", cwasm_out.display());
    }
    println!(
        "cargo:warning=rubyrs-wasm-embed: baked cwasm from {} ({} bytes)",
        wasm_path.display(),
        cwasm_bytes.len()
    );
}

/// Write a zero-byte stub so `include_bytes!` succeeds. The reason
/// is printed as a `cargo:warning` so it shows up in `cargo build`
/// output — invisible to release piping (warnings go to stderr but
/// are typically not silenced even there) and surfaced clearly when
/// someone wonders why the binary later complains.
fn stub(path: &std::path::Path, reason: &str) {
    // If the stub itself can't be written, `include_bytes!` in
    // `main.rs` will fail at compile time with a confusing
    // "file not found" pointing at $OUT_DIR — the user wouldn't
    // know to look at OUT_DIR's permissions / disk. Surface the
    // real error immediately as a build failure with a
    // diagnostic pointing at the actual problem.
    if let Err(e) = fs::write(path, []) {
        panic!("failed to write cwasm stub {}: {e}", path.display());
    }
    println!(
        "cargo:warning=rubyrs-wasm-embed: no cwasm baked in ({}). \
         Set RUBYRS_CWASM=<path> at runtime, or re-build with \
         RUBYRS_WIZER_WASM=<path> after running perf/build_embedder.sh.",
        reason
    );
}
