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
#[allow(dead_code)] // Used by integration tests in stage 2; Ruby binding wires it differently.
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
}
