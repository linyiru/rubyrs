//! `rubyrs-wasm-embed` — PoC minimal embedder.
//!
//! Links `wasmtime` and `wasmtime-wasi` as Rust libraries and runs
//! a precompiled `rubyrs.cwasm` directly, deliberately skipping
//! everything the `wasmtime` CLI does on top of the wasmtime core
//! (argv parsing for the CLI's own flags, command dispatch through
//! the `--{run,serve,compile,explore}` matcher, default Config
//! plumbing, the file-config layer, signal handling, profiling
//! hooks). The goal is to quantify how much of `wasmtime --run`'s
//! ~5 ms init is the CLI's own framing vs the wasmtime runtime
//! that any embedder still has to pay.
//!
//! Not a shipping artifact: deserialization of a `.cwasm` ties
//! this binary to one specific wasmtime version + host arch.
//! `Module::deserialize_file` is `unsafe` for the same reason — a
//! mismatched / tampered file is a UB attack surface. Used here
//! only against cwasm WE just compiled with the same wasmtime
//! version.
//!
//! Usage:
//!   rubyrs-wasm-embed <rubyrs.cwasm> <script.rb> [arg ...]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

fn main() -> ExitCode {
    let mut argv = std::env::args_os().skip(1);
    let cwasm_path: PathBuf = match argv.next() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: rubyrs-wasm-embed <rubyrs.cwasm> <script.rb> [arg ...]");
            return ExitCode::from(2);
        }
    };
    let script_path: PathBuf = match argv.next() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: rubyrs-wasm-embed <rubyrs.cwasm> <script.rb> [arg ...]");
            return ExitCode::from(2);
        }
    };
    let extra_args: Vec<String> = argv.filter_map(|a| a.into_string().ok()).collect();

    let engine = Engine::default();

    // SAFETY: cwasm files are deserialized as raw machine code; a
    // mismatched wasmtime version or a tampered file is UB. We
    // restrict use to cwasm WE just compiled with the same wasmtime
    // version (`perf/wasm_breakdown.sh` regenerates it every run).
    let module = match unsafe { Module::deserialize_file(&engine, &cwasm_path) } {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "rubyrs-wasm-embed: failed to deserialize {}: {e}",
                cwasm_path.display()
            );
            return ExitCode::from(2);
        }
    };

    // Mirror `wasmtime run --dir $PARENT cwasm script.rb`:
    // - inherit stdio + env so script `puts`/`p` and `ENV[...]`
    //   work transparently
    // - argv = ["rubyrs", "<script-filename>", extras...]; rubyrs's
    //   main reads args[1] as the script path
    // - preopen the script's parent directory so `eval_file` can
    //   open the script through the wasi sandbox
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
            // wasi's `_start` surfaces `process::exit(N)` from the
            // guest as a wasmtime error wrapping `I32Exit(N)`.
            // Forward the code so callers see the same exit shape
            // they would under `wasmtime run`.
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
