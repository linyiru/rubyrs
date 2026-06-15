# 0029: `_openssl` battery — rustls TLS-client slice backing `Net::HTTP` https

## Status

Accepted — **Phase 2/3 SHIPPED** (2026-06, `184f21fc`). Fourth
per-battery ADR after 0022 (`_http_server`), 0027 (`_sqlite`), 0028
(`_socket`), following the ADR 0019 v3 Rule 7 template. The
sibling-battery half ADR 0028 §Context flagged: it specs the consuming
side of the `_socket` → TLS handle hand-off.

**Shipped:** `crates/rubyrs/src/openssl.rs` (4 host fns over rustls 0.23
+ ring + webpki-roots) + `preamble/openssl.rb` (the
`OpenSSL::SSL::SSLSocket`/`SSLContext` MRI-shape veneer). The
cross-battery seam `socket::take_stream` (ADR 0019 Rule 5) hands the
connected `TcpStream` to the TLS session. cfg-gated behind `_openssl`
(implies `_socket`), wired into `everything`. End-to-end validated:
rubyrs + the real `net/http` + `uri` gems → `status=200` over a real
loopback TLS server. Regression test: `tests/openssl_battery.rs`
(rcgen self-signed cert + rustls `ServerConnection`) — full client-stack
roundtrip + a handshake-fails-against-plaintext-peer negative test.

Driven by the same Bridgetown / Hanami boot path as ADR 0028: faraday's
net_http adapter does `require 'net/https'`, and any `https://` URL drives
`OpenSSL::SSL::SSLSocket`. ADR 0028 deliberately put plain TCP in
`_socket` and deferred TLS here.

## Context

ADR 0019's matrix slots `OpenSSL (low-level crypto)` at **Tier 3,
`_openssl` battery, rustls, deviation class `e` (RNG)`. This ADR
realises **only the TLS-client slice `Net::HTTP` needs** — it is NOT the
full MRI OpenSSL surface.

`Net::HTTP` for `https` does, per the net/http discovery spike:

```ruby
sock = TCPSocket.open(host, port)                 # the _socket battery
ssl  = OpenSSL::SSL::SSLSocket.new(sock, ctx)      # wrap it
ssl.hostname = host                                # SNI
ssl.connect                                        # TLS handshake
# then read/write through `ssl` via the same BufferedIO
# read_nonblock / write_nonblock surface as plain TCP.
```

So the slice is: `OpenSSL::SSL::SSLSocket` (client), `OpenSSL::SSL::SSLContext`,
the `VERIFY_PEER` / `VERIFY_NONE` constants, and `OpenSSL::SSL::SSLError`.
`Cipher`, `HMAC`, `PKey`, `X509::Certificate` building, `Digest` (already
its own battery), server-side TLS, and the rest of OpenSSL are
**deferred** — no consumer on the net/http path needs them.

## Decision

### 1. Vendor crate: `rustls` 0.23 + `ring` provider + `webpki-roots`

```toml
_openssl = ["dep:rustls", "dep:webpki-roots", "dep:rustls-pki-types"]

