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

    let wasm_bytes = match fs::read(&wasm_path) {
        Ok(b) => b,
        Err(e) => {
            stub(
                &cwasm_out,
                &format!("failed to read {}: {e}", wasm_path.display()),
            );
            return;
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
    let _ = fs::write(path, []);
    println!(
        "cargo:warning=rubyrs-wasm-embed: no cwasm baked in ({}). \
         Set RUBYRS_CWASM=<path> at runtime, or re-build with \
         RUBYRS_WIZER_WASM=<path> after running perf/build_embedder.sh.",
        reason
    );
}
