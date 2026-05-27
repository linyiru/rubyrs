# 0022: `_http_server` battery — Rust HTTP front, Ruby app handler

## Status

Proposed (2026-05-27). First Tier 3 native battery ADR per
[ADR 0019 v3](0019-tier2-tier3-boundary.md) Rule 7 ("each
Tier 3 battery gets its own ADR"). Establishes the template
for subsequent battery ADRs.

## Context

ADR 0019 v3's matrix names `_http` (outbound HTTP client) as
a candidate Tier 3 battery. Inbound HTTP — the server side —
is not explicitly listed but emerged from a strategic
discussion as the **load-bearing differentiator** for the
project's Bun-class positioning:

- CRuby + Puma: Puma is a Ruby HTTP server using C extensions
  for socket I/O. Tops out at ~5k RPS for Sinatra hello-world.
- CRuby + Falcon: Fiber-based, ~10k RPS. State of the art for
  pure-Ruby web.
- **rubyrs + Rust HTTP front**: hyper handles socket / parse /
  serialize entirely in Rust; rubyrs VM only runs the Ruby
  app code. Estimated 20-40k RPS, plus HTTP/2 + HTTP/3 +
  rustls TLS that pure-Ruby servers don't have.

The win comes from **moving the wire-protocol work out of the
Ruby VM**, not from making the VM itself faster. This is the
same play Bun makes against Node (`Bun.serve` uses zig-native
HTTP server) — except we keep the unmodified Rack/Sinatra
ecosystem on top.

For v1 we ship the Rust HTTP server battery with a minimal
in-process Rack-env-shaped adapter. Real `require "rack"` from
the unmodified gem source needs `autoload` (issue #224) + 7
stdlib batteries (uri, time, cgi/util, forwardable, singleton,
plus the 2 we already have) before it can run; the
`_http_server` battery ships independently of that work and
talks to either a mini-Rack stub (Tier 3 pure-Ruby) or the
real Rack gem once it can load.

## Decision

### Vendor crate

**`hyper` 1.x + `hyper-util` + `tokio` (current-thread
runtime).** No `axum` — its routing layer is what the Ruby
app provides; axum's middleware tower is what Rack middleware
provides. We need accept + parse + serialize, which is the
`hyper` surface exactly.

Cargo deps when feature enabled:

```toml
[dependencies]
hyper = { version = "1", features = ["server", "http1", "http2"], optional = true }
hyper-util = { version = "0.1", features = ["tokio", "server-auto"], optional = true }
tokio = { version = "1", features = ["rt", "net", "io-util", "sync"], optional = true }
http-body-util = { version = "0.1", optional = true }
bytes = { version = "1", optional = true }

[features]
_http_server = [
    "dep:hyper", "dep:hyper-util", "dep:tokio",
    "dep:http-body-util", "dep:bytes",
]
```

Optional later (own per-battery ADRs):
- `_http_server_tls` — adds `rustls` + `tokio-rustls`
- `_http_server_h3` — adds `quinn` for HTTP/3

### Deviation classes claimed (per ADR 0019 v3 Rule 4)

- **Class a (owned-resource I/O)** — server binds to a
  caller-supplied `(host, port)`. The address is part of
  the Ruby app's explicit config; the battery doesn't
  inspect arbitrary network state.
- **Class g (native-thread spawn)** — tokio uses an
  internal worker thread for I/O even when the runtime is
  configured `current_thread`. The blocking thread pool
  for filesystem ops is NOT initialised by this battery
  (we never call `tokio::task::spawn_blocking`).

Classes **NOT** claimed:
- ❌ Class c (multi-host network reach) — server is
  **inbound only**. It does not initiate outbound
  connections. Reverse-proxy or external API calls are an
  app responsibility going through `_http` (outbound
  battery), not this one.
- ❌ Class f (mmap / heap-cap bypass) — no.

### Runtime allowlist (per ADR 0019 v3 Rule 4 sub-rule)

Class **a** requires an embedder-supplied allowlist. For
this battery the natural shape is:

```rust
pub struct HttpServerConfig {
    /// Bind address. None = battery is loaded but server
    /// not started until `Rack::Server.run(bind)` script-
    /// side call.
    pub bind: Option<std::net::SocketAddr>,

    /// Max concurrent in-flight requests. None = unlimited
    /// (not recommended). Backpressure via tokio semaphore.
    pub max_concurrent_requests: Option<usize>,

    /// Per-request wall-clock deadline. Independent of the
    /// VM's `Config::deadline` — that one resets per
    /// `eval()`, this one applies per HTTP request. None =
    /// no timeout (`Config::deadline` is still enforced
    /// within the VM).
    pub per_request_deadline: Option<std::time::Duration>,

    /// Max request body size. None = unlimited.
    pub max_request_body_bytes: Option<usize>,
}
```

Exposed via `Config::http_server: Option<HttpServerConfig>`.
Default `None` — no server runs.

### Surface freeze policy

Per ADR 0019 v3 Rule 7 surface freeze:

- **v0.x (unstable)** — Ruby-side API can change between
  releases. Embedders pin a specific rubyrs version.
- **v1.0 (stable on the named Ruby surface)**: this set of
  Ruby API names freezes:
  - `Rack::Handler::Rubyrs.run(app, bind:, ...)` —
    entrypoint that mounts a Rack app
  - Adherence to the [Rack SPEC](https://github.com/rack/rack/blob/main/SPEC.rdoc)
    env hash conventions
  - `Rack::Handler::Rubyrs::Config` constants for the
    embedder-supplied settings above
- **Adding** Ruby methods to the battery's surface is a
  patch bump. **Removing** is a minor bump (semver 0.y.z
  rules).

### Error mapping

Rust-side errors surface as Ruby exception classes via the
existing `RubyError` machinery (ADR 0008):

| Rust error | Ruby exception | When |
|---|---|---|
| `hyper::Error` parse failure | `IOError` | malformed request from client |
| Socket bind failure | `Errno::EADDRINUSE` (or `Errno::EACCES`) | server start |
| Request body size exceeded | `Errno::EMSGSIZE` | streaming body read |
| Per-request deadline trap | `Timeout::Error` | per-request timeout |
| VM `Config::deadline` trap | `ResourceExhausted` | VM-level cap (existing) |
| App-side Ruby exception | propagated to Rack response 500 | app `raise`d uncaught |

The "app-side uncaught exception → 500 with backtrace as
response body" behaviour is the standard Rack contract; we
match it.

### Capability host-fns the battery consumes

The battery itself owns the `tokio` runtime + `hyper` server
state inside the Rust crate. It does NOT route through host
fns the embedder set via `register_fn` — those host fns are
for the *Ruby app* to call. The battery exposes:

- `Rubyrs::HttpServer.bind(addr) -> ServerHandle`
- `ServerHandle#run(rack_app)` — starts the loop, blocks
  the calling Ruby thread until `Ctrl-C` / `#shutdown`
- `ServerHandle#shutdown` — graceful stop

These are the Ruby-side API; their implementations call into
Rust directly (no `register_fn` indirection — that's the
embed API for end-user host functions).

### VM concurrency model

**v1 ships single-threaded.** Tokio uses
`Builder::new_current_thread()`. All HTTP I/O happens on the
same OS thread as the rubyrs VM. Requests are serialised:
one Ruby `app.call(env)` at a time.

Rationale:
- The VM is not `Send` (RefCell, Rc throughout). Bridging to
  a multi-threaded runtime requires either `Mutex<Vm>` (kills
  performance) or per-thread VMs (state isolation problem).
- Single-threaded still wins big against Puma for short
  requests because the per-request overhead (parse +
  serialize + socket I/O) drops from Ruby-time to Rust-time.
  The application latency is now the dominant cost — exactly
  where you want it.
- Tokio's current-thread runtime + cooperative async lets
  N concurrent requests interleave (each `await` point yields
  back to the runtime). So we can have 1000 connections open,
  even though they take turns running app code.

v2 considerations (out of scope for v1):
- **Multi-Vm pool** — N rubyrs Vms behind a tokio
  multi-threaded runtime. Shared state (sessions, caches)
  must move to a Rust-side store. Complex but the natural
  scaling path.
- **Fiber-on-Vm scheduling** — once ADR 0017's Tier 2 Fiber
  lands (issue TBD), the battery's request handler can
  yield-on-await inside Ruby code (Falcon-shape). Bigger
  payoff but bigger dependency.

### env hash construction

The Rack SPEC env hash gets built in Rust and passed to the
Ruby VM:

```rust
// Pseudocode for the adapter
fn http_request_to_rack_env(req: hyper::Request<Incoming>) -> Value /* Hash */ {
    let mut env = Hash::new();
    env.set("REQUEST_METHOD", req.method().as_str());
    env.set("PATH_INFO", req.uri().path());
    env.set("QUERY_STRING", req.uri().query().unwrap_or(""));
    env.set("SERVER_NAME", listener_host);
    env.set("SERVER_PORT", listener_port);
    env.set("SCRIPT_NAME", "");
    env.set("HTTP_VERSION", req.version_str());
    for (name, value) in req.headers() {
        env.set(format!("HTTP_{}", name.as_str().to_uppercase().replace('-', "_")), value);
    }
    env.set("rack.url_scheme", scheme);
    env.set("rack.input", LazyBodyReader::new(req.into_body()));  // ← key trick
    env.set("rack.errors", stderr_sink);
    env.set("rack.version", [1, 6]);
    env.set("rack.multithread", false);
    env.set("rack.multiprocess", false);
    env.set("rack.run_once", false);
    env
}
```

**`rack.input` is lazy.** A `LazyBodyReader` Ruby object
wraps the hyper body stream; when the Ruby app calls
`#read` / `#gets` / `#each`, the wrapper consumes the next
chunk from the underlying tokio `BodyStream`. This:
- Avoids buffering the entire request body in memory before
  app code runs (key for streaming uploads)
- Lets the app early-reject before reading the body (the
  multi-MB POST stays on the wire if `app.call(env)`
  returns 401 immediately)
- Respects `Config::max_request_body_bytes` via the wrapper's
  per-chunk accounting

### Response handling

The Ruby app returns `[status, headers, body]`:
- `status` — integer, written to `hyper::Response::status_mut`
- `headers` — Hash<String, String> (or Hash<String, Array<String>>
  for repeated headers like Set-Cookie); each goes to
  `response.headers_mut().append`
- `body` — must respond to `#each(&block)`. Each yielded
  string becomes one chunk in a `hyper::Body` stream. The
  battery iterates the body lazily; the Ruby Enumerator can
  yield arbitrary number of chunks (streaming response).

The streaming response path is critical — it's how SSE and
chunked transfer work without buffering.

### What v1 ships

- `_http_server` Cargo feature
- HTTP/1.1 support (HTTP/2 in v2 — needs tokio's TLS-ALPN
  story sorted)
- Rack SPEC env hash conformance
- Streaming request body (`rack.input` lazy reader)
- Streaming response body (Ruby Enumerator yields chunks)
- One Ruby class `Rubyrs::HttpServer` with `.bind` / `.run` /
  `.shutdown`
- Per-request deadline (via tokio `time::timeout`)
- Max body size enforcement
- Graceful shutdown on `Ctrl-C` (SIGINT handler in the
  battery)

### What v1 explicitly defers

- HTTP/2 — needs ALPN + h2 frame handling, plus TLS story
- HTTP/3 (QUIC) — needs `quinn`; separate battery
  `_http_server_h3`
- TLS — separate battery `_http_server_tls` adding `rustls`
  + `tokio-rustls`. v1 ships HTTP-only; deploy behind nginx
  / Caddy / Cloudflare for TLS in v1.
- Multi-Vm pool — v2 concern
- WebSocket upgrade — `_websocket` battery (separate per
  ADR 0019 v3 matrix)
- Server-Sent Events — works in v1 via streaming response
  body, but no `EventSource` helper class until v2
- Multipart parsing — Ruby app's responsibility in v1
  (using mini-Rack's `Rack::Multipart` if implemented, or
  a separate `_multipart` battery)
- Per-IP rate limiting — out of scope; deploy a real proxy
- Access logs — Ruby app's responsibility; v2 may add a
  `tracing`-integrated default

## Consequences

### What gets easier

- **The Bun-class story has a concrete demo target.**
  `rubyrs --features _http_server` + a 20-line Sinatra-like
  Ruby script + `wrk -c 100 -d 30s http://localhost:3000/` →
  a real number we can put in a benchmark blog post.
- **Rack ecosystem becomes a credible roadmap.** Once
  autoload + 7 stdlib batteries land, real Rack apps run on
  this battery. The HTTP-server piece doesn't have to
  wait — the mini-Rack stub (Tier 3 pure-Ruby per the
  earlier discussion) is enough to demo.
- **Per-request VM isolation isn't required for v1.** The
  serialised single-thread model is the right starting
  point; complexity stays in v2.
- **Streaming body in / out works correctly.** The lazy
  `rack.input` and chunked response body are core SSE /
  long-poll / large-upload requirements — getting them
  right in v1 saves an architecture revisit later.

### What gets harder

- **Tokio-VM interop.** The VM isn't `Send`; the battery
  has to carefully scope where Rust async code crosses into
  the Ruby call site. Current design keeps the Vm
  ownership on the main thread; tokio runs on the same
  thread. Diverging from this (e.g. adding `spawn_blocking`
  for filesystem ops) requires re-design.
- **Two error paths.** Rust-level errors (parse failure,
  socket bind) and Ruby-level errors (app raises) both
  surface as Rack-shaped responses. The mapping table
  above is the v1 contract; expanding it (especially for
  the multi-Vm v2) is more invasive than it sounds.
- **Binary size impact.** hyper + tokio + dependencies are
  ~15-25 MB stripped. Per ADR 0019 v3 Part D, this fits in
  the `everything` shape's 150 MB ceiling but pushes
  `cli-defaults` (40 MB) close to its limit if we ever
  consider promoting `_http_server` there. Recommend
  leaving `_http_server` out of `cli-defaults` for now —
  in `everything` only.
- **Testing requires running a real HTTP server in tests.**
  Spawn server on `127.0.0.1:0` (kernel-assigned port),
  hit it with a tokio client, assert response shape. The
  pattern is established (axum / hyper have docs); the
  cost is realistic CI test time.

### What we explicitly accept trading away

- **HTTP/2 + TLS at v1.** Production deploys go behind nginx
  / Caddy / Cloudflare for v1. This matches what most Puma
  + Rails deploys actually do; cleartext HTTP/1.1 from
  Sinatra → reverse-proxy → user is the standard shape.
- **Multi-core scaling at v1.** Single-thread runtime caps
  throughput at one core. Acceptable for the demo + early
  embedder use cases; multi-Vm pool is the v2 lift.
- **`tokio` dependency in `everything` builds.** Brings
  in the full tokio runtime + ~50 transitive crates. We're
  already in `dep:tokio` territory via cext (and possibly
  `_http`, `_s3`); adding here doesn't fundamentally
  change the dependency surface for `everything`-shape
  builds.

## Alternatives considered

1. **`axum` instead of `hyper`.** Axum is hyper + routing +
   middleware. The Ruby app provides routing (via Rack),
   so axum's routing is dead weight. Middleware is Rack's
   responsibility. Rejected — axum's value is on the wrong
   side of the boundary for us.

2. **`actix-web`.** Excellent perf, mature. Comes with its
   own runtime instead of tokio; ecosystem fragmentation
   (`actix::net` vs `tokio::net`). Rejected for ecosystem
   alignment — `_http` outbound battery wants tokio + reqwest,
   so picking tokio + hyper here keeps one async runtime
   in `everything`.

3. **`warp`.** Filter-combinator design; trickier to
   integrate with "give me an HTTP request, I'll call the
   Ruby app". Rejected for fit, not capability.

4. **Build it as an embedder concern (don't ship a battery).**
   Each embedder spawns their own hyper / axum server and
   manually marshals to env hash via `register_fn`.
   Rejected — duplicates work, kills the "ships with
   batteries" demo, fails the Bun-class story.

5. **Ship as part of `_http` (combine inbound + outbound).**
   Reasonable on paper (HTTP is HTTP) but the surfaces are
   wildly different (client vs server), the deviation
   classes differ (outbound is class a + c; inbound is
   class a + g), and the deps don't fully overlap
   (`reqwest` for outbound is hyper + cookies + json
   helpers — overkill for inbound). Two batteries is
   cleaner.

6. **Multi-threaded tokio runtime from v1.** Per the VM
   concurrency model section, the cost is `Mutex<Vm>`
   contention or per-thread Vm state isolation —
   non-trivial. v1 single-threaded validates the
   architecture; v2 earns the right to multi-thread.

## Migration plan

### Phase H1 — minimal viable battery (v0.2.0)

- Implement `Rubyrs::HttpServer` Ruby class
- HTTP/1.1 only
- env hash construction (Rack SPEC v1.6)
- Lazy `rack.input`
- Streaming response body
- Per-request deadline + max body size enforcement
- Graceful shutdown on SIGINT
- Unit tests: hyper server stub, hit with hyper client
- Integration test: 100-line Sinatra-like Ruby app +
  `wrk -c 10 -d 5s` smoke test

### Phase H2 — mini-Rack integration (v0.2.0 or v0.2.1)

- Tier 3 pure-Ruby `Rack::Request` / `Rack::Response`
  (per the earlier discussion — separate ADR if it grows)
- Demo: a Sinatra-shape micro-framework in 200 LOC
  pure-Ruby on top of `_http_server` + mini-Rack
- Benchmark vs Puma + Sinatra: target 4-10× RPS, 1/10
  RSS

### Phase H3 — real Rack gem (v0.3.0+)

- Depends on issues #224 (autoload) + #225
  (`Config::load_paths`) + #227's stdlib batteries (uri,
  time, cgi, forwardable, singleton) landing first
- Smoke test: load unmodified rack from
  `vendor/bundle/.../rack-3.x.x/lib/`
- Smoke test: load unmodified sinatra similarly

### Phase H4 — TLS + HTTP/2 (v0.4.0+)

- `_http_server_tls` battery (rustls + tokio-rustls)
- ALPN negotiation → HTTP/2 upgrade
- HTTP/3 (`_http_server_h3`) as a v0.5.0+ battery

## Related

- [ADR 0019 v3 — Tier 2 / Tier 3 boundary](0019-tier2-tier3-boundary.md)
  — Rule 7 (ADR-per-battery), Rule 4 (deviation taxonomy),
  Rule 8 (`require "rubyrs/http_server"` namespace).
  **This ADR is the first concrete instance of Rule 7's
  per-battery ADR pattern.**
- [ADR 0008 — Resource caps for untrusted scripts](0008-resource-caps-for-untrusted-scripts.md)
  — `Config::deadline` is per-eval; per-request deadline
  is a new orthogonal cap defined here.
- [ADR 0017 — Tier 1 boundary](0017-tier1-boundary.md) —
  this battery is firmly outside Tier 1; the host capability
  injection rules at line 47 are honored via the
  embedder-supplied `HttpServerConfig`.
- [Rack SPEC v1.6](https://github.com/rack/rack/blob/main/SPEC.rdoc)
  — env hash conventions the battery implements.
- Issues #224 (autoload), #225 (Config::load_paths), #226
  (Kernel#load), #227 (stdlib batteries) — H3 depends on
  all four
- Bun's `Bun.serve` ([docs](https://bun.com/docs/api/http))
  — strategic precedent: native HTTP server + scripting VM,
  competes successfully against Node + Express
- Falcon ([github.com/socketry/falcon](https://github.com/socketry/falcon))
  — Fiber-based Rack server in pure Ruby; reference for
  Phase H4's Fiber-aware scheduling
