use std::env;
use std::path::Path;
use std::process;

use rubyrs::{Config, Runtime};

// EXPERIMENT (feature `mimalloc`): replace the system allocator. The
// VM mints a Frame + locals per call, an Rc per block invocation, and a
// heap object per Array/Hash/Object/String — a small-object, high-churn
// pattern the system malloc handles poorly. mimalloc is tuned for it.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
// Alternative allocator (feature `jemalloc`): trades 3-4% wall for
// 8-9% lower peak RSS on the Jekyll benches — see the feature's
// comment in Cargo.toml for the measured numbers. mimalloc wins
// when both features are enabled (the cfg below keeps the
// global_allocator unique).
#[cfg(all(feature = "jemalloc", not(feature = "mimalloc")))]
#[global_allocator]
static GLOBAL_JE: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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
        #[cfg(feature = "ic-stats")]
        eprintln!("  RUBYRS_IC_STATS=1       dump per-call-site IC hit/miss counters on exit");
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
        // Preamble bytecode cache (`preamble-cache` feature): the
        // CLI opts in by default — cold start is the CLI's
        // headline metric — with `RUBYRS_NO_PREAMBLE_CACHE=1` as
        // the off switch. Library embedders stay opted out unless
        // they set the field themselves (ADR 0017 posture).
        #[cfg(feature = "preamble-cache")]
        preamble_cache_dir: if env_lookup("RUBYRS_NO_PREAMBLE_CACHE").is_some() {
            None
        } else {
            rubyrs::preamble_cache::default_cache_dir()
        },
        #[cfg(not(feature = "preamble-cache"))]
        preamble_cache_dir: None,
        fuel: parse_env_cap("RUBYRS_FUEL", env_lookup("RUBYRS_FUEL")),
        max_heap_objects: parse_env_cap("RUBYRS_MAX_OBJECTS", env_lookup("RUBYRS_MAX_OBJECTS")),
        max_frames: parse_env_cap("RUBYRS_MAX_FRAMES", env_lookup("RUBYRS_MAX_FRAMES")),
        max_dispatch_depth: parse_env_cap("RUBYRS_MAX_DISPATCH_DEPTH", env_lookup("RUBYRS_MAX_DISPATCH_DEPTH")),
        deadline: parse_env_cap::<u64>("RUBYRS_DEADLINE_MS", env_lookup("RUBYRS_DEADLINE_MS"))
            .map(std::time::Duration::from_millis),
        max_symbols: parse_env_cap("RUBYRS_MAX_SYMBOLS", env_lookup("RUBYRS_MAX_SYMBOLS")),
        max_value_bytes: parse_env_cap("RUBYRS_MAX_VALUE_BYTES", env_lookup("RUBYRS_MAX_VALUE_BYTES")),
        #[cfg(feature = "_fiber")]
        max_live_fibers: parse_env_cap("RUBYRS_MAX_LIVE_FIBERS", env_lookup("RUBYRS_MAX_LIVE_FIBERS")),
        #[cfg(feature = "_fiber")]
        max_fiber_frame_depth: parse_env_cap("RUBYRS_MAX_FIBER_FRAME_DEPTH", env_lookup("RUBYRS_MAX_FIBER_FRAME_DEPTH")),
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
        // Wall-clock sleep injection for `Kernel#sleep`.
        // CLI binary opts in so `rubyrs script.rb` matches
        // CRuby; embed users get the deterministic default
        // (sleep raises) unless they wire their own.
        // ADR 0025 Phase 3: polling sleep. Sleep up to the
        // requested duration (or forever if None) in 50ms
        // chunks, checking the interrupt flag between chunks.
        // Returns the actually-elapsed Duration. 50ms bounds
        // the worst-case SIGINT response latency to ~one
        // chunk; production code that wants tighter bounds
        // can inject its own polling resolution.
        sleep_for: Some(std::sync::Arc::new(|requested, flag| {
            use std::time::{Duration, Instant};
            use std::sync::atomic::Ordering;
            let start = Instant::now();
            let chunk = Duration::from_millis(50);
            loop {
                if flag.load(Ordering::Relaxed) {
                    return start.elapsed();
                }
                match requested {
                    None => {
                        // Sleep forever until the flag flips.
                        std::thread::sleep(chunk);
                    }
                    Some(d) => {
                        let elapsed = start.elapsed();
                        if elapsed >= d {
                            return d;
                        }
                        let remaining = d - elapsed;
                        std::thread::sleep(remaining.min(chunk));
                    }
                }
            }
        })),
        // Immediate-exit injection for `Kernel#exit!`. CLI binary
        // wires `std::process::exit` so `rubyrs script.rb` matches
        // CRuby; embed users get the deterministic default (exit!
        // raises) unless they wire their own.
        process_exit: Some(std::sync::Arc::new(|status: i32| {
            std::process::exit(status);
        })),
        // ADR 0025 Phase 1: SIGINT capture for Ctrl+C against
        // `rubyrs script.rb`. CLI binary opts in so the
        // interrupt_pending flag flips on Ctrl+C; Phase 2 will
        // translate the flag into a Ruby `Interrupt` raise.
        // Until then the flag is set but unobserved — the script
        // continues until natural completion, same as before.
        install_signal_handler: true,
        // ADR 0024 Phase A: CLI gets the defensive default
        // (None = unlimited). CLI scripts trust their own code;
        // sandbox embedders should set a finite cap.
        max_yield_recursion: None,
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
        // CLI binary is the canonical "run untrusted-ish but
        // trusted-enough Ruby" host — File.* / require /
        // require_relative MUST work, matching `ruby script.rb`'s
        // POLA. Embed users that want a sandbox leave the
        // `Config::allow_filesystem_io: false` default.
        allow_filesystem_io: true,
        // Subprocess spawning: the CLI runs trusted local scripts —
        // same opt-in rationale as filesystem IO above.
        allow_process_spawn: true,
        // CLI binary: no path narrowing — `rubyrs script.rb`
        // behaves like `ruby script.rb` and can touch any path
        // the shell can. Embed users wanting scope (rubund for
        // gemspec evaluation, etc.) supply Some(prefixes).
        allowed_paths: None,
        #[cfg(feature = "_sqlite")]
        sqlite_allow_paths: None,
        #[cfg(feature = "_sqlite")]
        sqlite_max_result_bytes: None,
        // CLI binary: outbound network allowed — `rubyrs script.rb`
        // behaves like `ruby script.rb` (Net::HTTP works). Embedders
        // running untrusted scripts keep the secure-by-default `false`
        // and opt in / narrow via `Config::socket_allow_hosts`.
        #[cfg(feature = "_socket")]
        allow_network_io: true,
        #[cfg(feature = "_socket")]
        socket_allow_hosts: None,
        #[cfg(feature = "_socket")]
        socket_max_read_bytes: None,
        // CLI binary: no seeded `$LOAD_PATH` — scripts opt in
        // explicitly via `$LOAD_PATH.unshift(...)`, matching
        // CRuby's `ruby script.rb` shape (CRuby's gem env
        // pre-populates $LOAD_PATH but that's a gem-host
        // responsibility, not the CLI's). Embed users shipping
        // bundled .rb files supply Some(paths).
        load_paths: None,
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
    rt.set_stderr(Box::new(std::io::stderr()));
    // Script arguments: everything after the script path becomes
    // ARGV, CRuby-style (`rubyrs script.rb a b` → ARGV == ["a","b"]).
    rt.set_argv(&args[2..]);
    // Stage 7d: expose `_http_server` host fns to scripts
    // when the feature is built in. Keeps the binary
    // useful for prefork subprocess tests + examples.
    #[cfg(feature = "_http_server")]
    rubyrs::register_http_server_host_fns(&mut rt);
    // `_json_native` accelerator: when the feature is built in,
    // expose `__rubyrs_json_native_*` host fns so the pure-Ruby
    // JSON canon (stdlib_vendor/json.rb) auto-detects and routes
    // hot calls through serde_json. Pure-canon path stays as
    // the reference behaviour for non-accelerator builds.
    #[cfg(feature = "_json_native")]
    rubyrs::register_json_native_host_fns(&mut rt);
    // `_rouge_native` accelerator: expose the carmine engine host fns;
    // the require("rouge") hook injects the shim that detects + uses
    // them. Without this registration the shim stays inert.
    #[cfg(feature = "_rouge_native")]
    rubyrs::register_rouge_native_host_fns(&mut rt);
    // `_kramdown_native` accelerator: expose the rostdown renderer host
    // fns; the require("kramdown-parser-gfm") hook injects the shim
    // that detects + uses them. Without this registration the shim
    // stays inert.
    #[cfg(feature = "_kramdown_native")]
    rubyrs::register_kramdown_native_host_fns(&mut rt);
    // `_yaml_native` accelerator: expose the native translation of the
    // blessed YAML loader; stdlib_vendor/yaml.rb detects + uses it.
    #[cfg(feature = "_yaml_native")]
    rubyrs::register_yaml_native_host_fns(&mut rt);
    // `_liquid_native` accelerator: expose the liquidus engine host
    // fns; the require("jekyll") hook injects the shim that detects +
    // uses them.
    #[cfg(feature = "_liquid_native")]
    rubyrs::register_liquid_native_host_fns(&mut rt);
    // `_sqlite` battery: when built in, expose the
    // SQLite3::Database + 25-class exception hierarchy + 9
    // host fns per ADR 0027 §"Capability host-fns consumed".
    #[cfg(feature = "_sqlite")]
    rubyrs::register_sqlite_host_fns(&mut rt);
    // `_socket` battery: the pure-Ruby `TCPSocket` veneer + 4 host fns
    // (connect/write/read/close) backing Net::HTTP, per ADR 0028.
    // Outbound network stays gated by `Config::allow_network_io`.
    #[cfg(feature = "_socket")]
    rubyrs::register_socket_host_fns(&mut rt);
    // `_openssl` battery: rustls TLS-client slice (OpenSSL::SSL::SSLSocket)
    // backing Net::HTTP https, layered over a `_socket` connection. ADR 0029.
    #[cfg(feature = "_openssl")]
    rubyrs::register_openssl_host_fns(&mut rt);
    let result = rt.eval_file(path);
    trace.at("eval_done");
    match result {
        Ok(_) => {}
        Err(trap) => {
            eprint!("{}", rt.format_trap(&trap));
            process::exit(1);
        }
    }
    // `RUBYRS_IC_STATS=1` (only meaningful when built with
    // `--features ic-stats`): dump the inline-cache hit/miss
    // counters to stderr before exit. Without the feature, the
    // counters are ZST/no-op and printing them is a noisy no-op
    // — guarded behind the env var either way so production
    // invocations stay silent.
    // `RUBYRS_REGEX_STATS=1`: dump regex-cache occupancy (total
    // constructed vs engines actually built — the gap is the lazy-
    // build win). Debug knob in the `RUBYRS_IC_STATS` shape; used
    // to size the RSS impact of eager regex building (352 built /
    // 39 used on the Jekyll chain → lazy building landed).
    #[cfg(feature = "regex")]
    if std::env::var_os("RUBYRS_REGEX_STATS").is_some() {
        let (total, built) = rt.regex_cache_stats();
        eprintln!("regex-stats\ttotal={}\tbuilt={}", total, built);
    }
    #[cfg(feature = "ic-stats")]
    if env_lookup("RUBYRS_IC_STATS").is_some() {
        let s = rt.ic_stats();
        eprintln!(
            "ic-stats\thits={}\tmisses={}\ttoplevel_hits={}\ttoplevel_misses={}\thit_rate={:.4}",
            s.hits, s.misses, s.toplevel_hits, s.toplevel_misses, s.hit_rate()
        );
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
