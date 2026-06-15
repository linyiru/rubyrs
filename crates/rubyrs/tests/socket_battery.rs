//! `_socket` battery integration test (ADR 0028 Phase 3).
//!
//! Spins up a real loopback TCP server on a background OS thread and
//! drives the pure-Ruby `TCPSocket` veneer (over the
//! `__rubyrs_socket_*` host fns) end to end: connect → write request →
//! read_nonblock the response → close. Also checks the
//! `Config::allow_network_io` gate. No external network.

#![cfg(feature = "_socket")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use rubyrs::{Config, Runtime, Value};

/// Accept exactly one connection, read the request, write `response`,
/// then close. Returns the bound port.
fn spawn_oneshot_server(response: &'static [u8]) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 4096];
        let _ = conn.read(&mut buf); // drain the request line(s)
        conn.write_all(response).expect("write response");
        // conn drops here → socket closed (EOF for the client read loop).
    });
    (port, handle)
}

fn rt_with_network() -> Runtime {
    let cfg = Config {
        allow_network_io: true,
        ..Config::default()
    };
    let mut rt = Runtime::with_config(cfg);
    rubyrs::register_socket_host_fns(&mut rt);
    rt
}

#[test]
fn tcpsocket_roundtrip_over_loopback() {
    let (port, server) = spawn_oneshot_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhi rs",
    );
    let mut rt = rt_with_network();
    let script = format!(
        r#"
        s = TCPSocket.new("127.0.0.1", {port})
        s.write("GET / HTTP/1.0\r\nHost: x\r\n\r\n")
        buf = +""
        while (chunk = s.read_nonblock(1024, nil, exception: false))
          buf << chunk
        end
        s.close
        # return [body, closed?]
        [buf.split("\r\n\r\n", 2).last, s.closed?]
        "#
    );
    let v = rt.eval(&script, "<socket_test>").expect("eval");
    server.join().unwrap();

    // v is the [body, closed?] Array.
    let arr = rt.resolve_array(&v).expect("expected Array result");
    match &arr[0] {
        Value::Str(s) => assert_eq!(s.to_string_lossy(), "hi rs"),
        other => panic!("expected body String, got {:?}", other),
    }
    assert!(matches!(arr[1], Value::Bool(true)), "socket should be closed");
}

#[test]
fn connect_blocked_when_network_io_disabled() {
    // Default Config has allow_network_io = false → connect raises.
    let mut rt = Runtime::with_config(Config::default());
    rubyrs::register_socket_host_fns(&mut rt);
    let res = rt.eval(
        r#"TCPSocket.new("127.0.0.1", 9)"#,
        "<socket_gate_test>",
    );
    let err = res.expect_err("connect must be blocked when allow_network_io is false");
    let msg = rt.format_trap(&err);
    assert!(
        msg.contains("network blocked") || msg.contains("SocketError"),
        "expected a network-blocked SocketError, got: {msg}"
    );
}
