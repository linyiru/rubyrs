use std::env;
use std::path::Path;
use std::process;

use rubyrs::{Config, Runtime};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: rubyrs <file.rb>");
        eprintln!();
        eprintln!("Optional env vars:");
        eprintln!("  STRESS_GC=1             collect on every alloc (debug)");
        eprintln!("  RUBYRS_FUEL=N           trap after N ops dispatched");
        eprintln!("  RUBYRS_MAX_OBJECTS=N    trap when live heap objects > N");
        eprintln!("  RUBYRS_MAX_FRAMES=N     trap when frame stack depth > N");
        eprintln!("  RUBYRS_DEADLINE_MS=N    trap when wall-clock per eval exceeds N ms");
        eprintln!("  RUBYRS_MAX_SYMBOLS=N    trap when interner grows beyond N symbols");
        eprintln!("  RUBYRS_MAX_VALUE_BYTES=N trap when any single String/Array/Hash exceeds N bytes");
        process::exit(1);
    }
    let path = Path::new(&args[1]);

    // The CLI binary IS the host, and it chooses to wire every
    // ADR-0017-style capability (env, pid, stdout) through to the
    // real host process so `rubyrs script.rb` behaves like CRuby.
    // Library/embed users that construct a `Runtime` directly do
    // NOT inherit these defaults — they get sandbox-friendly empty
    // ENV, `$$ == 0`, and a silent stdout until they opt in.
    let cfg = Config {
        stress_gc: env::var("STRESS_GC").is_ok(),
        fuel: env::var("RUBYRS_FUEL").ok().and_then(|s| s.parse().ok()),
        max_heap_objects: env::var("RUBYRS_MAX_OBJECTS").ok().and_then(|s| s.parse().ok()),
        max_frames: env::var("RUBYRS_MAX_FRAMES").ok().and_then(|s| s.parse().ok()),
        deadline: env::var("RUBYRS_DEADLINE_MS").ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(std::time::Duration::from_millis),
        max_symbols: env::var("RUBYRS_MAX_SYMBOLS").ok().and_then(|s| s.parse().ok()),
        max_value_bytes: env::var("RUBYRS_MAX_VALUE_BYTES").ok().and_then(|s| s.parse().ok()),
        env: Some(env::vars().collect()),
        // `std::process::id()` panics on wasm32-wasip1 (wasi has no
        // process-ID concept). The runtime treats `pid: None` as
        // "host did not provide one" — `$$` in script then surfaces
        // a no-pid trap rather than crashing the interpreter.
        #[cfg(not(target_os = "wasi"))]
        pid: std::num::NonZeroU32::new(process::id()),
        #[cfg(target_os = "wasi")]
        pid: None,
    };

    let mut rt = Runtime::with_config(cfg);
    rt.set_stdout(Box::new(std::io::stdout()));
    match rt.eval_file(path) {
        Ok(_) => {}
        Err(trap) => {
            eprint!("{}", rt.format_trap(&trap));
            process::exit(1);
        }
    }
}
