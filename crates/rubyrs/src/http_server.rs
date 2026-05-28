//! `_http_server` battery — Rust HTTP front with Ruby app
//! handler. Implements [ADR 0022 v4](../../docs/adr/0022-http-server-battery.md).
//!
//! Phase H1 PoC stage 2: hyper accept loop with hardcoded
//! response. Proves wire-protocol path works end-to-end
//! before Ruby integration arrives in stage 3.
//!
//! ## Out of stage 2 scope (lands later)
//!
//! - Ruby block invocation (stage 3) — server still returns
//!   hardcoded `Hello from rubyrs!\n` regardless of request
//! - Rack SPEC env hash construction (stage 4)
//! - `VmBorrow<'_>` RAII type (stage 5)
//! - `Runtime::reset_between_requests` API (stage 5)
//! - `Runtime::refill_fuel` per-request fuel re-anchor (stage 5)
//! - Per-request I/O deadline (stage 6)
//! - `max_header_bytes` config (stage 6)
//! - SIGINT / SIGTERM graceful shutdown (stage 6)
//! - `on_worker_boot` + `fork_workers` pre-fork (stage 7)

#![cfg(feature = "_http_server")]

use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::LocalSet;

/// Per-server configuration. v1 field set per ADR 0022 v4
/// "HttpServerConfig" section. PoC only honours `bind`;
/// other fields are accepted but ignored.
#[derive(Debug, Clone, Default)]
pub struct HttpServerConfig {
    /// Bind address. None = no auto-start.
    pub bind: Option<SocketAddr>,

    /// Max concurrent in-flight requests. PoC: ignored
    /// (tokio LocalSet polls cooperatively).
    pub max_concurrent_requests: Option<usize>,

    /// Max request body size in bytes. PoC: enforced via
    /// `http_body_util::Limited`. None = 16 MB default.
    pub max_request_body_bytes: Option<usize>,

    /// Max total header bytes per request. PoC: ignored
    /// (hyper default applies).
    pub max_header_bytes: Option<usize>,

    /// Per-request I/O-phase deadline. PoC: ignored.
    pub per_request_io_deadline: Option<std::time::Duration>,

    /// Per-request fuel budget. PoC: ignored.
    pub per_request_fuel: Option<u64>,

    /// SIGINT/SIGTERM handler opt-in. PoC: ignored
    /// (single-shot Ctrl+C aborts the process).
    pub install_signal_handler: bool,
}

