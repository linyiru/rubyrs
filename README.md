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
