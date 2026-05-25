# Development

## Prerequisites

- Rust 1.95+ (stable). Newer is fine. Earlier may work but is untested.
- A C compiler (clang/gcc). Required by `ruby-prism-sys` to build the
  vendored Prism parser.

## Build and run

```bash
cargo build --release
./target/release/rubyrs path/to/script.rb
```

Debug + safety flags via environment variables:

| Var | Effect |
|-----|--------|
| `DEBUG_AST=1` | Print the translated `Expr` IR before execution |
| `DEBUG_BC=1` | Print compiled bytecode (every Proto, every Op) |
| `GC_STATS=1` | Print final heap stats on exit |
| `STRESS_GC=1` | Collect on every potential GC point (debug / regression) |
| `RUBYRS_FUEL=N` | Trap as `ResourceExhausted` after `N` ops dispatched |
| `RUBYRS_MAX_OBJECTS=N` | Trap when live heap objects exceed `N` |
| `RUBYRS_MAX_FRAMES=N` | Trap when frame stack depth exceeds `N` |

The four `RUBYRS_*` variables are useful when running untrusted scripts
from the CLI; they are the same knobs exposed via [`Config`](#embedding)
to library users.

## Tests

```bash
cargo test --release
```

To add a new fixture:

```bash
echo 'puts 42' > tests/fixtures/example.rb
UPDATE_EXPECTED=1 cargo test --release example  # generates .expected
# Inspect generated file, commit both .rb and .expected.
```

Then register the test in `tests/integration.rs`:

```rust
#[test] fn example() { run_fixture("example"); }
```

## WebAssembly target

One-time setup:

```bash
rustup target add wasm32-wasip1

# wasi-sdk 24 (other versions may work; 25 has a thread-init shim issue).
# arm64 macOS shown; pick the right asset for your host:
curl -L https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-24/wasi-sdk-24.0-arm64-macos.tar.gz \
  | tar -xz -C /opt
```

Build:

```bash
export WASI_SDK_PATH=/opt/wasi-sdk-24.0-arm64-macos
cargo build --release --target wasm32-wasip1
```

Run (needs wasmtime or equivalent):

```bash
wasmtime run --dir=. target/wasm32-wasip1/release/rubyrs.wasm script.rb
```

Notes:
- The `build.rs` ships a tiny `__wasi_init_tp` no-op stub so Rust std's
  threading init resolves at link time.
- The resulting `.wasm` is ~650 KB; cold start under wasmtime is ~12 ms
  on Apple Silicon.

## Profiling

```bash
# Cycle counts and peak memory:
/usr/bin/time -lp ./target/release/rubyrs script.rb

# Wall-clock comparisons:
hyperfine --warmup 2 \
  './target/release/rubyrs script.rb' \
  'ruby --disable=yjit script.rb' \
  'ruby --yjit script.rb'
```

See [BENCHMARKS.md](BENCHMARKS.md) for the canonical numbers and
methodology.

## Embedding

rubyrs ships as both a binary and a library. See
[ARCHITECTURE.md § Public embedding API](ARCHITECTURE.md#public-embedding-api)
for the surface, [`examples/embed.rs`](../examples/embed.rs) for a
worked example, and `tests/embed.rs` for the pinned semantics.

Add to `Cargo.toml`:

```toml
[dependencies]
rubyrs = "0.1"
```

Use:

```rust
use rubyrs::{Runtime, Config, Value};

let mut rt = Runtime::with_config(Config {
    fuel: Some(1_000_000),
    max_heap_objects: Some(10_000),
    max_frames: Some(128),
    ..Default::default()
});
rt.register_fn("now_ms", |_| Ok(Value::Int(/* ... */ 0)));
rt.eval(r#"puts "ok at #{now_ms}""#, "snippet")?;
```

## Project layout

```
rubyrs/
├── Cargo.toml
├── build.rs                  # WASI stub linker shim
├── src/
│   ├── lib.rs                # Public API (Runtime, Config, re-exports)
│   ├── main.rs               # CLI shim around Runtime
│   ├── ast.rs                # Expr IR + Prism→Expr translation
│   ├── value.rs              # Value enum + heap-object structs
│   ├── intern.rs             # SymId + Interner
│   ├── heap.rs               # Mark-sweep GC heap
│   ├── bytecode.rs           # Op + Proto
│   ├── compiler.rs           # Expr → bytecode
│   ├── error.rs              # Span, RubyError, Trap
│   └── vm.rs                 # Vm, dispatch, builtins, host fn dispatch
├── examples/
│   └── embed.rs              # Worked embedding example
├── tests/
│   ├── integration.rs        # Fixture-based golden harness (stdout)
│   ├── embed.rs              # Public API smoke tests
│   └── fixtures/             # .rb + .expected pairs, plus errors/
├── docs/
│   ├── ARCHITECTURE.md       # How it works internally
│   ├── ROADMAP.md            # What's done + what's next
│   ├── TESTING.md            # ruby/spec ingestion strategy
│   ├── SUBSET.md             # What we do / don't support
│   ├── BENCHMARKS.md         # Performance numbers + how to reproduce
│   ├── DEVELOPMENT.md        # This file
│   └── adr/                  # Architecture Decision Records (8 so far)
├── README.md                 # Drive-by visitor pitch
├── CHANGELOG.md              # Per-version user-facing changes
└── CONTRIBUTING.md           # PR flow
```

## Common pitfalls

- **`error: ... __wasi_init_tp ...`** when running the WASM binary —
  this is the threading shim. Make sure `build.rs` compiled it. A clean
  rebuild (`cargo clean --target wasm32-wasip1`) typically fixes.
- **Prism build slow on first `cargo build`** — it's vendored C. Subsequent
  builds are cached.
- **`cargo fmt` would touch a lot** — we use a deliberately compact style
  for single-arm matches and short tests. `rustfmt` is not enforced in CI.