/// PoC default request-body cap (16 MB) per ADR 0022 v4.
#[allow(dead_code)] // Used once the Limited wrapper lands in stage 4.
pub(crate) const DEFAULT_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Build a Rack SPEC v1.6 env Hash from request parts.
///
/// Stage 4c.1 of Phase H1 PoC. Constructs the Hash directly
/// on the Vm's heap and returns the `ObjId` — caller wraps
/// in `Value::Hash` for handing to the Ruby app's block.
///
/// **Caller MUST hold `&mut Vm`** — either the canonical
/// `&mut self` (in tests / direct callers) or via the
/// `current_vm_ptr()` re-borrow contract from ADR 0013 (in
/// the per-request hyper handler, stage 4c.3).
///
/// ## What v1 includes
///
/// Per Rack SPEC + ADR 0022 v5 env hash construction:
/// - CGI-style: REQUEST_METHOD / PATH_INFO / QUERY_STRING /
///   SERVER_NAME / SERVER_PORT / SCRIPT_NAME / HTTP_VERSION
/// - REMOTE_ADDR / REMOTE_PORT (v5; spec-optional but
///   ubiquitous)
/// - HTTP_<HEADER> for each request header (uppercase,
///   dashes → underscores)
/// - CONTENT_TYPE / CONTENT_LENGTH (no HTTP_ prefix, per spec)
/// - rack.url_scheme
/// - rack.multithread (false)
/// - rack.multiprocess (true — anticipates pre-fork)
/// - rack.run_once (false)
///
/// ## What v1 stubs with TODO
///
/// - `rack.input` → set to `Value::Nil`. Should be a
///   StringIO-like wrapper around `body_bytes`; once stage
///   4c.3 wires the per-request handler, StringIO comes from
///   `stdlib_vendor/stringio.rb` per ADR 0022 v4. PoC apps
///   that don't read the request body work today; ones that
///   do see Nil instead of StringIO.
/// - `rack.errors` → `Value::Nil`. Should be the stderr sink.
/// - `rack.version` → `Value::Nil`. Should be `[1, 6]` Array.
///   Real apps that check this for the protocol version see
///   nil and may behave wrongly; documented as PoC gap.
/// - Non-UTF-8 header values: lossy-decode only; the
///   parallel `_BYTES` key from ADR 0022 v5 is deferred.
#[allow(clippy::too_many_arguments)] // 9 args — every one carries request shape; reducing would just bag into a struct
#[allow(dead_code)] // Used by stage 4c.1 test + the stage 4c.3 per-request handler when that lands.
pub(crate) fn build_rack_env(
    vm: &mut crate::vm::Vm,
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, Vec<u8>)],
    _body_bytes: &[u8],
    listener_addr: SocketAddr,
    peer_addr: SocketAddr,
    scheme: &str,
) -> crate::value::ObjId {
    use crate::heap::{HashObj, HeapObj};
    use crate::value::Value;

    let key = |s: &str| Value::new_str(s.to_string());
    let val = |s: String| Value::new_str(s);

    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(20 + headers.len());

    // CGI-style required keys
    pairs.push((key("REQUEST_METHOD"), val(method.to_string())));
    pairs.push((key("PATH_INFO"), val(path.to_string())));
    pairs.push((key("QUERY_STRING"), val(query.to_string())));
    pairs.push((key("SERVER_NAME"), val(listener_addr.ip().to_string())));
    pairs.push((key("SERVER_PORT"), val(listener_addr.port().to_string())));
    pairs.push((key("SCRIPT_NAME"), val(String::new())));
    pairs.push((key("HTTP_VERSION"), val("HTTP/1.1".to_string())));

    // ADR 0022 v5: explicit REMOTE_ADDR/REMOTE_PORT
    pairs.push((key("REMOTE_ADDR"), val(peer_addr.ip().to_string())));
    pairs.push((key("REMOTE_PORT"), val(peer_addr.port().to_string())));

    // Headers — HTTP_<UPPER_NAME_WITH_DASHES_AS_UNDERSCORES>
    // for everything except Content-Type / Content-Length
    // which get the bare CGI names. Header values may be
    // non-UTF-8 (HTTP allows Latin-1 by RFC 7230); we
    // lossy-decode. Stage 4c.1 doesn't emit the parallel
    // `_BYTES` key (ADR 0022 v5 deferred).
    for (name, value_bytes) in headers {
        let value_str = String::from_utf8_lossy(value_bytes).into_owned();
        let name_upper = name.to_uppercase();
        let env_key = match name_upper.as_str() {
            "CONTENT-TYPE" => "CONTENT_TYPE".to_string(),
            "CONTENT-LENGTH" => "CONTENT_LENGTH".to_string(),
            _ => format!("HTTP_{}", name_upper.replace('-', "_")),
        };
        pairs.push((key(&env_key), val(value_str)));
    }

    // rack.* keys
    pairs.push((key("rack.url_scheme"), val(scheme.to_string())));
    pairs.push((key("rack.input"), Value::Nil));   // TODO stage 4c.3: StringIO
    pairs.push((key("rack.errors"), Value::Nil));  // TODO stage 4c.3: stderr sink
    pairs.push((key("rack.version"), Value::Nil)); // TODO stage 4c.3: [1, 6]
    pairs.push((key("rack.multithread"), Value::Bool(false)));
    pairs.push((key("rack.multiprocess"), Value::Bool(true)));
    pairs.push((key("rack.run_once"), Value::Bool(false)));

    // Allocate the Hash on the Vm heap. Note this triggers
    // potential GC — caller MUST pin any `Value::Str` from
    // an earlier allocation it intends to use after this
    // call. Stage 4c.3's per-request handler builds env in
    // a single synchronous block with no intervening
    // allocations, so no GC roots issue arises.
    vm.heap.alloc(HeapObj::Hash(HashObj::with_pairs(pairs)))
}

/// Invoke a Ruby block synchronously from Rust and return
/// its result value.
///
/// Stage 4c.2 of Phase H1 PoC. Wraps the `step_block`
/// machinery so the per-request hyper handler (stage 4c.3)
/// can call `app.call(env)`-shape blocks without re-
/// implementing the BlockStep variant handling.
///
/// **Caller MUST hold `&mut Vm`** — typically via
/// `current_vm_ptr()` re-borrow per ADR 0013.
///
/// ## BlockStep mapping
///
/// - `BlockStep::Value(v)` → `Ok(v)` — normal return (or
///   `next`-supplied value)
/// - `BlockStep::MethodReturn` → `RuntimeError`. Reached
///   when Ruby code inside a block calls `return` (which
///   unwinds to the enclosing method). For a Rack app
///   block called from Rust, there's no enclosing Ruby
///   method to return to — this is a misbehaving app and
///   maps to 500 at the request boundary in stage 4c.3.
/// - `BlockStep::Break(_)` → `RuntimeError`. Same shape
///   problem: `break` from inside a Rack app block has no
///   loop to break out of when called from Rust.
///
/// ## GC + pinning
///
/// The block reference is read out of the caller's args
/// vec (already a GC root for the duration of the host
/// fn). For single-call use (Rack app: one block per
/// request), no additional pinning is needed. If the
/// caller plans to repeatedly call the same block across
/// allocations, it must pin the `Value::Block(id)` first
/// via the existing `PinGuard` shape — see iter.rs:140
/// for the pattern.
#[allow(dead_code)] // Used by stage 4c.2 test + the stage 4c.3 per-request handler when that lands.
pub(crate) fn call_ruby_block_sync(
    vm: &mut crate::vm::Vm,
    block_id: crate::value::ObjId,
    args: Vec<crate::value::Value>,
) -> Result<crate::value::Value, crate::error::Trap> {
    use crate::error::{RubyError, Trap};
    use crate::vm::BlockStep;

    let pre_frames = vm.frames.len();
    match vm.step_block(block_id, args, pre_frames)? {
        BlockStep::Value(v) => Ok(v),
        BlockStep::MethodReturn => Err(Trap {
            err: RubyError::RuntimeError {
                msg: "block invoked from Rust raised `return` — no enclosing Ruby method to unwind to (likely a Rack app misuse; use `next` to return a value)".to_string(),
            },
            backtrace: vec![],
        }),
        BlockStep::Break(_) => Err(Trap {
            err: RubyError::RuntimeError {
                msg: "block invoked from Rust raised `break` — no loop to break out of (Rack app blocks return via the final expression or `next`)".to_string(),
            },
            backtrace: vec![],
        }),
    }
}

