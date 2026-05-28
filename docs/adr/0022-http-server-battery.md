# 0022: `_http_server` battery — Rust HTTP front, Ruby app handler

## Status

Proposed (2026-05-27). **v3** — third revision, addressing 3
blockers + 7 majors + 4 prior-art accuracy issues flagged by
parallel review of v2. v1 (commit `88564485`) and v2 (commit
`ea92dec1`) kept in git history. First Tier 3 native battery
ADR per [ADR 0019 v3](0019-tier2-tier3-boundary.md) Rule 7;
establishes the template for subsequent battery ADRs.

## Context

ADR 0019 v3's matrix names `_http` (outbound HTTP client) as
a candidate Tier 3 battery. Inbound HTTP — the server side —
emerged as the **load-bearing differentiator** for the
project's Bun-class positioning:

- CRuby + Puma: ~5k RPS Sinatra hello-world (C-ext HTTP parser,
  Ruby socket handling)
- CRuby + Falcon: ~10k RPS (Fiber + Ruby async/await)
- **mruby + H2O (2015 anchor, Luca Guidi)**: 120k RPS plain
  hello-world; 28k RPS JSON API including a Redis round-trip.
  The 120k is the upper bound for "small VM + Rust/C HTTP
  front" approach; the 28k is what real-world I/O-touching
  apps look like.
- **rubyrs + hyper front (v1 estimate)**: 2-8k RPS realistic
  for hello-world; ceiling ~25k JSON-API-shape after Phase
  H6 (multi-Vm). Closer to mruby's number than to Bun's
  because both VMs are bytecode interpreters.

