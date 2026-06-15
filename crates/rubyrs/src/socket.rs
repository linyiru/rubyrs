//! `_socket` battery — blocking `std::net` TCP client per ADR 0028.
//!
//! Phase 3 of the net/http track. Ships:
//!
//!   - Per-Vm `SOCKET_CONNS: HashMap<i64, SockState>` keyed by opaque
//!     integer handles (the rubyrs VM is single-threaded, so per-thread
//!     ≈ per-Vm), mirroring the `_sqlite` handle-table pattern.
//!   - 4 host fns (the surface the net/http discovery spike measured —
//!     `poc/net-http-spike/FINDINGS.md` §1):
//!       __rubyrs_socket_connect(host, port[, open_timeout]) → handle
//!       __rubyrs_socket_write(handle, bytes)               → bytes written
//!       __rubyrs_socket_read(handle, maxlen[, read_timeout]) → String(BINARY) | nil(EOF)
//!       __rubyrs_socket_close(handle)                       → nil
//!   - The pure-Ruby `TCPSocket` veneer + `Socket` sockopt constants +
//!     `SocketError`, loaded from `preamble/socket.rb`.
//!
//! Blocking by design (ADR 0028 §1): `read` blocks up to the socket's
//! read-timeout deadline and returns bytes / nil(EOF) / raises — it never
//! returns `:wait_readable`, so net/protocol's `to_io.wait_readable`
//! sub-surface is never reached (no tokio, no `io/wait`, no class-`g`
//! deviation). `TCP_NODELAY` is set inside `connect` (net/http's only
//! sockopt), so there is no `setsockopt` host fn.

#![cfg(feature = "_socket")]

use crate::error::{RubyError, Trap};
use crate::value::Value;
use crate::vm::current_vm_ptr;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Per-connection state. `total_read` backs the `socket_max_read_bytes`
/// cap (ADR 0028 §2, class-`f`).
struct SockState {
    stream: TcpStream,
    total_read: usize,
}

thread_local! {
    static SOCKET_CONNS: RefCell<HashMap<i64, SockState>> = RefCell::new(HashMap::new());
    // Shares NO numbering with `_sqlite`'s table — each battery keeps its
    // own counter; handles are only ever looked up in their own map.
    static NEXT_HANDLE: Cell<i64> = const { Cell::new(1) };
}

/// Remove and return the `TcpStream` for `handle` — the single declared
/// cross-battery seam (ADR 0029 §2 / ADR 0019 Rule 5). The `_openssl`
/// battery calls this to TAKE a connected socket and layer rustls TLS
/// over it; ownership transfers, so the original `TCPSocket` becomes
/// defunct (its `close` no-ops). Returns `None` if the handle is unknown
/// (already closed / transferred).
#[cfg(feature = "_openssl")]
pub(crate) fn take_stream(handle: i64) -> Option<TcpStream> {
    SOCKET_CONNS.with(|m| m.borrow_mut().remove(&handle).map(|st| st.stream))
}

fn arg_err(msg: &str) -> Trap {
    Trap { err: RubyError::ArgumentError { msg: msg.to_string() }, backtrace: vec![] }
}

/// Raise a Ruby exception of `class_name` (must resolve to a defined
/// class — `Errno::*` live in the preamble exceptions, `SocketError` /
/// `Net::ReadTimeout` in `socket.rb` / `net/http`).
fn host_exc(class_name: &str, message: String) -> Trap {
    Trap {
        err: RubyError::HostException { class_name: class_name.to_string(), message },
        backtrace: vec![],
    }
}

fn closed_socket() -> Trap {
    host_exc("IOError", "closed stream".to_string())
}

fn handle_arg(args: &[Value], shape: &str) -> Result<i64, Trap> {
    match args {
        [Value::Int(h)] => Ok(*h),
        _ => Err(arg_err(shape)),
    }
}

/// Map a Rust `io::Error` to the Ruby exception class net/http expects
/// (ADR 0028 §4). Unmapped kinds fall back to `SocketError`.
fn map_io_err(e: &std::io::Error, ctx: &str) -> Trap {
    use std::io::ErrorKind::*;
    let class = match e.kind() {
        ConnectionRefused => "Errno::ECONNREFUSED",
        ConnectionReset => "Errno::ECONNRESET",
        ConnectionAborted => "Errno::ECONNABORTED",
        BrokenPipe => "Errno::EPIPE",
        TimedOut => "Errno::ETIMEDOUT",
        AddrNotAvailable => "Errno::EADDRNOTAVAIL",
        _ => "SocketError",
    };
    host_exc(class, format!("{}: {}", ctx, e))
}

/// Capability gate (ADR 0028 §2): the `allow_network_io` master switch
/// + the optional `socket_allow_hosts` allowlist. Both read from the Vm
/// mirror of `Config`. A null Vm pointer (raw embedder context) lets the
/// connect through — same stance as `_sqlite`'s `check_path_allowed`.
fn check_connect_allowed(host: &str, port: i64) -> Result<(), Trap> {
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return Ok(());
    }
    let vm = unsafe { &*ptr };
    if !vm.allow_network_io {
        return Err(host_exc(
            "SocketError",
            format!(
                "network blocked: connect to {}:{} (Config::allow_network_io is false)",
                host, port
            ),
        ));
    }
    if let Some(allow) = &vm.socket_allow_hosts {
        let hostport = format!("{}:{}", host, port);
        let ok = allow.iter().any(|h| h == host || h == &hostport);
        if !ok {
            return Err(host_exc(
                "SecurityError",
                format!("connect to {} blocked: not in Config::socket_allow_hosts", hostport),
            ));
        }
    }
    Ok(())
}

