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
use http_body_util::{BodyExt, Full};
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

/// Maximum workers a single prefork invocation can spawn.
/// Cap exists because the child-pid table for the parent's
/// async-signal-safe forwarding handler is a fixed-size
/// static array. 64 covers any realistic CPU count.
#[cfg(target_family = "unix")]
const MAX_PREFORK_WORKERS: usize = 64;

/// Parent-side state for FU1 signal forwarding. The
/// SIGINT/SIGTERM handler reads PREFORK_CHILD_PIDS and
/// forwards the signal to each non-zero entry via
/// `kill(pid, sig)`. AtomicI32 lets the handler load
/// values without taking a lock — async-signal-safe.
///
/// Reused across invocations of the prefork host fn;
/// reset at the top of each fork loop.
#[cfg(target_family = "unix")]
static PREFORK_CHILD_PIDS: [std::sync::atomic::AtomicI32; MAX_PREFORK_WORKERS] = [
    // 64 zeros — manual expansion of `[AtomicI32::new(0); N]`,
    // which clippy flags via declare_interior_mutable_const
    // when used inside a `const ZERO = ...; [ZERO; N]` form.
    // The repeated-call form is the idiomatic alternative.
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
    const { std::sync::atomic::AtomicI32::new(0) },
];

/// Set by the SIGINT/SIGTERM handler to tell the FU2
/// supervisor "do NOT restart exited children — we asked
/// them to die." Without this, the supervisor can't
/// distinguish a clean-shutdown exit from a crash and
/// would keep respawning workers we just SIGTERMed.
///
/// AtomicBool is async-signal-safe (lock-free atomic
/// store is on POSIX §2.4.3's safe list in practice).
#[cfg(target_family = "unix")]
static PREFORK_SHUTDOWN_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Signal-handler entry point that forwards SIGINT/SIGTERM
/// to all known prefork children. Must remain async-
/// signal-safe — only `kill(2)` + atomic stores here.
///
/// Without this, external `kill <parent_pid>` (e.g., from
/// a process manager that doesn't know about the
/// children's pgroup) wouldn't propagate to the workers,
/// and they'd serve until their duration timer expired.
#[cfg(target_family = "unix")]
extern "C" fn prefork_forward_signal(sig: libc::c_int) {
    PREFORK_SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
    for slot in PREFORK_CHILD_PIDS.iter() {
        let pid = slot.load(std::sync::atomic::Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill(2) is async-signal-safe (POSIX
            // §2.4.3). Stale pids return ESRCH harmlessly.
            unsafe { libc::kill(pid, sig); }
        }
    }
}

/// Child-side entry: bind a SO_REUSEPORT listener, run
/// on_worker_boot (if supplied), serve until duration or
/// signal. Never returns — always `libc::exit()`s, matching
/// Stage 7d's fork-safety contract. Extracted here so the
/// FU2 restart-on-crash supervisor can re-invoke a fresh
/// worker into the same slot when a previous one dies.
///
/// On a clean serve completion, exit 0. On bind / boot /
/// serve error, print a diagnostic to inherited stderr
/// and exit 1 — the parent observes via WEXITSTATUS but
/// the exit reason isn't structured.
#[cfg(target_family = "unix")]
fn run_one_child_worker(
    addr: SocketAddr,
    duration_secs: i64,
    block_id: crate::value::ObjId,
    on_worker_boot_id: Option<crate::value::ObjId>,
    worker_index: i64,
) -> ! {
    use crate::value::Value;

    // FU1: reset SIGINT/SIGTERM to defaults before tokio's
    // runtime installs its own. The parent's forwarding
    // handler was inherited but the child's COW copy of
    // the pid table is stale; tokio overwrites these once
    // serving starts.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }

    let result: Result<(), String> = (|| {
        let listener = bind_reuseport_v4(addr)
            .map_err(|e| format!("child {worker_index} bind: {e}"))?;
        if let Some(boot_id) = on_worker_boot_id {
            let ptr = crate::vm::current_vm_ptr();
            if ptr.is_null() {
                return Err(format!("child {worker_index}: CURRENT_VM_PTR null"));
            }
            // SAFETY: same Vm pointer as parent pre-fork;
            // COW means we now own the child's copy.
            let vm = unsafe { &mut *ptr };
            let idx_val = Value::Int(worker_index);
            call_ruby_block_sync(vm, boot_id, vec![idx_val])
                .map_err(|trap| format!(
                    "child {worker_index} on_worker_boot raised: {}",
                    trap.err.message(),
                ))?;
        }
        let duration = std::time::Duration::from_secs(duration_secs as u64);
        run_blocking_for_duration_with_app_on_listener(
            listener, duration, block_id, None, None,
            DEFAULT_MAX_BODY_BYTES, None, None, None, true,
        ).map_err(|e| format!("child {worker_index} serve: {e}"))?;
        Ok(())
    })();
    let exit_code = match result {
        Ok(()) => 0,
        Err(msg) => {
            eprintln!("rubyrs prefork: {msg}");
            1
        }
    };
    // Flush stdio buffers before libc::exit — that path
    // skips Rust's normal drop handlers, so a piped
    // stdout (fully-buffered, common in subprocess
    // pipelines) would otherwise drop the BOOTED/etc
    // markers the parent expects.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: end of child's responsibility. libc::exit
    // skips Rust drop handlers — fork-safe per ADR 0022 v3.
    unsafe { libc::exit(exit_code) };
}

/// Install the SIGINT + SIGTERM forwarding handler. Idempotent
/// in practice — `sigaction` overwrites prior handlers, and
/// the static state is per-process so a second prefork call
/// reuses the same handler safely.
#[cfg(target_family = "unix")]
fn install_prefork_signal_handlers() {
    use std::mem::MaybeUninit;
    unsafe {
        let mut act: MaybeUninit<libc::sigaction> = MaybeUninit::zeroed();
        let ptr = act.as_mut_ptr();
        (*ptr).sa_sigaction = prefork_forward_signal as *const () as libc::sighandler_t;
        // SA_RESTART: the parent's blocking waitpid resumes
        // after the handler returns instead of failing with
        // EINTR. The handler's only job is forwarding; the
        // actual reap stays in the waitpid loop.
        (*ptr).sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut (*ptr).sa_mask);
        let act_init = act.assume_init();
        libc::sigaction(libc::SIGINT, &act_init, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &act_init, std::ptr::null_mut());
    }
}

/// Bind a TCP listener with `SO_REUSEADDR + SO_REUSEPORT`
/// set, returning a `std::net::TcpListener` (NOT tokio's —
/// the tokio runtime must be built post-`fork(2)` in each
/// child per ADR 0022 v3 §"Multi-core scaling", so we keep
/// the listener in std form until each worker converts it
/// inside its own runtime).
///
/// `SO_REUSEPORT` (Linux 3.9+, BSDs incl. macOS) lets
/// multiple processes bind the same `(addr, port)` —
/// the kernel hash-distributes incoming connections across
/// the bound sockets. This is the foundational primitive
/// for Stage 7 pre-fork: each child opens its own listener
/// on the same port, no cross-child socket sharing or
/// accept-thundering-herd.
///
/// `SO_REUSEADDR` is set alongside to allow rapid restarts
/// (TIME_WAIT skip), matching Puma's listener defaults.
///
/// On non-Unix targets returns `Unsupported` — Windows
/// has no equivalent kernel-level load-balancing primitive
/// and Stage 7 N>=2 is gated off there. See ADR 0022 v3.
#[cfg(unix)]
#[allow(dead_code)] // wired up in 7b refactor + 7c/7d prefork host fn
pub(crate) fn bind_reuseport_v4(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    // Backlog 1024 matches Puma's default and is high
    // enough that bursts don't fall off the SYN queue
    // before the worker can accept. Linux clamps to
    // `net.core.somaxconn` (default 4096), so 1024 is
    // safe everywhere.
    socket.listen(1024)?;
    Ok(socket.into())
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub(crate) fn bind_reuseport_v4(_addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SO_REUSEPORT pre-fork is Unix-only (no Windows equivalent); use N=1 single-process mode",
    ))
}

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
/// Marshal a Rack triplet `[status, headers, body]` back into
/// HTTP-response components for hyper.
///
/// Stage 4c.3 of Phase H1 PoC. Expects the result of
/// `call_ruby_block_sync(app, [env])` — a `Value` that
/// SHOULD be a `Value::Array(id)` holding 3 elements.
///
/// ## Triplet shape (per Rack SPEC v1.6)
///
/// - **status**: `Value::Int` (HTTP status code, typically
///   100..=599). Negative values + values > u16::MAX trap.
/// - **headers**: `Value::Hash` with `String => String`
///   pairs. (Future-spec extension: `String => Array<String>`
///   for repeated headers like `Set-Cookie`; PoC v1 supports
///   single-value only.)
/// - **body**: `Value::Array` of `String`s. Iterated to a
///   single contiguous byte buffer (per ADR 0022 v4
///   "buffered response body" decision; streaming via
///   chunked transfer is Phase H3 + Fiber).
///
/// ## Error mapping
///
/// Any structural mismatch returns `Err(String)` — caller
/// (`handle_request_with_app`) maps to HTTP 500 with the
/// message as the response body. PoC doesn't yet route
/// through the `on_error` config field (ADR 0022 v5);
/// stage 6 wires that.
/// `(status, headers, body)` — the components a marshaled
/// Rack response decomposes into for hyper.
pub(crate) type MarshaledResponse = (u16, Vec<(String, String)>, bytes::Bytes);

#[allow(dead_code)] // Used by stage 4c.3 handler when wired.
pub(crate) fn marshal_rack_response(
    vm: &crate::vm::Vm,
    app_result: crate::value::Value,
) -> Result<MarshaledResponse, String> {
    use crate::value::Value;

    let arr_id = match app_result {
        Value::Array(id) => id,
        other => return Err(format!(
            "Rack app must return Array<status, headers, body>; got {}",
            other.type_name(),
        )),
    };
    let arr = vm.heap.array(arr_id);
    if arr.len() != 3 {
        return Err(format!(
            "Rack app Array must have exactly 3 elements (status, headers, body); got {}",
            arr.len(),
        ));
    }
    let status = match &arr[0] {
        Value::Int(n) => {
            if *n < 0 || *n > u16::MAX as i64 {
                return Err(format!(
                    "Rack status must be 0..=65535; got {n}",
                ));
            }
            *n as u16
        }
        other => return Err(format!(
            "Rack status must be Integer; got {}",
            other.type_name(),
        )),
    };
    let headers_id = match &arr[1] {
        Value::Hash(id) => *id,
        other => return Err(format!(
            "Rack headers must be Hash; got {}",
            other.type_name(),
        )),
    };
    let body_arr_id = match &arr[2] {
        Value::Array(id) => *id,
        other => return Err(format!(
            "Rack body must be Array<String>; got {} (streaming bodies need Fiber, Phase H3)",
            other.type_name(),
        )),
    };

    // Headers — Hash<String, String> only in PoC v1.
    let header_pairs = vm.heap.hash(headers_id);
    let mut headers: Vec<(String, String)> = Vec::with_capacity(header_pairs.len());
    for (k, v) in header_pairs {
        let key_str = match k {
            Value::Str(s) => s.to_string_lossy(),
            other => return Err(format!(
                "Rack header name must be String; got {}",
                other.type_name(),
            )),
        };
        let val_str = match v {
            Value::Str(s) => s.to_string_lossy(),
            other => return Err(format!(
                "Rack header value must be String (Array<String> for repeated headers not yet supported); got {} for key {:?}",
                other.type_name(), key_str,
            )),
        };
        headers.push((key_str, val_str));
    }

    // Body — Array<String> only in PoC v1. Concatenate to
    // a single Bytes for hyper's Full<Bytes> response.
    let body_chunks = vm.heap.array(body_arr_id);
    let total_len: usize = body_chunks.iter().map(|v| match v {
        Value::Str(s) => s.content.borrow().len(),
        _ => 0,
    }).sum();
    let mut body_bytes = Vec::with_capacity(total_len);
    for chunk in body_chunks {
        match chunk {
            Value::Str(s) => body_bytes.extend_from_slice(&s.content.borrow()),
            other => return Err(format!(
                "Rack body Array elements must be String; got {} in body",
                other.type_name(),
            )),
        }
    }

    Ok((status, headers, bytes::Bytes::from(body_bytes)))
}

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

