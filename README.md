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
[![Supply-chain](https://github.com/linyiru/rubyrs/actions/workflows/cargo-deny.yml/badge.svg)](https://github.com/linyiru/rubyrs/actions/workflows/cargo-deny.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.95+-orange.svg)](https://www.rust-lang.org/)
[![Status: experimental](https://img.shields.io/badge/status-experimental-yellow)](docs/SUBSET.md)

A Ruby implementation in Rust, built on
[Prism](https://github.com/ruby/prism) (Ruby's official parser),
that runs **real, unmodified gems** — validated by differential
testing against CRuby.

The flagship proof: rubyrs builds real [Jekyll](https://jekyllrb.com)
4.4.1 sites — the actual gem sources, with real
[rouge](https://github.com/rouge-ruby/rouge) 4.7.0 syntax
highlighting, kramdown markdown, and Liquid templates — producing
output **byte-identical to CRuby's**, and faster:

| Jekyll 4.4.1, 1000-post site | rubyrs | CRuby 3.4 |
|------------------------------|--------|-----------|
| Build (posts + rouge + kramdown) | **0.59 s** | 0.70 s |
| Build (with Liquid layouts)      | **0.75 s** | 0.75 s |
| Peak RSS                         | **68 MB**  | 80 MB |
| Output                           | byte-identical | (reference) |

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

**Honesty up front**: rubyrs is not a complete Ruby. There is no
Encoding system (strings are bytes + UTF-8 assumptions), `freeze`
doesn't freeze, Thread is a stub, and ~25 documented divergences
remain — see [docs/SUBSET.md](docs/SUBSET.md) for the precise
boundary, starting with its at-a-glance table. The claim we *do*
make is narrower and verifiable: **for the surface rubyrs covers,
behaviour is pinned to CRuby 3.4 by 585 differential fixtures**
(every fixture runs on both engines; stdout must match exactly,
including under GC stress), and that surface is now wide enough to
run one of Ruby's most-used real-world applications byte-for-byte.

## Positioning

**vs CRuby** — CRuby is the reference implementation and rubyrs
treats it as ground truth: the test suite's oracle IS CRuby
(`tests/diff/`, 585 fixtures, stdout compared byte-for-byte). Where
rubyrs covers a feature, it aims for exact parity — divergences are
bugs or documented trade-offs, never silent. Where it doesn't
(Encoding, real threads, Marshal, `ObjectSpace`, …), it says so in
[docs/SUBSET.md](docs/SUBSET.md). On performance: rubyrs wins on
real Jekyll builds (table above) thanks to native accelerator
batteries (rouge/kramdown/YAML/Liquid/JSON engines in Rust behind a
"byte-identical or decline to pure Ruby" contract); on pure
VM-dispatch microbenchmarks CRuby is still ~1.4-3× faster — both
numbers live in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

**vs mruby** — mruby trades the CRuby gem ecosystem away for
embeddability (its own mrbgems world, no rubygems compatibility).
rubyrs makes the opposite bet: keep the ecosystem — `require` loads
real gem sources from a `$LOAD_PATH` (Jekyll, rouge, kramdown,
Liquid, and parts of Sinatra run today), and a CRuby-shaped C
extension ABI hosts real native gems (msgpack, bcrypt) — while
still being a small, memory-safe, embeddable Rust crate with
capability sandboxing, per-run resource caps, and a
WebAssembly target.

| | CRuby | mruby | rubyrs |
|---|---|---|---|
| Real rubygems sources | ✅ all | ❌ (mrbgems) | ✅ growing (Jekyll-class today) |
| Embedding | C API | C, mature | Rust crate; caps, sandbox, WASM |
| Memory safety | C | C | Rust; linear-time regex by default (ReDoS-immune) |
| Encoding / threads | full | reduced | not yet (documented) |
| Jekyll 1k-post build | 0.70 s | — | **0.59 s, byte-identical** |

Where the cold-start + footprint profile matters (CLI tools, DSL
hosts, sandboxed script execution), rubyrs starts ~50× faster than
CRuby and holds a fraction of the RSS:

| Cold start | rubyrs (native) | rubyrs.cwasm (AOT + wizer) | CRuby 3.4 |
|------------|----------------|----------------------------|-----------|
| `puts 1+2` | **1.5 ms** | ~7 ms | 78 ms |

| End-to-end DSL hosting (Brewfile, ~50 lines) | rubyrs | CRuby 3.4 |
|----------------------------------------------|--------|-----------|
| Time | **1.8 ms** | 74.7 ms |

### What works with `require`

`require` resolves real gem sources: point `$LOAD_PATH` at unpacked
gem `lib/` directories (what Bundler does under the hood) and the
require chain loads them — Jekyll's full chain (jekyll → kramdown →
liquid → rouge → pathutil → addressable → …) loads and runs today.
Alongside that:

- `require "json"` / `yaml` / `set` / `pathname` / `stringio` /
  `strscan` / `digest` / `logger` / `cgi` / `bigdecimal` / ~25 more
  resolve to **vendored stdlib implementations** (with
  `--features stdlib`), behaviour pinned by the same differential
  fixtures.
- `require "msgpack"` / `bcrypt`-class **native gems** load through
  the CRuby-shaped C extension ABI (`--features cext`, on by
  default).
- Five **accelerator batteries** transparently take over hot paths
  when enabled (`_json_native`, `_rouge_native`, `_kramdown_native`,
  `_yaml_native`, `_liquid_native`): each is a Rust engine behind a
  right-or-decline contract — produce byte-identical output or fall
  back to the pure-Ruby path. This is how Jekyll gets faster than
  CRuby without sacrificing the byte-identity guarantee.
- `autoload`, `Kernel#load`, `require_relative` work; `$LOAD_PATH`
  starts empty by design (embedders/scripts populate it — CRuby
  auto-fills stdlib + gem paths, rubyrs does not).

What does NOT work yet: anything needing the Encoding system, real
Thread concurrency, Marshal, or the other gaps catalogued in
[docs/SUBSET.md](docs/SUBSET.md). Gems relying on those will fail —
loudly, not silently wrong.

## Install

### As a library

Depend on the git repository directly — `master` is kept green by
the full CI gate (differential fixtures, GC-stress, coverage /
panic / RSS ratchets) on every commit:

```toml
[dependencies]
rubyrs = { git = "https://github.com/linyiru/rubyrs" }
```

History note: the crates.io entries (`rubyrs@0.1.0`,
`rubyrs-cext@0.1.0`, published 2026-05-25) are name-registration
placeholders from before the Jekyll-era work, and the
[`v0.1.0` git tag](https://github.com/linyiru/rubyrs/releases/tag/v0.1.0)
predates it too (263 fixtures vs today's 585). The next tagged
release will be the first one published to crates.io as a real
artifact; until then, git `master` is the source of truth. The
sibling engine crates extracted from this work ARE current on
crates.io: [carmine](https://crates.io/crates/carmine)
(rouge-compatible highlighting),
[rostdown](https://crates.io/crates/rostdown)
(kramdown-compatible markdown), and
[liquidus](https://crates.io/crates/liquidus) (Liquid templates).

### CLI from source

```bash
git clone https://github.com/linyiru/rubyrs
cd rubyrs
cargo build --release
./target/release/rubyrs your_script.rb
```

For the full Jekyll-capable build (accelerators + stdlib + sass +
mimalloc — what the benchmark table at the top measures):

```bash
cargo build --release -p rubyrs \
  --features stdlib,sass,_rouge_native,_kramdown_native,_yaml_native,_liquid_native,mimalloc
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
