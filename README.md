# rubyrs (workspace)

This is a Cargo workspace. It currently hosts one crate
([`crates/rubyrs/`](crates/rubyrs/)) — the Ruby-subset
interpreter described below. A second crate, `rubund` (a Rust
implementation of Bundler), is planned and will be added as a
sibling under `crates/`. `rubund` is the first real driver of
`rubyrs`'s embedding API — Gemfile and `*.gemspec` files are
Ruby DSLs, so the Bundler-in-Rust work doubles as in-tree
dogfooding of the interpreter.

## rubyrs

[![CI](https://github.com/linyiru/rubyrs/actions/workflows/ci.yml/badge.svg)](https://github.com/linyiru/rubyrs/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.95+-orange.svg)](https://www.rust-lang.org/)
[![Status: experimental](https://img.shields.io/badge/status-experimental-yellow)](docs/SUBSET.md)

A tiny Ruby-subset interpreter written in Rust, built on
[Prism](https://github.com/ruby/prism).

```ruby
class Greeter
  def initialize(name)
    @name = name
  end

  def hello
    "Hello, #{@name}!"
  end
end

["Ruby", "Rust", "Prism"].each { |w| puts Greeter.new(w).hello }
```

```
$ rubyrs greet.rb
Hello, Ruby!
Hello, Rust!
Hello, Prism!
```

## Positioning

rubyrs is **not** a CRuby replacement. It targets the same niche as
[mruby](https://github.com/mruby/mruby): a small, memory-safe, embeddable
Ruby-flavored runtime — but written in Rust, with the option of compiling
to WebAssembly.

| End-to-end DSL hosting (Brewfile, ~50 lines) | rubyrs | CRuby 3.4 | CRuby + YJIT |
|----------------------------------------------|--------|-----------|--------------|
| Time | **1.8 ms** | 74.7 ms | 75.5 ms |

→ **rubyrs is 42× faster end-to-end** on this shape of workload — the
actual product-niche benchmark. See
[`examples/brewfile/`](crates/rubyrs/examples/brewfile/) for the
simpler tap/brew/cask DSL, or
[`examples/gemfile/`](crates/rubyrs/examples/gemfile/) for an
unmodified Rails-style Gemfile (`*splat`, `**kwargs`, multi-symbol
`group … do … end` blocks, file-scope conditionals — all the
real-world shapes a Bundler Gemfile uses, running in ~0.4 ms
end-to-end).

| Cold start | rubyrs (native) | rubyrs.wasm (raw, JIT) | rubyrs.cwasm (AOT + wizer) | CRuby 3.4 |
|------------|----------------|------------------------|----------------------------|-----------|
| `puts 1+2` | **1.5 ms** | 12.7 ms | **~7 ms** | 78 ms |

The wasm column is the raw `.wasm` shipping shape under
`wasmtime run`; `cwasm` adds a one-time `wasmtime compile`
step plus `wizer` pre-initialization (preamble snapshot)
— and is what `perf/wasm_check.sh` measures end-to-end. See
`docs/DEVELOPMENT.md` for the build pipeline.

| 1M fizzbuzz | rubyrs | CRuby | CRuby + YJIT |
|-------------|--------|-------|--------------|
| Time | 0.33 s (1.76× of CRuby) | 0.19 s | 0.15 s |
| Peak memory | 2.1 MB | 18.4 MB | 19.1 MB |

| Method-heavy (Counter.inc × 1M) | rubyrs | CRuby (no JIT) |
|---------------------------------|--------|----------------|
| Time | 0.15 s (**1.43× of CRuby**) | 0.11 s |

If you need Rails, Sinatra, Bundler, or gems — use CRuby.

### What works with `require`

By design, rubyrs is **not a Ruby gem host**. The `require`
mechanism resolves these shapes:

- `require "/abs/path.rb"` — absolute paths to user `.rb` files
- `require "relative/path"` — relative to caller's source dir
- `require "name"` with `$LOAD_PATH << dir` set by the script
- `require "pathname"` / `set` / `stringio` / `strscan` —
  the four vendored stdlib modules with real implementations
- `require "uri"` / `json` / `yaml` / `csv` / `logger` /
  ~25 other stdlib names — these **succeed silently** as
  lenient "feature-present" stubs; method calls on the
  resulting modules raise `NoMethodError`. With
  `--features stdlib` the vendored modules above behave
  CRuby-compatibly; everything else stays stub-shaped.

What deliberately does NOT work (all are documented Tier 2 /
Tier 3 deferrals — see [docs/SUBSET.md](docs/SUBSET.md) line
"`require / load / autoload`"):

- **`autoload :Foo, "foo"`** — accepts the call as a silent
  no-op for arity-compat; does not register a real lazy
  load. Referencing `Foo` later still raises `NameError`.
- **`Kernel#load`** — not implemented at all
- **Auto-populated `$LOAD_PATH`** — empty by default.
  Embedders set it via `Config::load_paths` or script-side
  `$LOAD_PATH.unshift(dir)`. CRuby auto-fills stdlib + gem
  paths; rubyrs does not.
- **Real stdlib coverage beyond the four vendored modules**
  — `URI.parse`, `JSON.parse`, `YAML.load`, etc. are all
  Tier 3 batteries (per [ADR 0019](docs/adr/0019-tier2-tier3-boundary.md)),
  none shipped today.

The shape is `Lua-in-Rust + Ruby grammar + sandbox`, not
`CRuby with fewer features`. ADR 0017 codifies the boundary
intentionally — embedders building sandboxed DSL hosts
benefit from the deterministic-by-default behaviour these
omissions guarantee.

## Install

### As a library (recommended for v0.1.0)

The canonical v0.1.0 release lives at the
[`v0.1.0` git tag](https://github.com/linyiru/rubyrs/releases/tag/v0.1.0).
Depend on it via Cargo's git dependency:

```toml
[dependencies]
rubyrs = { git = "https://github.com/linyiru/rubyrs", tag = "v0.1.0" }
```

The crates.io entries (`rubyrs@0.1.0`, `rubyrs-cext@0.1.0`)
were published 2026-05-25 as **name-registration placeholders**
and predate the substantive v0.1.0 work (the ADR-driven
architecture work, 263-fixture diff_cruby surface, OutputSink
trait, untrusted-input cap model). They are intentionally NOT
the canonical v0.1.0 — they exist only to reserve the names.
A future `0.1.x` / `0.2.0` release will publish the real
artifact to crates.io. For now, the git tag is the source of
truth.

### CLI from source

```bash
git clone https://github.com/linyiru/rubyrs --branch v0.1.0
cd rubyrs
cargo build --release
./target/release/rubyrs your_script.rb
```

## Build

```bash
cargo build --release
./target/release/rubyrs your_script.rb
```

Per-run resource caps (useful when running scripts you don't fully
trust):

```bash
RUBYRS_FUEL=1000000 \
RUBYRS_MAX_OBJECTS=10000 \
RUBYRS_MAX_FRAMES=128 \
  ./target/release/rubyrs script.rb
```

Any cap that trips returns a `ResourceExhausted` trap with a normal
backtrace (no host panic). See
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full list of env
vars and the `wasm32-wasip1` build instructions.

## Embedding

rubyrs is also a Rust crate: drop it into a `Cargo.toml`, build a
`Runtime`, and run scripts in process.

```rust
use rubyrs::{Config, Runtime, Value};

let mut rt = Runtime::with_config(Config {
    // Resource caps for untrusted scripts. All optional; None = unlimited.
    fuel: Some(1_000_000),
    max_heap_objects: Some(10_000),
    max_frames: Some(128),
    ..Default::default()
});

// Expose a host function to the Ruby side.
rt.register_fn("host_pid", |_args| {
    Ok(Value::Int(std::process::id() as i64))
});

// Capture stdout into your own sink (defaults to process stdout).
// rt.set_stdout(Box::new(my_writer));

rt.eval(r#"puts "pid is #{host_pid}""#, "inline").unwrap();
```

The runtime is incremental — class and method definitions persist across
`eval` calls, so you can split DSL setup and script execution into
multiple chunks. See
[`crates/rubyrs/examples/embed.rs`](crates/rubyrs/examples/embed.rs)
for the fuller story (captured stdout, persistent classes, Trap
propagation) and
[`crates/rubyrs/tests/embed.rs`](crates/rubyrs/tests/embed.rs)
for the pinned API surface.

Run the example:

```bash
cargo run --release -p rubyrs --example embed
```

## HTTP server battery (preview)

`_http_server` is an opt-in Phase H1 PoC of a Rack-shape HTTP
server hosted inside the rubyrs runtime — Rust front (`hyper 1.x`
+ `tokio` current_thread), Ruby app handler. The full design
lives in [docs/adr/0022-http-server-battery.md](docs/adr/0022-http-server-battery.md).

Single process:

```ruby
app = ->(env) {
  [200, {"Content-Type" => "text/plain"}, ["hello from rubyrs"]]
}
# (addr, duration_secs, app[, per_request_fuel, max_body, ...])
__rubyrs_http_serve_with_app("127.0.0.1:9292", 60, app)
```

Multi-core via pre-fork (Stage 7, Unix only):

```ruby
on_worker_boot = ->(idx) { puts "[worker #{idx}] booted" }
__rubyrs_http_serve_prefork(
  "127.0.0.1:9292", 60, app, 4,  # 4 workers
  { on_worker_boot: on_worker_boot, per_request_fuel: 1_000_000 },
)
```

See [crates/rubyrs/examples/prefork_server.rb](crates/rubyrs/examples/prefork_server.rb)
for a runnable example.

**Platform support** (per ADR 0022 v3 §"Multi-core scaling"):

| Platform | Single-process | Pre-fork N≥2 | Notes |
|----------|---------------|--------------|-------|
| Linux 3.9+ | ✅ | ✅ — kernel hash-balanced SO_REUSEPORT | Production target |
| macOS    | ✅ | ⚠️  dev-only | Workers fork + boot + serve, but Darwin has no `SO_REUSEPORT_LB` — kernel typically routes new connections to the most-recent listener, NOT hash-distributed. Apple's CoreFoundation/dispatch are officially fork-unsafe. |
| FreeBSD  | ✅ | ✅ | Wires both `SO_REUSEPORT` + `SO_REUSEPORT_LB` (kernel hash-LB, same shape as Linux). |
| Windows  | ✅ | ❌ | No `fork(2)`, no `SO_REUSEPORT` equivalent. N≥2 returns ArgumentError. |

**Vm state across fork**: class defs, method tables, constants, and
host fn closures inherit via copy-on-write. File descriptors opened
pre-fork ARE shared kernel FDs — DB connections, logfile handles
etc. MUST be closed and reopened in `on_worker_boot` (same discipline
as Puma's `on_worker_boot`). Globals are cleared between requests
by the per-request reset; persistent worker state should use class
instance variables.

**Supervisor env vars** (Stage 7d):

- `RUBYRS_PREFORK_MAX_RESTARTS` — N restarts allowed inside the
  crash-loop window before the supervisor halts (default 5).
- `RUBYRS_PREFORK_RESTART_WINDOW_SECS` — sliding window for the
  restart count (default 60). Restarts older than this are pruned.

A child that crashes on `on_worker_boot` triggers a restart; if
the same boot path keeps failing, the guard prevents fork-bombing.
Defaults are conservative — production should leave them alone
unless a known-good upstream regression needs a workaround.

Build with: `cargo build --features _http_server -p rubyrs`. The
feature adds ~12-18 MB stripped to the binary; off by default per
ADR 0019 v3 Rule 3.

### Streaming responses (SSE, long-poll, large files)

By default `_http_server` collects the Rack body before sending the
response — fine for HTML, JSON, and other one-shot payloads, but
useless for Server-Sent Events, long-poll, or any open-ended
generator (chunks would batch into a single end-of-body write).

Combining `_http_server` with the `_fiber` feature unlocks true
async streaming: each `yield` from a Rack 3 `each`-shape body — or
each `stream.write` from a `call`-shape body — becomes one
HTTP/1.1 chunked frame, flushed to the socket before the next chunk
is produced. The full design and a phased correctness argument live
in [docs/adr/0023-true-async-streaming.md](docs/adr/0023-true-async-streaming.md).

```ruby
class SSEStream
  def each
    10.times { |i| yield "data: tick #{i}\n\n" }
  end
  def close
    # Rack 3 SPEC: rubyrs invokes close exactly once
    # after the stream completes, on both paths.
  end
end

app = ->(env) {
  [200,
   {"Content-Type" => "text/event-stream", "Cache-Control" => "no-cache"},
   SSEStream.new]
}
__rubyrs_http_serve_with_app("127.0.0.1:9292", 60, app)
```

Run [crates/rubyrs/examples/sse_server.rb](crates/rubyrs/examples/sse_server.rb)
and connect with `curl -N` to watch each event arrive as its own
chunked frame.

**Detection order** (Rack 3 SPEC `Array → each → call → to_a`):

| Body shape | `_fiber` off | `_fiber` on |
|------------|--------------|-------------|
| `Array<String>` | buffered (fast path) | buffered (fast path — Array bypasses Fiber) |
| responds to `each` | buffered (P2b.1 each-helper) | **streaming Fiber** |
| responds to `call` | buffered (P2b.1 call-helper) | **streaming Fiber** |
| responds to `to_a` | buffered | buffered |

Build with: `cargo build --features _http_server,_fiber -p rubyrs`.
The `_fiber` feature is independently useful (Ruby `Fiber.new` /
`Fiber.yield` / `Fiber#resume` from ADR 0017 Tier 2); enabling it
with `_http_server` simply opts the streaming path in automatically.

## Status

Experimental. See [docs/SUBSET.md](docs/SUBSET.md) for what works today
and [docs/ROADMAP.md](docs/ROADMAP.md) for what's next. The testing
strategy — including our plan to ingest `ruby/spec` as the quality bar —
is described in [docs/TESTING.md](docs/TESTING.md).

### Subset coverage (gapscan)

A second binary in this workspace, `rubyrs-gapscan`, scans a Ruby
codebase and classifies every AST node as supported, supported-via-
rides-along, or missing. Used as a quantitative quality bar against
real Ruby corpora. Running it against the in-tree Brewfile demo
(`crates/rubyrs/examples/brewfile/`) gives the canonical
"is the niche we claim to serve actually served?" number:

```
$ cargo run --release --bin rubyrs-gapscan -- scan crates/rubyrs/examples/brewfile
Files scanned: 2
Total AST nodes: 277
  Supported:        195 (70.40%)
  RidesAlong:        68 (24.55%)
  Missing:           14 (5.05%)

Missing node classes:
  GlobalVariableReadNode    10  ($taps)
  GlobalVariableWriteNode    4  ($taps = [])
```

The "missing" 5% is two related nodes — global variables, used only
by the DSL host code (the Brewfile script body itself is 100%
supported). The CI workflow `gapscan-pr.yml` runs this against
representative corpora on every PR and posts a diff comment so
regressions land visibly.

## Docs

- [docs/SUBSET.md](docs/SUBSET.md) — supported and unsupported semantics
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the runtime works
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — performance numbers + how
  to reproduce
- [docs/TESTING.md](docs/TESTING.md) — testing strategy and `ruby/spec`
  ingestion
- [docs/ROADMAP.md](docs/ROADMAP.md) — what's next and why
- [docs/SECURITY.md](docs/SECURITY.md) — trust model, resource
  caps, and known attack surface
- [docs/PANIC_AUDIT.md](docs/PANIC_AUDIT.md) — inventory of every
  `panic!` / `unwrap` / `expect` and how the CI ratchet works
- [docs/adr/](docs/adr/) — Architecture Decision Records
- [CONTRIBUTING.md](CONTRIBUTING.md) — PR flow

## License

Dual-licensed under either of

- MIT License ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)

at your option.