rustls           = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"], optional = true }
webpki-roots     = { version = "0.26", optional = true }
rustls-pki-types = { version = "1",    optional = true }
```

- **`ring` provider, not `aws-lc-rs`** (rustls 0.23's default). `ring`
  builds without a C toolchain / NASM, matching the "works on a minimal
  `cargo install` container" promise (same spirit as `_sqlite`'s bundled
  libsqlite3). `aws-lc-rs` is faster but adds a heavy build dependency.
- **`webpki-roots` (bundled Mozilla root store), not system roots.**
  Deterministic + portable (no `/etc/ssl` dependency), cross-host
  reproducible — the same rationale ADR 0027 used for bundled SQLite.
  System-root verification (`rustls-platform-verifier`) is a future
  opt-in if an embedder needs it.
- `tls12` feature on: real-world endpoints still negotiate TLS 1.2;
  TLS 1.3 alone would break a meaningful slice of hosts.

Not chosen: `native-tls` / `openssl` crate (links the C OpenSSL — the
build-fragility this whole tier exists to avoid).

### 2. The `_socket` handle hand-off (ADR 0019 Rule 5)

`OpenSSL::SSL::SSLSocket.new(tcp, ctx)` wraps a `TCPSocket` whose TLS
session must run over the SAME already-connected `TcpStream`. The
TcpStream lives in the `_socket` battery's per-Vm handle table. The
hand-off:

1. `ssl.connect` calls `__rubyrs_openssl_connect(tcp.__socket_handle,
   hostname, verify_mode)`.
2. The battery **takes** (removes) the `TcpStream` from `_socket`'s table
   via a crate-internal `socket::take_stream(handle)` and wraps it in a
   rustls `StreamOwned<ClientConnection, TcpStream>`, stored in
   `_openssl`'s OWN handle table; returns an `ssl_handle`.
3. Subsequent `ssl.read_nonblock` / `write_nonblock` route to
   `__rubyrs_openssl_{read,write}(ssl_handle, …)`, which drive rustls
   (handshake completion + record layer) over the owned TcpStream.

This is **NOT** the inadmissible "silent cross-battery state leakage"
(Rule 4): the hand-off is an EXPLICIT, user-mediated transfer —
`net/http.rb` passes the TCPSocket to `SSLSocket.new`, and the handle
moves ownership. After the take, the original `TCPSocket` is defunct (its
`close` no-ops; `_openssl` owns the stream and closes it on `ssl.close`).
`socket::take_stream` is the single declared seam between the two
batteries; it appears in this ADR per Rule 5.

### 3. Capability gating

The outbound reach is already gated at `TCPSocket.connect` time by the
`_socket` battery (`Config::allow_network_io` + `socket_allow_hosts`).
`_openssl` layers TLS over an ALREADY-permitted connection, so it adds no
new network-reach gate. Its only fresh capability is the handshake's
entropy/clock use.

### 4. Deviations (ADR 0019 Rule 4 closed taxonomy)

| Class | Item | Detail |
|---|---|---|
| **a** (owned-resource) | wraps the caller's socket | `SSLSocket.new(tcp, …)` operates on the TcpStream the caller already opened + the hostname it supplied. No implicit reach. |
| **e** (time / entropy) | TLS handshake | rustls/`ring` read system entropy (key exchange randoms) and the wall clock (cert validity windows). Documented, non-deterministic. |

NOT claimed: `b`/`c`/`d`/`f`/`g` — no subprocess, no reach beyond the
caller's host, no FS walk, no heap-cap bypass, no thread spawn (rustls
is synchronous over the blocking TcpStream).

### 5. Certificate verification

- Default `VERIFY_PEER`: rustls validates the chain against
  `webpki-roots` + checks the SNI hostname.
- `OpenSSL::SSL::VERIFY_NONE` (net/http `verify_mode=`): builds a rustls
  `ClientConfig` with a no-op `ServerCertVerifier` (dangerous-API,
  gated behind the explicit caller opt-in). Needed for self-signed /
  test endpoints and the loopback parity test.
- `ctx.cert_store` / CA-file customisation is deferred (webpki-roots
  only in v1); documented divergence.

### 6. Error mapping

rustls / handshake / cert errors → `OpenSSL::SSL::SSLError` (the class
`Net::HTTP` rescues). Connection-level io errors that surface during the
TLS record layer keep their `Errno::*` mapping from `_socket`'s table
where applicable; rustls protocol errors become `SSLError` with the
rustls message.

### 7. Surface freeze (Rule 7)

v1 Ruby surface (bare `require "openssl"`, MRI-shape — Rule 8
pure-Ruby-keeps-bare-name):

- `OpenSSL::SSL::SSLSocket.new(io, context = nil)`, `#hostname=`,
  `#connect`, `#read_nonblock` / `#write_nonblock` (the BufferedIO
  surface), `#read` / `#write` / `#<<`, `#close`, `#closed?`,
  `#sync` / `#sync=`, `#to_io`, `#peeraddr`, `#post_connection_check`
  (no-op when verified), `#peer_cert` (deferred → nil).
- `OpenSSL::SSL::SSLContext.new`, `#verify_mode` / `#verify_mode=`,
  `#min_version=` / `#max_version=` (accepted, best-effort),
  `#set_params` (accepts the options Hash net/http passes).
