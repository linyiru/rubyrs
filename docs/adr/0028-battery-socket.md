# 0028: `_socket` battery — blocking std::net TCP primitives backing pure-Ruby `Net::HTTP`

## Status

Proposed (2026-06). Not yet accepted. Third per-battery ADR after
ADR 0022 (`_http_server`) and ADR 0027 (`_sqlite`), following the
ADR 0019 v3 Rule 7 template both established.

Driven by the Bridgetown boot spike (`poc/bridgetown-spike/`):
`require "bridgetown-core"` reaches `require "faraday"`, which
unconditionally requires its `net_http` adapter, which requires the
stdlib `net/http` — the first hard outbound-network dependency on the
Bridgetown / Hanami boot path. ADR 0019's placement question for
`net/http` was answered (see "Context") as **Tier 3, pure-Ruby
`Net::HTTP` on a native socket battery**; this ADR specs that battery.

## Context

### Where net/http lands (the placement decision this ADR implements)

Per [ADR 0019 v3](0019-tier2-tier3-boundary.md) Part A
(implementation-locus axis): `net/http` needs **no interpreter
changes** — no VM ops, no `Value` variant, no GC root, no parser
surface. It ships as require-able code. That makes it **Tier 3**, not
Tier 2. [ADR 0017](0017-tier1-boundary.md) already removed `Net` from
Tier 1 ("no script-accessible OS capabilities by default"), and it is
not CRuby-ABI shaped, so not Tier 4.

ADR 0019 Rule 5 names the exact shape: *"a Tier 3 pure-Ruby module may
depend on a Tier 3 native battery (e.g. `Net::HTTP` pure-Ruby on top of
`_http`)."* This ADR refines "`_http`" to a **socket primitive battery
(`_socket`)** rather than the matrix's high-level `_http` (reqwest/ureq)
fetch battery, because `Net::HTTP` is a *low-level* client — it owns the
socket (connect, per-request write, chunked read, keep-alive reuse,
TLS upgrade). A high-level fetch crate hides exactly the surface
`Net::HTTP` exposes, which would force a class-`h` semantic-parity
divergence on nearly every method. A blocking socket primitive lets us
vendor MRI's `net/http.rb` almost verbatim (Rule 6: pure-Ruby is
canonical; the native battery is the substrate, not the behaviour).

### The three-part layering

```
require "net/http"   →  pure-Ruby Net::HTTP  (Tier 3 canon, bare name)
                          │  uses
                          ├── require "socket"   → TCPSocket  (this battery's Ruby veneer, bare name)
                          │                          └── __rubyrs_socket_* host-fns  (this battery, native)
                          └── require "openssl"  → OpenSSL::SSL::SSLSocket  (the _openssl battery — sibling, its own ADR)
```

- **`net/http.rb`** and **`socket.rb` (`TCPSocket`)** are pure-Ruby,
  keep their **bare** require names (Rule 8: pure-Ruby Tier 3 keeps the
  MRI-shape name; the `rubyrs/` prefix is for native-only batteries).
- **This battery (`_socket`)** is the native substrate: blocking TCP
  via host-fns. Single-layer discipline (ADR 0019 Alternative 6, as in
  ADR 0027) — the host-fns are internal (`__rubyrs_socket_*`); the
  user-facing, parity-tested surface is the bare `TCPSocket` Ruby class.
