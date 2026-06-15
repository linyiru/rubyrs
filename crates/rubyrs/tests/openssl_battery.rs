//! `_openssl` battery integration test (ADR 0029 Phase 3).
//!
//! Spins up a real loopback TLS server (self-signed cert via `rcgen`,
//! served by a rustls `ServerConnection`) on a background OS thread and
//! drives the full client stack: the `_socket` `TCPSocket` veneer →
//! `OpenSSL::SSL::SSLSocket#connect` (rustls handshake over the
//! handed-off `TcpStream`) → write request → read the response → close.
//! Verification is disabled client-side (`VERIFY_NONE`) so the
//! self-signed cert is accepted. No external network.

#![cfg(feature = "_openssl")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use rubyrs::{Config, Runtime, Value};

/// Accept exactly one TLS connection, read the request, write
/// `response`, send close_notify, then drop. Returns the bound port.
fn spawn_oneshot_tls_server(response: &'static [u8]) -> (u16, thread::JoinHandle<()>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("generate self-signed cert");
    let cert_der = certified.cert.der().clone();
    let key_der =
        rustls::pki_types::PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der());

    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("server protocol versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert_der], key_der.into())
    .expect("server single cert");
    let server_config = Arc::new(server_config);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut conn =
            rustls::ServerConnection::new(server_config).expect("server connection");
        {
            let mut tls = rustls::Stream::new(&mut conn, &mut sock);
            let mut buf = [0u8; 4096];
            let _ = tls.read(&mut buf); // drive handshake + drain request
            tls.write_all(response).expect("write response");
            tls.flush().expect("flush response");
        }
        conn.send_close_notify();
        let mut tls = rustls::Stream::new(&mut conn, &mut sock);
        let _ = tls.flush();
        // sock drops → connection closed.
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
    rubyrs::register_openssl_host_fns(&mut rt);
    rt
}

#[test]
fn sslsocket_roundtrip_over_loopback_tls() {
    let (port, server) = spawn_oneshot_tls_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\ntls rs!!",
    );
    let mut rt = rt_with_network();
    let script = format!(
        r#"
        s = TCPSocket.new("127.0.0.1", {port})
        ctx = OpenSSL::SSL::SSLContext.new
        ctx.verify_mode = OpenSSL::SSL::VERIFY_NONE
        ssl = OpenSSL::SSL::SSLSocket.new(s, ctx)
        ssl.hostname = "localhost"
        ssl.connect
        ssl.write("GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        buf = +""
        while (chunk = ssl.read_nonblock(1024, nil, exception: false))
          buf << chunk
        end
        ssl.close
        [buf.split("\r\n\r\n", 2).last, ssl.closed?]
        "#
    );
    let v = rt.eval(&script, "<openssl_test>").expect("eval");
    server.join().unwrap();

    let arr = rt.resolve_array(&v).expect("expected Array result");
    match &arr[0] {
        Value::Str(s) => assert_eq!(s.to_string_lossy(), "tls rs!!"),
        other => panic!("expected body String, got {:?}", other),
    }
    assert!(matches!(arr[1], Value::Bool(true)), "SSL socket should be closed");
}

#[test]
fn handshake_fails_against_plaintext_peer() {
    // A non-TLS server: the rustls handshake must fail (SSLError),
    // never hang or succeed.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        // Speak plaintext at the TLS client, then close.
        let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
    });
    let mut rt = rt_with_network();
    let script = format!(
        r#"
        s = TCPSocket.new("127.0.0.1", {port})
        ctx = OpenSSL::SSL::SSLContext.new
        ctx.verify_mode = OpenSSL::SSL::VERIFY_NONE
        ssl = OpenSSL::SSL::SSLSocket.new(s, ctx)
        ssl.hostname = "localhost"
        ssl.connect
        "#
    );
    let res = rt.eval(&script, "<openssl_handshake_fail>");
    let _ = server.join();
    let err = res.expect_err("handshake against a plaintext peer must fail");
    let msg = rt.format_trap(&err);
    assert!(
        msg.contains("SSLError") || msg.contains("handshake"),
        "expected a TLS handshake SSLError, got: {msg}"
    );
}