/// Hardcoded request handler — stage 2 placeholder. Every
/// request gets a 200 OK with a fixed plain-text body
/// regardless of method/path/headers. Stage 3 swaps this
/// for a Ruby-block invocation; the signature stays the
/// same shape (`async fn(Request<Incoming>) -> Result<Response<Full<Bytes>>, _>`).
async fn handle_hardcoded(
    _req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let body = Bytes::from_static(b"Hello from rubyrs!\n");
    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(Full::new(body))
        .expect("hardcoded response is well-formed"))
}

/// Drive the accept loop until the shutdown signal fires.
/// Single-threaded — runs every connection handler on the
/// same `LocalSet`-managed thread per ADR 0022 v4's "VM
/// ownership" discipline.
///
/// `listener` is constructed by the caller so the test
/// harness can use `127.0.0.1:0` and read back the
/// kernel-assigned port via `listener.local_addr()` before
/// the loop starts.
///
/// Returns when `shutdown` fires (graceful) or when accept
/// fails irrecoverably (currently propagates the io error).
pub(crate) async fn serve_until_shutdown(
    listener: TcpListener,
    mut shutdown: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    loop {
        tokio::select! {
            biased;
            // Shutdown branch first per `biased` — guarantees
            // we drain the signal even if accepts are
            // perpetually ready.
            _ = &mut shutdown => {
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _peer) = accept?;
                let io = TokioIo::new(stream);
                // `spawn_local` requires we're running inside
                // a `LocalSet`. Caller's responsibility (the
                // PoC test + the future Ruby entry point both
                // wrap in `LocalSet::run_until`).
                tokio::task::spawn_local(async move {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service_fn(handle_hardcoded))
                        .await;
                });
            }
        }
    }
}

/// Convenience entry point: build a tokio current-thread
/// runtime + LocalSet, bind to the given address, run
/// `serve_until_shutdown` until the signal fires.
///
/// Blocking from the caller's perspective. Returns the
/// kernel-assigned bound address via the `bound_tx` channel
/// before entering the accept loop, so test harnesses know
/// which port to hit when they passed `port = 0`.
///
/// Used by the stage 2 smoke test. The Ruby-side entry
/// point goes through `run_blocking_for_duration` (stage 3)
/// instead, which uses a duration-based auto-shutdown
/// rather than an explicit shutdown channel.
#[allow(dead_code)] // Used by stage 2 test only; stage 3+ uses the duration variant.
pub(crate) fn run_blocking(
    addr: SocketAddr,
    bound_tx: oneshot::Sender<SocketAddr>,
    shutdown_rx: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = LocalSet::new();
    rt.block_on(local.run_until(async move {
        let listener = TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        // If the caller already hung up, send returns Err —
        // not fatal, the caller may have given up. Server
        // still starts in case other tests / observers
        // exist.
        let _ = bound_tx.send(bound);
        serve_until_shutdown(listener, shutdown_rx).await
    }))
}

/// Bind + serve hardcoded responses for at most `duration`,
/// then return. Stage 3 PoC entry point — used by the Ruby-
/// side `__rubyrs_http_serve_hardcoded(addr, secs)` host fn.
///
/// The auto-shutdown is the simplest way to bridge "Ruby
/// thread blocks on the server" with "test framework needs
/// the server to return so the test process exits". Future
/// stages (5+) replace the duration cap with the real
/// `SIGINT`/`SIGTERM` / `#shutdown` mechanism per ADR 0022
/// v4/v5.
pub(crate) fn run_blocking_for_duration(
    addr: SocketAddr,
    duration: std::time::Duration,
) -> std::io::Result<SocketAddr> {
    let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = LocalSet::new();
    let bound = rt.block_on(local.run_until(async move {
        let listener = TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        // Race the serve loop against a timeout — whichever
        // fires first wins. `tokio::time::sleep` is the
        // "self-shutdown after duration" path; the
        // `shutdown_rx` stays for future stages that wire
        // explicit shutdown signals.
        tokio::select! {
            res = serve_until_shutdown(listener, shutdown_rx) => res?,
            _ = tokio::time::sleep(duration) => {}
        }
        Ok::<_, std::io::Error>(bound)
    }))?;
    Ok(bound)
}