- **TLS is NOT in this battery.** `https` goes through the **`_openssl`
  battery** (rustls; ADR 0019 matrix slot, its own ADR), which layers a
  TLS session over a connected `_socket` handle. Declared per Rule 5
  (flat intra-Tier-3 dependency; the user/`net/http.rb` mediates the
  handle hand-off, so it is not the inadmissible "silent cross-battery
  state leakage" — the socket handle is an explicit argument).

### Why ADR-first

`_socket` shapes a new `Config` capability gate (`socket_allow_hosts` +
a master `allow_network_io`), a new deviation surface (outbound network
reach), and an error-mapping table (`Errno::*` / `SocketError`). Those
deserve one `git blame` target, not five sites in the battery PR.

## Decision

### 1. Vendor crate: none — blocking `std::net`

```toml
# No new dependency. _socket uses std::net + std::io.
_socket = ["dep:libc"]   # libc only for a few errno refinements; std::net is the workhorse
```

`std::net::TcpStream` / `ToSocketAddrs` / `std::io::{Read,Write}`,
blocking, no async runtime. Rationale:

- **No tokio → no deviation class `g`.** `Net::HTTP` is a synchronous,
  one-request-at-a-time API; a blocking socket matches its semantics
  exactly. An async substrate would add a tokio runtime (binary size +
  the class-`g` native-thread deviation) for zero behavioural gain.
  Contrast `_http_server` (ADR 0022), which legitimately needs tokio
  for inbound concurrency — outbound `Net::HTTP` does not.
- **Zero new supply-chain surface.** `std::net` ships with the
  toolchain; `cargo-deny` review is a no-op. `libc` is already a
  dependency (`_http_server`, `_sqlite`); we reuse it for a couple of
  `errno` refinements (`ECONNRESET` vs `EPIPE`).
- **Faithful to CRuby.** MRI's `net/http` is itself pure Ruby over a
  blocking `TCPSocket` + `OpenSSL::SSL::SSLSocket`. Mirroring that stack
  keeps `net/http.rb` near-verbatim and the parity gate tight.

Not chosen:

- **`reqwest`** (ADR 0019 matrix `_http`): async (tokio), high-level —
  hides sockets/keep-alive/chunking, forcing class-`h` divergence on
  the `Net::HTTP` surface. Reserve `_http` for a *separate*, future
  high-level fetch battery (`HTTP.get(url)`-shape), not for backing
  `Net::HTTP`.
- **`ureq`**: blocking and closer, but still a full HTTP client — we'd
  be wrapping an HTTP client to implement an HTTP client. We only need
  the socket.
- **`mio` / `socket2` raw**: more control than needed; `std::net`'s
  `connect_timeout` + `set_read_timeout` + `set_write_timeout` cover
  `Net::HTTP`'s `open_timeout` / `read_timeout` / `write_timeout`.
  (`socket2` is already pulled by `_http_server` for `SO_REUSEPORT`; we
  don't need its bind-side knobs here.)

### 2. Capability host-fns consumed (ADR 0019 Rule 7 checklist)

The battery consumes ONE host capability — **outbound TCP reach** —
gated by **two** `Config` fields (master switch + allowlist), both
checked at connect time:

```rust
pub struct Config {
    // ... existing fields ...
    /// Master gate for outbound network I/O (parallels
    /// `allow_filesystem_io`). Default false → Tier 1's "no network"
    /// stays the embed/sandbox default even with `_socket` compiled in.
    pub allow_network_io: bool,
    /// Host[:port] allowlist (ADR 0019 Rule 4 runtime sub-rule for
    /// deviation class `a`). When `Some`, a connect outside the list
    /// traps `SecurityError`. When `None`, no host restriction (only
    /// the master gate applies).
    pub socket_allow_hosts: Option<Vec<String>>,
    /// Heap-cap on total bytes read per socket (ADR 0019 class `f`).
    /// Net::HTTP assembles response bodies in Ruby; this bounds a
    /// runaway/streaming server. `None` = unbounded (CRuby default).
    pub socket_max_read_bytes: Option<usize>,
}
```

Host-fns exposed to scripts (registered via `Runtime::register_fn` when
`_socket` is built in), internal names — the bare `TCPSocket` Ruby class
wraps them (single-layer discipline, ADR 0027 §"Capability host-fns"):

```
__rubyrs_socket_connect(host: String, port: Integer, open_timeout: Float|nil) → handle (Integer)
__rubyrs_socket_write(handle, bytes: String) → Integer (bytes written)
__rubyrs_socket_read(handle, maxlen: Integer, read_timeout: Float|nil) → String (BINARY) | nil (EOF)
__rubyrs_socket_close(handle) → nil
__rubyrs_socket_peeraddr(handle) → [family, port, host, ip]   # Net::HTTP diagnostics
```

`handle` is an opaque integer index into a per-Vm
`HashMap<i64, TcpStream>` (same pattern as `_sqlite`'s `ConnState`
table). Connect resolves the caller's `host` via
`std::net::ToSocketAddrs` (system resolver), checks the allowlist on
the *literal* host the caller supplied, then `TcpStream::connect_timeout`.
The handle table is dropped on `Vm` teardown (sockets closed).

### 3. Deviations (ADR 0019 Rule 4 closed taxonomy)

| Class | Item | Detail |
|---|---|---|
| **a** (owned-resource I/O) | All TCP reach | The caller supplies `host`+`port` (via `TCPSocket.new` / `Net::HTTP.new(host, port)` / a URI). The battery connects to exactly that endpoint and nothing else. DNS resolution is of the caller's literal host. Gated by `Config::socket_allow_hosts`. |
| **e** (time/entropy) | Connect/read deadlines | Timeouts read the wall clock. Non-deterministic but documented; no entropy source (TLS RNG lives in the `_openssl` battery, its class `e`). |
| **f** (heap-cap) | Per-socket read total | `Config::socket_max_read_bytes` bounds total bytes read on one handle; trap `Errno::EMSGSIZE`-shaped `Net::HTTP`-level error when exceeded. Default `None` (CRuby parity). |

**Explicitly NOT claimed:**

- **`c` (network reach beyond a single URL)** — the `_socket` battery
  connects to exactly the caller's host:port. `Net::HTTP` does **not**
  auto-follow redirects (CRuby parity — the caller re-issues), so even
  the pure-Ruby layer stays class `a`. A future high-level `_http`
  fetch battery that follows redirects WOULD claim `c`; this battery
  does not.
- **`g` (native-thread spawn)** — blocking `std::net`, no tokio, no
  thread pool. This is the load-bearing reason for the crate choice.
- **`b` (subprocess), `d` (FS walk)** — none.

No env-var trapdoors; no privilege escalation; no silent cross-battery
state leakage (the `_openssl` handle hand-off is an explicit argument
mediated by `net/http.rb`, §Context).

### 4. Error mapping

Rust `std::io::Error` / `ErrorKind` → Ruby exception classes, matching
what MRI's socket + `net/http` raise so ported `rescue` clauses work:

| Rust side | Ruby exception | Layer |
|---|---|---|
| `ErrorKind::ConnectionRefused` | `Errno::ECONNREFUSED` | socket |
| `ErrorKind::ConnectionReset` | `Errno::ECONNRESET` | socket |
| `ErrorKind::BrokenPipe` | `Errno::EPIPE` | socket |
| `ErrorKind::TimedOut` (connect) | `Net::OpenTimeout` | net/http |
| read deadline elapsed | `Net::ReadTimeout` | net/http |
| `ToSocketAddrs` resolution failure | `SocketError` (`"getaddrinfo: …"`) | socket |
| allowlist / master-gate rejection | `SecurityError` ("network blocked") | battery gate |
| `socket_max_read_bytes` exceeded | `Net::HTTP`-level body-too-large error | battery cap |

`Errno::*` are the existing rubyrs `SystemCallError` subclasses; the
socket battery maps `ErrorKind` → the right `Errno` constant. The
`Net::OpenTimeout` / `Net::ReadTimeout` / `SocketError` classes ship in
the pure-Ruby `net/http.rb` / `socket.rb` veneer.

### 5. Encoding

Response bodies are **bytes**. `__rubyrs_socket_read` returns
ASCII-8BIT (BINARY) strings (rubyrs supports BINARY strings today).
`net/http.rb` assembles the body as a BINARY string; charset decoding
to UTF-8/other follows the response `Content-Type` and is the caller's
job — the same contract ADR 0019's open-question fixed for `_http`
("returns bytes; String decoding is the caller's job") and consistent
with [ADR 0020](0020-encoding-placement.md). No textual-battery
encoding block (ADR 0019) applies — `_socket` is byte-level.

### 6. Surface freeze policy (ADR 0019 Rule 7)

v1 Ruby surface (the parity-tested API):

- `TCPSocket.new(host, port[, opts])` / `.open(...)` — client only.
  `#read(maxlen)`, `#write(str)`, `#<<`, `#close`, `#closed?`,
  `#peeraddr`, timeout accessors. (`gets`/`readline`/`each_line` come
  from the `IO`-veneer mix-in; v1 implements the minimum `Net::HTTP`
  drives.)
- `Net::HTTP` — `.new(host, port)`, `#start`, `#request`, `#get`,
  `#post`, `#request_get`, `Net::HTTP.get(uri)`, `Net::HTTPResponse`
  hierarchy, `open_timeout=` / `read_timeout=`, `use_ssl=` (delegates
  to `_openssl`).
- `Net::OpenTimeout`, `Net::ReadTimeout`, `Net::HTTPError` family,
  `SocketError`.

Status: **unstable** until shipped in one tagged release; then
**stable** (semver-tracked). Removing a stable method needs a new ADR.

`TCPServer`, `UDPSocket`, `UNIXSocket`, `BasicSocket#recvmsg`, and the
full `Socket` (BSD-level `Socket.new(AF_INET, …)`) surface are
**deferred** — `Net::HTTP` (the driver) needs none of them. A future
consumer (a pure-Ruby server, DNS-over-UDP) re-opens this ADR.

### 7. Feature aggregates (ADR 0019 Part C)

```toml
_socket = ["dep:libc"]
# `_socket` joins neither cli-defaults nor everything in this ADR's
# initial cut — outbound network is the strongest sandbox-default-off
# capability. A follow-up may add it to `everything`; cli-defaults
# stays network-free until the host-allowlist UX is proven.
```

Binary-size delta: ~0 (std::net is in the toolchain; libc already
linked). Recorded per ADR 0019 Part D's per-battery accounting.

## Consequences

### Positive

- `require "faraday"` (and thus the Bridgetown / Hanami boot path) gets
  a real `net/http` to load against — the current frontier wall clears.
- `net/http` lands as **canonical pure Ruby** (Rule 6) — vendored MRI
  source, maximally faithful, parity-testable against CRuby.
- No new async runtime, no tokio, no class-`g` deviation, ~0 binary
  delta — the cheapest possible network battery.
- Establishes the `Config::allow_network_io` + `socket_allow_hosts`
  capability surface that every future network battery (`_http` fetch,
  `_s3`, `_websocket`) reuses.

### Negative

- Blocking sockets mean no concurrent in-flight requests from one Vm
  without `_thread`/`_fiber` — fine for `Net::HTTP`'s synchronous model,
  a ceiling for a future high-throughput client (which would be the
  separate async `_http` battery).
- TLS is split into `_openssl` — `https` needs **two** batteries
  (`_socket` + `_openssl`) compiled in. The handle hand-off across the
  battery boundary is a real (if explicit, user-mediated) coupling that
  the `_openssl` ADR must spec the consuming side of.
- Vendoring MRI's `net/http.rb` pulls a large pure-Ruby surface; parts
  beyond v1's frozen surface load but are untested until exercised.

### Explicitly traded away

- **A high-level ergonomic fetch API** (`HTTP.get(url) → body`). That is
  a *different* battery (`_http`, ADR 0019 matrix, reqwest) with a
  different deviation profile (class `c` redirects, class `g` tokio).
  `_socket` deliberately stays the low-level primitive.
- **Server/UDP/UNIX sockets** — deferred to a consumer-driven re-open.

## Alternatives considered

1. **Back `Net::HTTP` with the `_http` (reqwest) battery.** Rejected:
   reqwest hides sockets/keep-alive/chunked transfer; mapping
   `Net::HTTP`'s socket-level surface onto it is class-`h` divergence on
   nearly every method, and drags tokio (class `g`) in for a
   synchronous API. ADR 0019 Rule 6 wants the pure form canonical;
   reqwest can't be the faithful substrate.
2. **Put TLS inside `_socket` (bundle rustls here).** Rejected: ADR
   0019's matrix already slots `OpenSSL` as its own `_openssl` battery
   (rustls), and `Net::HTTP` reaches TLS via `OpenSSL::SSL::SSLSocket`.
   Folding TLS into `_socket` duplicates the crypto surface and couples
   two deviation profiles (class `e` RNG) into one battery. Keep them
   flat-composed per Rule 5.
3. **A two-layer `Native.tcp_connect` primitive + pure-Ruby wrapper as
   a distinct design.** This IS the chosen shape (host-fns + bare
   `TCPSocket`), but framed as single-layer discipline per ADR 0019
   Alternative 6 / ADR 0027 — the host-fns are internal, the Ruby
   `TCPSocket` is THE surface, not a second public layer.
4. **`async-std`/`tokio` blocking-bridge.** Rejected: a runtime for a
   synchronous API. `std::net` blocking is the right tool.

## Migration plan

New feature; no migration. Phases (one atomic commit each):

| Phase | Commit | Notes |
|---|---|---|
| 1 | `poc/net-http-spike/FINDINGS.md` | Discovery: what surface of `net/http.rb` faraday actually drives |
| 2 | This ADR (`docs/adr/0028-battery-socket.md`) | **this commit** |
| 3 | `_socket` battery PoC — `src/socket.rs` host-fns + `Config` fields + `lib.rs` `register_socket_host_fns` export + `Cargo.toml` `_socket` feature | pending |
| 4 | Pure-Ruby `TCPSocket` veneer (`preamble`/`stdlib_vendor`) + bare `require "socket"` resolution | pending |
| 5 | Vendor/trim MRI `net/http.rb` + `Net::HTTPResponse` hierarchy; bare `require "net/http"` | pending |
| 6 | `_openssl` battery (separate ADR) for `use_ssl=` / `https` | separate track |
| 7 | diff_cruby parity fixtures (loopback server via `_http_server`, or a fixed-response stub) + faraday-loads smoke | pending |

`_openssl` is on its own track (Phase 6) — `http://` works after Phase
5; `https://` waits for the TLS battery.

## Related

- [ADR 0015 — Concentric architecture](0015-concentric-architecture.md)
  — `_net` was sketched at the `stdlib` tier; this ADR realises it as
  `_socket`.
- [ADR 0017 — Tier 1 boundary](0017-tier1-boundary.md) — removed `Net`
  from Tier 1; this battery is the opt-in re-introduction.
- [ADR 0019 v3 — Tier 2/3 boundary](0019-tier2-tier3-boundary.md) —
  Rule 4 (deviation taxonomy + runtime allowlist), Rule 5 (flat
  intra-Tier-3 dep on `_openssl`), Rule 6 (pure-Ruby canonical), Rule 7
  (this ADR), Rule 8 (bare names for pure-Ruby `net/http` / `socket`).
- [ADR 0020 — Encoding placement](0020-encoding-placement.md) — bodies
  are BINARY bytes; charset decoding is the caller's.
- [ADR 0022 — `_http_server` battery](0022-http-server-battery.md) —
  the **inbound** counterpart (tokio+hyper); `_socket` is **outbound**
  and deliberately does NOT reuse its runtime (Rule 5: no silent
  sibling import).
- [ADR 0027 — `_sqlite` battery](0027-battery-sqlite.md) — the
  per-Vm-handle-table + single-layer-discipline pattern this ADR mirrors.
- The future `_openssl` battery ADR (TLS via rustls) — consumes a
  `_socket` handle; specs the TLS-over-handle hand-off.