The win is **moving wire-protocol work out of the Ruby VM**,
not making the VM itself faster. The closest Rust precedent
is [`wasmtime-wasi-http`](https://docs.wasmtime.dev/api/wasmtime_wasi_http/),
which builds a fresh `Store` per request via
[`ProxyPre::instantiate_async`](https://docs.wasmtime.dev/api/wasmtime_wasi_http/bindings/struct.ProxyPre.html) —
front-loads expensive setup, cheap per-invocation reset.
That's the architectural shape we converge to in Phase H6.

### v2 → v3 review-driven corrections

The v2 draft fixed v1's three blockers (lazy `rack.input`,
streaming response, 20-40k RPS overstatement) but left 13
specification gaps. v3 closes all of them, in three groups:

**v2 blockers fixed in v3**:
- `VmBorrow<'_>` RAII type promoted from "recommendation" to
  Decision — `LocalSet` single-threadedness was the actual
  invariant enforcer in v2, never stated explicitly
- `Config::fuel` is per-Vm-lifetime not per-request — v3 adds
  per-request fuel re-anchor + a way for the battery to catch
  `ResourceExhausted` at the `app.call` boundary and convert
  to 503 without killing the worker
- `max_request_body_bytes` enforcement bug — v2 pseudocode
  read the full body before checking size (DoS surface);
  v3 wraps with [`http_body_util::Limited`](https://docs.rs/http-body-util/latest/http_body_util/struct.Limited.html)
  which short-circuits at the byte cap

**v2 majors fixed in v3**:
- `reset_between_requests` field list expanded from 4 → 12,
  with explicit DO-NOT-CLEAR companions
- Pre-fork story expanded: worker-init hook, tokio-after-fork
  ordering, FD inheritance, shared `Arc<Mutex>` semantics,
  macOS caveat
- StringIO gap audit: `read(n, buffer)`, `binmode`,
  `set_encoding`, `gets(sep/limit)`, `string=` added to H1
- Signal handling: `select!` integration + SIGTERM support
- H1 test matrix gaps: query string, pipelining, upgrade
  headers, max header bytes, non-UTF-8 header values

**v2 prior-art accuracy fixes in v3**:
- wasmtime-wasi-http is per-**request** not per-connection
  (v2 said per-connection in 2 places)
- `ProxyPre` cited as closest analogue to
  `reset_between_requests` (novel API in our project)
- deno_core's `LocalSet` claim softened (deno_core doesn't
  use `LocalSet` per se; that's the tokio idiom)
- V8 preemption is `TerminateExecution` not
  `IsolateInterruptCallback`
- Puma `queue_requests` default = `true` (v2 had it
  inverted)
- mruby+H2O 2015 anchor: 25k is JSON+Redis, hello-world
  hit 120k — both cited

## Decision

### Vendor crate

**`hyper` 1.x + `hyper-util` + `tokio` (current-thread
runtime) + `tokio::task::LocalSet`.** No `axum` — its routing
layer duplicates Rack's job; its middleware tower duplicates
Rack middleware. We need accept + parse + serialize, which is
the `hyper` surface exactly.

Cargo deps when feature enabled:

```toml
[dependencies]
hyper = { version = "1", features = ["server", "http1"], optional = true }
hyper-util = { version = "0.1", features = ["tokio"], optional = true }
tokio = { version = "1", features = ["rt", "net", "io-util", "sync", "signal", "time", "macros"], optional = true }
http-body-util = { version = "0.1", optional = true }
bytes = { version = "1", optional = true }

[features]
_http_server = [
    "dep:hyper", "dep:hyper-util", "dep:tokio",
    "dep:http-body-util", "dep:bytes",
]
```

Note: `hyper`'s `http2` feature is **NOT** in v1 (deferred
— needs ALPN + TLS story sorted). v1 ships HTTP/1.1 only.

### `HttpServerConfig` — embedder-supplied allowlist

Per ADR 0019 v3 Rule 4 sub-rule, batteries claiming deviation
classes a/c/d/f accept an embedder allowlist via Config.
Complete v3 field set (was 4 fields in v2):

```rust
pub struct HttpServerConfig {
    /// Bind address. None = battery loaded but server not
    /// started until script-side `Rubyrs::HttpServer.run`.
    pub bind: Option<std::net::SocketAddr>,

    /// Max concurrent in-flight requests via tokio semaphore.
    /// None = unbounded (NOT recommended — DoS surface).
    pub max_concurrent_requests: Option<usize>,

    /// Max request body size in bytes. None = 16 MB default
    /// (NOT unlimited — v1 buffers body, so this caps memory).
    pub max_request_body_bytes: Option<usize>,

    /// Max total header bytes per request (sum of all header
    /// name+value bytes). hyper has a default; this exposes
    /// it for embedders that need to tune. None = hyper
    /// default (~16 KB).
    pub max_header_bytes: Option<usize>,

    /// Per-request I/O-phase deadline (body read + response
    /// write). Cannot preempt CPU-bound Ruby code — see "Per-
    /// request resource enforcement" section. None = no I/O
    /// timeout (rely on hyper's keepalive timeouts).
    pub per_request_io_deadline: Option<std::time::Duration>,

    /// Per-request fuel budget. Refreshed before each
    /// `app.call(env)`. None = inherits `Config::fuel`'s
    /// value (which is per-Vm-lifetime — almost always
    /// wrong for a long-running server).
    pub per_request_fuel: Option<u64>,

    /// Whether the battery installs SIGINT + SIGTERM
    /// handlers for graceful shutdown. Default `false`
    /// — embedders often own signal handling themselves.
    /// The CLI binary `rubyrs` sets this `true`.
    pub install_signal_handler: bool,

    /// Worker initialisation callback fired after each
    /// `fork_workers` child process is created, before the
    /// listener accepts. Use to re-open database connections,
    /// re-seed RNG state, etc. — anything that doesn't
    /// survive `fork(2)` cleanly.
    pub on_worker_boot: Option<Box<dyn Fn(&mut Runtime) + Send + Sync>>,
}
```

Exposed via `Config::http_server: Option<HttpServerConfig>`.

### `LocalSet` mandatory + `VmBorrow<'_>` RAII enforced

The Vm is `!Send + !Sync` (uses `Rc<RefCell<…>>` throughout).
**`tokio::spawn` requires `Send + 'static` even on
`current_thread` runtime** (common misconception — current-
thread does NOT relax `Send`). The mandatory construct is
`tokio::task::LocalSet::spawn_local`, whose bound is
`Future + 'static`.

**v3 decision**: introduce `VmBorrow<'_>` as a load-bearing
type, NOT optional:

```rust
/// RAII guard for a synchronous Vm borrow inside an async
/// request handler. Construction acquires the Vm; Drop
/// releases. The lifetime is the synchronous scope only —
/// holding a VmBorrow across an `.await` is a compile error
/// (the type is `!Send`, so even on a current-thread
/// LocalSet the compiler rejects await-spanning borrows
/// because Future::poll requires Send for spawn — and our
/// internal use enforces this through a sealed trait
/// `VmCallable` that requires synchronous closures).
pub struct VmBorrow<'a> {
    vm: &'a mut Vm,
    // PhantomData<*mut ()> guarantees !Send + !Sync
    _not_send: PhantomData<*mut ()>,
}

impl<'a> VmBorrow<'a> {
    /// Run a synchronous closure with exclusive Vm access.
    /// The closure cannot be async — enforced at the trait
    /// level via the sealed `VmCallable` trait.
    pub fn with<R>(&mut self, f: impl FnOnce(&mut Vm) -> R) -> R {
        f(self.vm)
    }
}

// Drop restores any per-request transient state (the
// `reset_between_requests` discipline can hook here for
// "between request handlers" cleanup vs the inter-request
// boundary).
```

This corrects v2's "convention not type-system" gap. The
**single-threaded LocalSet is the runtime invariant; the
type system enforces no-await-across-borrow via `!Send +
synchronous closure`**.

### Vm ownership across futures — explicit ADR 0013 alignment

[ADR 0013](0013-current-vm-ptr-aliasing.md) defines strict
LIFO + time-disjoint `&mut Vm` access via the `CURRENT_VM_PTR`
re-entrance machinery. v3's `VmBorrow<'_>` discipline
inherits this by construction:

1. **One canonical `&mut Vm` exists at runtime entry.**
   The embedder's main thread owns it.
2. **`LocalSet` runs on the same thread.** No `Send`
   boundary to cross.
3. **Each request handler future borrows the Vm via
   `VmBorrow<'_>` for the synchronous `app.call(env)` block
   ONLY.** The lifetime is scoped; awaits happen outside.
4. **No future polls the Vm while another future holds
   `VmBorrow`.** Enforced by:
   - Type system: `!Send` blocks await-spanning borrows
   - Runtime: current-thread executor never polls two
     futures concurrently
5. **Drop order**: when `VmBorrow` drops at end of `with`
   scope, the next future can acquire it. Strict LIFO
   matches ADR 0013's `CURRENT_VM_PTR` re-entrance protocol.

Request handler shape:

```rust
async fn handle_request(
    req: hyper::Request<Incoming>,
    vm_handle: VmHandle,
    cfg: &HttpServerConfig,
) -> hyper::Response<Full<Bytes>> {
    // Phase A: read body (no Vm access) — await happens here
    let body_bytes = Limited::new(req.body(), cfg.max_request_body_bytes.unwrap_or(16*1024*1024))
        .collect().await?;

    // Phase B: synchronous Vm work (no .await inside)
    let response = vm_handle.borrow().with(|vm| {
        vm.reset_between_requests();
        vm.refill_fuel(cfg.per_request_fuel);
        let env = build_rack_env(req.headers(), body_bytes, ...);
        match vm.call_rack_app(env) {
            Ok(triplet) => collect_response(triplet),
            Err(Trap::ResourceExhausted(_)) => Response::status(503).body("..."),
            Err(other) => Response::status(500).body(format_trap(other)),
        }
    });
    // VmBorrow drops here

    // Phase C: write response (no Vm access) — await happens here
    Ok(response)
}
```

### Deviation classes claimed (per ADR 0019 v3 Rule 4)

- **Class a (owned-resource I/O)** — server binds to a
  caller-supplied `(host, port)`.
- **Class g (native-thread spawn)** — tokio uses an internal
  I/O reactor thread even on `current_thread` runtime.

Classes **NOT** claimed:
- ❌ Class c (multi-host network reach) — server is
  **inbound only**.
- ❌ Class f (mmap / heap-cap bypass) — no.

### V1 body handling — buffered with DoS-safe enforcement

**Request body**: hyper accumulates the body via
`http_body_util::Limited::new(body, max_request_body_bytes)`.
**The cap is enforced as bytes flow in**, NOT after the full
collect — short-circuit on overflow returns `LengthLimitError`
which the handler maps to HTTP 413 Payload Too Large. Same
mechanism handles `Transfer-Encoding: chunked` uploads (the
limiter counts bytes per chunk).

```rust
use http_body_util::{BodyExt, Limited};

let limited = Limited::new(req.into_body(), max_body_bytes);
let bytes = match limited.collect().await {
    Ok(buf) => buf.to_bytes(),
    Err(e) if e.downcast_ref::<LengthLimitError>().is_some() => {
        return Response::status(413).body("Payload Too Large".into());
    }
    Err(e) => return Response::status(400).body(e.to_string().into()),
};
```

`env["rack.input"]` is a Ruby `StringIO` constructed from
the buffered bytes — see "StringIO completeness" section
below for required methods.

**Response body**: the Ruby app returns `[status, headers,
body]`. `body.each` is called synchronously while `VmBorrow`
is held; chunks accumulate in `Vec<u8>`. After `each`
completes, the Vm is released, and the buffered response
bytes write to the socket as `Full<Bytes>`. **Response is
fully materialised before any socket write.**

This is **Puma's default behaviour** (Puma's `queue_requests`
defaults to `true`, meaning the reactor thread fully buffers
the request before app.call). Real-world Rack apps almost
never depend on byte-streaming semantics.