fn read_cap() -> Option<usize> {
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return None;
    }
    unsafe { &*ptr }.socket_max_read_bytes
}

pub fn register_host_fns(rt: &mut crate::Runtime) {
    const PREAMBLE: &str = include_str!("preamble/socket.rb");
    if let Err(trap) = rt.eval(PREAMBLE, "<rubyrs:socket>") {
        panic!("ICE: _socket failed to load preamble: {trap:?}");
    }

    rt.register_fn("__rubyrs_socket_connect", |args| {
        let (host, port, open_to) = match args {
            [Value::Str(h), Value::Int(p)] => (h.to_string_lossy(), *p, None),
            [Value::Str(h), Value::Int(p), Value::Float(t)] => (h.to_string_lossy(), *p, Some(*t)),
            [Value::Str(h), Value::Int(p), Value::Nil] => (h.to_string_lossy(), *p, None),
            _ => return Err(arg_err(
                "__rubyrs_socket_connect(host: String, port: Integer[, open_timeout: Float])",
            )),
        };
        check_connect_allowed(&host, port)?;
        if !(0..=65535).contains(&port) {
            return Err(host_exc("SocketError", format!("invalid port {}", port)));
        }
        // System-resolver DNS of the caller's literal host (deviation
        // class `a` — owned-resource).
        let addrs = (host.as_str(), port as u16)
            .to_socket_addrs()
            .map_err(|e| host_exc("SocketError", format!("getaddrinfo: {}", e)))?;
        let mut last_err: Option<std::io::Error> = None;
        let mut stream_opt: Option<TcpStream> = None;
        for addr in addrs {
            let res = match open_to {
                Some(t) if t > 0.0 => TcpStream::connect_timeout(&addr, Duration::from_secs_f64(t)),
                _ => TcpStream::connect(addr),
            };
            match res {
                Ok(s) => { stream_opt = Some(s); break; }
                Err(e) => last_err = Some(e),
            }
        }
        let stream = match stream_opt {
            Some(s) => s,
            None => {
                let e = last_err.unwrap_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::Other, "no address resolved")
                });
                return Err(map_io_err(&e, "connect"));
            }
        };
        // net/http's only sockopt — fold it into connect (ADR 0028 §1.3).
        let _ = stream.set_nodelay(true);
        let handle = NEXT_HANDLE.with(|c| {
            let h = c.get();
            c.set(h + 1);
            h
        });
        SOCKET_CONNS.with(|m| {
            m.borrow_mut().insert(handle, SockState { stream, total_read: 0 });
        });
        Ok(Value::Int(handle))
    });

    rt.register_fn("__rubyrs_socket_write", |args| {
        let (handle, bytes) = match args {
            [Value::Int(h), Value::Str(s)] => (*h, s.borrow().clone()),
            _ => return Err(arg_err("__rubyrs_socket_write(handle, bytes: String)")),
        };
        SOCKET_CONNS.with(|m| -> Result<Value, Trap> {
            let mut map = m.borrow_mut();
            let st = map.get_mut(&handle).ok_or_else(closed_socket)?;
            st.stream.write_all(&bytes).map_err(|e| map_io_err(&e, "write"))?;
            Ok(Value::Int(bytes.len() as i64))
        })
    });

    rt.register_fn("__rubyrs_socket_read", |args| {
        let (handle, maxlen, read_to) = match args {
            [Value::Int(h), Value::Int(n)] => (*h, *n, None),
            [Value::Int(h), Value::Int(n), Value::Float(t)] => (*h, *n, Some(*t)),
            [Value::Int(h), Value::Int(n), Value::Nil] => (*h, *n, None),
            _ => return Err(arg_err(
                "__rubyrs_socket_read(handle, maxlen: Integer[, read_timeout: Float])",
            )),
        };
        let cap = read_cap();
        SOCKET_CONNS.with(|m| -> Result<Value, Trap> {
            let mut map = m.borrow_mut();
            let st = map.get_mut(&handle).ok_or_else(closed_socket)?;
            // Blocking read bounded by the deadline; we surface a
            // timeout as Net::ReadTimeout rather than ever returning
            // :wait_readable to the Ruby veneer.
            let to = match read_to {
                Some(t) if t > 0.0 => Some(Duration::from_secs_f64(t)),
                _ => None,
            };
            let _ = st.stream.set_read_timeout(to);
            let n = maxlen.max(0) as usize;
            let mut buf = vec![0u8; n];
            match st.stream.read(&mut buf) {
                Ok(0) => Ok(Value::Nil), // EOF
                Ok(got) => {
                    buf.truncate(got);
                    st.total_read = st.total_read.saturating_add(got);
                    if let Some(limit) = cap
                        && st.total_read > limit
                    {
                        return Err(host_exc(
                            "SocketError",
                            format!("socket read limit exceeded ({} bytes)", limit),
                        ));
                    }
                    Ok(Value::new_str_bytes_binary(buf))
                }
                Err(e)
                    if matches!(e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    Err(host_exc("Net::ReadTimeout", "read timed out".to_string()))
                }
                Err(e) => Err(map_io_err(&e, "read")),
            }
        })
    });

    rt.register_fn("__rubyrs_socket_close", |args| {
        let handle = handle_arg(args, "__rubyrs_socket_close(handle)")?;
        SOCKET_CONNS.with(|m| {
            // Dropping the SockState closes the TcpStream.
            m.borrow_mut().remove(&handle);
        });
        Ok(Value::Nil)
    });
}