/// Per-request handler that calls a Ruby block with the
/// Rack env hash and marshals the triplet back to a hyper
/// response.
///
/// Stage 4c.3 of Phase H1 PoC. Combines:
///   1. `build_rack_env` — synthesise env from request parts
///   2. `call_ruby_block_sync` — invoke the app with `[env]`
///   3. `marshal_rack_response` — Rack triplet → hyper
///
/// Returns 500 with a plain-text error message for any
/// marshaling failure (non-Array result, wrong-arity Array,
/// non-Integer status, non-String body chunk, etc.) — the
/// `on_error` ADR 0022 v5 hook isn't wired until stage 6.
///
/// **Vm access**: takes `block_id` + `listener_addr` by value;
/// retrieves `&mut Vm` from `current_vm_ptr()` for the
/// synchronous block of (env-build + call + marshal). Per
/// ADR 0013, this is time-disjoint with the outer
/// `invoke_host_fn`'s `&mut Vm` borrow.
#[allow(clippy::too_many_arguments)] // 8 args; flat for stage-by-stage growth
async fn handle_request_with_app(
    req: Request<Incoming>,
    block_id: crate::value::ObjId,
    on_error_block: Option<crate::value::ObjId>,
    listener_addr: SocketAddr,
    peer_addr: SocketAddr,
    per_request_fuel: Option<u64>,
    max_request_body_bytes: usize,
    per_request_io_deadline: Option<std::time::Duration>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    use crate::value::Value;
    use http_body_util::Limited;

    // Phase A: buffer request body (no Vm access; pure async I/O).
    //
    // Two protective layers stacked here:
    //   1. `Limited` short-circuits at `max_request_body_bytes`
    //      mid-stream — bounds memory regardless of how much
    //      the client claims to be sending.
    //   2. `tokio::time::timeout` (stage 6b) bounds wall-clock
    //      spent reading the body — defends against slow-
    //      upload attacks where a client claims a large
    //      Content-Length and dribbles bytes to hold the
    //      connection + handler reservation idle.
    //
    // The deadline ONLY covers the async I/O phase, not the
    // Ruby app's CPU work in Phase B. Per ADR 0022 v5: tokio
    // cannot preempt a synchronous Ruby block. CPU-bound
    // Ruby is bounded by per_request_fuel (stage 5d).
    let (parts, body) = req.into_parts();
    let limited = Limited::new(body, max_request_body_bytes);
    let collect_future = limited.collect();
    let collect_result = match per_request_io_deadline {
        Some(deadline) => match tokio::time::timeout(deadline, collect_future).await {
            Ok(inner) => inner,
            Err(_elapsed) => {
                return Ok(error_response(
                    504,
                    format!(
                        "Gateway Timeout: request body read exceeded {} ms",
                        deadline.as_millis(),
                    ),
                ));
            }
        },
        None => collect_future.await,
    };
    let body_bytes_full = match collect_result {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            // The `Limited` error type is boxed; downcast to
            // distinguish "too big" (413) from generic IO
            // failures (400). The downcast target is the
            // `LengthLimitError` type http-body-util uses
            // internally.
            let too_big = e.downcast_ref::<http_body_util::LengthLimitError>().is_some();
            let (status, msg) = if too_big {
                (413, format!(
                    "Payload Too Large: request body exceeds {max_request_body_bytes} bytes",
                ))
            } else {
                (400, format!("body read failed: {e}"))
            };
            return Ok(error_response(status, msg));
        }
    };

    // Extract request fields we need for env construction.
    // hyper's URI is already path-decoded; query is raw per
    // Rack SPEC. Headers preserved as (name, value-bytes)
    // tuples — `build_rack_env` lossy-decodes values that
    // aren't valid UTF-8.
    let method_str = parts.method.as_str().to_string();
    let path_str = parts.uri.path().to_string();
    let query_str = parts.uri.query().unwrap_or("").to_string();
    let headers_vec: Vec<(String, Vec<u8>)> = parts.headers.iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect();
    let body_vec: Vec<u8> = body_bytes_full.to_vec();

    // Phase B: synchronous Vm work — reset + refill + build
    // env + call block + marshal response. No .await between
    // these steps so the VmBorrow contract (ADR 0022 v5)
    // holds: while the Vm is borrowed, control does not yield
    // back to the tokio executor.
    //
    // Result type carries the HTTP status separately so the
    // `ResourceExhausted` path can map to 503 while other
    // app-side errors stay 500.
    let response_components: Result<MarshaledResponse, (u16, String)> = {
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Ok(error_response(
                500,
                "internal: CURRENT_VM_PTR null inside _http_server handler".to_string(),
            ));
        }
        // SAFETY: ADR 0013 contract. Outer `&mut Vm` parked
        // by `invoke_host_fn`; we re-borrow time-disjointly
        // inside this synchronous block. No .await reached
        // while the borrow is live.
        let vm = unsafe { &mut *ptr };

        // Stage 5d: per-request cleanup + fuel re-anchor.
        // Clears request-N's globals / control-flow signals
        // / pinned roots so they don't leak to request N+1.
        // Refill resets vm.fuel to the per-request budget
        // (when supplied) so a CPU-runaway request traps
        // without depleting the worker's lifetime budget.
        vm.reset_between_requests_inner();
        if let Some(n) = per_request_fuel {
            vm.fuel = Some(n);
        }

        let env_id = build_rack_env(
            vm,
            &method_str,
            &path_str,
            &query_str,
            &headers_vec,
            &body_vec,
            listener_addr,
            peer_addr,
            "http",  // PoC stage 4c.3: HTTPS via _http_server_tls battery (H5)
        );
        let env_val = Value::Hash(env_id);

        match call_ruby_block_sync(vm, block_id, vec![env_val.clone()]) {
            Ok(app_result) => marshal_rack_response(vm, app_result)
                .map_err(|msg| (500, msg)),
            Err(trap) => {
                // Stage 5d: ResourceExhausted (fuel / heap
                // / frames / deadline cap exhausted)
                // surfaces as 503 Service Unavailable —
                // distinct from 500 Internal Server Error
                // for app-side exceptions. The worker
                // SURVIVES this trap; the next request gets
                // its own reset_between_requests + refill.
                use crate::error::RubyError;
                let is_resource_exhausted = matches!(&trap.err, RubyError::ResourceExhausted { .. });

                // Stage 6f: when an `on_error` block is
                // configured AND the trap is not
                // ResourceExhausted, hand the error to the
                // embedder's mapper instead of returning a
                // hardcoded 500. ResourceExhausted stays
                // 503 unconditionally — it's a security
                // signal (worker hit a cap) and overriding
                // it would let app code mask runaways. The
                // on_error block receives `(env, err_class,
                // err_message)` and must return a Rack
                // triplet. If it itself raises or returns
                // malformed, we fall back to the plain 500.
                if !is_resource_exhausted {
                    if let Some(err_id) = on_error_block {
                        let class_str = Value::Str(std::rc::Rc::new(
                            crate::value::RStr::new(trap.err.class_name().to_string()),
                        ));
                        let msg_str = Value::Str(std::rc::Rc::new(
                            crate::value::RStr::new(trap.err.message()),
                        ));
                        match call_ruby_block_sync(vm, err_id, vec![env_val, class_str, msg_str]) {
                            Ok(handler_result) => marshal_rack_response(vm, handler_result)
                                .map_err(|msg| (500, format!("on_error block returned malformed Rack triplet: {msg}"))),
                            Err(handler_trap) => Err((500, format!(
                                "on_error block itself raised: {} (original: {})",
                                handler_trap.err.message(),
                                trap.err.message(),
                            ))),
                        }
                    } else {
                        Err((500, format!("Rack app raised: {}", trap.err.message())))
                    }
                } else {
                    Err((503, format!("Rack app raised: {}", trap.err.message())))
                }
            }
        }
    };
    // VmBorrow scope ends; Phase C can resume tokio await.

    // Phase C: marshal to hyper response.
    let response = match response_components {
        Ok((status, headers, body)) => {
            let mut builder = Response::builder().status(status);
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
            builder.body(Full::new(body)).unwrap_or_else(|e| {
                error_response(500, format!("response builder failed: {e}"))
            })
        }
        Err((status, msg)) => error_response(status, msg),
    };
    Ok(response)
}

/// Build a plain-text error response with the given status
/// and message body. Used by `handle_request_with_app` for
/// every internal/marshaling failure. PoC stage 4c.3 -- the
/// embedder-supplied `on_error` config field (ADR 0022 v5)
/// is wired in stage 6.
fn error_response(status: u16, msg: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(msg)))
        .expect("error response is well-formed")
}

