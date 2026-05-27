# 0022: `_http_server` battery — Rust HTTP front, Ruby app handler

## Status

Proposed (2026-05-27). **v2 revised after parallel agent review
caught 3 blockers + 7 majors in v1 (commit `88564485`, kept in
git history).** First Tier 3 native battery ADR per
[ADR 0019 v3](0019-tier2-tier3-boundary.md) Rule 7; establishes
the template for subsequent battery ADRs.

## Context

ADR 0019 v3's matrix names `_http` (outbound HTTP client) as
a candidate Tier 3 battery. Inbound HTTP — the server side —
emerged as the **load-bearing differentiator** for the
project's Bun-class positioning:

- CRuby + Puma: ~5k RPS Sinatra hello-world (C-ext HTTP parser,
  Ruby socket handling)
- CRuby + Falcon: ~10k RPS (Fiber + Ruby async/await)
- **mruby + H2O (2015 reference)**: 25k RPS for JSON API —
  the strongest historical anchor for "small VM + Rust/C HTTP
  front" approach
- **rubyrs + hyper front (v1 estimate)**: 2-8k RPS realistic
  for hello-world; 25k RPS is the long-term ceiling assuming
  interpreter optimisations + multi-process scaling

The win is **moving wire-protocol work out of the Ruby VM**,
not making the VM itself faster. Same play deno_core makes
with V8 (`!Send` engine + tokio current-thread). The
[wasmtime-wasi-http](https://docs.wasmtime.dev/api/wasmtime_wasi_http/)
crate is the closest Rust precedent (hyper + per-connection
`Store`, single-threaded engine).

### v1 → v2 review-driven scope correction

The v1 draft of this ADR proposed lazy `rack.input` streaming
and streaming response body via Ruby Enumerators. **Three
parallel reviews independently identified both as
unimplementable on the current sync VM**:

- Lazy `rack.input.read(n)` calls a synchronous Ruby method
  that must drive an async tokio body stream. The three
  candidate implementations (block_on, pre-buffer, Fiber
  yield) all fail: block_on deadlocks the current-thread
  runtime; pre-buffer contradicts "no buffering"; Fiber is
  Tier 2 work not yet shipped.
- Streaming response body has the symmetric problem: Ruby
  Enumerator's synchronous `#each` cannot drive async
  `mpsc::Sender::send` without block_on (which panics from
  within a runtime: "Cannot start a runtime from within a
  runtime").

v2 explicitly defers both to Phase H3 (depends on Fiber
landing per ADR 0017 Tier 2). v1 ships with **buffered
request body + buffered response body** — matching Puma's
default behaviour. SSE / chunked transfer / long-poll all
defer to H3.

## Decision

### Vendor crate

**`hyper` 1.x + `hyper-util` + `tokio` (current-thread
runtime) + `tokio::task::LocalSet`.** No `axum` — its routing
layer duplicates Rack's job; its middleware tower duplicates
Rack middleware. We need accept + parse + serialize, which
is the `hyper` surface exactly.

Cargo deps when feature enabled:

```toml
[dependencies]
hyper = { version = "1", features = ["server", "http1"], optional = true }
hyper-util = { version = "0.1", features = ["tokio"], optional = true }
tokio = { version = "1", features = ["rt", "net", "io-util", "sync", "signal"], optional = true }
http-body-util = { version = "0.1", optional = true }
bytes = { version = "1", optional = true }

[features]
_http_server = [
    "dep:hyper", "dep:hyper-util", "dep:tokio",
    "dep:http-body-util", "dep:bytes",
]
```

Note: `hyper` feature `http2` is **NOT** in v1 (deferred — needs
ALPN + TLS story sorted). v1 ships HTTP/1.1 only.

### `LocalSet` is mandatory — explicit Vm ownership contract

The Vm is `!Send + !Sync` (uses `Rc<RefCell<…>>` throughout).
**`tokio::spawn` requires `Send + 'static` even on
`current_thread` runtime** (common misconception that
current_thread relaxes `Send` — it does not). The mandatory
construct is `tokio::task::LocalSet::spawn_local`, whose bound
is only `Future + 'static`.

Implementation contract (enforced by the type system):

```rust
// Pseudocode — the actual server entry point
fn run_server(rt: &mut Runtime, bind: SocketAddr) -> Result<(), Trap> {
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    tokio_rt.block_on(local.run_until(async {
        let listener = TcpListener::bind(bind).await?;
        loop {
            let (stream, _) = listener.accept().await?;
            let vm = rt.vm_handle();  // see "Vm ownership" below
            tokio::task::spawn_local(async move {
                serve_connection(stream, vm).await
            });
        }
    }))
}
```

### Vm ownership across futures — explicit ADR 0013 alignment

[ADR 0013](0013-current-vm-ptr-aliasing.md) defines strict
LIFO + time-disjoint `&mut Vm` access via the `CURRENT_VM_PTR`
re-entrance machinery. **v2 inherits the same discipline by
construction**:

1. **One canonical `&mut Vm` exists at runtime entry.** The
   embedder's main thread owns it.
2. **`LocalSet` runs on the same thread.** No `Send` boundary
   to cross.
3. **Each request handler future borrows the Vm via a
   `VmHandle` reference type** that internally calls into
   `with_vm_ptr_set` for the duration of `app.call(env)`. The
   borrow is **scope-bounded by the synchronous call**; tokio
   await points happen *outside* the borrow (between requests,
   not inside `app.call`).
4. **No future polls the Vm while another holds it.** The
   request handler's structure is:
   ```
   await body_buffered (no Vm access)
   borrow Vm → build env Hash → call app → take response
                              (synchronous block, no await)
   release Vm
   await response_write (no Vm access)
   ```
   The Vm is only borrowed inside the synchronous block. Across
   await points, no Vm access happens.

This is the **deno_core `JsRuntime` pattern** — `JsRuntime`
itself is a `Future` that the runtime polls, and inside its
`poll` it owns `&mut self` exclusively for the duration.

### Deviation classes claimed (per ADR 0019 v3 Rule 4)

- **Class a (owned-resource I/O)** — server binds to a
  caller-supplied `(host, port)`. The address is part of
  the Ruby app's explicit config; the battery doesn't
  inspect arbitrary network state.
- **Class g (native-thread spawn)** — tokio uses an internal
  I/O reactor thread even when the runtime is configured
  `current_thread`. The blocking thread pool is NOT
  initialised (we never call `spawn_blocking`).

Classes **NOT** claimed:
- ❌ Class c (multi-host network reach) — server is
  **inbound only**. It does not initiate outbound
  connections.
- ❌ Class f (mmap / heap-cap bypass) — no.

### Runtime allowlist (per ADR 0019 v3 Rule 4 sub-rule)

```rust
pub struct HttpServerConfig {
    /// Bind address. None = battery loaded but server not
    /// started until script-side `Rubyrs::HttpServer.run`.
    pub bind: Option<std::net::SocketAddr>,

    /// Max concurrent in-flight requests via tokio semaphore.
    /// None = unbounded (NOT recommended — denial-of-service
    /// surface).
    pub max_concurrent_requests: Option<usize>,

    /// Max request body size in bytes. None = 16 MB default
    /// (NOT unlimited — v1 buffers body before app.call, so
    /// this is a hard memory cap).
    pub max_request_body_bytes: Option<usize>,

    /// Whether the battery installs a SIGINT handler for
    /// graceful shutdown. Default `false` — embedders
    /// often own signal handling themselves (e.g. CLI
    /// tools coordinating multiple sub-systems). The CLI
    /// binary `rubyrs` sets this `true`.
    pub install_signal_handler: bool,
}
```

Exposed via `Config::http_server: Option<HttpServerConfig>`.

### V1 body handling — buffered

**Request body**: hyper accumulates the full body into
`Bytes` (subject to `max_request_body_bytes`) BEFORE the
Vm is borrowed. `env["rack.input"]` is a Ruby `StringIO`
constructed from the buffered bytes. The synchronous Ruby
`rack.input.read(n)` reads from the StringIO — no async,
no Fiber, no deadlock surface.

**Response body**: the Ruby app returns `[status, headers,
body]` where `body.each` is called *synchronously* by the
battery while the Vm is still borrowed. Each yielded chunk
appends to an in-memory `Vec<u8>`. After `body.each`
completes, the Vm is released, and the buffered response
bytes are written to the socket via hyper. **The response
is fully materialised before any socket write.**

This is **Puma's default behaviour** — Puma 5+ defaults to
`queue_requests: false` meaning the entire request body is
read before app.call, and the response body is consumed
before any write. Real-world Rack apps almost never depend
on byte-streaming semantics; the cost of buffering is
bounded by `max_request_body_bytes`.

**What v1 explicitly cannot do** (need Phase H3 + Fiber):
- Server-Sent Events (SSE)
- HTTP chunked transfer encoding for streaming responses
- Long-poll / WebSocket upgrade
- Multi-MB upload streaming without buffering

These are real limitations. The "Bun-class story" for v1 is
**"throughput on short requests"**, not "every Rack feature
works."

### Per-request resource enforcement — uncatchable, unified with ADR 0008

Per-request limits (deadline, body size) fire as
**`ResourceExhausted` traps** — the same ADR 0008
uncatchable variant as the existing fuel / max-frames /
max-heap caps. Bare `rescue` in app code cannot swallow
them. This corrects v1's "Timeout::Error" mapping
(catchable) which would have let an app silently absorb
its own per-request cap.

When a per-request deadline fires:
1. Tokio's `time::timeout` future races against the request
   handler future. On timeout, the request handler future
   is dropped.
2. **If the drop happens between requests (no Vm borrowed)**:
   clean shutdown, log the timeout, send 503 to client.
3. **If the drop happens while Vm is borrowed**:
   - The Vm borrow itself doesn't span tokio await points
     (per "Vm ownership" above), so this case only arises
     if the Ruby app code itself is taking too long
   - In the synchronous Ruby block, **tokio cannot drop
     the handler future** — the await isn't reached. The
     timeout is effectively dormant during CPU-bound app
     work.
   - **This is a real limitation**: per-request deadline
     cannot preempt a CPU-bound Ruby loop. **The defence
     against runaway Ruby code is `Config::fuel`**, not
     `per_request_deadline`. The ADR text now reflects this
     — `per_request_deadline` is for I/O-side stalls (slow
     body read, slow socket write) where the await points
     exist.

This is the standard deno_core / wasmtime-wasi-http
limitation; even the V8 isolate model needs an out-of-band
interrupt mechanism (V8's `IsolateInterruptCallback`) to
preempt user code. Fuel-style accounting is rubyrs's
equivalent and the v1 answer.

### Per-request Vm reset hook (NEW)

After each request, the Vm needs a light-weight cleanup
between handler invocations. Today's `Runtime::reset()` is
heavy — it re-loads the entire preamble (~10ms). A new
**`Runtime::reset_between_requests()`** API:

- Clear operand stack
- Clear frame stack down to base
- Clear pending loop-transfer / break / return signals
- Clear `pending_method_return`
- **DO NOT** clear: class/method definitions (those
  persist across requests, intentional — that's how
  Rack/Sinatra app state stays alive between requests)
- **DO NOT** clear: heap (GC handles unused objects)

This is essentially what `Runtime::reset()` does EXCEPT the
preamble re-load. Implementation: factor out the
control-state-clear portion of `reset()` into a separate
function called by both `reset()` and the new method.

API surface added in v1:

```rust
impl Runtime {
    /// Lightweight cleanup between HTTP requests (or other
    /// recurring eval contexts). Clears VM control-flow
    /// state without touching class / method / heap state.
    /// Called automatically by `_http_server` between
    /// requests; exposed publicly for embedders running
    /// other request-shaped loops.
    pub fn reset_between_requests(&mut self);
}
```

### Multi-core scaling story — pre-fork

**v1 single-threaded tokio caps throughput at one CPU
core.** The official scaling story matches Puma + Falcon +
H2O: **pre-fork N processes, each binding to the same port
via `SO_REUSEPORT`**.

- Linux: kernel-load-balances incoming connections across
  N processes via `SO_REUSEPORT`
- macOS: same syscall name, slightly different semantics
  (round-robin instead of hash-based; acceptable)
- Windows: deferred (no good `SO_REUSEPORT` equivalent)

The battery exposes a `Rubyrs::HttpServer.fork_workers(n)`
helper that:
1. `fork()`s N times
2. Each child enables `SO_REUSEPORT` on the listener
3. Parent supervises children (restart on crash)

This is **explicitly the v1 multi-core path**. Multi-Vm pool
in one process is v2/v3 work (and per the deno_core /
wasmtime-wasi-http review feedback, the natural shape is
per-connection `Vm` clones — not `Mutex<Vm>`).

### env hash construction

The Rack SPEC env hash is built in Rust and passed to the
Vm. **All keys/values are concrete `Value`s — no lazy
wrappers in v1**:

```rust
// Pseudocode for the adapter — v1 buffered shape
fn build_rack_env(req: hyper::Request<Bytes>) -> Value /* Hash */ {
    let mut env = Hash::new();
    env.set("REQUEST_METHOD", req.method().as_str());
    env.set("PATH_INFO", req.uri().path());
    env.set("QUERY_STRING", req.uri().query().unwrap_or(""));
    env.set("SERVER_NAME", listener_host);
    env.set("SERVER_PORT", listener_port);
    env.set("SCRIPT_NAME", "");
    env.set("HTTP_VERSION", "HTTP/1.1");
    for (name, value) in req.headers() {
        env.set(format!("HTTP_{}", name.as_str().to_uppercase().replace('-', "_")), value);
    }
    env.set("rack.url_scheme", scheme);
    env.set("rack.input", StringIO::new(req.into_body().to_bytes()));  // buffered, sync
    env.set("rack.errors", stderr_sink);
    env.set("rack.version", [1, 6]);
    env.set("rack.multithread", false);
    env.set("rack.multiprocess", true);  // pre-fork makes this true
    env.set("rack.run_once", false);
    env
}
```

The body is read fully (subject to `max_request_body_bytes`)
before the env is built. The Ruby `StringIO` is real
`StringIO` from the existing `stdlib_vendor/stringio.rb`
(184 LOC, already shipped) — no new wrapper type.

### Response handling

The Ruby app returns `[status, headers, body]`:

- `status` — integer, written to hyper response
- `headers` — Hash<String, String>; each header set via
  `response.headers_mut().append`
- `body` — must respond to `#each(&block)`. The battery
  iterates synchronously, appending each yielded String to
  a `Vec<u8>` buffer. **All-at-once write**: after `#each`
  completes, the full Vec is wrapped in `Full<Bytes>` and
  sent to hyper as the response body. No chunked transfer
  in v1.

### What v1 ships

- `_http_server` Cargo feature
- HTTP/1.1 (no HTTP/2 in v1)
- Rack SPEC env hash conformance (v1.6)
- **Buffered** request body (StringIO-backed, sync reads)
- **Buffered** response body (all-at-once write)
- One Ruby class: `Rubyrs::HttpServer`
  - `.bind(addr)` — creates handle
  - `#run(rack_app)` — starts loop, blocks until shutdown
  - `#shutdown` — graceful stop
  - `.fork_workers(n)` — multi-process pre-fork
- Per-request body size enforcement (`max_request_body_bytes`)
- Per-request deadline for I/O-side stalls (NOT CPU-bound
  preemption — use `Config::fuel` for that)
- Optional SIGINT graceful shutdown
  (`install_signal_handler: true`)
- Per-request light-weight Vm reset
  (`reset_between_requests`)
- Cleartext HTTP only (TLS deferred)

### What v1 explicitly defers

- **Streaming request body** (lazy `rack.input`) → Phase H3,
  depends on Fiber (Tier 2)
- **Streaming response body** (chunked transfer) → H3, Fiber
- **Server-Sent Events** → H3
- **WebSocket** → separate `_websocket` battery
- **HTTP/2** → `_http_server_h2` battery (needs ALPN + TLS)
- **HTTP/3 / QUIC** → `_http_server_h3` battery
- **TLS** → `_http_server_tls` battery (rustls)
- **Multi-Vm in one process** (per-connection Vm cloning,
  wasmtime-wasi-http style) → v2 work; v1 multi-core is
  pre-fork-only
- **Per-request CPU preemption** → use `Config::fuel`;
  preemption of CPU-bound Ruby needs a V8-IsolateInterrupt
  shape that's significant Tier 1 work
- **Multipart parsing** → Ruby app's responsibility (or
  separate `_multipart` battery)
- **Access logs** → embedder wires via `tracing` if needed
- **Per-IP rate limiting** → deploy a real proxy
- **Windows multi-core (SO_REUSEPORT)** → v2

## Honest performance estimates

V1 single-thread + buffered + interpreted Ruby:

| Workload | Estimate | Confidence |
|---|---|---|
| Empty 200 (no Ruby code beyond return) | 30-50k RPS | Medium — hyper alone does 150k+ |
| Sinatra-style hello-world | 2-5k RPS | Medium — VM dispatch dominates |
| Rack JSON API (5 KB response) | 1-3k RPS | Low — depends on JSON serialisation |
| Pre-fork × N cores (4-core machine) | 4×above | High — kernel SO_REUSEPORT works |

**Comparison anchors**:
- Puma + CRuby Sinatra: ~5k RPS (this is the floor we
  need to beat to claim a win)
- Falcon + CRuby Sinatra: ~10k RPS (this is what we
  match at pre-fork N=4-8)
- mruby + H2O JSON API (2015): 25k RPS (ceiling for
  "small VM + Rust HTTP" approach — we target this for
  v2 with multi-Vm pool)
- Bun.serve + Express: 52k RPS (different ecosystem; not
  directly comparable)

**v1 marketing target**: "Comparable to Falcon at the same
core count; faster per-MB of memory; 1/10 cold start. The
real perf story unlocks with Phase H3 (Fiber) and v2
(multi-Vm pool)."

This is honest framing — not the 20-40k RPS v1 claimed.

## Consequences

### What gets easier

- **The Bun-class story has an honest concrete demo
  target**: ~5k RPS hello-world matches Puma; pre-fork × 4
  matches Falcon. Add 1/10 cold start and 1/10 RSS for the
  real differentiation.
- **Rack ecosystem becomes a credible roadmap.** Once
  autoload + 7 stdlib batteries land, real Rack apps run.
  Buffered-body Rack apps are >90% of real apps.
- **No new VM design work for v1.** The buffered body
  approach uses existing primitives only — StringIO is
  shipped (`stdlib_vendor/stringio.rb`), tokio is well-
  understood, hyper is mature.
- **Streaming + Fiber dependency is now explicit**, not
  assumed. Phase H3 has a clean "depends on Fiber" gate.

### What gets harder

- **Tokio-VM interop discipline.** The "Vm ownership"
  section's discipline (LocalSet + scope-bounded borrows +
  no Vm access across await) is the load-bearing
  invariant. Easy to break; needs care in code review.
  Recommend a `VmBorrow<'_>` RAII type to enforce at the
  type level.
- **`tokio::task::LocalSet` is non-obvious.** The
  `tokio::spawn`-vs-`LocalSet::spawn_local` distinction
  surprises Rust developers familiar with multi-threaded
  tokio. ADR + code comments must call it out.
- **Pre-fork is the multi-core story.** Embedders who want
  one-process multi-core scaling have to wait for v2's
  multi-Vm work. For most embed scenarios (CLI tools,
  edge functions, single-tenant) one process is fine; for
  multi-tenant web servers, pre-fork matches Puma/Falcon's
  shape anyway.
- **Per-request CPU preemption is `Config::fuel` only.**
  `per_request_deadline` only kills I/O-stalled requests,
  not CPU-loops. Embedders running untrusted Ruby code
  MUST set `Config::fuel` to bound CPU usage.

### What we explicitly accept trading away

- **No SSE, chunked transfer, or streaming bodies in v1.**
  Real cost for use cases that need them (LLM streaming,
  large file downloads); these are common enough that v1
  embedders must deploy alongside another solution or
  wait for H3.
- **HTTP/1.1 only in v1.** Production deploys behind a
  reverse proxy (nginx / Caddy / Cloudflare) for HTTP/2 /
  HTTP/3 upgrade.
- **Pre-fork only for multi-core in v1.** Loses some
  request-routing flexibility that single-process multi-
  worker setups have (e.g. work-stealing). Acceptable
  matching of the Puma + Rails production shape.

## Alternatives considered

1. **`axum` instead of `hyper`.** Axum's routing layer
   duplicates Rack's job. Rejected.

2. **`actix-web`.** Own runtime fragmenting ecosystem.
   Rejected for tokio alignment.

3. **`warp`.** Filter-combinator design awkward for
   "give me an HTTP request, I'll call the Ruby app."
   Rejected for fit.

4. **Build it as an embedder concern (no battery).**
   Duplicates work, kills the demo, fails the Bun-class
   story. Rejected.

5. **Ship as part of `_http` (combine inbound + outbound).**
   Different surfaces, different deviation classes,
   different deps. Two batteries cleaner.

6. **Multi-threaded tokio + Mutex<Vm>**. Mutex contention
   kills the perf story. Per-connection Vm clones (the
   wasmtime-wasi-http pattern) is the right shape but
   needs measuring `Vm::new()` cost first (currently
   ~10ms with preamble). Deferred to v2.

7. **Per-connection Vm spawn** (wasmtime-wasi-http
   pattern). Right architectural shape for v2 but
   requires:
   - Vm cold-start ≪ 1ms (today ~10ms with preamble)
   - Wizer-style pre-init to amortise preamble cost
   - Shared state between Vms moved to Rust side
   This is real work; v1's "single Vm, serialised
   requests" is the conservative starting point.

8. **Fiber-aware request handler (Falcon-shape).** Async
   Ruby code yielding to tokio. Requires Tier 2 Fiber
   landing. The right v3+ shape; v1 sticks with sync
   buffered requests.

9. **Streaming via blocking-thread-pool**
   (`tokio::task::spawn_blocking`). Moves the Vm call
   onto a different thread; breaks the `!Send` invariant.
   Would require `Vm::clone()` to be cheap (it isn't).
   Rejected.

## Migration plan

### Phase H1 — minimal viable battery (v0.2.0)

- Implement `Rubyrs::HttpServer` Ruby class
- HTTP/1.1 only
- Buffered request body via `StringIO`
- Buffered response body
- Rack SPEC env hash (v1.6)
- `LocalSet`-based runtime entry
- `Runtime::reset_between_requests()` API
- `max_concurrent_requests` semaphore
- `max_request_body_bytes` enforcement (default 16 MB)
- `per_request_deadline` for I/O stalls only
- Optional SIGINT handler (`install_signal_handler`)
- Unit tests: hyper server stub, hyper client smoke
- Integration test: 50-line Sinatra-shape Ruby app + wrk

### Phase H2 — mini-Rack integration (v0.2.x)

- Tier 3 pure-Ruby `Rack::Request` / `Rack::Response`
  per the earlier discussion (separate Tier 3 canon
  ADR if it grows; today: `stdlib_vendor/rack.rb`)
- Sinatra-shape micro-framework demo
- Pre-fork worker support (`fork_workers(n)`)
- Benchmark vs Puma + Sinatra: target Falcon-comparable
  RPS at pre-fork × cores; 1/10 RSS; 1/10 cold start

### Phase H3 — streaming via Fiber (v0.3.0+)

**Depends on Tier 2 Fiber landing** (per ADR 0017 — issue
TBD). Once Fibers exist, the request handler can
cooperatively yield at body read / write points:

- Lazy `rack.input` (real streaming)
- Streaming response body (Ruby Enumerator yielding to
  tokio mpsc via Fiber)
- Server-Sent Events
- HTTP chunked transfer encoding
- Long-poll patterns

### Phase H4 — real Rack gem (v0.3.0+)

**Depends on**: issues #224 (autoload) + #225
(`Config::load_paths`) + #227's stdlib batteries (uri,
time, cgi/util, forwardable, singleton). Independent
work track from H3; either can land first.

- Smoke test: load unmodified rack from `vendor/bundle/...`
- Smoke test: load unmodified sinatra

### Phase H5 — TLS + HTTP/2 (v0.4.0+)

- `_http_server_tls` battery (rustls + tokio-rustls)
- ALPN negotiation → HTTP/2 upgrade
- `_http_server_h2` feature
- HTTP/3 via `_http_server_h3` later

### Phase H6 — multi-Vm in one process (v0.5.0+ or v1.0)

- Per-connection Vm clone (wasmtime-wasi-http pattern)
- Requires Vm cold-start cost reduction
- Or: Vm pool with Vm checkout/checkin per request
- Multi-threaded tokio runtime; Vms still single-threaded
  individually

## What changes vs v1 (the original draft of this ADR)

| v1 said | v2 says | Reason |
|---|---|---|
| Lazy `rack.input` for streaming upload | **Buffered body via StringIO** (16 MB default cap) | v1 was unimplementable on sync VM; needs Fiber → Phase H3 |
| Streaming response via Ruby Enumerator | **Buffered response, all-at-once write** | Same reason — needs Fiber → H3 |
| 20-40k RPS estimate | **2-8k RPS hello-world; ceiling 25k via H6 multi-Vm** | v1 was multi-Vm pool number cited as if v1; corrected with mruby+H2O 2015 anchor |
| `tokio::spawn` for request handlers | **`tokio::task::LocalSet::spawn_local` mandatory** | `spawn` requires `Send`, even on `current_thread` runtime. v1 would fail to compile |
| `Timeout::Error` for per-request deadline | **`ResourceExhausted` (uncatchable)** | `Timeout::Error` is catchable, defeats the cap. Aligns with ADR 0008's existing pattern |
| `per_request_deadline` preempts CPU-bound Ruby | **`per_request_deadline` only catches I/O stalls; `Config::fuel` handles CPU** | tokio cannot preempt a synchronous Ruby block; future never gets polled. Honest framing. |
| No reset between requests | **`Runtime::reset_between_requests()` API** | Vm state needs cleanup between handler invocations; full `reset()` is too heavy (10ms preamble re-load) |
| SIGINT handler always-on | **`install_signal_handler: bool` (default false)** | Embedders often own signal handling; default true would silently break their setup |
| ADR 0013 not mentioned | **Explicit `Vm` ownership section + ADR 0013 cross-ref** | Tokio future state machines could violate ADR 0013's LIFO invariant; v2 explicit "no Vm access across await" rule prevents this |
| Bun cited as primary precedent | **deno_core `JsRuntime` cited as primary; wasmtime-wasi-http for v6 pattern** | More accurate technical fit; Bun uses thread pool internally (different shape) |
| No multi-core scaling story | **Pre-fork via SO_REUSEPORT documented** | Matches Puma/Falcon/H2O production shape; multi-Vm one-process is v6 |
| 4 phases (H1-H4) | **6 phases (H1-H6)** | Splits streaming/Fiber (H3) from real-rack-gem (H4); adds H6 for multi-Vm |

## Revision log

- **2026-05-27 — v2 (this revision).** Major rewrite after
  three parallel agent reviews flagged 3 blockers + 7
  majors in v1. Resolutions table immediately above. v1
  committed at `88564485`, kept in git history.
- **2026-05-27 — v1 (commit `88564485`).** First draft;
  proposed lazy `rack.input` and streaming response,
  both unimplementable without Fiber. 20-40k RPS estimate
  was multi-Vm pool's number cited as v1.

## Related

- [ADR 0019 v3 — Tier 2 / Tier 3 boundary](0019-tier2-tier3-boundary.md)
  — Rule 7 (ADR-per-battery), Rule 4 (deviation taxonomy),
  Rule 8 (`require "rubyrs/http_server"` namespace).
  **This ADR is the first concrete instance of Rule 7.**
- [ADR 0008 — Resource caps for untrusted scripts](0008-resource-caps-for-untrusted-scripts.md)
  — `per_request_deadline` extends the existing cap model;
  `ResourceExhausted` uncatchable variant reused.
- [ADR 0013 — CURRENT_VM_PTR borrow aliasing](0013-current-vm-ptr-aliasing.md)
  — load-bearing constraint on Vm ownership across tokio
  await points; "Vm ownership" section is the alignment.
- [ADR 0017 — Tier 1 boundary](0017-tier1-boundary.md) —
  this battery is firmly Tier 3; capability injection rules
  honored via `HttpServerConfig`.
- Issues #224 (autoload), #225 (Config::load_paths), #226
  (Kernel#load), #227 (stdlib batteries) — H4 depends on
  all four
- [Rack SPEC v1.6](https://github.com/rack/rack/blob/main/SPEC.rdoc)
  — env hash conventions
- [`deno_core::JsRuntime`](https://docs.rs/deno_core/latest/deno_core/struct.JsRuntime.html)
  — closest Rust precedent; `!Send` engine + current-thread
  tokio is exactly the v2 architecture
- [`wasmtime-wasi-http`](https://docs.wasmtime.dev/api/wasmtime_wasi_http/)
  — per-connection Store pattern; H6's target architecture
- [`tokio::task::LocalSet`](https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html)
  — the API that makes `!Send` futures schedulable
- Bun's `Bun.serve` ([docs](https://bun.com/docs/api/http))
  — strategic precedent for marketing positioning;
  technically different (uses thread pool internally)
- Falcon ([github.com/socketry/falcon](https://github.com/socketry/falcon))
  — Fiber-based Rack server reference for H3
- mruby + H2O ([Luca Guidi 2015](https://lucaguidi.com/2015/12/09/25000-requests-per-second-for-rack-json-api-with-mruby/))
  — historical anchor for the 25k RPS ceiling claim
