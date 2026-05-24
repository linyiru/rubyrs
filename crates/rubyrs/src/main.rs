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
        process::exit(1);
    }
    let path = Path::new(&args[1]);

    let cfg = Config {
        stress_gc: env::var("STRESS_GC").is_ok(),
        fuel: env::var("RUBYRS_FUEL").ok().and_then(|s| s.parse().ok()),
        max_heap_objects: env::var("RUBYRS_MAX_OBJECTS").ok().and_then(|s| s.parse().ok()),
        max_frames: env::var("RUBYRS_MAX_FRAMES").ok().and_then(|s| s.parse().ok()),
        deadline: env::var("RUBYRS_DEADLINE_MS").ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(std::time::Duration::from_millis),
    };

    let mut rt = Runtime::with_config(cfg);
    match rt.eval_file(path) {
        Ok(_) => {}
        Err(trap) => {
            eprint!("{}", rt.format_trap(&trap));
            process::exit(1);
        }
    }
}
