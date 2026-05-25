# rubyrs

[![CI](https://github.com/linyiru/rubyrs/actions/workflows/ci.yml/badge.svg)](https://github.com/linyiru/rubyrs/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Status: experimental](https://img.shields.io/badge/status-experimental-yellow)](https://github.com/linyiru/rubyrs/blob/master/docs/SUBSET.md)

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
Ruby-flavoured runtime — but written in Rust, with the option of compiling
to WebAssembly.

| End-to-end DSL hosting (Brewfile, ~50 lines) | rubyrs | CRuby 3.4 | CRuby + YJIT |
|----------------------------------------------|--------|-----------|--------------|
| Time | **1.8 ms** | 74.7 ms | 75.5 ms |

→ **rubyrs is 42× faster end-to-end** on this shape of workload.

| Cold start | rubyrs (native) | rubyrs.wasm | CRuby 3.4 |
|------------|-----------------|-------------|-----------|
| `puts 1+2` | **1.5 ms** | 12.7 ms | 78 ms |

If you need Rails, Sinatra, Bundler, or gems — use CRuby.

## Install

```bash
# CLI
cargo install rubyrs

# As a library in your Cargo.toml
cargo add rubyrs
```

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

rt.eval(r#"puts "pid is #{host_pid}""#, "inline").unwrap();
```

The runtime is incremental — class and method definitions persist across
`eval` calls, so you can split DSL setup and script execution into
multiple chunks.

## Resource caps (untrusted scripts)

Per-run caps; any cap that trips returns a `ResourceExhausted` trap
with a normal backtrace (no host panic):

```bash
RUBYRS_FUEL=1000000 \
RUBYRS_MAX_OBJECTS=10000 \
RUBYRS_MAX_FRAMES=128 \
  ./target/release/rubyrs script.rb
```

## Status

Experimental. The supported subset is documented at
[`docs/SUBSET.md`](https://github.com/linyiru/rubyrs/blob/master/docs/SUBSET.md);
the roadmap at
[`docs/ROADMAP.md`](https://github.com/linyiru/rubyrs/blob/master/docs/ROADMAP.md);
the architecture at
[`docs/ARCHITECTURE.md`](https://github.com/linyiru/rubyrs/blob/master/docs/ARCHITECTURE.md).

The companion crate
[`rubyrs-cext`](https://crates.io/crates/rubyrs-cext) implements the
spike-level CRuby-shape C ABI used to host C extensions, and
[`rubund`](https://crates.io/crates/rubund) is an in-tree Rust
implementation of Bundler that dogfoods rubyrs's embedding API.

## License

Dual-licensed under either of

- MIT License ([LICENSE-MIT](https://github.com/linyiru/rubyrs/blob/master/LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/linyiru/rubyrs/blob/master/LICENSE-APACHE))

at your option.
