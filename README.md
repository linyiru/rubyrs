# rubyrs

[![CI](https://github.com/linyiru/rubyrs/actions/workflows/ci.yml/badge.svg)](https://github.com/linyiru/rubyrs/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org/)
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

| Cold start | rubyrs (native) | rubyrs.wasm | CRuby 3.4 |
|------------|----------------|-------------|-----------|
| `puts 1+2` | **1.5 ms** | 12.7 ms | 78 ms |

| 1M fizzbuzz | rubyrs | CRuby | CRuby + YJIT |
|-------------|--------|-------|--------------|
| Time | 0.44 s | 0.19 s | 0.15 s |
| Peak memory | 2.1 MB | 18.4 MB | 19.1 MB |

If you need Rails, Sinatra, Bundler, or gems — use CRuby.

## Build

```bash
cargo build --release
./target/release/rubyrs your_script.rb
```

For the `wasm32-wasip1` build and other details, see
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Embedding

rubyrs is also a Rust crate: drop it into a `Cargo.toml`, build a
`Runtime`, and run scripts in process.

```rust
use rubyrs::{Runtime, Value};

let mut rt = Runtime::new();

// Expose a host function to the Ruby side.
rt.register_fn("host_pid", |_args| {
    Ok(Value::Int(std::process::id() as i64))
});

// Capture stdout into your own sink (optional; defaults to process stdout).
// rt.set_stdout(Box::new(my_writer));

rt.eval(r#"puts "pid is #{host_pid}""#, "inline").unwrap();
```

The runtime is incremental — class and method definitions persist across
`eval` calls, so you can split your DSL setup and script execution into
multiple chunks. See [`examples/embed.rs`](examples/embed.rs) for the
fuller story (captured stdout, persistent classes, Trap propagation).

Run the example:

```bash
cargo run --release --example embed
```

## Status

Experimental. See [docs/SUBSET.md](docs/SUBSET.md) for what works today
and [docs/ROADMAP.md](docs/ROADMAP.md) for what's next. The testing
strategy — including our plan to ingest `ruby/spec` as the quality bar —
is described in [docs/TESTING.md](docs/TESTING.md).

## Docs

- [docs/SUBSET.md](docs/SUBSET.md) — supported and unsupported semantics
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the runtime works
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — performance numbers + how
  to reproduce
- [docs/TESTING.md](docs/TESTING.md) — testing strategy and `ruby/spec`
  ingestion
- [docs/ROADMAP.md](docs/ROADMAP.md) — what's next and why
- [docs/adr/](docs/adr/) — Architecture Decision Records
- [CONTRIBUTING.md](CONTRIBUTING.md) — PR flow

## License

Dual-licensed under either of

- MIT License ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)

at your option.
