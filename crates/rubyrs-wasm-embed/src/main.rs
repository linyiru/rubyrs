//! `rubyrs-wasm-embed` — minimal embedder, single-binary shape.
//!
//! Links `wasmtime` + `wasmtime-wasi` as Rust libraries and
//! deserializes a baked-in `rubyrs.cwasm` to run a user script.
//! The cwasm is produced at build time by `build.rs` calling
//! `Engine::precompile_module` on a wizer-pre-initialized
//! `rubyrs.wasm` and `include_bytes!`d into this binary, so the
//! shipping artifact is a SINGLE executable — no external cwasm
//! file required at runtime. ~7 MB on macOS arm64 with the
//! trimmed-feature `release-min` build (see [dependencies] in
//! Cargo.toml).
//!
//! Two modes:
//!
//! * **Baked cwasm (default).** `cargo build` with
//!   `RUBYRS_WIZER_WASM=<path>` set picks up that wizer'd wasm,
//!   AOT-compiles it, embeds the result. Use
//!   `perf/build_embedder.sh` to do the full pipeline
//!   (rubyrs wasm32-wasip1 build → wasm-opt → wizer → wasm-opt
//!   → build.rs precompile → cargo build embedder).
//!
//! * **External cwasm via `RUBYRS_CWASM=<path>`.** Overrides the
//!   baked cwasm at runtime. Useful for dev iteration (rebuild
//!   the rubyrs wasm without rebuilding the embedder) and for
//!   testing alternative wasm builds without baking. Goes
//!   through `Module::deserialize_file` (mmap) — slightly
//!   slower than the baked path's `Module::deserialize` over a
//!   `&'static [u8]` slice.
//!
//! `cargo build` always works (even on a clean checkout with no
//! wasm pipeline): the build script falls back to a zero-byte
//! stub and the runtime surfaces an actionable error if neither
//! a baked nor external cwasm is available.
//!
//! Usage:
//!   rubyrs-wasm-embed <script.rb> [arg ...]
//!   RUBYRS_CWASM=path/to/rubyrs.cwasm rubyrs-wasm-embed <script.rb>

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

/// Baked-at-build-time cwasm. `build.rs` writes either:
///   - the real AOT cwasm (when `RUBYRS_WIZER_WASM` was set), or
///   - a zero-byte stub (when the env var was unset or the wasm
///     wasn't found at build time).
///
/// Runtime distinguishes the two by length: empty → no baked
/// cwasm available, fall back to `RUBYRS_CWASM` or error out.
static EMBEDDED_CWASM: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/rubyrs.cwasm"));

fn main() -> ExitCode {
    let mut argv = std::env::args_os().skip(1);
    let script_path: PathBuf = match argv.next() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: rubyrs-wasm-embed <script.rb> [arg ...]");
            eprintln!();
            eprintln!("Optional env vars:");
            eprintln!("  RUBYRS_CWASM=<path>   Use an external cwasm instead of the");
            eprintln!("                        one baked at build time.");
            return ExitCode::from(2);
        }
    };
    let extra_args: Vec<String> = argv.filter_map(|a| a.into_string().ok()).collect();

    let engine = Engine::default();

    // Resolve cwasm source. SAFETY on both branches: `Module::
    // deserialize` is `unsafe` because cwasm files are raw machine
    // code stamped with a wasmtime version + host arch — a
    // mismatched or tampered file is UB. The baked path is safe
    // by construction (build.rs and runtime link the same
    // wasmtime version; the bytes are immutable from the user's
    // perspective). The external path trusts the caller.
    let module = if let Some(external) = std::env::var_os("RUBYRS_CWASM") {
        let path = PathBuf::from(external);
        match unsafe { Module::deserialize_file(&engine, &path) } {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "rubyrs-wasm-embed: failed to load RUBYRS_CWASM={}: {e}",
                    path.display()
                );
                return ExitCode::from(2);
            }
        }
    } else if EMBEDDED_CWASM.is_empty() {
        eprintln!(
            "rubyrs-wasm-embed: no cwasm available — this binary was \
             built without RUBYRS_WIZER_WASM set, so no cwasm is baked in. \
             Either run `perf/build_embedder.sh` (which sets the env var) or \
             set `RUBYRS_CWASM=<path>` to load an external cwasm at runtime."
        );
        return ExitCode::from(2);
    } else {
        match unsafe { Module::deserialize(&engine, EMBEDDED_CWASM) } {
            Ok(m) => m,
            Err(e) => {
                eprintln!("rubyrs-wasm-embed: failed to deserialize baked cwasm: {e}");
                return ExitCode::from(2);
            }
        }
    };

    // Mirror what the rubyrs CLI's main() expects: argv[1] is the
    // script path, inherit stdio + env, preopen the script's
    // parent dir for `eval_file`.
    let script_parent = script_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let script_filename = script_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| script_path.to_string_lossy().into_owned());

    let mut wasi_argv: Vec<String> = vec!["rubyrs".to_string(), script_filename];
    wasi_argv.extend(extra_args);

    let mut wasi_builder = WasiCtxBuilder::new();
    wasi_builder.inherit_stdio().inherit_env().args(&wasi_argv);
    if let Err(e) =
        wasi_builder.preopened_dir(&script_parent, ".", DirPerms::READ, FilePerms::READ)
    {
        eprintln!(
            "rubyrs-wasm-embed: preopen {} failed: {e}",
            script_parent.display()
        );
        return ExitCode::from(2);
    }
    let wasi_ctx: WasiP1Ctx = wasi_builder.build_p1();

    let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
    if let Err(e) = p1::add_to_linker_sync(&mut linker, |s| s) {
        eprintln!("rubyrs-wasm-embed: add_to_linker_sync failed: {e}");
        return ExitCode::from(2);
    }

    let mut store = Store::new(&engine, wasi_ctx);
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("rubyrs-wasm-embed: instantiate failed: {e}");
            return ExitCode::from(2);
        }
    };

    let start = match instance.get_typed_func::<(), ()>(&mut store, "_start") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("rubyrs-wasm-embed: missing `_start`: {e}");
            return ExitCode::from(2);
        }
    };
    match start.call(&mut store, ()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                let code = exit.0;
                if code == 0 {
                    return ExitCode::SUCCESS;
                }
                return ExitCode::from((code as u32 & 0xff) as u8);
            }
            eprintln!("rubyrs-wasm-embed: guest trap: {e:#}");
            ExitCode::from(1)
        }
    }
}