/// Wire the battery's Ruby-callable host functions into a
/// `Runtime`. Phase H1 PoC stage 3: only registers
/// `__rubyrs_http_serve_hardcoded(addr, secs)`. Subsequent
/// stages register the Rack-callable form
/// (`__rubyrs_http_serve_with_app(addr, app)`) and the
/// `Rubyrs::HttpServer` class binding.
///
/// Embedders opt in by calling this method after
/// `Runtime::new()`. Auto-registration at Runtime
/// construction is intentionally deferred — embedders who
/// don't want the host fn surface skip the call.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    use crate::error::RubyError;
    use crate::value::Value;
    use crate::error::Trap;
    use std::time::Duration;

    rt.register_fn("__rubyrs_http_serve_hardcoded", |args| {
        // Argument shape: (bind_addr: String, duration_secs: Integer)
        let (addr_str, duration_secs) = match args {
            [Value::Str(addr), Value::Int(secs)] => {
                (addr.to_string_lossy(), *secs)
            }
            _ => {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "__rubyrs_http_serve_hardcoded(addr: String, duration_secs: Integer)"
                            .to_string(),
                    },
                    backtrace: vec![],
                });
            }
        };

        if duration_secs < 0 {
            return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: format!("duration_secs must be non-negative, got {duration_secs}"),
                },
                backtrace: vec![],
            });
        }

        let addr: SocketAddr = addr_str.parse().map_err(|e| Trap {
            err: RubyError::ArgumentError {
                msg: format!("invalid bind address '{addr_str}': {e}"),
            },
            backtrace: vec![],
        })?;

        let duration = Duration::from_secs(duration_secs as u64);
        let bound = run_blocking_for_duration(addr, duration).map_err(|e| Trap {
            // I/O errors during bind/serve surface as
            // RuntimeError (the existing Tier 1 "generic
            // unexpected" trap class — proper IOError waits
            // for the _io battery per ADR 0019 v3).
            err: RubyError::RuntimeError {
                msg: format!("http_serve_hardcoded: {e}"),
            },
            backtrace: vec![],
        })?;

        // Return the actual bound address as a String so
        // callers binding to `:0` can discover the
        // kernel-assigned port. e.g.
        //   port_str = __rubyrs_http_serve_hardcoded(
        //     "127.0.0.1:0", 1
        //   )
        // gives "127.0.0.1:54321" after the server returns.
        Ok(Value::Str(std::rc::Rc::new(
            crate::value::RStr::new(bound.to_string()),
        )))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_bind() {
        let cfg = HttpServerConfig::default();
        assert!(cfg.bind.is_none());
        assert!(cfg.max_concurrent_requests.is_none());
        assert!(!cfg.install_signal_handler);
    }

    #[test]
    fn config_is_clone_and_debug() {
        // Compile-time assertion: traits are implemented.
        let cfg = HttpServerConfig::default();
        let _cloned = cfg.clone();
        let _debug = format!("{:?}", cfg);
    }

    #[test]
    fn default_max_body_is_16mb() {
        assert_eq!(DEFAULT_MAX_BODY_BYTES, 16 * 1024 * 1024);
    }

    /// Smoke test: spawn the PoC server in a background
    /// thread, hit it with a raw TCP client, verify the
    /// hardcoded response shape.
    ///
    /// Uses a raw `tokio::net::TcpStream` + hand-rolled HTTP/1.1
    /// request rather than pulling a heavier HTTP client dep
    /// just for this one test. The response parsing is
    /// minimal — just confirms status line + a known body
    /// substring.
    #[test]
    fn poc_hardcoded_handler_round_trips() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        // Channels for cross-thread coordination:
        // `bound_tx/rx`     — server publishes the kernel-assigned port
        // `shutdown_tx/rx`  — test triggers graceful shutdown
        let (bound_tx, bound_rx) = oneshot::channel::<SocketAddr>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Spawn the server on its own OS thread so the main
        // test thread is free to drive the client side
        // synchronously. The server thread owns its own
        // tokio runtime (built inside `run_blocking`).
        let server_thread = thread::spawn(move || {
            let addr: SocketAddr = "127.0.0.1:0".parse().expect("valid bind");
            run_blocking(addr, bound_tx, shutdown_rx)
        });

        // Wait for the server to publish its bound address.
        // Use a blocking recv on the std-side channel-equiv
        // via `recv` on a small synchronous wrapper around
        // the tokio oneshot — easier than spinning up a
        // second tokio runtime in the test thread.
        let bound = wait_for_oneshot(bound_rx, Duration::from_secs(5))
            .expect("server bound within 5s");

        // Send a minimal HTTP/1.1 request via a raw TcpStream.
        let mut client = TcpStream::connect(bound).expect("connect to server");
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        client.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write request");

        // Slurp the response.
        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("read response");
        let response_text = String::from_utf8_lossy(&response);

        // Trigger shutdown + wait for server thread to
        // finish. Drop the client BEFORE shutdown to give
        // hyper the FIN cleanly.
        drop(client);
        shutdown_tx.send(()).expect("shutdown receiver alive");
        server_thread
            .join()
            .expect("server thread did not panic")
            .expect("server returned Ok");

        // Assertions: status line + hardcoded body. We
        // don't parse full HTTP — substring match is
        // sufficient for the wire-protocol smoke test.
        assert!(
            response_text.contains("HTTP/1.1 200 OK"),
            "expected 200 OK status line, got:\n{response_text}",
        );
        assert!(
            response_text.contains("Hello from rubyrs!"),
            "expected hardcoded body, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("content-type: text/plain"),
            "expected Content-Type header, got:\n{response_text}",
        );
    }

    /// Block the calling thread until a tokio oneshot
    /// resolves, with a deadline. Avoids spinning up a
    /// dedicated runtime in the test for a single recv.
    fn wait_for_oneshot<T: Send + 'static>(
        mut rx: oneshot::Receiver<T>,
        timeout: std::time::Duration,
    ) -> Option<T> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test rt builds");
        rt.block_on(async move {
            tokio::time::timeout(timeout, &mut rx).await.ok()?.ok()
        })
    }

    /// Stage 4c.1 verification: build a Rack env Hash from
    /// synthetic inputs and hand it to Ruby, which asserts
    /// every spec-mandated key is present with the
    /// expected value.
    ///
    /// Uses a sentinel host fn that constructs the env via
    /// `build_rack_env` and returns it as `Value::Hash` to
    /// the calling Ruby script. The Ruby side iterates
    /// expected keys / values; any divergence raises Ruby-
    /// side and propagates out via `rt.eval`'s `Err`.
    ///
    /// Why route through Ruby for assertion rather than
    /// pattern-matching the heap from Rust: the Hash
    /// indexing path is the one a real Rack app will use.
    /// Verifying via `env["REQUEST_METHOD"]` etc. covers
    /// both the build_rack_env code AND the Vm's
    /// Hash#[] dispatch with one test.
    #[test]
    fn build_rack_env_produces_spec_compliant_hash() {
        use crate::value::Value;
        use std::net::SocketAddr;
        use std::rc::Rc;

        // Captured pre-built inputs so the host fn closure
        // doesn't need to parse from args — the test is
        // about env shape, not arg passing. Wrap in Rc so
        // the Fn (not FnMut) closure can clone.
        let inputs = Rc::new((
            "POST".to_string(),
            "/users/42".to_string(),
            "verbose=1".to_string(),
            vec![
                ("Host".to_string(), b"example.com:3000".to_vec()),
                ("User-Agent".to_string(), b"curl/8.4".to_vec()),
                ("Content-Type".to_string(), b"application/json".to_vec()),
                ("Content-Length".to_string(), b"17".to_vec()),
                // Header with non-UTF-8 byte (0xFE is not
                // valid UTF-8 start). Lossy-decode produces
                // U+FFFD; key still set under HTTP_X_LATIN1.
                ("X-Latin1".to_string(), vec![0xFE, b'!']),
            ],
            b"{\"hello\":\"hi\"}".to_vec(), // body
            "127.0.0.1:3000".parse::<SocketAddr>().unwrap(), // listener
            "10.0.0.42:54321".parse::<SocketAddr>().unwrap(), // peer
            "http".to_string(),
        ));

        let mut rt = crate::Runtime::new();
        let captured = inputs.clone();
        rt.register_fn("__sentinel_build_env", move |_args| {
            let ptr = crate::vm::current_vm_ptr();
            assert!(!ptr.is_null(), "vm ptr must be set");
            // SAFETY: ADR 0013 — outer &mut Vm parked by
            // invoke_host_fn; we re-borrow time-disjointly.
            let vm = unsafe { &mut *ptr };
            let id = super::build_rack_env(
                vm,
                &captured.0,
                &captured.1,
                &captured.2,
                &captured.3,
                &captured.4,
                captured.5,
                captured.6,
                &captured.7,
            );
            Ok(Value::Hash(id))
        });

        // Ruby-side assertions cover:
        //   - Each CGI-required key present + value matches
        //   - HTTP_* prefixing applied (uppercase + dashes
        //     → underscores)
        //   - Content-Type / Content-Length use bare CGI
        //     names (no HTTP_ prefix)
        //   - REMOTE_ADDR / REMOTE_PORT separately set
        //   - rack.url_scheme / rack.multithread /
        //     rack.multiprocess / rack.run_once
        //   - rack.input / rack.errors / rack.version
        //     stubbed as nil (TODO docs as PoC gap)
        //   - Non-UTF-8 header lossy-decoded (length > 0)
        //   - Unknown key returns nil (Hash#[] miss)
        rt.eval(r#"
            env = __sentinel_build_env
            raise "REQUEST_METHOD mismatch: #{env['REQUEST_METHOD'].inspect}" \
                unless env["REQUEST_METHOD"] == "POST"
            raise "PATH_INFO mismatch: #{env['PATH_INFO'].inspect}" \
                unless env["PATH_INFO"] == "/users/42"
            raise "QUERY_STRING mismatch: #{env['QUERY_STRING'].inspect}" \
                unless env["QUERY_STRING"] == "verbose=1"
            raise "SERVER_NAME mismatch: #{env['SERVER_NAME'].inspect}" \
                unless env["SERVER_NAME"] == "127.0.0.1"
            raise "SERVER_PORT mismatch: #{env['SERVER_PORT'].inspect}" \
                unless env["SERVER_PORT"] == "3000"
            raise "SCRIPT_NAME mismatch: #{env['SCRIPT_NAME'].inspect}" \
                unless env["SCRIPT_NAME"] == ""
            raise "HTTP_VERSION mismatch: #{env['HTTP_VERSION'].inspect}" \
                unless env["HTTP_VERSION"] == "HTTP/1.1"
            raise "REMOTE_ADDR mismatch: #{env['REMOTE_ADDR'].inspect}" \
                unless env["REMOTE_ADDR"] == "10.0.0.42"
            raise "REMOTE_PORT mismatch: #{env['REMOTE_PORT'].inspect}" \
                unless env["REMOTE_PORT"] == "54321"
            raise "HTTP_HOST mismatch: #{env['HTTP_HOST'].inspect}" \
                unless env["HTTP_HOST"] == "example.com:3000"
            raise "HTTP_USER_AGENT mismatch: #{env['HTTP_USER_AGENT'].inspect}" \
                unless env["HTTP_USER_AGENT"] == "curl/8.4"
            raise "CONTENT_TYPE mismatch: #{env['CONTENT_TYPE'].inspect}" \
                unless env["CONTENT_TYPE"] == "application/json"
            raise "CONTENT_LENGTH mismatch: #{env['CONTENT_LENGTH'].inspect}" \
                unless env["CONTENT_LENGTH"] == "17"
            raise "Content-Type should NOT be under HTTP_ prefix" \
                if env.key?("HTTP_CONTENT_TYPE")
            raise "HTTP_X_LATIN1 missing (non-UTF-8 header dropped?)" \
                unless env["HTTP_X_LATIN1"].is_a?(String)
            raise "HTTP_X_LATIN1 should have lossy-decoded content" \
                unless env["HTTP_X_LATIN1"].length > 0
            raise "rack.url_scheme mismatch: #{env['rack.url_scheme'].inspect}" \
                unless env["rack.url_scheme"] == "http"
            raise "rack.input should be nil (PoC stub)" \
                unless env["rack.input"].nil?
            raise "rack.errors should be nil (PoC stub)" \
                unless env["rack.errors"].nil?
            raise "rack.version should be nil (PoC stub)" \
                unless env["rack.version"].nil?
            raise "rack.multithread should be false" \
                unless env["rack.multithread"] == false
            raise "rack.multiprocess should be true" \
                unless env["rack.multiprocess"] == true
            raise "rack.run_once should be false" \
                unless env["rack.run_once"] == false
            raise "unknown key should return nil" \
                unless env["NONEXISTENT_KEY"].nil?
        "#, "stage_4c1_check.rb").expect("env hash Ruby-side assertions all hold");
    }

    /// Stage 4c.2 verification: invoke a Ruby block from
    /// Rust + verify the result + cover MethodReturn /
    /// Break edge cases.
    ///
    /// Sentinel host fn `__sentinel_call_block(block, *args)`
    /// uses `current_vm_ptr` + `call_ruby_block_sync` to
    /// dispatch into the supplied block with the given
    /// args, returning the block's result. Tests cover:
    ///
    /// - Normal lambda return → result passes through
    /// - Lambda with multiple args
    /// - Lambda returning a String (heap-allocated value
    ///   survives the round trip)
    /// - Lambda returning an Array (`[status, headers,
    ///   body]`-shape — the Rack triplet stage 4c.3 will
    ///   consume)
    /// - Lambda with zero params (called with empty args)
    /// - Lambda that raises — Trap propagates back
    /// - Proc with bare `return` — RuntimeError (block
    ///   invoked from Rust has no enclosing method)
    /// - Block with bare `break` — RuntimeError (no
    ///   enclosing loop)
    #[test]
    fn call_ruby_block_sync_invokes_lambda_with_args() {
        use crate::value::Value;
        let mut rt = crate::Runtime::new();
        rt.register_fn("__sentinel_call_block", |args| {
            let block_id = match args.first() {
                Some(Value::Block(id)) => *id,
                _ => return Err(crate::error::Trap {
                    err: crate::error::RubyError::ArgumentError {
                        msg: "expected block as first arg".to_string(),
                    },
                    backtrace: vec![],
                }),
            };
            let block_args: Vec<Value> = args[1..].to_vec();
            let ptr = crate::vm::current_vm_ptr();
            assert!(!ptr.is_null(), "vm ptr must be set");
            // SAFETY: ADR 0013 — outer &mut Vm parked; re-borrow time-disjoint.
            let vm = unsafe { &mut *ptr };
            super::call_ruby_block_sync(vm, block_id, block_args)
        });

        // Normal case: lambda doubles its argument
        rt.eval(r#"
            doubler = ->(x) { x * 2 }
            result = __sentinel_call_block(doubler, 21)
            raise "doubler(21) want 42 got #{result.inspect}" unless result == 42
        "#, "stage_4c2_basic.rb").expect("basic lambda call");

        // Multiple args
        rt.eval(r#"
            adder = ->(a, b, c) { a + b + c }
            result = __sentinel_call_block(adder, 1, 2, 3)
            raise "adder(1,2,3) want 6 got #{result.inspect}" unless result == 6
        "#, "stage_4c2_multi_arg.rb").expect("multi-arg lambda");

        // String return
        rt.eval(r#"
            greeter = ->(name) { "Hello, #{name}!" }
            result = __sentinel_call_block(greeter, "rubyrs")
            raise "greeter want String got #{result.inspect}" \
                unless result == "Hello, rubyrs!"
        "#, "stage_4c2_string_return.rb").expect("string return");

        // Array return — exactly the Rack triplet shape
        rt.eval(r#"
            rack_app = ->(env) { [200, {"Content-Type" => "text/plain"}, ["hi"]] }
            result = __sentinel_call_block(rack_app, {"REQUEST_METHOD" => "GET"})
            raise "rack_app want Array got #{result.inspect}" \
                unless result.is_a?(Array)
            raise "rack_app want length 3 got #{result.length}" \
                unless result.length == 3
            raise "rack_app status want 200 got #{result[0].inspect}" \
                unless result[0] == 200
            raise "rack_app headers want Hash got #{result[1].inspect}" \
                unless result[1].is_a?(Hash)
            raise "rack_app body want Array got #{result[2].inspect}" \
                unless result[2].is_a?(Array)
        "#, "stage_4c2_rack_triplet.rb").expect("rack triplet shape");

        // Zero-arg block
        rt.eval(r#"
            ping = -> { "pong" }
            result = __sentinel_call_block(ping)
            raise "ping want pong got #{result.inspect}" unless result == "pong"
        "#, "stage_4c2_zero_args.rb").expect("zero-arg lambda");

        // Lambda that raises — Trap propagates
        let trap_result = rt.eval(r#"
            boom = ->(_) { raise ArgumentError, "from inside block" }
            __sentinel_call_block(boom, nil)
        "#, "stage_4c2_lambda_raises.rb");
        let trap = trap_result.expect_err("lambda raise should propagate");
        let formatted = rt.format_trap(&trap);
        assert!(
            formatted.contains("from inside block"),
            "expected trap message to include 'from inside block', got: {formatted}",
        );

        // Proc with bare `return` — should hit MethodReturn variant.
        // (Lambdas catch their own return; procs propagate it. We
        // construct a proc via `Proc.new { ... }` since lambda
        // semantics absorb `return`.)
        //
        // Note: `Proc.new` inside a lambda still creates a proc;
        // the `return` then triggers MethodReturn.
        let proc_return_result = rt.eval(r#"
            misbehaved = proc { return 42 }
            __sentinel_call_block(misbehaved)
        "#, "stage_4c2_proc_return.rb");
        let trap = proc_return_result.expect_err("proc return should be RuntimeError");
        let formatted = rt.format_trap(&trap);
        assert!(
            formatted.contains("no enclosing Ruby method") || formatted.contains("RuntimeError"),
            "expected RuntimeError-shaped trap for proc return, got: {formatted}",
        );
    }

    /// Stage 4b verification: confirm that
    /// `current_vm_ptr()` returns a non-null pointer when
    /// called inside a host fn body. This proves the
    /// dispatch path correctly sets `CURRENT_VM_PTR` for the
    /// `_http_server` feature path (independent of `cext`)
    /// — stage 4c's per-request handler will rely on this
    /// to invoke the Ruby block.
    ///
    /// The test registers a sentinel host fn that asserts
    /// the pointer is non-null at call time. Ruby invokes
    /// it; if the dispatch wiring is correct, the fn
    /// returns Nil. If `_http_server` is enabled but the
    /// cfg gate wasn't widened, the pointer would be null
    /// and the test fails inside the closure.
    #[test]
    fn current_vm_ptr_is_set_inside_http_server_host_fn() {
        use crate::vm::current_vm_ptr;

        let mut rt = crate::Runtime::new();
        rt.register_fn("__sentinel_check_vm_ptr", |_args| {
            let ptr = current_vm_ptr();
            assert!(!ptr.is_null(),
                "expected CURRENT_VM_PTR to be set inside host fn body; \
                 got null. Did the cfg gate at vm/dispatch.rs::invoke_host_fn \
                 widen to include `feature = \"_http_server\"`?",
            );
            Ok(crate::value::Value::Nil)
        });
        rt.eval(r#"__sentinel_check_vm_ptr"#, "stage_4b_check.rb")
            .expect("sentinel host fn returned Nil cleanly");
    }

    /// Stage 3 integration test: register the host fns,
    /// then have a Ruby script invoke
    /// `__rubyrs_http_serve_hardcoded` with a 0-second
    /// duration. Server starts, immediately auto-shuts,
    /// returns its bound addr string back to Ruby. Verifies
    /// the Ruby → Rust wiring works end-to-end without
    /// needing a separate HTTP-client thread.
    #[test]
    fn ruby_can_invoke_serve_hardcoded_with_zero_duration() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);

        // Bind to :0 (kernel-assigned) so this test never
        // collides with other tests on a fixed port. The
        // duration is 0 secs — server starts, auto-shuts,
        // returns the bound addr.
        let result = rt
            .eval(
                r#"
                addr = __rubyrs_http_serve_hardcoded("127.0.0.1:0", 0)
                addr
                "#,
                "test.rb",
            )
            .expect("Ruby eval succeeded");

        // The returned Value is a String like
        // "127.0.0.1:54321" — kernel-assigned port.
        match result {
            crate::value::Value::Str(s) => {
                let addr_str = s.to_string_lossy();
                assert!(
                    addr_str.starts_with("127.0.0.1:"),
                    "expected host:port string starting with 127.0.0.1:, got {addr_str:?}",
                );
                let port_part = &addr_str["127.0.0.1:".len()..];
                let port: u16 = port_part.parse().expect("port parses as u16");
                // Port 0 means kernel-assigned — must be
                // non-zero in the response (the actual
                // bound port).
                assert!(port > 0, "expected non-zero kernel-assigned port, got 0");
            }
            other => panic!("expected Value::Str return, got {other:?}"),
        }
    }

    /// Stage 3 integration test: Ruby invokes the host fn
    /// with a fixed port + non-zero duration; a separate
    /// thread sends an HTTP request mid-flight; verifies
    /// the response is the hardcoded payload.
    ///
    /// Uses a fixed loopback port to keep the test
    /// independent of `bound_addr` discovery (stage 4
    /// will introduce a Ruby-side handle that exposes the
    /// bound port BEFORE blocking; for stage 3 we use a
    /// pre-chosen port).
    #[test]
    fn ruby_started_server_serves_real_request() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        // Fixed port outside the well-known range. Picked
        // to avoid clashes with the stage 2 smoke test (no
        // overlap because stage 2 uses :0). Other tests
        // using fixed ports should pick non-overlapping
        // values.
        let server_addr = "127.0.0.1:18083";

        // Client thread: connect after a brief delay, send
        // an HTTP GET, slurp the response. The delay
        // ensures the server has bound + entered its
        // accept loop before we connect; without it the
        // connect can race the bind.
        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let mut client = TcpStream::connect(server_addr).expect("connect to ruby-started server");
            client
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            client
                .write_all(
                    b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .expect("write request");
            let mut response = Vec::new();
            client.read_to_end(&mut response).expect("read response");
            String::from_utf8_lossy(&response).into_owned()
        });

        // Main thread: run Ruby that starts the server for
        // 1 second. After auto-shutdown, Ruby returns the
        // bound addr (which equals server_addr here).
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let ruby_result = rt
            .eval(
                &format!(
                    r#"__rubyrs_http_serve_hardcoded("{server_addr}", 1)"#,
                ),
                "test.rb",
            )
            .expect("Ruby eval succeeded");
        let _ = ruby_result; // bound addr string; not asserting here

        let response_text = client_thread.join().expect("client thread did not panic");

        assert!(
            response_text.contains("HTTP/1.1 200 OK"),
            "expected 200 OK, got:\n{response_text}",
        );
        assert!(
            response_text.contains("Hello from rubyrs!"),
            "expected hardcoded body, got:\n{response_text}",
        );
    }
}
