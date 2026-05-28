//! `_http_server` battery — Rust HTTP front with Ruby app
//! handler. Implements [ADR 0022 v4](../../docs/adr/0022-http-server-battery.md).
//!
//! Phase H1 PoC scope:
//! - HTTP/1.1 only, buffered body in + out
//! - Rack SPEC env hash construction (v1.6)
//! - `LocalSet`-based tokio current-thread integration
//! - Single-threaded; multi-core via pre-fork is later
//!
//! Out of PoC scope (lands later in H1 / H2+):
//! - `VmBorrow<'_>` RAII type — currently the Vm access
//!   discipline is reviewer-enforced
//! - `Runtime::reset_between_requests` API
//! - `Runtime::refill_fuel` per-request fuel re-anchor
//! - Per-request I/O deadline
//! - `max_header_bytes` config
//! - SIGINT / SIGTERM graceful shutdown
//! - `on_worker_boot` + `fork_workers` pre-fork
//! - `ResourceExhausted` → 503 catch at app boundary
//! - Non-UTF-8 header `_BYTES` parallel keys
//! - The `Rubyrs::HttpServer` Ruby class binding (PoC
//!   exposes the entry point as a `register_fn`-style host
//!   function instead)

#![cfg(feature = "_http_server")]

use std::net::SocketAddr;

/// Per-server configuration. v1 field set per ADR 0022 v4
/// "HttpServerConfig" section. PoC only honours `bind`;
/// other fields are accepted but ignored.
#[derive(Debug, Clone)]
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

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            bind: None,
            max_concurrent_requests: None,
            max_request_body_bytes: None,
            max_header_bytes: None,
            per_request_io_deadline: None,
            per_request_fuel: None,
            install_signal_handler: false,
        }
    }
}

/// PoC default request-body cap (16 MB) per ADR 0022 v4.
#[allow(dead_code)] // Used once the request handler lands in Phase H1.2.
pub(crate) const DEFAULT_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

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
}
