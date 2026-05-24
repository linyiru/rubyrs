# rubyrs

A tiny Ruby-subset interpreter written in Rust, built on top of [Prism](https://github.com/ruby/prism).

**Status:** experimental PoC. Not a Ruby implementation. Not a CRuby replacement.

## Positioning

rubyrs is **not** trying to be Rails-compatible or even close to CRuby in
coverage. The deliberate target is the same niche as **mruby**: a small,
memory-safe, embeddable Ruby-flavored runtime — but written in Rust instead of
C, with the option of compiling to WebAssembly.

| Implementation | Target use | Compat with MRI | Language |
|----------------|------------|-----------------|----------|
| CRuby (MRI)    | General-purpose Ruby | 100% (reference) | C |
| JRuby / TruffleRuby | Server-side Ruby on JVM | High (slow tail) | Java |
| mruby          | Embedded scripting | Subset, no Rails | C |
| **rubyrs**     | **Embedded / WASM scripting** | **Tiny subset** | **Rust** |

If you need Rails, Sinatra, Bundler, gems, or `eval` — use CRuby.

## Supported language features

- Integer (i64), String, true / false, nil
- Local variables, instance variables (`@x`)
- `if / elsif / else`, `while`
- `def` (top-level and instance methods)
- `class` with `initialize` and instance methods
- `self`, implicit-self method calls
- Builtins: `puts`, `print`
- Primitive methods: arithmetic (`+ - * / %`), comparisons (`== != < <= > >=`),
  `Integer#to_s`, `String#+`, `String#==`, `String#length`, `String#to_s`

That's it.

## Not supported (explicitly)

These are out of scope and **will not be added** unless the scope changes:

- Modules, mixins, inheritance, `super`
- Blocks, `yield`, Proc, lambda
- Arrays, Hashes, Ranges, Symbols
- String interpolation `"#{x}"`, regex, heredocs
- Exception handling (`raise / rescue / ensure / retry`)
- `return / break / next / redo`
- Float, Rational, Complex, BigDecimal
- `require / load / autoload`, gems, Bundler
- Fiber, Thread, Mutex, Ractor
- `eval`, `define_method`, `method_missing`, `ObjectSpace`
- File / Socket / IO beyond stdout
- Frozen strings, encodings beyond UTF-8 view
- Refinements, pattern matching, keyword args, default args

## Build & run

Requires Rust (any recent stable) and a C compiler for the vendored Prism parser.

```bash
cargo build --release
./target/release/rubyrs your_script.rb
```

Environment flags:

- `DEBUG_AST=1` — print the translated IR before running
- `DEBUG_BC=1` — print the compiled bytecode for every proto
- `GC_STATS=1` — print final heap stats

## WebAssembly target

```bash
# One-time setup
rustup target add wasm32-wasip1
# Download wasi-sdk 24 to a local dir, then:
export WASI_SDK_PATH=/path/to/wasi-sdk-24.0-arm64-macos
cargo build --release --target wasm32-wasip1

wasmtime run --dir=. target/wasm32-wasip1/release/rubyrs.wasm script.rb
```

Notes:
- Tested with wasi-sdk 24. Newer SDKs may need different stubs for
  `__wasi_init_tp` (provided by `build.rs`).
- Resulting `.wasm` is ~640 KB (gzipped: smaller).

## Numbers (M-series mac, release builds)

### Cold start (`puts 1+2`)

| Implementation | Time |
|----------------|------|
| rubyrs (native) | 1.5 ms |
| rubyrs.wasm + wasmtime | 12.7 ms |
| CRuby 3.4 | 78 ms |
| CRuby 3.4 + YJIT | 78 ms |

### Throughput (1M fizzbuzz with `acc += s.length`)

| Implementation | Time | Peak memory |
|----------------|------|------------|
| rubyrs tree-walker (removed) | 1.50 s | 1.4 MB |
| rubyrs bytecode VM | 0.67 s | 1.3 MB |
| rubyrs.wasm + wasmtime | 0.86 s | 16.7 MB |
| CRuby 3.4 | 0.19 s | 10.6 MB |
| CRuby 3.4 + YJIT | 0.15 s | 10.8 MB |

### GC: cycle-leak regression (200k Ruby cycles)

| Implementation | Peak memory |
|----------------|------------|
| rubyrs without GC (Rc only) | 117 MB (leaks) |
| rubyrs with mark-sweep | 2.4 MB |
| CRuby 3.4 | 10.6 MB |

Tests scaled to 2M cycles still cap at 2.4 MB.

## Architecture

```
.rb source
  │
  ▼  Prism (C, FFI'd via ruby-prism crate)
Prism AST  (Node<'pr>)
  │
  ▼  tr()  — single-pass translation, drops lifetime to source
Expr IR  (owned, Clone)
  │
  ▼  compile_proto() / compile_expr()
Vec<Proto>, each with Vec<Op>
  │
  ▼  Vm::run() — switch dispatch over Op
Output

GC: Instance objects live in Vm.heap (Slot::Live | Slot::Dead).
On Class.new past the heap threshold, a stop-the-world mark phase walks
roots from the operand stack + every Frame, then sweeps unmarked slots
into the free list. Class / Method / String stay on Rc — they can't form
cycles in the current language subset.
```

## What's not here (yet)

In rough order of usefulness for the target niche:

1. **Arrays + Hashes** — needed for any non-trivial script
2. **String interpolation + Symbol** — quality-of-life for any real DSL
3. **Blocks + yield** — gateway to Enumerable, the heart of "Ruby feel"
4. **Exceptions** — currently all errors `panic!`; needs `Result<Value, ControlFlow>`
5. **Inline caches for method dispatch** — main remaining gap vs CRuby interpreter

Items 1–3 are scope-decisions, not technical blockers. Item 4 is a refactor.
Item 5 is the path to closing the remaining 3.5× speed gap to CRuby.

## Why this exists

This started as a one-day PoC to test whether Prism + Rust could deliver a
minimal Ruby runtime in less time than the canonical "write a Ruby in Rust"
attempt (Artichoke, archived 2025-11 after multiple years).

The PoC reached FizzBuzz + classes in under an hour. The follow-up work in
this repo (bytecode VM, mark-sweep GC, WASM target) explores whether the
**cold start + memory** numbers are competitive enough for the
"Ruby on the edge" niche to be worth pursuing. The current numbers say yes;
the next gating question is whether Arrays, blocks, and exceptions can be
added without bloating the runtime past the point where mruby is still a
better choice.

## License

Dual-licensed under either of

- MIT License ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)

at your option.