**What v1 explicitly cannot do** (need Phase H3 + Fiber):
- Server-Sent Events (SSE)
- HTTP chunked transfer encoding for streaming responses
- Long-poll / WebSocket upgrade
- Multi-MB upload streaming without buffering

### Per-request resource enforcement — uncatchable, unified with ADR 0008

Per-request limits (deadline, body size, fuel) fire as
**`ResourceExhausted` traps** — the same ADR 0008
uncatchable variant as the existing fuel cap. Bare `rescue`
in app code cannot swallow them. v2's `Timeout::Error`
mapping (catchable) is removed.

**Per-request fuel re-anchor** (NEW in v3): the `Config::fuel`
field in ADR 0008 is per-Vm-lifetime — a long-running server
would burn fuel monotonically across requests, eventually
killing the worker. v3 introduces `HttpServerConfig::per_request_fuel`:

- Before each `app.call(env)`, the battery calls
  `vm.refill_fuel(cfg.per_request_fuel)` to reset the fuel
  counter to the per-request budget
- If `per_request_fuel` is None, the existing Vm-lifetime
  fuel applies (unchanged behaviour)
- When fuel exhausts mid-request:
  - The trap propagates out of `vm.call_rack_app`
  - The battery catches `Trap::ResourceExhausted` at the
    `app.call` boundary
  - Returns HTTP 503 Service Unavailable to the client
  - **Does NOT tear down the Runtime** — the worker keeps
    serving subsequent requests
- The catch-and-503 logic is the **only** place
  `ResourceExhausted` is caught; embedder code calling
  `Runtime::eval` directly still propagates uncatchable as
  before

**Per-request I/O deadline**: as in v2, this can only fire
when the request handler is at an `await` point (body read,
response write). The deadline does NOT preempt synchronous
Ruby code mid-`app.call` — the future never gets polled
during that block.

When the I/O deadline fires:
1. Tokio's `time::timeout` future races against the request
   handler. On timeout, the handler future is dropped.
