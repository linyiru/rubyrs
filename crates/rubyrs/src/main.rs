use std::env;
use std::path::Path;
use std::process;

use rubyrs::{Config, Runtime};

/// Cold-start phase tracer for the `trace-startup` feature.
///
/// Zero-sized struct when the feature is off — every `at()` call is
/// an inlined no-op, so production binaries pay nothing. When the
/// feature is on, the constructor anchors `Instant::now()` at
/// process entry and each `at(label)` emits a tab-separated line on
/// stderr:
///
///   `trace-startup\t<label>\t<microseconds>us`
///
/// The format is intentionally machine-parseable and goes to
/// stderr so it doesn't collide with the script's `puts` output on
/// stdout. `perf/wasm_breakdown.sh` consumes it.
#[cfg(feature = "trace-startup")]
struct Trace {
    start: std::time::Instant,
}
#[cfg(not(feature = "trace-startup"))]
struct Trace;

impl Trace {
    #[cfg(feature = "trace-startup")]
    fn new() -> Self {
        Self { start: std::time::Instant::now() }
    }
    #[cfg(not(feature = "trace-startup"))]
    #[inline(always)]
    fn new() -> Self {
        Self
    }

    #[cfg(feature = "trace-startup")]
    fn at(&self, label: &str) {
        // `eprintln!` on wasi calls `fd_write` to fd 2 — that import
        // is fine to use at runtime (we're past wizer at this point;
        // `wizer.initialize` never invokes `Trace`). Sub-microsecond
        // overhead per print is good enough for ~10 ms-scale phase
        // budgets — measurement noise dominates.
        eprintln!(
            "trace-startup\t{}\t{}us",
            label,
            self.start.elapsed().as_micros()
        );
    }
    #[cfg(not(feature = "trace-startup"))]
    #[inline(always)]
    fn at(&self, _label: &str) {}
}

/// Read wasi env via raw `environ_get` syscall, bypassing Rust std's
/// `env::vars()` — which on wasm32-wasip1 reads `__environ` from
/// wasi-libc, a global pointer set up during the C runtime startup.
///
/// Wizer snapshots linear memory at init time, freezing that
/// `__environ` pointer to whatever state existed when
/// `wizer.initialize` ran — typically empty (the init function
/// runs without `--env` args). When wasmtime later invokes `_start`
/// with `--env=FOO=bar`, the wasi env IS populated at the import
/// level, but `__environ` stays frozen to its wizer-time snapshot.
///
/// Reading via the syscall directly sidesteps the cache. Verified
/// experimentally: under wizer, `environ_sizes_get` returns the
/// true post-run env (`vars=1` for `--env=FOO=bar`), while
/// `std::env::vars_os()` returns empty.
///
/// Returns `(KEY, VALUE)` pairs collected from the raw
/// `KEY=VALUE\0` C-string buffer. On any wasi error, returns
/// `None` — the caller falls back to `std::env::vars()`, which
/// is correct on non-wizer'd builds and produces the same
/// "empty env" we'd get from this path on a genuinely envless
/// invocation.
#[cfg(target_os = "wasi")]
fn collect_wasi_env() -> Option<Vec<(String, String)>> {
    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    unsafe extern "C" {
        fn environ_sizes_get(environc: *mut usize, environ_buf_size: *mut usize) -> u16;
        fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
    }
    let mut n_vars = 0usize;
    let mut buf_size = 0usize;
    unsafe {
        if environ_sizes_get(&mut n_vars, &mut buf_size) != 0 { return None; }
    }
    if n_vars == 0 { return Some(Vec::new()); }
    let mut ptrs: Vec<*mut u8> = vec![std::ptr::null_mut(); n_vars];
    let mut buf: Vec<u8> = vec![0u8; buf_size];
    unsafe {
        if environ_get(ptrs.as_mut_ptr(), buf.as_mut_ptr()) != 0 { return None; }
    }
    let mut out = Vec::with_capacity(n_vars);
    for &p in &ptrs {
        if p.is_null() { continue; }
        // Each entry is a NUL-terminated C string of the form
        // `KEY=VALUE`. The pointer is into our `buf` allocation;
        // its lifetime is the lifetime of `buf`. We read up to
        // the NUL using CStr to copy out, then split on the
        // first `=`.
        let s = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
        let bytes = s.to_bytes();
        if let Some(eq) = bytes.iter().position(|&b| b == b'=') {
            let k = String::from_utf8_lossy(&bytes[..eq]).into_owned();
            let v = String::from_utf8_lossy(&bytes[eq + 1..]).into_owned();
            out.push((k, v));
        }
    }
    Some(out)
}

