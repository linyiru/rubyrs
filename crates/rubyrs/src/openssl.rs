//! `_openssl` battery — rustls TLS-client slice per ADR 0029.
//!
//! The minimal TLS surface `Net::HTTP` https drives: wrap a connected
//! `_socket` `TcpStream` in a rustls client session and expose the same
//! blocking `read`/`write` contract as `_socket`. Backed by rustls 0.23
//! + the `ring` provider + bundled `webpki-roots` (no C OpenSSL link, no
//! system-cert dependency).
//!
//! Cross-battery seam (ADR 0029 §2, ADR 0019 Rule 5): `connect` TAKES the
//! TcpStream from the `_socket` handle table via `socket::take_stream`
//! and owns it for the TLS session's lifetime. The hand-off is explicit
//! (net/http.rb passes the TCPSocket to `SSLSocket.new`).
//!
//! 4 host fns (mirror `_socket`'s shape):
//!   __rubyrs_openssl_connect(socket_handle, hostname, verify) → ssl_handle
//!   __rubyrs_openssl_write(ssl_handle, bytes)                 → bytes written
//!   __rubyrs_openssl_read(ssl_handle, maxlen)                 → String(BINARY) | nil(EOF)
//!   __rubyrs_openssl_close(ssl_handle)                        → nil

#![cfg(feature = "_openssl")]

use crate::error::{RubyError, Trap};
use crate::value::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};

/// A live TLS session over an owned TcpStream.
struct TlsState {
    conn: ClientConnection,
    sock: TcpStream,
}

thread_local! {
    static TLS_CONNS: RefCell<HashMap<i64, TlsState>> = RefCell::new(HashMap::new());
    static NEXT_HANDLE: Cell<i64> = const { Cell::new(1) };
    // Verify-peer ClientConfig is built once per thread (webpki-roots
    // parsing is non-trivial); cached behind the first connect.
    static VERIFY_CONFIG: RefCell<Option<Arc<ClientConfig>>> = const { RefCell::new(None) };
}

fn arg_err(msg: &str) -> Trap {
    Trap { err: RubyError::ArgumentError { msg: msg.to_string() }, backtrace: vec![] }
}

fn ssl_err(msg: String) -> Trap {
    Trap {
        err: RubyError::HostException { class_name: "OpenSSL::SSL::SSLError".to_string(), message: msg },
        backtrace: vec![],
    }
}

fn closed_ssl() -> Trap {
    Trap {
        err: RubyError::HostException { class_name: "IOError".to_string(), message: "closed SSL stream".to_string() },
        backtrace: vec![],
    }
}