/// Variant of `serve_until_shutdown` that invokes a Ruby
/// block per request instead of returning a hardcoded
/// response. Caller supplies the block_id; the listener's
/// connection handlers all close over the same block.
#[allow(clippy::too_many_arguments)] // 10 args; flat for stage-by-stage growth
async fn serve_with_app_until_shutdown(
    listener: TcpListener,
    block_id: crate::value::ObjId,
    on_error_block: Option<crate::value::ObjId>,
    listener_addr: SocketAddr,
    per_request_fuel: Option<u64>,
    max_request_body_bytes: usize,
    per_request_io_deadline: Option<std::time::Duration>,
    max_headers: Option<usize>,
    idle_timeout: Option<std::time::Duration>,
    install_signal_handler: bool,
    mut shutdown: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    // Stage 6d signal handlers.
    //
    // When `install_signal_handler` is true, the accept
    // loop races against SIGINT (Ctrl+C from a TTY) and
    // SIGTERM (default for k8s pod termination + systemd
    // `systemctl stop`). On Unix both wire up; on Windows
    // only Ctrl+C is available (tokio::signal::unix is
    // cfg(unix)-only). Either signal triggers graceful
    // shutdown — the accept loop breaks; in-flight
    // connection tasks complete on their own.
    //
    // When false (default), signals fall through to the
    // embedder's own handler, and shutdown only happens
    // via the explicit oneshot or the duration timeout
    // from the caller. This is the right default for
    // embeds where rubyrs is one subsystem among many and
    // the host owns signal routing.
    //
    // Pinned `Future` boxes let `select!` drive them by
    // reference each iteration without consuming. When
    // install_signal_handler is false we use
    // `future::pending` which never resolves — the
    // `select!` arm is effectively disabled.
    let mut sigint_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> =
        if install_signal_handler {
            Box::pin(async {
                let _ = tokio::signal::ctrl_c().await;
            })
        } else {
            Box::pin(std::future::pending::<()>())
        };
    // SIGTERM is Unix-only; on non-Unix the future is
    // `pending` so the `select!` arm is inert. This keeps
    // the macro free of cfg-attributes (which it doesn't
    // accept on branches).
    let mut sigterm_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> = {
        #[cfg(unix)]
        {
            if install_signal_handler {
                Box::pin(async {
                    use tokio::signal::unix::{signal, SignalKind};
                    if let Ok(mut sig) = signal(SignalKind::terminate()) {
                        sig.recv().await;
                    }
                })
            } else {
                Box::pin(std::future::pending::<()>())
            }
        }
        #[cfg(not(unix))]
        {
            Box::pin(std::future::pending::<()>())
        }
    };

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => return Ok(()),
            _ = &mut sigint_fut => return Ok(()),
            _ = &mut sigterm_fut => return Ok(()),
            accept = listener.accept() => {
                let (stream, peer_addr) = accept?;
                let io = TokioIo::new(stream);
                tokio::task::spawn_local(async move {
                    let svc = service_fn(move |req| {
                        handle_request_with_app(
                            req, block_id, on_error_block, listener_addr, peer_addr,
                            per_request_fuel, max_request_body_bytes,
                            per_request_io_deadline,
                        )
                    });
                    // hyper auto-responds 431 Request Header
                    // Fields Too Large when the request has
                    // more headers than `max_headers` (counts
                    // headers, not bytes). The parser layer
                    // handles this BEFORE our service_fn
                    // runs, so the app block is never
                    // invoked for an oversized-header
                    // request — same security guarantee as
                    // the Limited body cap (stage 6a) and
                    // the I/O deadline (stage 6b).
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    if let Some(n) = max_headers {
                        builder.max_headers(n);
                    }
                    // Stage 6e idle timeout (Bun parity).
                    //
                    // `header_read_timeout` caps the wait
                    // for HEADERS bytes — both on a freshly
                    // accepted TCP connection AND between
                    // requests on a keep-alive connection
                    // (when the server is waiting for the
                    // next request line). That maps exactly
                    // to Bun's `idleTimeout`: how long an
                    // otherwise-quiet keep-alive socket can
                    // hold a worker slot.
                    //
                    // hyper requires a Timer installed for
                    // header_read_timeout to fire — we wire
                    // TokioTimer only on the connection path
                    // (cheap, scoped per-conn).
                    if let Some(d) = idle_timeout {
                        builder.timer(hyper_util::rt::TokioTimer::new());
                        builder.header_read_timeout(d);
                    }
                    let _ = builder.serve_connection(io, svc).await;
                });
            }
        }
    }
}

/// Bind + serve via a Ruby app block for at most `duration`
/// seconds, then auto-shut. Returns the actual bound addr.
///
/// Stage 4c.3 entry point — wired into the
/// `__rubyrs_http_serve_with_app(addr, secs, app)` host fn
/// via `register_host_fns`.
#[allow(clippy::too_many_arguments)] // 10 args; flat for stage-by-stage growth
pub(crate) fn run_blocking_for_duration_with_app(
    addr: SocketAddr,
    duration: std::time::Duration,
    block_id: crate::value::ObjId,
    on_error_block: Option<crate::value::ObjId>,
    per_request_fuel: Option<u64>,
    max_request_body_bytes: usize,
    per_request_io_deadline: Option<std::time::Duration>,
    max_headers: Option<usize>,
    idle_timeout: Option<std::time::Duration>,
    install_signal_handler: bool,
) -> std::io::Result<SocketAddr> {
    let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = LocalSet::new();
    let bound = rt.block_on(local.run_until(async move {
        let listener = TcpListener::bind(addr).await?;
        let listener_addr = listener.local_addr()?;
        tokio::select! {
            res = serve_with_app_until_shutdown(
                listener, block_id, on_error_block, listener_addr,
                per_request_fuel, max_request_body_bytes,
                per_request_io_deadline,
                max_headers,
                idle_timeout,
                install_signal_handler,
                shutdown_rx,
            ) => res?,
            _ = tokio::time::sleep(duration) => {}
        }
        Ok::<_, std::io::Error>(listener_addr)
    }))?;
    Ok(bound)
}