/// Parse a numeric env-var value, warning on stderr and
/// returning `None` if the value is present-but-malformed.
///
/// Previously every `RUBYRS_*` cap used
/// `env_lookup("X").and_then(|s| s.parse().ok())`, which
/// silently dropped typos: `RUBYRS_MAX_FRAMES=oops rubyrs
/// script.rb` ran with the default cap and gave no hint that
/// the env var had been ignored. The user got a useless
/// out-of-frames trap minutes later under load, or — worse —
/// thought their cap was active.
///
/// Mirrors CRuby's `RUBYOPT` / `--enable-frozen-string-literal`
/// stance: refuse to keep going *silently* when an explicit
/// runtime knob is malformed; print one line and continue with
/// the default, so the script still runs but the operator sees
/// the warning in their CI / shell history.
fn parse_env_cap<T: std::str::FromStr>(key: &str, raw: Option<&str>) -> Option<T> {
    let s = raw?;
    match s.parse::<T>() {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!(
                "rubyrs: warning: invalid value for {}={:?}, ignoring (expected {})",
                key,
                s,
                std::any::type_name::<T>(),
            );
            None
        }
    }
}

fn main() {
    let trace = Trace::new();
    trace.at("entry");
    let args: Vec<String> = env::args().collect();
    trace.at("args");
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

    // On wasi, read environ via raw syscall to sidestep Rust std's
    // cached `__environ` (frozen by wizer). On other targets just
    // collect `env::vars()`. Either way `host_env` is the canonical
    // env map for the rest of main().
    #[cfg(target_os = "wasi")]
    let host_env: Vec<(String, String)> =
        collect_wasi_env().unwrap_or_else(|| env::vars().collect());
    #[cfg(not(target_os = "wasi"))]
    let host_env: Vec<(String, String)> = env::vars().collect();
    trace.at("env_collected");

    // Lookup helper for the ENV-driven config caps. Walks `host_env`
    // once per cap (small; the env is typically <50 entries). Using
    // `host_env` instead of `env::var()` ensures wizer'd builds
    // honour `RUBYRS_FUEL=…` etc. set at wasmtime invocation time.
    let env_lookup = |key: &str| -> Option<&str> {
        host_env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    };

    // The CLI binary IS the host, and it chooses to wire every
    // ADR-0017-style capability (env, pid, stdout) through to the
    // real host process so `rubyrs script.rb` behaves like CRuby.
    // Library/embed users that construct a `Runtime` directly do
    // NOT inherit these defaults — they get sandbox-friendly empty
    // ENV, `$$ == 0`, and a silent stdout until they opt in.
    let cfg = Config {
        stress_gc: env_lookup("STRESS_GC").is_some(),
        fuel: parse_env_cap("RUBYRS_FUEL", env_lookup("RUBYRS_FUEL")),
        max_heap_objects: parse_env_cap("RUBYRS_MAX_OBJECTS", env_lookup("RUBYRS_MAX_OBJECTS")),
        max_frames: parse_env_cap("RUBYRS_MAX_FRAMES", env_lookup("RUBYRS_MAX_FRAMES")),
        deadline: parse_env_cap::<u64>("RUBYRS_DEADLINE_MS", env_lookup("RUBYRS_DEADLINE_MS"))
            .map(std::time::Duration::from_millis),
        max_symbols: parse_env_cap("RUBYRS_MAX_SYMBOLS", env_lookup("RUBYRS_MAX_SYMBOLS")),
        max_value_bytes: parse_env_cap("RUBYRS_MAX_VALUE_BYTES", env_lookup("RUBYRS_MAX_VALUE_BYTES")),
        env: Some(host_env.iter().cloned().collect()),
        // `std::process::id()` panics on wasm32-wasip1 (wasi has no
        // process-ID concept). The runtime treats `pid: None` as
        // a sentinel and surfaces `$$` as `Int(0)` (see
        // `vm/step.rs::"$$"`), not a trap — wasi scripts that
        // depend on a non-zero PID need to detect the zero
        // sentinel themselves. Leaving the interpreter alive
        // rather than panicking at construction is the load-
        // bearing fix for wasi.
        #[cfg(not(target_os = "wasi"))]
        pid: std::num::NonZeroU32::new(process::id()),
        #[cfg(target_os = "wasi")]
        pid: None,
        // Wall-clock injection for the Tier 1 `Time` class. CLI
        // binary opts the host process clock in so `rubyrs
        // script.rb` matches CRuby; library / embed users that
        // construct a `Runtime` directly get the deterministic
        // default (Time.now raises) until they wire their own
        // `Config::time_now` (potentially a fixed-clock for
        // reproducible tests).
        time_now: Some(std::sync::Arc::new(|| {
            // `UNIX_EPOCH` is the documented zero anchor; the
            // SystemTime returned by `now()` may be before or
            // after it. The pre-1970 case is rare in practice
            // (only some embedded boards with no RTC) but
            // handled — `duration_since(UNIX_EPOCH)` returns
            // an Err carrying the magnitude.
            use std::time::{SystemTime, UNIX_EPOCH};
            match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
                Err(e) => {
                    let d = e.duration();
                    // Negate the seconds; nsec stays positive
                    // because Duration is always non-negative.
                    (-(d.as_secs() as i64), d.subsec_nanos())
                }
            }
        })),
    };

    // wasi-wizer fast path: if the binary was put through
    // `wizer`, the static `WIZER_RUNTIME` holds a Runtime whose
    // classes and preamble bytecode were built at wizer time. Take
    // it and apply the host Config on top — skips the ~3-6 ms of
    // class registration + preamble parse + bytecode compile every
    // subsequent invocation would otherwise repeat. Falls back to
    // a fresh `Runtime::with_config` when Wizer wasn't used
    // (native targets, or wasi builds shipped without the
    // pre-init step).
    #[cfg(target_os = "wasi")]
    let mut rt = match rubyrs::take_wizer_runtime() {
        Some(mut rt) => { rt.apply_config(cfg); rt }
        None => Runtime::with_config(cfg),
    };
    #[cfg(not(target_os = "wasi"))]
    let mut rt = Runtime::with_config(cfg);
    trace.at("runtime_ready");
    rt.set_stdout(Box::new(std::io::stdout()));
    let result = rt.eval_file(path);
    trace.at("eval_done");
    match result {
        Ok(_) => {}
        Err(trap) => {
            eprint!("{}", rt.format_trap(&trap));
            process::exit(1);
        }
    }
    // Final checkpoint AFTER the success-path match returns. This
    // closes the gap between `eval_done` and process exit so the
    // breakdown harness's `wall - last_checkpoint` formula
    // represents strictly pre-`main()` work (wasmtime CLI launch,
    // wasi-libc init, cwasm load, _start dispatch) rather than
    // also folding in stdout flush + Runtime drop time. On the
    // error path we already process::exit() above, so `done`
    // doesn't fire — that's fine; the harness only times the
    // success path (puts 1+2 returns Ok).
    trace.at("done");
}