/// rustls 0.23 `ring` provider, shared.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Cached verify-peer config (webpki-roots + SNI hostname check).
fn verify_config() -> Result<Arc<ClientConfig>, Trap> {
    VERIFY_CONFIG.with(|c| {
        if let Some(cfg) = c.borrow().as_ref() {
            return Ok(cfg.clone());
        }
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| ssl_err(format!("rustls protocol setup: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let arc = Arc::new(cfg);
        *c.borrow_mut() = Some(arc.clone());
        Ok(arc)
    })
}

/// VERIFY_NONE config — a no-op certificate verifier behind rustls's
/// dangerous API. Built per-connect (rare; not cached). Used only when
/// the caller explicitly sets `verify_mode = OpenSSL::SSL::VERIFY_NONE`
/// (self-signed / test endpoints), never the default.
fn no_verify_config() -> Result<Arc<ClientConfig>, Trap> {
    let cfg = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| ssl_err(format!("rustls protocol setup: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(danger::NoVerifier::new()))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

mod danger {
    //! rustls `VERIFY_NONE` verifier — accepts any chain/signature.
    //! Gated behind the explicit `OpenSSL::SSL::VERIFY_NONE` opt-in.
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct NoVerifier(rustls::crypto::CryptoProvider);

    impl NoVerifier {
        pub(super) fn new() -> Self {
            NoVerifier(rustls::crypto::ring::default_provider())
        }
    }

    impl ServerCertVerifier for NoVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }
}

pub fn register_host_fns(rt: &mut crate::Runtime) {
    const PREAMBLE: &str = include_str!("preamble/openssl.rb");
    if let Err(trap) = rt.eval(PREAMBLE, "<rubyrs:openssl>") {
        panic!("ICE: _openssl failed to load preamble: {trap:?}");
    }

    rt.register_fn("__rubyrs_openssl_connect", |args| {
        let (socket_handle, hostname, verify) = match args {
            [Value::Int(h), Value::Str(host), Value::Int(v)] => (*h, host.to_string_lossy(), *v),
            _ => return Err(arg_err(
                "__rubyrs_openssl_connect(socket_handle: Integer, hostname: String, verify: Integer)",
            )),
        };
        // Take the connected TcpStream from the _socket battery (Rule 5).
        let mut sock = crate::socket::take_stream(socket_handle)
            .ok_or_else(|| ssl_err("underlying socket already closed or transferred".to_string()))?;
        let server_name = ServerName::try_from(hostname.clone())
            .map_err(|_| ssl_err(format!("invalid SNI hostname {:?}", hostname)))?
            .to_owned();
        // verify == 0 → VERIFY_NONE (caller opt-in); else verify-peer.
        let config = if verify == 0 { no_verify_config()? } else { verify_config()? };
        let mut conn = ClientConnection::new(config, server_name)
            .map_err(|e| ssl_err(format!("TLS client setup: {e}")))?;
        // Drive the handshake now so `ssl.connect` surfaces cert /
        // protocol failures as SSLError (CRuby raises at connect).
        conn.complete_io(&mut sock)
            .map_err(|e| ssl_err(format!("TLS handshake failed: {e}")))?;
        let handle = NEXT_HANDLE.with(|c| {
            let h = c.get();
            c.set(h + 1);
            h
        });
        TLS_CONNS.with(|m| m.borrow_mut().insert(handle, TlsState { conn, sock }));
        Ok(Value::Int(handle))
    });

    rt.register_fn("__rubyrs_openssl_write", |args| {
        let (handle, bytes) = match args {
            [Value::Int(h), Value::Str(s)] => (*h, s.borrow().clone()),
            _ => return Err(arg_err("__rubyrs_openssl_write(ssl_handle, bytes: String)")),
        };
        TLS_CONNS.with(|m| -> Result<Value, Trap> {
            let mut map = m.borrow_mut();
            let st = map.get_mut(&handle).ok_or_else(closed_ssl)?;
            let mut tls = rustls::Stream::new(&mut st.conn, &mut st.sock);
            tls.write_all(&bytes).map_err(|e| ssl_err(format!("TLS write: {e}")))?;
            tls.flush().map_err(|e| ssl_err(format!("TLS flush: {e}")))?;
            Ok(Value::Int(bytes.len() as i64))
        })
    });

    rt.register_fn("__rubyrs_openssl_read", |args| {
        let (handle, maxlen) = match args {
            [Value::Int(h), Value::Int(n)] => (*h, *n),
            // accept a trailing timeout arg for symmetry with _socket; ignored
            [Value::Int(h), Value::Int(n), _] => (*h, *n),
            _ => return Err(arg_err("__rubyrs_openssl_read(ssl_handle, maxlen: Integer)")),
        };
        TLS_CONNS.with(|m| -> Result<Value, Trap> {
            let mut map = m.borrow_mut();
            let st = map.get_mut(&handle).ok_or_else(closed_ssl)?;
            let mut tls = rustls::Stream::new(&mut st.conn, &mut st.sock);
            let n = maxlen.max(0) as usize;
            let mut buf = vec![0u8; n];
            match tls.read(&mut buf) {
                Ok(0) => Ok(Value::Nil), // clean EOF / close_notify
                Ok(got) => {
                    buf.truncate(got);
                    Ok(Value::new_str_bytes_binary(buf))
                }
                // A peer that drops the TCP connection without a TLS
                // close_notify surfaces as UnexpectedEof; net/http treats
                // a Connection: close response body end the same as EOF,
                // so map it to nil rather than raising.
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(Value::Nil),
                Err(e) => Err(ssl_err(format!("TLS read: {e}"))),
            }
        })
    });

    rt.register_fn("__rubyrs_openssl_close", |args| {
        let handle = match args {
            [Value::Int(h)] => *h,
            _ => return Err(arg_err("__rubyrs_openssl_close(ssl_handle)")),
        };
        TLS_CONNS.with(|m| {
            if let Some(mut st) = m.borrow_mut().remove(&handle) {
                // Best-effort TLS close_notify, then the TcpStream drops.
                st.conn.send_close_notify();
                let mut tls = rustls::Stream::new(&mut st.conn, &mut st.sock);
                let _ = tls.flush();
            }
        });
        Ok(Value::Nil)
    });

    // Symmetric-crypto slice for `Rack::Session::Encryptor` (ADR 0029
    // addendum). The `OpenSSL::Cipher` / `OpenSSL::HMAC` veneers in
    // preamble/openssl.rb call these; the AES / HMAC maths lives in
    // `crate::aes` (FIPS-197 + RFC 2104), validated against NIST /
    // RFC 4231 vectors there.

    // `__rubyrs_hmac_sha256(key, data)` → 32-byte BINARY String.
    rt.register_fn("__rubyrs_hmac_sha256", |args| {
        let (key, data) = match args {
            [Value::Str(k), Value::Str(d)] => (k.borrow().clone(), d.borrow().clone()),
            _ => return Err(arg_err("__rubyrs_hmac_sha256(key: String, data: String)")),
        };
        let mac = crate::aes::hmac_sha256(&key, &data);
        Ok(Value::new_str_bytes_binary(mac.to_vec()))
    });

    // `__rubyrs_aes256_ctr(key, iv, byte_offset, data)` → BINARY String.
    // CTR is a stream cipher: encrypt and decrypt are the same call. The
    // 32-byte key and 16-byte IV are validated for length here so a
    // wrong-sized key surfaces as an OpenSSLError, not a panic.
    rt.register_fn("__rubyrs_aes256_ctr", |args| {
        let (key, iv, offset, data) = match args {
            [Value::Str(k), Value::Str(v), Value::Int(off), Value::Str(d)] => (
                k.borrow().clone(),
                v.borrow().clone(),
                *off,
                d.borrow().clone(),
            ),
            _ => return Err(arg_err(
                "__rubyrs_aes256_ctr(key: String, iv: String, offset: Integer, data: String)",
            )),
        };
        let key: [u8; 32] = key.as_slice().try_into()
            .map_err(|_| ssl_err(format!("aes-256-ctr key must be 32 bytes (got {})", key.len())))?;
        let iv: [u8; 16] = iv.as_slice().try_into()
            .map_err(|_| ssl_err(format!("aes-256-ctr iv must be 16 bytes (got {})", iv.len())))?;
        let offset = u64::try_from(offset).unwrap_or(0);
        Ok(Value::new_str_bytes_binary(crate::aes::aes256_ctr_xor(&key, &iv, offset, &data)))
    });
}