/// Variant of `run_blocking_for_duration_with_app` that
/// takes a pre-bound `std::net::TcpListener` instead of
/// binding internally. Used by Stage 7's pre-fork path:
/// each forked child constructs its own listener via
/// `bind_reuseport_v4` and hands it here. The tokio
/// runtime is still built inside this fn — per ADR 0022
/// v3 §"Multi-core scaling", the runtime MUST be created
/// post-fork in each worker, never pre-fork in the
/// supervisor.
///
/// The std listener MUST already be non-blocking;
/// `bind_reuseport_v4` sets this, so callers using that
/// helper get it for free. If a caller hands a blocking
/// listener, `TcpListener::from_std` returns an error.
#[cfg(unix)]
#[allow(dead_code)] // wired up in 7c/7d prefork host fn
#[allow(clippy::too_many_arguments)] // 10 args; flat for stage-by-stage growth
pub(crate) fn run_blocking_for_duration_with_app_on_listener(
    std_listener: std::net::TcpListener,
    duration: std::time::Duration,
    block_id: crate::value::ObjId,
    on_error_block: Option<crate::value::ObjId>,
    per_request_fuel: Option<u64>,
    max_request_body_bytes: usize,
    per_request_io_deadline: Option<std::time::Duration>,
    max_headers: Option<usize>,
    idle_timeout: Option<std::time::Duration>,
    install_signal_handler: bool,
) -> std::io::Result<SocketAddr> {
    let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = LocalSet::new();
    let bound = rt.block_on(local.run_until(async move {
        // Convert std → tokio listener INSIDE the runtime.
        // `from_std` requires the underlying socket to be
        // non-blocking; `bind_reuseport_v4` sets this.
        let listener = TcpListener::from_std(std_listener)?;
        let listener_addr = listener.local_addr()?;
        tokio::select! {
            res = serve_with_app_until_shutdown(
                listener, block_id, on_error_block, listener_addr,
                per_request_fuel, max_request_body_bytes,
                per_request_io_deadline,
                max_headers,
                idle_timeout,
                install_signal_handler,
                shutdown_rx,
            ) => res?,
            _ = tokio::time::sleep(duration) => {}
        }
        Ok::<_, std::io::Error>(listener_addr)
    }))?;
    Ok(bound)
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

    rt.register_fn("__rubyrs_http_serve_with_app", |args| {
        // Argument shape (3 / 4 / 5 / 6 / 7 / 8 / 9 / 10 args, growing):
        //   (addr, secs, app)
        //   (addr, secs, app, per_request_fuel)
        //   (addr, secs, app, per_request_fuel, max_body_bytes)
        //   (addr, secs, app, per_request_fuel, max_body_bytes, io_deadline_ms)
        //   (addr, secs, app, per_request_fuel, max_body_bytes, io_deadline_ms, max_headers)
        //   (addr, secs, app, per_request_fuel, max_body_bytes, io_deadline_ms, max_headers, install_signal_handler)
        //   (addr, secs, app, per_request_fuel, max_body_bytes, io_deadline_ms, max_headers, install_signal_handler, idle_timeout_ms)
        //   (addr, secs, app, per_request_fuel, max_body_bytes, io_deadline_ms, max_headers, install_signal_handler, idle_timeout_ms, on_error)
        //
        // Each positional adds one more security knob. Per
        // ADR 0022 v5 these will eventually move into a
        // Hash arg (Bun-shape) to avoid 8-positional creep;
        // PoC keeps positional for now.
        //
        // Negative-value checks are factored into a helper
        // so each arity branch stays a one-liner.
        let check_non_negative = |label: &str, n: i64| -> Result<(), Trap> {
            if n < 0 {
                Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!("{label} must be non-negative, got {n}"),
                    },
                    backtrace: vec![],
                })
            } else {
                Ok(())
            }
        };
        // Helper for io_deadline_ms semantics (0 disables).
        let parse_io_deadline = |ms: i64| -> Option<Duration> {
            if ms == 0 { None } else { Some(Duration::from_millis(ms as u64)) }
        };
        // max_headers helper: 0 → hyper default (100); >0 →
        // explicit cap. Matches the "0 disables / unsets"
        // idiom used by io_deadline_ms above.
        let parse_max_headers = |max_h: i64| -> Result<Option<usize>, Trap> {
            if max_h < 0 {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!("max_headers must be non-negative, got {max_h}"),
                    },
                    backtrace: vec![],
                });
            }
            Ok(if max_h == 0 { None } else { Some(max_h as usize) })
        };
        // install_signal_handler validator — flag is binary
        // today; ambiguous input surfaces ArgumentError
        // rather than silently treating "2" as truthy.
        let parse_sig_flag = |sig: i64| -> Result<bool, Trap> {
            if sig != 0 && sig != 1 {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!("install_signal_handler must be 0 or 1, got {sig}"),
                    },
                    backtrace: vec![],
                });
            }
            Ok(sig == 1)
        };
        // idle_timeout helper: 0 → no cap (rely on the
        // OS/peer to close); >0 → header_read_timeout cap
        // applied per-connection (covers both initial-
        // headers wait and the inter-request gap on a
        // keep-alive socket).
        let parse_idle_timeout = |ms: i64| -> Result<Option<Duration>, Trap> {
            if ms < 0 {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!("idle_timeout_ms must be non-negative, got {ms}"),
                    },
                    backtrace: vec![],
                });
            }
            Ok(if ms == 0 { None } else { Some(Duration::from_millis(ms as u64)) })
        };
        let (addr_str, duration_secs, block_id, per_request_fuel, max_body_bytes, io_deadline, max_headers, install_signal_handler, idle_timeout, on_error_block) = match args {
            [Value::Str(addr), Value::Int(secs), Value::Block(id)] => {
                (addr.to_string_lossy(), *secs, *id, None, DEFAULT_MAX_BODY_BYTES, None, None, false, None, None)
            }
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), DEFAULT_MAX_BODY_BYTES, None, None, false, None, None)
            }
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel), Value::Int(max_body)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                check_non_negative("max_body_bytes", *max_body)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), *max_body as usize, None, None, false, None, None)
            }
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel), Value::Int(max_body), Value::Int(io_ms)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                check_non_negative("max_body_bytes", *max_body)?;
                check_non_negative("io_deadline_ms", *io_ms)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), *max_body as usize, parse_io_deadline(*io_ms), None, false, None, None)
            }
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel), Value::Int(max_body), Value::Int(io_ms), Value::Int(max_h)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                check_non_negative("max_body_bytes", *max_body)?;
                check_non_negative("io_deadline_ms", *io_ms)?;
                let max_headers = parse_max_headers(*max_h)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), *max_body as usize, parse_io_deadline(*io_ms), max_headers, false, None, None)
            }
            // Stage 6d: 8-arg form with install_signal_handler
            // flag. Integer 0/1 (rubyrs has no native Bool yet
            // in host-fn args; convention shared with existing
            // 0-disables knobs). When 1, the serve loop wires
            // SIGINT (all platforms) and SIGTERM (Unix only)
            // to graceful shutdown. Defaults to false in
            // shorter arities so existing embeds opt in.
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel), Value::Int(max_body), Value::Int(io_ms), Value::Int(max_h), Value::Int(sig)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                check_non_negative("max_body_bytes", *max_body)?;
                check_non_negative("io_deadline_ms", *io_ms)?;
                let max_headers = parse_max_headers(*max_h)?;
                let install_sig = parse_sig_flag(*sig)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), *max_body as usize, parse_io_deadline(*io_ms), max_headers, install_sig, None, None)
            }
            // Stage 6e: 9-arg form adds idle_timeout_ms.
            // Caps keep-alive idle time per-connection via
            // hyper's header_read_timeout (fires for both
            // slow initial headers AND idle gap between
            // requests on keep-alive). 0 → no cap.
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel), Value::Int(max_body), Value::Int(io_ms), Value::Int(max_h), Value::Int(sig), Value::Int(idle_ms)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                check_non_negative("max_body_bytes", *max_body)?;
                check_non_negative("io_deadline_ms", *io_ms)?;
                let max_headers = parse_max_headers(*max_h)?;
                let install_sig = parse_sig_flag(*sig)?;
                let idle = parse_idle_timeout(*idle_ms)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), *max_body as usize, parse_io_deadline(*io_ms), max_headers, install_sig, idle, None)
            }
            // Stage 6f: 10-arg form adds the optional
            // `on_error` block. When the main app raises a
            // non-ResourceExhausted trap, the server hands
            // (env, err_class, err_message) to this block
            // and expects a Rack triplet back. If on_error
            // itself raises or returns malformed, we fall
            // back to the plain 500. ResourceExhausted
            // stays 503 unconditionally — it's a security
            // signal that app code must not override.
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(fuel), Value::Int(max_body), Value::Int(io_ms), Value::Int(max_h), Value::Int(sig), Value::Int(idle_ms), Value::Block(err_id)] => {
                check_non_negative("per_request_fuel", *fuel)?;
                check_non_negative("max_body_bytes", *max_body)?;
                check_non_negative("io_deadline_ms", *io_ms)?;
                let max_headers = parse_max_headers(*max_h)?;
                let install_sig = parse_sig_flag(*sig)?;
                let idle = parse_idle_timeout(*idle_ms)?;
                (addr.to_string_lossy(), *secs, *id, Some(*fuel as u64), *max_body as usize, parse_io_deadline(*io_ms), max_headers, install_sig, idle, Some(*err_id))
            }
            _ => {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "__rubyrs_http_serve_with_app(addr: String, duration_secs: Integer, app: Proc/Lambda, per_request_fuel: Integer = nil, max_body_bytes: Integer = 16MB, io_deadline_ms: Integer = 0, max_headers: Integer = 0, install_signal_handler: Integer = 0, idle_timeout_ms: Integer = 0, on_error: Proc/Lambda = nil)"
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
        let bound = run_blocking_for_duration_with_app(
            addr, duration, block_id, on_error_block, per_request_fuel, max_body_bytes, io_deadline, max_headers, idle_timeout, install_signal_handler,
        ).map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("http_serve_with_app: {e}"),
            },
            backtrace: vec![],
        })?;

        Ok(Value::Str(std::rc::Rc::new(
            crate::value::RStr::new(bound.to_string()),
        )))
    });

    // Stage 7c PoC: `__rubyrs_http_serve_prefork`.
    //
    // Argument shape (4-5 args):
    //   (addr, secs, app, n_workers)
    //   (addr, secs, app, n_workers, on_worker_boot)
    //
    // For 7c only N=1 is honored — no actual fork(2); the
    // call runs in the current process. The `on_worker_boot`
    // block (if supplied) is invoked exactly once with the
    // worker index (always 0 for N=1) BEFORE the accept
    // loop starts. This proves the worker-boot semantics
    // in isolation: visible side effects (e.g., setting a
    // global) are observable from app calls, and a raise
    // in on_worker_boot fails the server fast.
    //
    // N>=2 (real libc::fork + waitpid supervisor) lands in
    // 7d, gated cfg(target_family = "unix"). On Windows,
    // N>=2 returns ArgumentError unconditionally — there's
    // no SO_REUSEPORT equivalent.
    rt.register_fn("__rubyrs_http_serve_prefork", |args| {
        let (addr_str, duration_secs, block_id, n_workers, on_worker_boot_id) = match args {
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(n)] => {
                (addr.to_string_lossy(), *secs, *id, *n, None)
            }
            [Value::Str(addr), Value::Int(secs), Value::Block(id), Value::Int(n), Value::Block(boot_id)] => {
                (addr.to_string_lossy(), *secs, *id, *n, Some(*boot_id))
            }
            _ => {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "__rubyrs_http_serve_prefork(addr: String, duration_secs: Integer, app: Proc/Lambda, n_workers: Integer, on_worker_boot: Proc/Lambda = nil)"
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
        if n_workers < 1 {
            return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: format!("n_workers must be >= 1, got {n_workers}"),
                },
                backtrace: vec![],
            });
        }
        // 7d: N>=2 path requires fork(2). Unix-only.
        // Windows has no SO_REUSEPORT equivalent + no fork
        // primitive at all, so we explicitly reject N>=2
        // on non-unix rather than silently degrade to N=1
        // (which would mask the user's scaling intent).
        #[cfg(not(target_family = "unix"))]
        if n_workers > 1 {
            return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: format!("n_workers > 1 unsupported on non-Unix targets (no fork(2) or SO_REUSEPORT); got n_workers={n_workers}"),
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

        // Stage 7d: N >= 2 path forks N workers. Each
        // child binds its own SO_REUSEPORT listener post-
        // fork (kernel hash-distributes connections),
        // calls on_worker_boot in its address space, then
        // serves. Parent does NOT bind, NOT accept; it
        // only waitpid-loops. The non-unix branch already
        // rejected N>=2 above; the cfg here lets the
        // libc::* calls compile.
        #[cfg(target_family = "unix")]
        if n_workers > 1 {
            if addr.port() == 0 {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "n_workers > 1 requires an explicit non-zero port (port 0 binds different kernel ports per worker)".to_string(),
                    },
                    backtrace: vec![],
                });
            }
            if (n_workers as usize) > MAX_PREFORK_WORKERS {
                // FU1: fixed-size pid table for async-
                // signal-safe forwarding; cap there.
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!(
                            "n_workers must be <= {MAX_PREFORK_WORKERS}, got {n_workers}"
                        ),
                    },
                    backtrace: vec![],
                });
            }

            // FU1: reset the parent's static child-pid
            // table from any prior invocation, install the
            // forwarding sig handler. Both run BEFORE fork
            // so children inherit the static (all zeros at
            // this point — child uses libc::exit, never
            // reads the table).
            for slot in PREFORK_CHILD_PIDS.iter() {
                slot.store(0, std::sync::atomic::Ordering::SeqCst);
            }
            // FU2: clear shutdown flag from any prior
            // invocation. Set by the sig handler when a
            // SIGINT/SIGTERM arrives — supervisor then
            // skips restart.
            PREFORK_SHUTDOWN_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
            install_prefork_signal_handlers();

            let mut child_pids: Vec<libc::pid_t> = Vec::with_capacity(n_workers as usize);
            for worker_index in 0..n_workers {
                // SAFETY: fork(2) is async-signal-safe but
                // the post-fork Rust state must be handled
                // carefully — see the child branch below.
                let pid = unsafe { libc::fork() };
                if pid < 0 {
                    let errno_msg = std::io::Error::last_os_error();
                    // Reap already-spawned children before
                    // returning the error.
                    for &cpid in &child_pids {
                        unsafe { libc::kill(cpid, libc::SIGTERM); }
                    }
                    for &cpid in &child_pids {
                        let mut status = 0;
                        unsafe { libc::waitpid(cpid, &mut status, 0); }
                    }
                    return Err(Trap {
                        err: RubyError::RuntimeError {
                            msg: format!("fork(2) failed at worker {worker_index}: {errno_msg}"),
                        },
                        backtrace: vec![],
                    });
                } else if pid == 0 {
                    // ===== Child process =====
                    // Body extracted to `run_one_child_worker`
                    // (top of this module) so FU2's restart
                    // supervisor can reuse it. Never returns.
                    run_one_child_worker(
                        addr, duration_secs, block_id,
                        on_worker_boot_id, worker_index,
                    );
                } else {
                    // FU1: publish pid into the static
                    // table BEFORE pushing into the Vec.
                    // Use SeqCst so the forwarding handler
                    // sees it atomically if SIGTERM/SIGINT
                    // races with the fork loop. usize cast
                    // is safe — worker_index < n_workers <=
                    // MAX_PREFORK_WORKERS.
                    PREFORK_CHILD_PIDS[worker_index as usize]
                        .store(pid, std::sync::atomic::Ordering::SeqCst);
                    child_pids.push(pid);
                }
            }

            // ===== Parent: FU2 supervisor poll loop =====
            //
            // Non-blocking `waitpid(-1, WNOHANG)` polls
            // for ANY child exit; when one is reaped, we
            // look up its worker slot via PREFORK_CHILD_PIDS
            // and (subject to deadline + crash-loop guard)
            // fork a replacement into the same slot. The
            // pool stays at N workers across crash events.
            //
            // Deadline respect: once `duration_secs` from
            // entry has elapsed, the supervisor stops
            // restarting + reaps remaining workers (they
            // exit on their own duration timer or via the
            // FU1 signal forwarding path).
            //
            // Crash-loop guard: if MAX_RESTARTS_WINDOW
            // restarts happen within RESTART_WINDOW_SECS,
            // print a diagnostic + signal-shutdown the
            // remaining workers. Prevents fork-bombing the
            // host on a deterministic boot failure.
            //
            // SIGINT/SIGTERM to the parent fires the FU1
            // handler, which kills each child. With
            // install_signal_handler=true in each child,
            // accept loops cut short and return; waitpid
            // observes their exit; the supervisor's
            // deadline-check path lets the loop unwind.
            const MAX_RESTARTS_WINDOW: usize = 5;
            const RESTART_WINDOW_SECS: u64 = 60;
            let supervisor_start = std::time::Instant::now();
            let deadline = supervisor_start + Duration::from_secs(duration_secs as u64);
            let mut alive: Vec<libc::pid_t> = child_pids.clone();
            let mut restart_log: Vec<std::time::Instant> = Vec::new();
            let mut crash_loop_tripped = false;

            while !alive.is_empty() {
                let now = std::time::Instant::now();
                // FU2: SIGINT/SIGTERM arrived — the
                // forwarding handler already killed each
                // alive child; just reap them and exit
                // without restarting. The blocking
                // waitpids are bounded because the
                // children received the signal already.
                if PREFORK_SHUTDOWN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
                    for &cpid in &alive {
                        let mut status = 0;
                        unsafe { libc::waitpid(cpid, &mut status, 0); }
                    }
                    alive.clear();
                    break;
                }
                if now >= deadline {
                    // Deadline reached — signal remaining,
                    // reap, exit loop. Children's own
                    // duration timer would also fire, but
                    // SIGTERM ensures fast cleanup.
                    for &cpid in &alive {
                        unsafe { libc::kill(cpid, libc::SIGTERM); }
                    }
                    for &cpid in &alive {
                        let mut status = 0;
                        unsafe { libc::waitpid(cpid, &mut status, 0); }
                    }
                    alive.clear();
                    break;
                }

                // Non-blocking reap. WNOHANG returns 0 if
                // no exited child is pending; we then
                // sleep briefly + retry.
                let mut status = 0;
                let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if reaped == 0 {
                    // No exits yet; back off. 50ms is the
                    // crash-detection latency upper bound;
                    // small enough to feel responsive,
                    // large enough not to thrash CPU.
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                if reaped < 0 {
                    // ECHILD = no more children; treat as
                    // empty alive list (defensive).
                    alive.clear();
                    break;
                }

                // Remove the exited pid from alive.
                if let Some(pos) = alive.iter().position(|&p| p == reaped) {
                    alive.remove(pos);
                }

                // Look up worker slot. Either find the
                // index that held this pid, or if the pid
                // was already cleared (race with parallel
                // restart) skip — no double-restart.
                let exit_code = (status >> 8) & 0xff;
                let slot = PREFORK_CHILD_PIDS.iter()
                    .position(|s| s.load(std::sync::atomic::Ordering::SeqCst) == reaped);
                if let Some(slot_idx) = slot {
                    // Always clear the slot so the FU1
                    // forwarding handler doesn't try to
                    // SIGTERM the stale (recycled) pid.
                    PREFORK_CHILD_PIDS[slot_idx]
                        .store(0, std::sync::atomic::Ordering::SeqCst);

                    if crash_loop_tripped {
                        // Already in shutdown mode; don't
                        // restart, just reap remaining.
                        continue;
                    }

                    // Crash-loop guard: prune old entries,
                    // count recent restarts.
                    restart_log.retain(|t| now.duration_since(*t).as_secs() < RESTART_WINDOW_SECS);
                    if restart_log.len() >= MAX_RESTARTS_WINDOW {
                        eprintln!(
                            "rubyrs prefork: crash-loop detected ({} restarts in {RESTART_WINDOW_SECS}s); halting supervisor",
                            restart_log.len(),
                        );
                        crash_loop_tripped = true;
                        for &cpid in &alive {
                            unsafe { libc::kill(cpid, libc::SIGTERM); }
                        }
                        continue;
                    }

                    // Restart only if we're not past the
                    // deadline (race-safe re-check).
                    if std::time::Instant::now() >= deadline {
                        continue;
                    }

                    let new_pid = unsafe { libc::fork() };
                    if new_pid < 0 {
                        eprintln!(
                            "rubyrs prefork: failed to restart worker {slot_idx} (was pid {reaped} exit {exit_code}): {}",
                            std::io::Error::last_os_error(),
                        );
                        continue;
                    } else if new_pid == 0 {
                        // Restarted child — same path as
                        // initial fork.
                        run_one_child_worker(
                            addr, duration_secs, block_id,
                            on_worker_boot_id, slot_idx as i64,
                        );
                    } else {
                        PREFORK_CHILD_PIDS[slot_idx]
                            .store(new_pid, std::sync::atomic::Ordering::SeqCst);
                        alive.push(new_pid);
                        restart_log.push(now);
                        eprintln!(
                            "rubyrs prefork: restarted worker {slot_idx} (was pid {reaped} exit {exit_code}); new pid {new_pid}",
                        );
                    }
                }
            }

            // Return the input addr as the "bound" — each
            // child has its own listener on the SAME port,
            // so the input string is the canonical endpoint.
            return Ok(Value::Str(std::rc::Rc::new(
                crate::value::RStr::new(addr.to_string()),
            )));
        }

        // ===== N=1 path (no fork) — original 7c logic =====

        // Bind ONCE in the parent (for N=1 this IS the
        // worker). 7d will fork before this point and each
        // child will re-bind via SO_REUSEPORT.
        #[cfg(unix)]
        let listener = bind_reuseport_v4(addr).map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("http_serve_prefork bind: {e}"),
            },
            backtrace: vec![],
        })?;
        #[cfg(not(unix))]
        let listener = std::net::TcpListener::bind(addr).map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("http_serve_prefork bind: {e}"),
            },
            backtrace: vec![],
        })?;
        #[cfg(not(unix))]
        listener.set_nonblocking(true).map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("http_serve_prefork set_nonblocking: {e}"),
            },
            backtrace: vec![],
        })?;
        let bound = listener.local_addr().unwrap_or(addr);

        // Invoke on_worker_boot if supplied. The Vm is
        // already parked into CURRENT_VM_PTR by the host
        // fn dispatcher (ADR 0013). Argument = worker
        // index (always 0 for N=1). A raise here surfaces
        // as the host fn's trap — the server never starts.
        if let Some(boot_id) = on_worker_boot_id {
            let ptr = crate::vm::current_vm_ptr();
            if ptr.is_null() {
                return Err(Trap {
                    err: RubyError::RuntimeError {
                        msg: "internal: CURRENT_VM_PTR null in serve_prefork host fn".to_string(),
                    },
                    backtrace: vec![],
                });
            }
            // SAFETY: ADR 0013 — outer &mut Vm parked by
            // invoke_host_fn; we re-borrow time-disjointly.
            let vm = unsafe { &mut *ptr };
            let worker_index = Value::Int(0);
            // call_ruby_block_sync propagates any trap (incl.
            // ResourceExhausted) as Err — fine; we surface
            // it to the embedder rather than silently
            // skipping the server.
            call_ruby_block_sync(vm, boot_id, vec![worker_index])?;
        }

        let duration = Duration::from_secs(duration_secs as u64);

        // Non-unix has no on_listener variant (cfg-gated);
        // fall back to the addr-taking entry. For N=1
        // this is semantically identical.
        #[cfg(unix)]
        let _serve_bound = run_blocking_for_duration_with_app_on_listener(
            listener, duration, block_id, None, None,
            DEFAULT_MAX_BODY_BYTES, None, None, None, false,
        ).map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("http_serve_prefork: {e}"),
            },
            backtrace: vec![],
        })?;
        #[cfg(not(unix))]
        {
            // Drop the bound std listener; the addr-taking
            // path will rebind. SO_REUSEADDR isn't set
            // there, so the TIME_WAIT skip is lost — but
            // Windows isn't the Stage 7 target.
            drop(listener);
            run_blocking_for_duration_with_app(
                addr, duration, block_id, None, None,
                DEFAULT_MAX_BODY_BYTES, None, None, None, false,
            ).map_err(|e| Trap {
                err: RubyError::RuntimeError {
                    msg: format!("http_serve_prefork: {e}"),
                },
                backtrace: vec![],
            })?;
        }

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

    /// Stage 6c: request with too many headers triggers
    /// HTTP 431 Request Header Fields Too Large at the
    /// hyper parser layer — the app block is NEVER
    /// invoked, the parser short-circuits before
    /// service_fn dispatch.
    ///
    /// hyper's `Builder::max_headers(N)` rejects any
    /// request with more than N headers. Default 100;
    /// caller passes a smaller value to lock it down for
    /// untrusted-client scenarios.
    ///
    /// Test shape:
    ///   - max_headers = 5 (very tight cap)
    ///   - Client sends 50 distinct headers
    ///   - Server returns 431
    ///   - App block never invoked (global stays false)
    #[test]
    fn too_many_headers_yields_431_without_invoking_app() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18099";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            // 50 distinct headers — way above the cap of 5
            // (Host counts as 1, so 49 X-Custom-N + Host =
            // 50 total).
            let mut req = String::from("GET /flood HTTP/1.1\r\nHost: localhost\r\n");
            for i in 0..49 {
                req.push_str(&format!("X-Custom-{i}: value{i}\r\n"));
            }
            req.push_str("Connection: close\r\n\r\n");
            client.write_all(req.as_bytes()).expect("write request");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        // 7-arg form: max_headers = 5
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            $reached = false
            app = ->(env) {{
              $reached = true
              [200, {{}}, ["should never reach this"]]
            }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 1, app,
              1_000_000, 10_000, 0, 5
            )
            raise "app must NOT run on header-count overflow; was reached" if $reached
        "#), "stage_6c_header_count.rb").expect("server ran + app stayed cold");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 431"),
            "expected 431 Request Header Fields Too Large, got:\n{response_text}",
        );
    }

    /// Stage 6c: when `max_headers = 0` (or arg omitted),
    /// hyper's default of 100 applies — a request with
    /// 10 headers passes through normally.
    #[test]
    fn header_count_default_allows_normal_traffic() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18100";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            // 10 headers — well under hyper's default (100)
            let mut req = String::from("GET /normal HTTP/1.1\r\nHost: localhost\r\n");
            for i in 0..9 {
                req.push_str(&format!("X-Custom-{i}: v{i}\r\n"));
            }
            req.push_str("Connection: close\r\n\r\n");
            client.write_all(req.as_bytes()).expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        // 7-arg form with max_headers = 0 (use hyper default)
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{
              count = env.keys.count {{ |k| k.start_with?("HTTP_X_CUSTOM_") }}
              [200, {{"Content-Type" => "text/plain"}}, ["custom_headers=#{{count}}"]]
            }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 1, app,
              1_000_000, 10_000, 0, 0
            )
        "#), "stage_6c_header_default.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 200"),
            "expected 200 OK for 10-header request, got:\n{response_text}",
        );
        assert!(
            response_text.contains("custom_headers=9"),
            "expected 9 X-Custom-* headers in env, got:\n{response_text}",
        );
    }

    /// Stage 6d: the 8-arg form (with the
    /// `install_signal_handler` flag) parses correctly,
    /// the duration-based shutdown still terminates the
    /// loop, and a real HTTP exchange still completes.
    ///
    /// We deliberately do NOT raise real SIGINT/SIGTERM in
    /// this test: the test runner shares the process, so
    /// a real signal would kill the whole `cargo test`
    /// invocation. The signal-arm correctness is exercised
    /// by inspection (both signal futures resolve to
    /// `return Ok(())` in the `select!`); this test
    /// guarantees that wiring signals in does not break
    /// the duration-shutdown path or the request flow.
    #[test]
    fn install_signal_handler_flag_does_not_break_serve() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18101";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client
                .write_all(b"GET /sig HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{
              [200, {{"Content-Type" => "text/plain"}}, ["ok"]]
            }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 1, app,
              1_000_000, 10_000, 0, 0, 1
            )
        "#), "stage_6d_signal_flag.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");
        assert!(
            response_text.contains("HTTP/1.1 200"),
            "expected 200 with signal handler enabled, got:\n{response_text}",
        );
    }

    /// Stage 6d: the 8-arg form rejects values for
    /// `install_signal_handler` outside the {0, 1} domain.
    /// Other non-negative integers are not a meaningful
    /// "level" — the flag is binary today, and ambiguous
    /// input ought to surface an ArgumentError rather than
    /// silently treating "2" as truthy.
    #[test]
    fn install_signal_handler_rejects_out_of_range_values() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r#"
            app = ->(env) { [200, {}, []] }
            __rubyrs_http_serve_with_app(
              "127.0.0.1:0", 0, app,
              1_000_000, 10_000, 0, 0, 2
            )
        "#, "stage_6d_signal_flag_bad.rb").expect_err("should reject sig=2");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("install_signal_handler must be 0 or 1"),
            "expected ArgumentError mentioning install_signal_handler, got: {msg}",
        );
    }

    /// Stage 6e: when `idle_timeout_ms` is set, a TCP
    /// connection that never sends headers gets closed by
    /// the server within the cap. We connect, send
    /// nothing, then call `read_to_end` — it should
    /// observe EOF promptly rather than blocking until the
    /// duration shutdown.
    ///
    /// `header_read_timeout` covers both the initial-
    /// headers wait AND the inter-request idle gap on a
    /// keep-alive connection — same knob, both shapes.
    #[test]
    fn idle_connection_closed_by_idle_timeout() {
        use std::io::Read;
        use std::net::TcpStream;
        use std::thread;
        use std::time::{Duration, Instant};

        let server_addr = "127.0.0.1:18102";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let client = TcpStream::connect(server_addr).expect("connect");
            // Generous read timeout — we expect the server
            // to FIN well before this fires; the assertion
            // below catches the regression if it doesn't.
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buf = Vec::new();
            let started = Instant::now();
            let result = (&client).read_to_end(&mut buf);
            (started.elapsed(), result.is_ok(), buf.len())
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // 9-arg form: idle_timeout_ms = 300
        rt.eval(&format!(r#"
            app = ->(env) {{ [200, {{}}, []] }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 2, app,
              1_000_000, 10_000, 0, 0, 0, 300
            )
        "#), "stage_6e_idle_timeout.rb").expect("server ran");

        let (elapsed, ok, bytes_read) = client_thread.join().expect("client thread");
        // Server should close within ~1.5s (cap = 300ms +
        // scheduling slack). If it took longer, the cap
        // didn't fire and the 2s duration shutdown closed
        // us instead — the regression we want to catch.
        assert!(
            ok,
            "expected clean EOF from server idle-close, got an error",
        );
        assert_eq!(
            bytes_read, 0,
            "server should close without writing bytes; got {bytes_read} bytes",
        );
        assert!(
            elapsed < Duration::from_millis(1500),
            "expected idle close <1.5s, took {elapsed:?} — idle_timeout cap likely not firing",
        );
    }

    /// Stage 6e: `idle_timeout_ms < 0` is an ArgumentError.
    #[test]
    fn idle_timeout_rejects_negative_value() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r#"
            app = ->(env) { [200, {}, []] }
            __rubyrs_http_serve_with_app(
              "127.0.0.1:0", 0, app,
              1_000_000, 10_000, 0, 0, 0, -1
            )
        "#, "stage_6e_idle_neg.rb").expect_err("should reject idle=-1");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("idle_timeout_ms must be non-negative"),
            "expected ArgumentError mentioning idle_timeout_ms, got: {msg}",
        );
    }

    /// Stage 6f: when the main app raises and an
    /// `on_error` block is configured, the server hands
    /// (env, err_class, err_message) to on_error and uses
    /// its Rack triplet for the response. We use a custom
    /// status (418) so the test pins on the on_error path
    /// (vs the hardcoded 500 fallback).
    #[test]
    fn on_error_block_handles_app_exception() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18103";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(b"GET /boom HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // 10-arg form: app raises, on_error maps to 418.
        // We pass two blocks: the failing app and the
        // recovering on_error mapper.
        rt.eval(&format!(r#"
            app = ->(env) {{ raise "kaboom" }}
            on_error = ->(env, klass, msg) {{
              body = "handled by on_error: #{{klass}}: #{{msg}}"
              [418, {{"Content-Type" => "text/plain", "X-Mapped" => "true"}}, [body]]
            }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 1, app,
              1_000_000, 10_000, 0, 0, 0, 0, on_error
            )
        "#), "stage_6f_on_error.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 418"),
            "expected 418 from on_error mapper, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("x-mapped: true"),
            "expected on_error's custom header, got:\n{response_text}",
        );
        assert!(
            response_text.contains("handled by on_error"),
            "expected on_error's custom body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("kaboom"),
            "expected original error message threaded through on_error, got:\n{response_text}",
        );
    }

    /// Stage 6f: when on_error itself raises, the server
    /// falls back to a plain 500 — it does not crash the
    /// worker and does not loop. The 500 body should
    /// reference both errors so an operator can debug.
    #[test]
    fn on_error_block_itself_raises_falls_back_to_500() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18104";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(b"GET /boom HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{ raise "primary boom" }}
            on_error = ->(env, klass, msg) {{ raise "secondary boom" }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 1, app,
              1_000_000, 10_000, 0, 0, 0, 0, on_error
            )
        "#), "stage_6f_on_error_raises.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 500"),
            "expected 500 fallback when on_error raises, got:\n{response_text}",
        );
        assert!(
            response_text.contains("on_error block itself raised"),
            "expected diagnostic body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("secondary boom") && response_text.contains("primary boom"),
            "expected both errors in body, got:\n{response_text}",
        );
    }

    /// Stage 6f: ResourceExhausted (fuel cap) BYPASSES the
    /// on_error block — it's a security signal that app
    /// code must not be able to mask. The server still
    /// returns 503 even when on_error is configured.
    ///
    /// We send TWO requests in sequence: first a runaway
    /// that exhausts per_request_fuel, second a trivial
    /// /ok. Same pattern as
    /// `runaway_request_503s_then_worker_survives` — the
    /// second request triggers a reset_between_requests +
    /// fuel refill so the top-level eval has enough fuel
    /// to complete after `__rubyrs_http_serve_with_app`
    /// returns.
    #[test]
    fn on_error_does_not_intercept_resource_exhausted() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18105";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let read_one = |path: &str| {
                let mut client = TcpStream::connect(server_addr).expect("connect");
                client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let req = format!(
                    "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                );
                client.write_all(req.as_bytes()).expect("write");
                let mut response = Vec::new();
                let _ = client.read_to_end(&mut response);
                String::from_utf8_lossy(&response).into_owned()
            };
            let r1 = read_one("/runaway");
            let r2 = read_one("/ok");
            (r1, r2)
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{
              if env['PATH_INFO'] == '/runaway'
                1_000_000.times {{ 1 + 1 }}
                [200, {{}}, ["never reached"]]
              else
                [200, {{"Content-Type" => "text/plain"}}, ["ok"]]
              end
            }}
            on_error = ->(env, klass, msg) {{
              [418, {{"Content-Type" => "text/plain"}}, ["should not be reached"]]
            }}
            __rubyrs_http_serve_with_app(
              "{server_addr}", 2, app,
              10_000, 10_000, 0, 0, 0, 0, on_error
            )
        "#), "stage_6f_resource_exhausted.rb").expect("server ran");

        let (r1, r2) = client_thread.join().expect("client thread");

        assert!(
            r1.contains("HTTP/1.1 503"),
            "request 1 (runaway): expected 503 for ResourceExhausted (on_error must NOT intercept), got:\n{r1}",
        );
        assert!(
            !r1.contains("should not be reached"),
            "request 1: on_error must not be invoked for ResourceExhausted, got body:\n{r1}",
        );
        assert!(
            r2.contains("HTTP/1.1 200"),
            "request 2 (/ok): worker should survive ResourceExhausted, got:\n{r2}",
        );
    }

    /// Stage 6b: slow-body upload triggers 504 Gateway
    /// Timeout BEFORE the app block is invoked. The
    /// `tokio::time::timeout` wrapping `limited.collect()`
    /// bounds wall-clock spent reading the body — defends
    /// against slow-loris-shape attacks where a client
    /// claims a large Content-Length then dribbles bytes
    /// to hold the handler reservation.
    ///
    /// Test shape:
    ///   - Client sends headers with Content-Length: 100
    ///     then writes only 10 bytes and then PAUSES
    ///     (without closing). hyper's body collect waits
    ///     for the remaining 90 bytes; deadline fires.
    ///   - Server returns 504; app block never invoked.
    ///   - Deadline = 300ms; client holds connection ~1s.
    #[test]
    fn slow_body_upload_triggers_504_timeout() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18097";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            // Claim 100-byte body, send 10, pause.
            client.write_all(
                b"POST /slow HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Content-Length: 100\r\n\
                  Content-Type: application/octet-stream\r\n\
                  Connection: close\r\n\r\n\
                  ten_byte_!",
            ).expect("write headers + partial body");
            // Hold the connection open — let the server's
            // I/O deadline fire while waiting for the
            // remaining 90 bytes.
            thread::sleep(Duration::from_millis(800));
            // Try to read whatever response the server
            // already sent. Broken-pipe / EOF is fine —
            // we just want the response bytes.
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        // 6-arg form: fuel=1M, max_body=10K, io_deadline=300ms
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            $reached = false
            app = ->(env) {{
              $reached = true
              [200, {{}}, ["should never reach this"]]
            }}
            __rubyrs_http_serve_with_app("{server_addr}", 2, app, 1_000_000, 10_000, 300)
            raise "app must NOT run when io_deadline fires; was reached" if $reached
        "#), "stage_6b_slow_body.rb").expect("server ran + app stayed cold");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 504"),
            "expected 504 Gateway Timeout for slow upload, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("gateway timeout"),
            "expected gateway timeout in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("300 ms") || response_text.contains("300ms"),
            "expected deadline value in body, got:\n{response_text}",
        );
    }

    /// Stage 6b: when `io_deadline_ms = 0` (or arg omitted),
    /// no I/O timeout — slow uploads complete normally.
    /// Verifies the disable-by-zero idiom works.
    #[test]
    fn io_deadline_zero_disables_timeout() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18098";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(
                b"POST /slow HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Content-Length: 20\r\n\
                  Content-Type: application/octet-stream\r\n\
                  Connection: close\r\n\r\n\
                  ten_byte_!",
            ).expect("write headers + partial");
            // ~400ms slow body — would normally trip 300ms
            // deadline; with disable-by-zero, no trip.
            thread::sleep(Duration::from_millis(400));
            client.write_all(b"final_10b!").expect("write rest");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        // io_deadline_ms = 0 → no timeout
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{
              [200, {{"Content-Type" => "text/plain"}}, ["body_len=#{{env['CONTENT_LENGTH']}}"]]
            }}
            __rubyrs_http_serve_with_app("{server_addr}", 2, app, 1_000_000, 10_000, 0)
        "#), "stage_6b_disable_timeout.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 200"),
            "expected 200 OK (no timeout when io_deadline=0), got:\n{response_text}",
        );
        assert!(
            response_text.contains("body_len=20"),
            "expected body_len=20 (full body received), got:\n{response_text}",
        );
    }

    /// Stage 6a: request body exceeding `max_body_bytes`
    /// returns 413 Payload Too Large BEFORE the full body
    /// is buffered. The `http_body_util::Limited` wrapper
    /// short-circuits mid-stream — a 100 GB attacker upload
    /// doesn't OOM the server even at a 100-byte cap.
    ///
    /// The test sends a 50 KB body against a 100-byte cap;
    /// asserts the server replies 413 and (importantly)
    /// that the app block is NEVER invoked (we'd see the
    /// global side-effect if it were).
    #[test]
    fn oversized_request_body_yields_413_without_invoking_app() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18095";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            // 50 KB POST body — way above the 100-byte cap.
            let big_body = "x".repeat(50_000);
            let req = format!(
                "POST /upload HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Content-Length: {}\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Connection: close\r\n\r\n{}",
                big_body.len(),
                big_body,
            );
            client.write_all(req.as_bytes()).expect("write");
            let mut response = Vec::new();
            // Reading might fail with broken pipe if the
            // server closes mid-upload after sending 413 —
            // that's fine, we just want whatever response
            // bytes we received.
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // App SHOULD NEVER be invoked for an oversized body.
        // The lambda sets $reached = true; we check post-
        // serve that it stayed false.
        rt.eval(&format!(r#"
            $reached = false
            app = ->(env) {{
              $reached = true
              [200, {{}}, ["should never see this"]]
            }}
            # 4-arg shape: per_request_fuel = nil-equiv (omit), but
            # we want max_body_bytes = 100 → use 5-arg form with
            # per_request_fuel = 1_000_000 (well above any need).
            __rubyrs_http_serve_with_app("{server_addr}", 1, app, 1_000_000, 100)
            # Post-serve assertion: app must NOT have run.
            raise "app should not be invoked on 413 path; was reached" if $reached
        "#), "stage_6a_oversized.rb").expect("server ran + app stayed cold");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 413"),
            "expected 413 Payload Too Large, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("payload too large"),
            "expected 413 reason in body, got:\n{response_text}",
        );
    }

    /// Stage 6a: a body UNDER the cap is accepted normally —
    /// verifies the cap doesn't false-positive at the boundary.
    /// Sends a 50-byte POST against a 1024-byte cap.
    #[test]
    fn within_cap_body_passes_through_to_app() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18096";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let body = "hello, world! (small body under cap)";
            let req = format!(
                "POST /echo HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Content-Length: {}\r\n\
                 Content-Type: text/plain\r\n\
                 Connection: close\r\n\r\n{}",
                body.len(),
                body,
            );
            client.write_all(req.as_bytes()).expect("write");
            let mut response = Vec::new();
            client.read_to_end(&mut response).expect("read");
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // App returns the request method + CONTENT_LENGTH so
        // we can verify the request reached the handler.
        rt.eval(&format!(r#"
            app = ->(env) {{
              body = "method=#{{env['REQUEST_METHOD']}};len=#{{env['CONTENT_LENGTH']}};"
              [200, {{"Content-Type" => "text/plain"}}, [body]]
            }}
            __rubyrs_http_serve_with_app("{server_addr}", 1, app, 1_000_000, 1024)
        "#), "stage_6a_within_cap.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 200"),
            "expected 200 OK, got:\n{response_text}",
        );
        assert!(
            response_text.contains("method=POST;"),
            "expected method=POST in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("len=36;"),
            "expected len=36 in body (body string is 36 bytes), got:\n{response_text}",
        );
    }

    /// Stage 5d integration: per_request_fuel cap +
    /// `ResourceExhausted` catch at the `app.call` boundary
    /// produces 503 — and the worker SURVIVES to serve a
    /// second request cleanly.
    ///
    /// This is the canonical "Ruby app misbehaves, server
    /// stays up" scenario the entire fuel-refill machinery
    /// exists for. Two requests on the same socket:
    ///   1. First request: runaway loop, fuel exhausts →
    ///      503 from server
    ///   2. Second request: well-behaved → 200 from server
    ///   3. State doesn't leak: $req_count global was 0
    ///      in request 1 (before runaway), should be 0 in
    ///      request 2 too (reset_between_requests cleared
    ///      it)
    #[test]
    fn runaway_request_503s_then_worker_survives_next_request() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18094";

        // Client thread: sends two requests, verifies each.
        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));

            // Request 1 — runaway path. Connect fresh,
            // send, read response.
            let mut c1 = TcpStream::connect(server_addr).expect("connect 1");
            c1.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c1.write_all(
                b"GET /runaway HTTP/1.1\r\n\
                  Host: localhost\r\nConnection: close\r\n\r\n",
            ).expect("write 1");
            let mut r1 = Vec::new();
            c1.read_to_end(&mut r1).expect("read 1");
            drop(c1);

            // Small pause between requests so the server has
            // time to land back at the accept loop.
            thread::sleep(Duration::from_millis(50));

            // Request 2 — well-behaved path.
            let mut c2 = TcpStream::connect(server_addr).expect("connect 2");
            c2.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c2.write_all(
                b"GET /ok HTTP/1.1\r\n\
                  Host: localhost\r\nConnection: close\r\n\r\n",
            ).expect("write 2");
            let mut r2 = Vec::new();
            c2.read_to_end(&mut r2).expect("read 2");

            (
                String::from_utf8_lossy(&r1).into_owned(),
                String::from_utf8_lossy(&r2).into_owned(),
            )
        });

        // Server: lambda checks $req_count global (should
        // be 0 every request — reset_between_requests
        // clears it). Path /runaway runs a tight loop that
        // exhausts fuel; /ok returns immediately.
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(
            r#"
            app = ->(env) {{
              # Verify reset_between_requests cleared the global.
              # If state leaks, $req_count would be 1 in request 2
              # (set by request 1 before its runaway), not 0.
              previous = $req_count || 0
              $req_count = previous + 1

              if env['PATH_INFO'] == '/runaway'
                # Tight loop to exhaust per_request_fuel (10_000).
                # Will trap with ResourceExhausted; server catches
                # → 503; worker survives.
                1_000_000.times {{ 1 + 1 }}
                [200, {{}}, ["never reached"]]
              else
                [200, {{"Content-Type" => "text/plain"}}, ["previous_req_count=#{{previous}}"]]
              end
            }}
            __rubyrs_http_serve_with_app("{server_addr}", 2, app, 10_000)
            "#,
        ), "stage_5d_runaway.rb").expect("server ran 2 seconds");

        let (r1, r2) = client_thread.join().expect("client thread did not panic");

        // Request 1 should be 503 (ResourceExhausted caught
        // and mapped at app.call boundary).
        assert!(
            r1.contains("HTTP/1.1 503"),
            "request 1 (runaway) want 503, got:\n{r1}",
        );

        // Request 2 should be 200 — worker survived.
        assert!(
            r2.contains("HTTP/1.1 200"),
            "request 2 (after runaway) want 200, got:\n{r2}",
        );

        // Request 2's body should show previous_req_count=0:
        // request 1's $req_count = 1 was cleared by
        // reset_between_requests before request 2's app.call.
        assert!(
            r2.contains("previous_req_count=0"),
            "expected reset_between_requests to clear $req_count between requests; \
             body was:\n{r2}",
        );
    }

    /// Stage 4c.3 end-to-end smoke test: Ruby block runs
    /// per HTTP request, env hash gets through, response
    /// triplet marshals back to a real hyper response.
    ///
    /// Setup:
    ///   - Client thread connects to the server mid-flight
    ///     (after 200ms delay so the server's bind + accept
    ///     loop is up), sends a GET / with a custom header,
    ///     reads the response.
    ///   - Server runs for 1 second invoking a Ruby lambda
    ///     that constructs a response from the env hash.
    ///
    /// The Ruby block echoes parts of the env back in the
    /// response body — the test verifies the env was built
    /// correctly AND that the response marshaling worked AND
    /// that custom headers in the response Hash come through.
    #[test]
    fn ruby_app_serves_real_request_end_to_end() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18091";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(
                b"GET /users/42?verbose=1 HTTP/1.1\r\n\
                  Host: example.com:18091\r\n\
                  User-Agent: rubyrs-test/0.1\r\n\
                  Connection: close\r\n\r\n",
            ).expect("write request");
            let mut response = Vec::new();
            client.read_to_end(&mut response).expect("read response");
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // Ruby app block echoes back several env entries +
        // sets a custom response header. Validates that:
        //   - PATH_INFO + QUERY_STRING + REQUEST_METHOD
        //     made it into env
        //   - HTTP_HOST + HTTP_USER_AGENT prefixing works
        //   - REMOTE_ADDR / REMOTE_PORT populated
        //   - Response Hash<String, String> headers go out
        //   - Response body Array<String> concatenates
        rt.eval(&format!(r#"
            app = ->(env) {{
              body = "method=#{{env['REQUEST_METHOD']}};" +
                     "path=#{{env['PATH_INFO']}};" +
                     "query=#{{env['QUERY_STRING']}};" +
                     "host=#{{env['HTTP_HOST']}};" +
                     "ua=#{{env['HTTP_USER_AGENT']}};" +
                     "remote=#{{env['REMOTE_ADDR']}};"
              [200, {{
                "Content-Type" => "text/plain; charset=utf-8",
                "X-Rubyrs-Echo" => "1",
              }}, [body]]
            }}
            __rubyrs_http_serve_with_app("{server_addr}", 1, app)
        "#), "stage_4c3_e2e.rb").expect("Ruby app served requests");

        let response_text = client_thread.join().expect("client thread did not panic");

        assert!(
            response_text.contains("HTTP/1.1 200 OK"),
            "expected 200 OK, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("content-type: text/plain"),
            "expected Content-Type header from app, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("x-rubyrs-echo: 1"),
            "expected custom X-Rubyrs-Echo header, got:\n{response_text}",
        );
        // Body assertions — confirm env got built correctly
        // AND that the block's interpolation came through
        assert!(
            response_text.contains("method=GET;"),
            "expected env REQUEST_METHOD=GET in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("path=/users/42;"),
            "expected env PATH_INFO=/users/42 in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("query=verbose=1;"),
            "expected env QUERY_STRING=verbose=1 in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("host=example.com:18091;"),
            "expected env HTTP_HOST in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("ua=rubyrs-test/0.1;"),
            "expected env HTTP_USER_AGENT in body, got:\n{response_text}",
        );
        assert!(
            response_text.contains("remote=127.0.0.1;"),
            "expected env REMOTE_ADDR=127.0.0.1 in body, got:\n{response_text}",
        );
    }

    /// Stage 4c.3 edge case: Ruby app returns non-Array.
    /// Verifies the marshaling layer produces a 500 + error
    /// message rather than panicking or sending a malformed
    /// response.
    #[test]
    fn ruby_app_non_array_result_yields_500() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18092";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(
                b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ).expect("write");
            let mut response = Vec::new();
            client.read_to_end(&mut response).expect("read");
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // App returns a String instead of [status, headers, body]
        // — should land 500 + msg containing "must return Array"
        rt.eval(&format!(r#"
            app = ->(env) {{ "this is not an Array" }}
            __rubyrs_http_serve_with_app("{server_addr}", 1, app)
        "#), "stage_4c3_non_array.rb").expect("server ran (app misbehaved)");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 500"),
            "expected 500 for non-Array result, got:\n{response_text}",
        );
        assert!(
            response_text.to_lowercase().contains("must return array"),
            "expected error message about Array, got:\n{response_text}",
        );
    }

    /// Stage 4c.3 edge case: Ruby app raises an exception.
    /// Verifies the trap propagates → 500 with the
    /// exception message in the body.
    #[test]
    fn ruby_app_raises_yields_500_with_message() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18093";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(
                b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ).expect("write");
            let mut response = Vec::new();
            client.read_to_end(&mut response).expect("read");
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{ raise "kaboom inside app" }}
            __rubyrs_http_serve_with_app("{server_addr}", 1, app)
        "#), "stage_4c3_raises.rb").expect("server ran (app raised)");

        let response_text = client_thread.join().expect("client thread");

        assert!(
            response_text.contains("HTTP/1.1 500"),
            "expected 500 for raise, got:\n{response_text}",
        );
        assert!(
            response_text.contains("kaboom inside app"),
            "expected raise message in body, got:\n{response_text}",
        );
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

    /// Stage 7a: two listeners can bind the same
    /// `(addr, port)` simultaneously when both have
    /// SO_REUSEPORT set — the foundational primitive for
    /// pre-fork multi-core scaling. Without SO_REUSEPORT
    /// the second bind would fail with EADDRINUSE.
    ///
    /// This test only exercises the primitive — it does
    /// NOT prove kernel-level load balancing across the
    /// two listeners (that requires multi-process
    /// observation, which lives in Stage 7d's manual
    /// verification).
    #[cfg(unix)]
    #[test]
    fn bind_reuseport_allows_two_listeners_on_same_port() {
        use std::net::SocketAddr;

        // Pick a fixed port outside the range used by the
        // other http_server tests (18080-18120).
        let addr: SocketAddr = "127.0.0.1:18130".parse().unwrap();

        let l1 = super::bind_reuseport_v4(addr).expect("first listener binds");
        let l2 = super::bind_reuseport_v4(addr).expect(
            "second listener should also bind via SO_REUSEPORT — \
             if this fails the setsockopt path isn't taking effect",
        );

        // Both must report the same local addr to prove
        // they're sharing the kernel's listening socket
        // group (not silently rebound to a different port).
        let a1 = l1.local_addr().unwrap();
        let a2 = l2.local_addr().unwrap();
        assert_eq!(a1.port(), addr.port(), "first listener port mismatch");
        assert_eq!(a2.port(), addr.port(), "second listener port mismatch");
    }

    /// Stage 7a: SO_REUSEPORT works for `:0` (kernel-
    /// assigned port). Two listeners on `:0` get different
    /// ports — that's expected, the kernel assigns each
    /// independently — but each individual bind succeeds.
    #[cfg(unix)]
    #[test]
    fn bind_reuseport_works_with_zero_port() {
        use std::net::SocketAddr;
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let l = super::bind_reuseport_v4(addr).expect("zero-port listener binds");
        let assigned = l.local_addr().unwrap();
        assert_ne!(
            assigned.port(),
            0,
            "kernel should have assigned a non-zero port",
        );
    }

    /// Stage 7c: `__rubyrs_http_serve_prefork(N=1)` invokes
    /// the `on_worker_boot` block exactly once with the
    /// worker index (0), BEFORE the accept loop starts.
    /// Side effects from on_worker_boot (here: setting a
    /// global) are observable from the app block.
    ///
    /// This is the API-shape proof that downstream forked
    /// workers (7d) will rely on: each child runs the boot
    /// hook in its own address space before accepting.
    #[test]
    fn prefork_n1_invokes_on_worker_boot_before_serving() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18140";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client
                .write_all(b"GET /boot HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // Globals are cleared by reset_between_requests
        // (intentional per-request isolation). on_worker_boot
        // state must live somewhere request-reset doesn't
        // touch — class-instance variables are the
        // idiomatic Ruby choice (Puma uses the same
        // pattern for boot-time DB connections).
        rt.eval(&format!(r#"
            class WorkerState
              @count = 0
              @worker_id = nil
              def self.boot!(idx)
                @count += 1
                @worker_id = idx
              end
              def self.count; @count; end
              def self.worker_id; @worker_id; end
            end
            app = ->(env) {{
              body = "boot_count=#{{WorkerState.count}} worker=#{{WorkerState.worker_id}}"
              [200, {{"Content-Type" => "text/plain"}}, [body]]
            }}
            on_boot = ->(idx) {{ WorkerState.boot!(idx) }}
            __rubyrs_http_serve_prefork("{server_addr}", 1, app, 1, on_boot)
        "#), "stage_7c_prefork_n1.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");
        assert!(
            response_text.contains("HTTP/1.1 200"),
            "expected 200 from prefork N=1, got:\n{response_text}",
        );
        assert!(
            response_text.contains("boot_count=1"),
            "on_worker_boot must run exactly once before serve, got:\n{response_text}",
        );
        assert!(
            response_text.contains("worker=0"),
            "worker index 0 must be visible to app, got:\n{response_text}",
        );
    }

    /// Stage 7c: when on_worker_boot raises, the server
    /// fails fast — no accept loop is entered, no requests
    /// served. The trap surfaces as the host fn's error.
    /// This is the contract that lets embedders use boot
    /// as the "must succeed or kill the worker" sanity
    /// gate (e.g., DB connection re-open).
    #[test]
    fn prefork_n1_on_worker_boot_raise_aborts_serve() {
        use std::time::Duration;

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let start = std::time::Instant::now();
        let result = rt.eval(r#"
            app = ->(env) { [200, {}, ["ok"]] }
            on_boot = ->(idx) { raise "db reconnect failed" }
            __rubyrs_http_serve_prefork("127.0.0.1:18141", 5, app, 1, on_boot)
        "#, "stage_7c_boot_raises.rb");
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "expected on_worker_boot raise to surface, got Ok",
        );
        let err_msg = format!("{result:?}");
        assert!(
            err_msg.contains("db reconnect failed"),
            "expected original boot error in trap, got: {err_msg}",
        );
        // Server's duration_secs was 5 — if boot-raise
        // didn't abort, eval would block ~5s. Bound at 2s
        // with slack for compilation.
        assert!(
            elapsed < Duration::from_secs(2),
            "expected fail-fast (<2s), took {elapsed:?} — on_worker_boot raise didn't abort serve",
        );
    }

    /// Stage 7c: the 4-arg form (no on_worker_boot) still
    /// serves normally. Equivalent to the existing addr-
    /// taking entry but via the prefork host fn (and via
    /// the on_listener internal path on unix).
    #[test]
    fn prefork_n1_without_on_worker_boot_serves() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        use std::time::Duration;

        let server_addr = "127.0.0.1:18142";

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(server_addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        rt.eval(&format!(r#"
            app = ->(env) {{ [200, {{"Content-Type" => "text/plain"}}, ["no_boot_ok"]] }}
            __rubyrs_http_serve_prefork("{server_addr}", 1, app, 1)
        "#), "stage_7c_no_boot.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");
        assert!(
            response_text.contains("HTTP/1.1 200") && response_text.contains("no_boot_ok"),
            "expected 200 + body, got:\n{response_text}",
        );
    }

    /// Stage 7d: N>=2 with port 0 is an explicit
    /// ArgumentError — each forked child would get a
    /// different kernel-assigned port, leaving the user
    /// with no canonical endpoint to connect to. Force
    /// an explicit non-zero port for multi-worker mode.
    #[cfg(target_family = "unix")]
    #[test]
    fn prefork_rejects_n_gte_2_with_port_zero() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r#"
            app = ->(env) { [200, {}, []] }
            __rubyrs_http_serve_prefork("127.0.0.1:0", 0, app, 4)
        "#, "stage_7d_n_with_port_zero.rb").expect_err("should reject port 0");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("requires an explicit non-zero port"),
            "expected port-0 ArgumentError, got: {msg}",
        );
    }

    /// Stage 7d: N>=2 on non-Unix targets is an
    /// ArgumentError — no fork(2), no SO_REUSEPORT
    /// equivalent. This test only runs on Windows-like
    /// targets; on unix it's a no-op.
    #[cfg(not(target_family = "unix"))]
    #[test]
    fn prefork_rejects_n_gte_2_on_non_unix() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r#"
            app = ->(env) { [200, {}, []] }
            __rubyrs_http_serve_prefork("127.0.0.1:18150", 0, app, 4)
        "#, "stage_7d_non_unix.rb").expect_err("should reject N>=2 on non-unix");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("unsupported on non-Unix"),
            "expected non-unix ArgumentError, got: {msg}",
        );
    }

    /// Stage 7c: n_workers < 1 is an ArgumentError.
    #[test]
    fn prefork_rejects_n_lt_1() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r#"
            app = ->(env) { [200, {}, []] }
            __rubyrs_http_serve_prefork("127.0.0.1:0", 0, app, 0)
        "#, "stage_7c_n_zero.rb").expect_err("should reject N=0");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("n_workers must be >= 1"),
            "expected ArgumentError on N=0, got: {msg}",
        );
    }

    /// Stage 7b: `run_blocking_for_duration_with_app_on_listener`
    /// accepts a pre-bound SO_REUSEPORT listener and serves
    /// requests over it identically to the addr-taking
    /// path. Proves the std → tokio listener handoff +
    /// runtime-built-post-bind ordering work end-to-end.
    ///
    /// We register a one-shot test host fn that wraps the
    /// on_listener entry — it needs the CURRENT_VM_PTR
    /// contract (set by `invoke_host_fn`), same as the
    /// real `__rubyrs_http_serve_with_app` path. This is
    /// the wiring 7c/7d build on; an in-process
    /// equivalent of what each forked child will do.
    #[cfg(unix)]
    #[test]
    fn run_on_listener_serves_real_request() {
        use std::io::{Read, Write};
        use std::net::{SocketAddr, TcpStream};
        use std::thread;
        use std::time::Duration;

        let addr: SocketAddr = "127.0.0.1:18131".parse().unwrap();

        let client_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let mut client = TcpStream::connect(addr).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            client
                .write_all(b"GET /pf HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write");
            let mut response = Vec::new();
            let _ = client.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });

        // Register a test-local host fn that:
        //   1. Binds a SO_REUSEPORT listener
        //   2. Hands it to the new on_listener entry
        // This mimics what each forked child will do in
        // 7d but without the fork.
        let mut rt = crate::Runtime::new();
        rt.register_fn("__test_serve_on_listener", move |args| {
            use crate::error::{RubyError, Trap};
            use crate::value::Value;
            let block_id = match args {
                [Value::Block(id)] => *id,
                _ => return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "__test_serve_on_listener(app)".to_string(),
                    },
                    backtrace: vec![],
                }),
            };
            let listener = super::bind_reuseport_v4(addr).map_err(|e| Trap {
                err: RubyError::RuntimeError {
                    msg: format!("bind_reuseport: {e}"),
                },
                backtrace: vec![],
            })?;
            super::run_blocking_for_duration_with_app_on_listener(
                listener,
                Duration::from_secs(1),
                block_id,
                None,
                Some(1_000_000),
                super::DEFAULT_MAX_BODY_BYTES,
                None,
                None,
                None,
                false,
            ).map_err(|e| Trap {
                err: RubyError::RuntimeError { msg: format!("serve: {e}") },
                backtrace: vec![],
            })?;
            Ok(Value::Nil)
        });
        rt.eval(r#"
            app = ->(env) { [200, {"Content-Type" => "text/plain"}, ["from_listener"]] }
            __test_serve_on_listener(app)
        "#, "stage_7b_on_listener.rb").expect("server ran");

        let response_text = client_thread.join().expect("client thread");
        assert!(
            response_text.contains("HTTP/1.1 200"),
            "expected 200 from on_listener path, got:\n{response_text}",
        );
        assert!(
            response_text.contains("from_listener"),
            "expected app's body, got:\n{response_text}",
        );
    }

    /// Stage 7a: bind to an invalid address surfaces as
    /// an io::Error rather than panicking. Sanity check
    /// for the error-propagation path.
    #[cfg(unix)]
    #[test]
    fn bind_reuseport_rejects_in_use_privileged_port() {
        use std::net::SocketAddr;
        // Privileged port (<1024) — unprivileged test
        // process can't bind. Expect Err.
        let addr: SocketAddr = "127.0.0.1:80".parse().unwrap();
        let result = super::bind_reuseport_v4(addr);
        assert!(
            result.is_err(),
            "expected bind to privileged port 80 to fail as non-root, got Ok",
        );
    }
}
