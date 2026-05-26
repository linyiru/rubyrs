// PoC entry point for the Cloudflare Workers PoC.
//
// Shape: read Ruby source from stdin, evaluate, write result /
// runtime stdout to stdout, exit non-zero on trap. The Worker
// pipes the HTTP request body as stdin and captures stdout as
// the HTTP response — see `poc/cf-worker/`.
//
// Why a separate bin (not main.rs): the CLI reads a path from
// argv, which is awkward to coordinate from workers-wasi since
// its public API does not expose pre-populating the in-isolate
// FS. Stdin is a `ReadableStream` in workers-wasi's option
// shape, which IS easy to drive from a Worker.
//
// Intentionally NOT a feature flag — keeping it as a separate
// bin avoids adding any conditional compilation to the
// well-trodden CLI / library paths. Build with:
//   cargo build --release --target wasm32-wasip1 \
//     --bin wasm_worker --no-default-features -p rubyrs

use std::io::Read;

use rubyrs::{Config, Runtime};

fn main() {
    let mut src = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut src) {
        eprintln!("wasm_worker: stdin read failed: {e}");
        std::process::exit(2);
    }
    // PoC: defaults only. Once cold-start + execution numbers
    // land, the right per-request caps (RUBYRS_DEADLINE_MS to
    // back-stop Workers' 30s CPU cap, max_value_bytes to keep a
    // runaway response from filling the 128MB isolate budget)
    // are an obvious follow-up.
    let cfg = Config {
        // wasi has no PID concept; CLI uses None for the same
        // reason. `$$` surfaces as 0 in Ruby-land.
        pid: None,
        ..Config::default()
    };
    // `take_wizer_runtime` only exists under `target_os = "wasi"`
    // (see lib.rs); on host targets we skip the fast path so this
    // bin still `cargo check`s without `--target wasm32-wasip1`.
    #[cfg(target_os = "wasi")]
    let mut rt = match rubyrs::take_wizer_runtime() {
        Some(mut rt) => { rt.apply_config(cfg); rt }
        None => Runtime::with_config(cfg),
    };
    #[cfg(not(target_os = "wasi"))]
    let mut rt = Runtime::with_config(cfg);
    rt.set_stdout(Box::new(std::io::stdout()));
    if let Err(trap) = rt.eval(&src, "(worker)") {
        eprint!("{}", rt.format_trap(&trap));
        std::process::exit(1);
    }
}