- `OpenSSL::SSL::VERIFY_NONE` (0), `VERIFY_PEER` (1),
  `OpenSSL::SSL::SSLError < OpenSSL::OpenSSLError < StandardError`.

Status **unstable** until one tagged release; then **stable**. `Cipher`,
`HMAC`, `PKey`, `X509`, server TLS are out of the freeze surface (not
shipped in v1).

## Capability host-fns consumed

```
__rubyrs_openssl_connect(socket_handle: Integer, hostname: String, verify: Integer) → ssl_handle (Integer)
__rubyrs_openssl_write(ssl_handle, bytes: String) → Integer (bytes written)
__rubyrs_openssl_read(ssl_handle, maxlen: Integer) → String(BINARY) | nil(EOF)
__rubyrs_openssl_close(ssl_handle) → nil
```

`socket::take_stream(handle) -> Option<TcpStream>` is the one cross-battery
seam (Rule 5). Same blocking-read contract as `_socket`: `read` blocks to
the deadline and returns bytes | nil(EOF) | raises `SSLError` — never
`:wait_readable`.

## Consequences

### Positive

- `https://` works end to end on the `Net::HTTP` path — the last piece
  for faraday / Bridgetown / Hanami outbound HTTP.
- Pure-Rust TLS (rustls/ring) — no C OpenSSL link, no system-cert
  dependency. Bundled roots → reproducible across hosts/containers.
- Reuses `_socket`'s connection + capability gate; `_openssl` adds only
  the TLS record layer.

### Negative

- `https` needs BOTH `_socket` + `_openssl` compiled in; the handle
  hand-off couples them (one declared seam).
- rustls + ring + webpki-roots add ~2–3 MB to the binary (recorded per
  ADR 0019 Part D in the feature's delta).
- v1 ships only the TLS-client slice; code reaching for `OpenSSL::Cipher`
  / `PKey` / `X509` gets NameError until a later phase.
- `VERIFY_NONE` uses rustls's dangerous no-verify API — gated behind the
  explicit caller opt-in, never the default.

## Alternatives considered

1. **`native-tls` / the `openssl` crate.** Links C OpenSSL — the build
   fragility (system OpenSSL version drift, missing headers) this tier
   was created to avoid. Rejected.
2. **Fold TLS into `_socket`.** Rejected in ADR 0028 §Alternatives 2:
   couples two deviation profiles + duplicates the crypto surface;
   keep them flat-composed (Rule 5).
3. **`aws-lc-rs` provider.** Faster, but needs a C toolchain / NASM —
   breaks the minimal-container build promise. `ring` is the portable
   default.
4. **System root store (`rustls-platform-verifier`).** Non-deterministic
   across hosts; deferred to an opt-in. Bundled `webpki-roots` is the
   reproducible v1 default.

## Migration plan

| Phase | Commit | Notes |
|---|---|---|
| 1 | This ADR (`docs/adr/0029-battery-openssl.md`) | **this commit** |
| 2 | `_openssl` battery — `src/openssl.rs` (host fns + rustls) + `socket::take_stream` seam + `preamble/openssl.rb` + Cargo deps + `register_openssl_host_fns` + CLI wiring | pending |
| 3 | Integration test (`tests/openssl_battery.rs`) — loopback **TLS** server (rustls server-side, self-signed) + `SSLSocket` round-trip with `VERIFY_NONE`; net/http `https` GET over the battery | pending |

## Related

- [ADR 0028 — `_socket` battery](0028-battery-socket.md) — the TCP half;
  this ADR consumes its `take_stream` seam.
- [ADR 0019 v3](0019-tier2-tier3-boundary.md) — Rule 4 (deviations),
  Rule 5 (flat intra-Tier-3 dep on `_socket`), Rule 7 (this ADR), Rule 8
  (bare `require "openssl"`).
- [ADR 0027 — `_sqlite`](0027-battery-sqlite.md) — the bundled-native
  (libsqlite3) precedent `webpki-roots` mirrors.
- `poc/net-http-spike/FINDINGS.md` — the net/http surface that scoped
  this slice.