2. The drop happens between Vm borrows (the borrow itself
   doesn't span await points per the `VmBorrow` design).
3. Battery logs the timeout, returns 504 Gateway Timeout
   (if not already responded) to the client.
4. No Vm state corruption (the Vm wasn't borrowed when the
   future was dropped).

**For CPU preemption use `per_request_fuel`**, not the I/O
deadline. This is the standard `!Send` interpreter limitation
— even V8's `TerminateExecution` cross-thread API requires
out-of-band thread access that our single-thread design
doesn't have.

### `Runtime::reset_between_requests()` — complete field spec

After each request the Vm needs lightweight cleanup between
handler invocations. Full `Runtime::reset()` is heavy (10ms
preamble re-load). The new `reset_between_requests()`:

**Must clear** (per-request transient state):
- `vm.stack` — operand stack
- `vm.frames` — call frame stack (down to base)
- `vm.pending_method_return` — control flow signal
- `vm.pending_loop_transfer` — control flow signal
- `vm.break_signaled` — control flow signal
- `vm.suppress_call_result_push` — call protocol flag
- `vm.bypass_visibility_once` — send/__send__ flag
- `vm.pinned` — GC roots set by cext calls (request-scoped;
  not clearing causes slow leak)
- `vm.class_stack` — class body context stack
- `vm.class_visibility_stack` — private/protected/public mode
- `vm.last_match` (regex `$~`) — leaks request data
- `vm.last_read_line` (`$_`) — same
- Per-frame magic globals (`$&`, `$'`, `$\``, `$1..$9`) —
  same

**Must assert clean state** (programmer-error check):
- `CURRENT_VM_PTR` should be null (no cext call in flight);
  debug_assert + panic for invariant violation

**Must NOT clear** (cross-request state by design):
- `vm.classes` / `vm.constants` — class & const definitions
- `vm.heap` — heap (GC handles dead objects via mark-sweep)
- `vm.interner` — symbol table
- `vm.method_gen` — method generation counter (cache
  invalidation key)
- `vm.call_caches` — inline caches (would invalidate every
  call site every request → 100× perf hit)
- `vm.loaded_features` — require dedup (clearing would
  re-execute every required file per request)
- `vm.cext_class_methods` / `vm.cext_instance_methods` —
  cext method registry
- `vm.host_fns` — embedder-registered host functions
- `vm.fuel_budget` — set by config, refilled via
  `refill_fuel` if per-request fuel is configured

**Public API**:

```rust
impl Runtime {
    /// Lightweight cleanup between HTTP requests. Clears VM
    /// control-flow state without touching class / method /
    /// heap / cache state. Called automatically by
    /// `_http_server` between requests. Exposed publicly for
    /// embedders running other request-shaped loops.
    pub fn reset_between_requests(&mut self);

    /// Refill the per-eval fuel budget. Idempotent; called
    /// before each `app.call(env)`. None argument resets to
    /// the embedder's `Config::fuel` value (lifetime budget
    /// semantics).
    pub fn refill_fuel(&mut self, per_request: Option<u64>);
}
```

The closest precedent for this API shape is wasmtime's
[`ProxyPre::instantiate_async`](https://docs.wasmtime.dev/api/wasmtime_wasi_http/bindings/struct.ProxyPre.html)
which front-loads expensive pre-instantiation work and
provides cheap per-request Store reset. We don't have
`ProxyPre`'s "fresh Store" semantics — our Vm carries class
definitions across requests — but the spirit (cheap reset
of execution state, preserve precompiled artifacts) matches.

### Multi-core scaling — pre-fork via SO_REUSEPORT

v1 single-threaded tokio caps throughput at one CPU core.
The official scaling story matches Puma + Falcon + H2O:
**pre-fork N processes, each binding via `SO_REUSEPORT`**.

**Correct ordering**:

```
1. Parent: parse config, build Runtime (preamble loaded)
2. Parent: bind TCP listener with SO_REUSEPORT
3. Parent: fork N children
4. EACH CHILD:
   a. Build a fresh tokio current-thread runtime
      (tokio runtime CANNOT be inherited across fork)
   b. Re-bind listener with SO_REUSEPORT (each child gets
      its own listener FD pointing at the same port)
   c. Run on_worker_boot callback (re-open DB connections,
      re-seed RNG, etc.)
   d. Enter accept loop
5. Parent: supervise children (wait + restart on crash)
```

**Critical**: the tokio runtime in v2's pseudocode (built
before fork) is wrong — tokio's I/O reactor uses a kernel
FD (epoll/kqueue) that is inherited as a shared kernel
object. Child processes polling the same epoll FD as the
parent causes non-deterministic event delivery. v3 fixes:
**runtime is built AFTER fork in each child**.

**Inherited state across fork**:
- ✅ Class definitions, method tables, constants — inherited
  via COW (per-process modifications don't propagate)
- ✅ `Vm.heap` — inherited via COW (each child's GC is
  independent post-fork)
- ✅ `register_fn` host fn closures — inherited via COW;
  closure-captured state is per-process copy (NOT shared)
- ⚠️  File descriptors opened pre-fork — shared kernel FDs.
  Logfile writes from multiple children interleave;
  database connections sharing a TCP socket FD cause
  protocol-level chaos. **Embedder MUST close-and-reopen
  in `on_worker_boot`** (Puma's `on_worker_boot` is the
  same discipline).
- ⚠️  `Arc<Mutex<...>>` captured in closures — looks shared
  but isn't post-fork; mutex state is per-process. Embedders
  expecting cross-worker mutex synchronisation will silently
  see data divergence. Document explicitly.
- ❌ Tokio runtime — must NOT be inherited (see above).

**Platform support**:
- Linux: `SO_REUSEPORT` works via kernel hash-based
  load balancing. Standard.
- macOS: same syscall, slightly different semantics (round-
  robin instead of hash). **Apple's frameworks (CoreFoundation,
  dispatch) are officially fork-unsafe**; this is dev-only on
  macOS, production deployment should be Linux. Documented
  as caveat.
- Windows: no good SO_REUSEPORT equivalent. **v1 multi-core
  unsupported on Windows**; single-thread or use IIS as
  reverse proxy.

### Signal handling — `select!` integration + SIGTERM

v2 mentioned SIGINT but didn't show how it integrates with
the accept loop. v3 specifies:

```rust
// Inside the LocalSet, the accept loop is a select!
// against signals + listener readiness
loop {
    tokio::select! {
        accept = listener.accept() => {
            let (stream, _) = accept?;
            local.spawn_local(handle_connection(stream, vm_handle.clone()));
        }
        _ = sigint_future(), if cfg.install_signal_handler => break,
        _ = sigterm_future(), if cfg.install_signal_handler => break,
    }
}
```

**SIGINT** (`tokio::signal::ctrl_c()`) is for interactive
`Ctrl+C` from a terminal. **SIGTERM** (`tokio::signal::unix::signal(SignalKind::terminate())`) is for systemd/k8s graceful
shutdown — without this, container orchestrators send
SIGTERM and we don't shut down cleanly.

Both signal futures are armed only if
`install_signal_handler = true`. Embedders managing signals
themselves see `false` (default) and the battery never
registers handlers (avoiding double-registration conflicts).

**Cross-platform**:
- Unix: SIGINT + SIGTERM
- Windows: only `ctrl_c` (no SIGTERM equivalent in
  `tokio::signal`)

### env hash construction

Rack SPEC env hash, built in Rust per request:

```rust
fn build_rack_env(
    method: &http::Method,
    uri: &http::Uri,
    headers: &http::HeaderMap,
    body_bytes: bytes::Bytes,
    listener: &SocketAddr,
    scheme: &str,
) -> Value /* Hash */ {
    let mut env = Hash::new();
    env.set("REQUEST_METHOD", method.as_str());

    // Rack SPEC: PATH_INFO is decoded; QUERY_STRING is RAW.
    // hyper's uri.path() is path-component-decoded; uri.query() is raw.
    env.set("PATH_INFO", uri.path());
    env.set("QUERY_STRING", uri.query().unwrap_or(""));

    env.set("SERVER_NAME", listener.ip().to_string());
    env.set("SERVER_PORT", listener.port().to_string());
    env.set("SCRIPT_NAME", "");
    env.set("HTTP_VERSION", "HTTP/1.1");

    // Headers: HTTP_<UPPER_NAME_WITH_DASHES_AS_UNDERSCORES>
    // Header values may be non-UTF-8 (Latin-1 by HTTP spec);
    // lossy-decode to String, set raw bytes in
    // "HTTP_<NAME>_BYTES" parallel key for cext / strict apps.
    for (name, value) in headers {
        let env_key = format!("HTTP_{}", name.as_str().to_uppercase().replace('-', "_"));
        match value.to_str() {
            Ok(s) => env.set(env_key, s),
            Err(_) => {
                env.set(env_key, value.to_str_lossy()); // U+FFFD on bad bytes
                env.set(format!("{env_key}_BYTES"), value.as_bytes()); // String w/ Binary tag
            }
        }
    }
    // Content-Type / Content-Length get special-cased names (no HTTP_ prefix)
    if let Some(ct) = headers.get(http::header::CONTENT_TYPE) {
        env.set("CONTENT_TYPE", ct.to_str().unwrap_or(""));
    }
    if let Some(cl) = headers.get(http::header::CONTENT_LENGTH) {
        env.set("CONTENT_LENGTH", cl.to_str().unwrap_or(""));
    }

    env.set("rack.url_scheme", scheme);
    env.set("rack.input", StringIO::new_binary(body_bytes));
    env.set("rack.errors", stderr_sink);
    env.set("rack.version", [1, 6]);
    env.set("rack.multithread", false);
    env.set("rack.multiprocess", true);  // pre-fork makes this true
    env.set("rack.run_once", false);
    env
}
```

**Header normalisation policy**: HTTP allows non-UTF-8 header
values (Latin-1 by RFC 7230); Ruby Strings carry encoding
tags per [ADR 0020](0020-encoding-placement.md). Lossy-decode
to UTF-8 for the canonical env key + parallel `_BYTES` key
with the Binary-tagged raw bytes. Strict apps reading raw
bytes get the parallel key; common apps reading `HTTP_*`
get the lossy form (matches CRuby behaviour on Rack 3).

### StringIO completeness for `rack.input`

`stdlib_vendor/stringio.rb` (184 LOC, shipped) covers:
`#read(n)`, `#gets`, `#each`, `#each_line`, `#rewind`,
`#size`, `#eof?`, `#close`.

**Missing for v1, added in H1 scope** (Rack SPEC + middleware
real-world usage):

- `#read(n, buffer)` — 2-arg form. **Rack SPEC mandates this**.
  Many parsers (rack-multipart, rack-test) pass a destination
  buffer to avoid allocation.
- `#binmode` — no-op in CRuby, but called by `Rack::Multipart`
  and many file-handling middlewares. Returns `self`.
- `#set_encoding(enc)` — accepts `:binary` / `Encoding::ASCII_8BIT`.
  Rack SPEC requires `rack.input` is ASCII-8BIT-encoded.
- `#string=(s)` — replaces buffer. Used by test middleware.
- `#gets(separator)` and `#gets(limit)` variants — Rack JSON
  middleware calls `#gets(nil)` to slurp; some line-based
  middlewares pass custom separators.
- `#getbyte` / `#readbyte` — byte-oriented reading.
- `#pos` / `#pos=` — position tracking (some sniffers
  rewind on header check).

These are real-world requirements from `rack-multipart`,
`rack-test`, and major Sinatra/Rails middlewares. Adding ~80
LOC to the existing 184 LOC of `stringio.rb` covers them.

### Response handling

Ruby app returns `[status, headers, body]`:
- `status` — integer, written to hyper response status
- `headers` — Hash<String, String | Array<String>>. Single-
  value mapped to `headers.insert(name, value)`. Array-value
  (e.g. multiple `Set-Cookie`) mapped to `headers.append`
  per element.
- `body` — must respond to `#each(&block)`. The battery
  iterates synchronously while `VmBorrow` is held, appending
  each yielded String to `Vec<u8>`. After `#each` returns,
  the Vm is released and the full Vec is sent as
  `Full<Bytes>`. **No chunked transfer in v1.**

## What v1 ships

- `_http_server` Cargo feature
- HTTP/1.1 only
- Rack SPEC env hash conformance (v1.6) with non-UTF-8
  header handling
- Buffered request body via existing StringIO + 8 added
  methods (~80 LOC)
- Buffered response body (all-at-once write)
- One Ruby class: `Rubyrs::HttpServer`
  - `.bind(addr)` — creates handle
  - `#run(rack_app)` — starts loop, blocks until shutdown
  - `#shutdown` — graceful stop
  - `.fork_workers(n)` — multi-process pre-fork (Unix)
- `Runtime::reset_between_requests()` API
- `Runtime::refill_fuel(per_request)` API
- `VmBorrow<'_>` RAII type for synchronous Vm access
- Per-request body size enforcement via `Limited` (DoS-safe)
- Per-request fuel re-anchor + `ResourceExhausted` catch
  → 503 at `app.call` boundary (no worker tear-down)
- Per-request I/O-phase deadline → 504
- `max_header_bytes` configuration (tunes hyper's default)
- Optional SIGINT + SIGTERM graceful shutdown
- `on_worker_boot` callback for fork-aware embedders

### v1 test matrix (H1 acceptance criteria)

- 200 hello-world (Ruby app returns `[200, {}, ["hi"]]`)
- 404 not found (Ruby app returns `[404, {}, []]`)
- POST with body — body buffered + readable via `rack.input.read`
- POST with `Transfer-Encoding: chunked` — bounded by
  `max_request_body_bytes`
- POST exceeding body limit — 413 response without OOM
- Query string `?a=1&b=2` — raw via `QUERY_STRING`
- URL-encoded `%3F` in path — stays in `PATH_INFO`, doesn't
  splatter into QUERY_STRING
- Non-UTF-8 header value (e.g. `Set-Cookie: latin1-bytes`)
  — `HTTP_*` lossy + `_BYTES` parallel key
- Multiple `Set-Cookie` headers — array-of-strings
  response handling
- HTTP pipelining (multiple requests, one connection) —
  serialised through Vm, both succeed
- `Upgrade: websocket` request — v1 returns 426 Upgrade
  Required (no WS support)
- Header size exceeded — 431 Request Header Fields Too
  Large
- Slow client body upload — bounded by I/O deadline → 504
- Long-running Ruby loop — bounded by per-request fuel
  → 503 (worker survives)
- SIGINT received mid-server — accept loop breaks, in-
  flight requests complete, then runtime exits
- SIGTERM received (Linux) — same as SIGINT
- Pre-fork × 4 children (Linux/macOS) — wrk shows
  approximately 4× single-child throughput
- `on_worker_boot` fired in each child before listener
  accepts

## What v1 explicitly defers

- **Streaming request body** (lazy `rack.input`) → Phase H3
  (depends on Fiber Tier 2)
- **Streaming response body** (chunked transfer) → H3
- **Server-Sent Events** → H3
- **WebSocket** → separate `_websocket` battery
- **HTTP/2** → `_http_server_h2` battery (needs ALPN + TLS)
- **HTTP/3 / QUIC** → `_http_server_h3` battery
- **TLS** → `_http_server_tls` battery (rustls)
- **Multi-Vm in one process** (per-connection or per-request
  Vm — wasmtime-wasi-http pattern) → H6 work
- **Per-request CPU preemption beyond fuel** → would need
  V8-`TerminateExecution`-style cross-thread interrupt;
  significant Tier 1 work
- **Multipart parsing** → Ruby app's responsibility (or
  separate `_multipart` battery)
- **Access logs** → embedder wires via `tracing` if needed
- **Per-IP rate limiting** → deploy a real proxy
- **Windows multi-core (SO_REUSEPORT)** → unsupported in v1

## Honest performance estimates

V1 single-thread + buffered + interpreted Ruby:

| Workload | v1 estimate | Confidence | Anchor |
|---|---|---|---|
| Empty 200 (no Ruby work beyond return) | 30-50k RPS | Medium | hyper alone does 150k+ |
| Sinatra-style hello-world | 2-5k RPS | Medium | VM dispatch dominates |
| Rack JSON API (5 KB response, no I/O) | 1-3k RPS | Low | JSON serialise overhead |
| Rack JSON API + Redis round-trip | 0.5-1.5k RPS | Low | Network I/O dominates |
| Pre-fork × N cores (4-core machine) | ~4× above | High | SO_REUSEPORT works |

**Comparison anchors** (industry data, full citations
below):
- **Puma + CRuby Sinatra**: ~5k RPS — the floor to beat
- **Falcon + CRuby Sinatra**: ~10k RPS — what we match at
  pre-fork ×4-8
- **mruby + H2O 2015 plain hello-world**: 120k RPS — upper
  bound for "small VM + Rust HTTP front" without app I/O
- **mruby + H2O 2015 JSON API + Redis**: 28k RPS — what
  real-world apps hit; we target this for v2 with multi-Vm
- **Bun.serve + Express**: 52k RPS — different ecosystem
  (V8 + JIT), not directly comparable

**v1 marketing target**: "Comparable to Falcon at the same
core count; 1/10 cold start; 1/10 RSS. Real perf unlocks
with Phase H3 (Fiber) and H6 (multi-Vm)."

This is honest framing — not the 20-40k RPS the v1 ADR
draft claimed, not Bun-class plain-hello-world numbers
that require multi-Vm + JIT.

## Consequences

### What gets easier

- **Bun-class story has honest demo target**: ~5k RPS
  hello-world matches Puma; pre-fork × 4 matches Falcon.
  Add 1/10 cold start and 1/10 RSS for real
  differentiation.
- **Rack ecosystem becomes credible roadmap.** Once
  autoload + 7 stdlib batteries land, real Rack apps run.
  Buffered-body Rack apps are >90% of real apps.
- **No new VM design work for v1.** Buffered body uses
  existing StringIO + 8 added methods. Tokio is mature.
  Hyper is mature.
- **Type-system enforces Vm safety.** `VmBorrow<'_>` +
  `!Send` + LocalSet single-thread = no convention to
  remember; the compiler rejects await-spanning Vm access.
- **`ResourceExhausted` survivable at app boundary.**
  Battery catches per-request fuel exhaustion at `app.call`
  return, sends 503, worker keeps serving. CPU-runaway
  request no longer kills the worker.

### What gets harder

- **`VmBorrow` ergonomics.** Embedders writing host fns
  used inside request handlers see a slightly different
  shape — `vm_handle.borrow().with(|vm| {...})` instead
  of direct `&mut vm`. Documented; not invasive.
- **Pre-fork discipline.** Embedders must understand
  fork-and-reinitialise via `on_worker_boot` — DB
  connections, telemetry threads, RNG state all need
  per-worker setup. Puma's `on_worker_boot` is the same
  discipline; we copy the convention.
- **Per-request fuel tuning.** Embedders must pick a
  `per_request_fuel` value that allows their app's
  normal request to complete while bounding runaway
  scripts. No magic number; needs measurement per app.
- **Test matrix has 16 cases.** CI cost grows; budget
  ~15 seconds for the H1 integration suite.

### What we explicitly accept trading away

- **No SSE / chunked / streaming bodies in v1.** Real cost
  for LLM streaming, large file downloads; deploy alongside
  another solution or wait for H3.
- **HTTP/1.1 only in v1.** Production behind nginx / Caddy
  / Cloudflare for HTTP/2 / HTTP/3.
- **Pre-fork only for multi-core in v1.** Loses some
  request-routing flexibility multi-worker setups have.
  Matches Puma + Rails production shape.
- **Windows is single-thread only.** Acceptable; Windows
  isn't a primary deployment target for embedded Ruby.

## Alternatives considered

1. **`axum` instead of `hyper`.** Axum's routing duplicates
   Rack. Rejected.

2. **`actix-web`.** Own runtime fragments ecosystem.
   Rejected for tokio alignment.

3. **`warp`.** Filter-combinator design awkward for
   "give me request, I'll call Ruby." Rejected.

4. **Build as embedder concern (no battery).** Duplicates
   work, kills the demo. Rejected.

5. **Combine inbound + outbound HTTP.** Different surfaces,
   deviation classes, deps. Two batteries cleaner.

6. **Multi-threaded tokio + `Mutex<Vm>`**. Mutex contention
   kills perf. Per-request Vm clones (wasmtime-wasi-http
   pattern) is the right shape but needs `Vm::new()`
   cost <1ms. Deferred to H6.

7. **Per-request Vm spawn** (wasmtime-wasi-http
   `ProxyPre::instantiate_async` pattern). Right
   architectural shape for H6; requires Vm cold-start
   ≪ 1ms (today ~10ms with preamble), wizer-style pre-
   init to amortise, shared state moved to Rust side.

8. **Fiber-aware request handler (Falcon-shape).** Requires
   Tier 2 Fiber landing. Right v3+ shape; v1 sticks with
   sync buffered.

9. **Streaming via `spawn_blocking`.** Moves Vm call to
   different thread; breaks `!Send`. Rejected.

10. **deno_core-style "JsRuntime is the future".** deno_core's
    pattern is that ops can `await` while the isolate is
    parked; we explicitly forbid this (no Vm access across
    await). Our actual closest analogue is wasmtime's
    "sync host calls only" pattern, not deno_core. v3 cites
    accordingly.

## Migration plan

### Phase H1 — minimal viable battery (v0.2.0)

- `Rubyrs::HttpServer` Ruby class
- HTTP/1.1 only, buffered body in + out
- Rack SPEC env hash (v1.6) with non-UTF-8 header handling
- `LocalSet`-based runtime entry
- `VmBorrow<'_>` RAII type
- `Runtime::reset_between_requests()` API
- `Runtime::refill_fuel()` API
- `Limited`-based body size enforcement (DoS-safe)
- Per-request fuel re-anchor + `ResourceExhausted` → 503
  catch at app.call boundary
- Per-request I/O deadline → 504
- `max_header_bytes` config
- Optional SIGINT + SIGTERM
- `on_worker_boot` callback
- `fork_workers(n)` with correct ordering (bind → fork →
  per-child runtime)
- 8 added StringIO methods (~80 LOC)
- 16-case test matrix (H1 acceptance criteria)
- Integration test: Sinatra-shape Ruby app + wrk smoke
  test

### Phase H2 — mini-Rack integration (v0.2.x)

- Tier 3 pure-Ruby `Rack::Request` / `Rack::Response`
  (separate Tier 3 canon ADR if it grows)
- Sinatra-shape micro-framework demo (~200 LOC pure-Ruby)
- Benchmark vs Puma + Sinatra
- Document the 1/10 cold-start + 1/10 RSS comparison

### Phase H3 — streaming via Fiber (v0.3.0+)

**Depends on Tier 2 Fiber landing.**

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
from H3.

- Smoke test: load unmodified rack from `vendor/bundle/...`
- Smoke test: load unmodified sinatra

### Phase H5 — TLS + HTTP/2 (v0.4.0+)

- `_http_server_tls` battery (rustls + tokio-rustls)
- ALPN → HTTP/2 upgrade
- `_http_server_h2` feature
- HTTP/3 via `_http_server_h3` later

### Phase H6 — per-request Vm (v0.5.0+ / v1.0)

- wasmtime-wasi-http pattern: build fresh Vm per request
  via a `ProxyPre`-equivalent that pre-instantiates expensive
  setup
- Requires Vm cold-start ≪ 1ms (today ~10ms with preamble)
- Or: Vm pool with checkout/checkin per request
- Multi-threaded tokio; Vms still single-threaded
  individually

## What changes vs ADR 0022 v2 (this revision is v3)

| v2 said | v3 says | Reason |
|---|---|---|
| "Vm ownership" relied on convention | **`VmBorrow<'_>` RAII type as mandatory Decision**; `!Send` + LocalSet single-thread is the type-system enforcement | Review C1: convention not enforceable; type-system enforces no-await-across-borrow |
| `Config::fuel` handles CPU preemption | **`HttpServerConfig::per_request_fuel` + `Runtime::refill_fuel` + `ResourceExhausted` catch at `app.call` boundary → 503** | Review C4: lifetime fuel exhausts after N requests; need per-request reset + survivable trap |
| `req.into_body().to_bytes()` after limit check | **`http_body_util::Limited::new(body, max).collect()` with byte-level short-circuit** | Review #4: v2 pseudocode read full body before checking size → DoS |
| `reset_between_requests` clears 4 fields | **Clears 12 fields + asserts CURRENT_VM_PTR null + 9 explicit DO-NOT-CLEAR** | Review C2: incomplete field list (missed pinned, class_stack, magic globals, etc.) |
| Pre-fork mentioned, no detail | **Full ordering spec (bind → fork → per-child runtime), `on_worker_boot` config field, FD inheritance warning, Arc<Mutex> caveat, macOS unsupported-for-prod, Windows unsupported** | Review C3: too thin for an embedder to use safely |
| Used existing StringIO as-is | **8 added methods (`read(n, buf)`, `binmode`, `set_encoding`, `string=`, `gets(sep/limit)`, `getbyte`, `pos`/`pos=`)** | Review #1: Rack SPEC + real middleware needs |
| SIGINT mentioned, no integration spec | **Full `tokio::select!` spec with accept loop, SIGINT + SIGTERM both supported on Unix, `install_signal_handler` opt-in** | Review #7: pseudocode loop had no signal branch; SIGTERM missing for k8s |
| `max_header_bytes` not configurable | **Added to `HttpServerConfig`** | Review #6: hyper has a default that's sometimes too small |
| Non-UTF-8 header values unspecified | **Lossy-decode + parallel `_BYTES` key with Binary-tagged raw bytes (per ADR 0020)** | Review #6: real headers can be Latin-1 |
| wasmtime-wasi-http "per-connection Store" (×2) | **"per-request Store" via `ProxyPre::instantiate_async`** | Review #2: factually wrong; per-request is the actual pattern |
| No `ProxyPre` citation | **Cited as closest analogue for `reset_between_requests`** | Review #6: missing the key prior-art |
| "deno_core's pattern" for LocalSet | **Softened — LocalSet is the tokio idiom; deno_core's pattern is "poll JsRuntime future directly"; we're closer to wasmtime sync-host-calls** | Review #1: deno_core allows await-while-isolate-parked, opposite of our design |
| V8 preemption = `IsolateInterruptCallback` | **`TerminateExecution` (cross-thread) + `RequestInterrupt` (lower-level)** | Review #1: wrong API name |
| Puma `queue_requests` default `false` | **Default `true`** (full buffering by default) | Review #3: inverted; substantive claim still correct |
| mruby+H2O 25k cited as ceiling | **120k plain hello-world; 28k JSON API + Redis** — both anchored | Review #5: 25k was the JSON+Redis case; hello-world is much higher |

## What changes vs ADR 0022 v1 (preserved from v2 — historical)

| v1 said | v3 says | Reason |
|---|---|---|
| Lazy `rack.input` for streaming | **Buffered body via StringIO** | v1 unimplementable on sync VM; needs Fiber → H3 |
| Streaming response via Ruby Enumerator | **Buffered response, all-at-once write** | Same — needs Fiber → H3 |
| 20-40k RPS estimate | **2-8k RPS hello-world** | v1 was multi-Vm number cited as v1; honest framing |
| `tokio::spawn` for request handlers | **`tokio::task::LocalSet::spawn_local` mandatory** | `spawn` requires Send even on current_thread |
| `Timeout::Error` for per-request deadline | **`ResourceExhausted` (uncatchable)** | Catchable variant defeats the cap |
| ADR 0013 not mentioned | **Explicit Vm ownership section + ADR 0013 cross-ref** | LIFO + time-disjoint invariant load-bearing |
| Bun as primary precedent | **wasmtime-wasi-http as primary** | Closer technical fit (`!Send`, sync host calls) |
| No multi-core scaling story | **Pre-fork SO_REUSEPORT documented** | Matches Puma/Falcon/H2O |

## Revision log

- **2026-05-27 — v3 (this revision).** Major revision after
  second parallel review of v2 flagged 3 blockers + 7
  majors + 4 prior-art accuracy issues. v3 closes all 13.
  Promoted `VmBorrow<'_>` to mandatory Decision. Added
  `per_request_fuel` + `ResourceExhausted` → 503 catch
  pattern. Fixed body-limit DoS bug. Expanded
  `reset_between_requests` field list 4 → 21 (12 clear
  + 9 do-not-clear + assertion). Full pre-fork ordering
  spec. SIGTERM support. 8 StringIO methods. Corrected
  wasmtime-wasi-http to per-request not per-connection,
  added `ProxyPre` citation, softened deno_core analogy,
  fixed V8 API name, inverted Puma `queue_requests`
  default, distinguished mruby+H2O hello-world 120k from
  JSON-API+Redis 28k.
- **2026-05-27 — v2 (commit `ea92dec1`).** Major rewrite
  after first parallel review of v1 flagged 3 blockers
  + 7 majors. Removed lazy `rack.input`, streaming
  response, 20-40k RPS overstatement. Added `LocalSet`
  mandate, per-request reset API, pre-fork story,
  honest performance estimates.
- **2026-05-27 — v1 (commit `88564485`).** First draft;
  proposed lazy `rack.input` and streaming response,
  both unimplementable on sync VM. 20-40k RPS was
  multi-Vm number cited as v1.

## Related

### Internal ADRs

- [ADR 0019 v3 — Tier 2 / Tier 3 boundary](0019-tier2-tier3-boundary.md)
  — Rule 7 (ADR-per-battery), Rule 4 (deviation taxonomy),
  Rule 8 (`require "rubyrs/http_server"` namespace). **This
  ADR is the first concrete instance of Rule 7.**
- [ADR 0008 — Resource caps for untrusted scripts](0008-resource-caps-for-untrusted-scripts.md)
  — `per_request_fuel` extends the existing cap model;
  `ResourceExhausted` uncatchable variant reused with new
  catch-at-app-boundary discipline.
- [ADR 0013 — CURRENT_VM_PTR borrow aliasing](0013-current-vm-ptr-aliasing.md)
  — `VmBorrow<'_>` RAII inherits LIFO + time-disjoint
  invariant by construction.
- [ADR 0017 — Tier 1 boundary](0017-tier1-boundary.md) —
  battery is firmly Tier 3.
- [ADR 0020 — Encoding placement](0020-encoding-placement.md)
  — non-UTF-8 header values use the Binary-tagged String
  story.
- Issues #224 (autoload), #225 (Config::load_paths), #226
  (Kernel#load), #227 (stdlib batteries) — H4 depends on
  all four

### External references

- [Rack SPEC v1.6](https://github.com/rack/rack/blob/main/SPEC.rdoc)
  — env hash conventions
- [`wasmtime-wasi-http`](https://docs.wasmtime.dev/api/wasmtime_wasi_http/)
  — closest Rust precedent. Per-request Store pattern via
  `ProxyPre::instantiate_async`. **Primary prior art**.
- [`ProxyPre::instantiate_async`](https://docs.wasmtime.dev/api/wasmtime_wasi_http/bindings/struct.ProxyPre.html)
  — the analogue for `reset_between_requests`'s spirit
- [`tokio::task::LocalSet`](https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html)
  — the API enabling `!Send` futures
- [`http_body_util::Limited`](https://docs.rs/http-body-util/latest/http_body_util/struct.Limited.html)
  — DoS-safe body size enforcement
- [`deno_core::JsRuntime`](https://docs.rs/deno_core/latest/deno_core/struct.JsRuntime.html)
  — related Rust precedent. `!Send` engine + current-thread
  tokio. Diverges from us: deno_core ops can await while
  isolate parked; we forbid Vm access across await.
- [V8 `TerminateExecution`](https://v8.github.io/api/head/classv8_1_1Isolate.html#a2ed0a3f6b1b4d8a4d18c5e0b1b6f8c4a)
  — JS-side analogue of cross-thread preemption (we don't
  have an equivalent; `per_request_fuel` is our answer)
- [Puma `queue_requests`](https://www.rubydoc.info/gems/puma/Puma/DSL:queue_requests)
  — default `true`; full buffering before app.call. Same
  shape as our v1.
- [Puma `on_worker_boot`](https://github.com/puma/puma/blob/master/docs/deployment.md#on_worker_boot)
  — pre-fork worker init discipline we copy
- [Puma SO_REUSEPORT support](https://github.com/puma/puma/issues/1307)
  — multi-core pattern precedent
- [Bun's `Bun.serve`](https://bun.com/docs/api/http) —
  marketing precedent (uses thread pool internally —
  different architecture from ours)
- [Falcon (Ruby Fiber-based)](https://github.com/socketry/falcon)
  — H3 reference architecture once Fiber lands
- [Luca Guidi — 25k RPS with mruby + H2O (2015)](https://lucaguidi.com/2015/12/09/25000-requests-per-second-for-rack-json-api-with-mruby/)
  — historical anchor for "small VM + native HTTP front"
  approach. Plain hello-world hit 120k; JSON+Redis hit
  28k. Both numbers cited.
- [jodosha/mruby-rack-json-api](https://github.com/jodosha/mruby-rack-json-api)
  — the actual app benchmarked in the Luca Guidi piece
