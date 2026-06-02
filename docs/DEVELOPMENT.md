# Development

## Prerequisites

- Rust 1.95+ (stable). Newer is fine. Earlier may work but is untested.
- A C compiler (clang/gcc). Required by `ruby-prism-sys` to build the
  vendored Prism parser.

## Workspace layout

rubyrs is a Cargo workspace with four crates under `crates/`:

| Crate | Role |
|---|---|
| `crates/rubyrs` | Core interpreter — parser bridge, compiler, VM, embedding API |
| `crates/rubyrs-cext` | C ABI bridge — `rb_*` FFI entry points C extensions call |
| `crates/rubund` | Bundler/Gemfile-aware runner (DSL hosting demo) |
| `crates/rubyrs-gapscan` | Subset-coverage scanner over real Ruby corpora |

`cargo build` from the repository root builds the whole workspace.
The CLI binary `rubyrs` lives in `crates/rubyrs` and lands at
`target/release/rubyrs`.

## Build and run

```bash
cargo build --release
./target/release/rubyrs path/to/script.rb
```

The release profile pins `lto = "thin"` (see `Cargo.toml`) to keep
cross-module inlining alive after the `vm/*.rs` split — see
[BENCHMARKS.md](BENCHMARKS.md) for the regression-and-recovery
record. Adds ~3s to release-build wall time; dev/test builds
unaffected.

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
cargo test --release            # whole workspace
cargo test --release --test diff_cruby  # the byte-compare suite vs CRuby
```

The `diff_cruby` harness runs each `crates/rubyrs/tests/diff/*.rb`
file through both rubyrs and the system `ruby` binary and asserts
stdout matches byte-for-byte. Currently 79 fixtures; every PR is
gated on it staying green.

To add a new diff fixture:

```bash
echo 'puts 42' > crates/rubyrs/tests/diff/example.rb
cargo test --release --test diff_cruby example
```

The harness auto-discovers `.rb` files; no separate registration
step.

For the older fixture/expected style (`crates/rubyrs/tests/fixtures/`):

```bash
echo 'puts 42' > crates/rubyrs/tests/fixtures/example.rb
UPDATE_EXPECTED=1 cargo test --release example
```

Register in `crates/rubyrs/tests/integration.rs`:

```rust
#[test] fn example() { run_fixture("example"); }
```

The embedding-API surface is pinned by `crates/rubyrs/tests/embed.rs`.

## CI gates

- **`diff_cruby`** — 79 fixtures, byte-identical stdout to CRuby.
- **`panic-budget`** — counts `panic!` / `unwrap` / `expect` per
  file; one-way ratchet down. Bumps require an explicit comment
  in `docs/PANIC_AUDIT.md`.
- **`perf/check.sh`** — peak-RSS + wall-time ratchet over
  `perf/baselines.tsv`. Run locally with `bash perf/check.sh`.
- **`STRESS_GC=1`** — second test job collects on every GC point.
- **`gapscan`** — per-PR diff comment summarising subset-coverage
  changes against real Ruby corpora (via the GitHub Actions
  workflow).
- **`cargo-deny`** — supply-chain gate: CVEs (RustSec advisory
  DB), license-policy violations, banned-crate enforcement, and
  source-registry pinning. Config at workspace-root `deny.toml`;
  workflow at [`.github/workflows/cargo-deny.yml`](../.github/workflows/cargo-deny.yml)
  (extracted from `ci.yml` so a `paths:` filter can skip the gate
  on docs-only / Ruby-source-only PRs). A weekly cron still runs
  on Sundays so an advisory-DB update against a frozen
  `Cargo.lock` doesn't go unnoticed. Run locally with
  `cargo deny check` (after
  `cargo install cargo-deny --locked --version 0.19.8`). Bumping
  the cargo-deny pin or adding a license/exception is a
  deliberate commit; the new ruleset must pass locally before
  push.

## Clippy

The workspace is currently at zero clippy warnings:

```bash
cargo clippy --release --all -- -D warnings
```

`#[allow(clippy::xxx)]` annotations carry a rationale comment.
The mechanical-fixes pass uses `cargo clippy --fix --allow-dirty`.

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
# `cext` requires a dynamic loader (libloading + dlopen) that
# wasm32-wasi does not provide, so the `cext` feature is incompatible
# with this target. Build with `--no-default-features` for the
# cext-off subset (the only meaningful wasm32-wasi shape today);
# omitting it triggers a clear `build.rs` panic per ADR 0015.
cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features
```

Run (needs wasmtime or equivalent):

```bash
wasmtime run --dir=. target/wasm32-wasip1/release/rubyrs.wasm script.rb
```

End-to-end smoke (build + run + stdout diff against the `smoke.expected`
golden, which is generated from CRuby):

```bash
export WASI_SDK_PATH=/opt/wasi-sdk-24.0-arm64-macos   # adjust path
bash crates/rubyrs/tests/wasm/smoke.sh
```

Diff matrix (runs a curated subset of `crates/rubyrs/tests/diff/*.rb` fixtures
under both `ruby` and `rubyrs.wasm`, asserts byte-identical stdout —
catches native-vs-wasi behaviour drift the smoke fixture can't
surface alone). Needs CRuby on PATH:

```bash
bash crates/rubyrs/tests/wasm/diff_matrix.sh
```

Manifest at `crates/rubyrs/tests/wasm/diff_manifest.txt` —
add a fixture name (without `.rb`) to expand coverage; the file
header documents selection rules. Both scripts run in the `wasm`
CI lane after the build step.

Perf gate (wasm-specific wall-time ratchet — sibling to the
native `perf/check.sh`):

```bash
bash perf/wasm_check.sh
```

The script builds an optimised AOT artifact before timing:

  raw `.wasm`  →  `wasm-opt -Oz`  →  `wizer`  →  `wasm-opt -Oz`  →  `wasmtime compile`  →  `.cwasm`

and runs every workload via `wasmtime run --allow-precompiled`.

`wasmtime compile` is the REQUIRED baseline of the gate — the
script always runs it because the gate measures the AOT-cwasm
path. `wasm-opt -Oz` and `wizer` are OPTIONAL layers stacked
on top of that baseline; each one's contribution is independent
and the gate falls back gracefully when either tool is missing.

Local M-series numbers, with the AOT-cwasm path as the
baseline (so every row already includes `wasmtime compile`):

  - raw `.wasm` + JIT-each-run (no AOT):  ~20 ms cold start
  - AOT cwasm baseline:                    ~8.6 ms
  - + `wasm-opt -Oz`:                      ~7.6 ms (binary
                                            1.48 → 1.17 MB
                                            for the .wasm
                                            input)
  - + `wizer` (on top of -Oz):             ~7.2 ms (~5%
                                            relative at this
                                            scale; cwasm
                                            4.4 → 4.6 MB +5%
                                            from the baked-in
                                            preamble heap)

`startup_floor.rb` total ~7-10 ms; `fizzbuzz_1m.rb` ~510 ms
(unchanged across pipeline variants — compute-bound, not
startup-bound).

Install dependencies:

  - `wasmtime` — REQUIRED (the gate measures the cwasm path).
  - `binaryen` (`brew install binaryen` / `apt install binaryen`)
    — OPTIONAL, enables `wasm-opt -Oz`.
  - `wizer` (`cargo install wizer --features 'env_logger structopt'`
    or the GitHub release tarball; CI uses the latter) — OPTIONAL,
    enables Runtime pre-initialization. The binary exports
    `wizer.initialize`, which wizer calls and snapshots; main()
    then picks up the pre-built Runtime via
    `rubyrs::take_wizer_runtime()` and applies the host Config
    on top.

Skipping either optional tool keeps the gate green; you just lose
that layer's contribution. The CI lane installs both.

### wasi env-var quirk

Rust's `std::env::vars()` on `wasm32-wasip1` reads from a libc
global (`__environ`) populated during C runtime init. On
wizer'd builds, `wizer.initialize` snapshots linear memory
BEFORE wasi-libc populates `__environ` with the user-provided
env, so `env::vars()` returns empty even when wasmtime is
invoked with `--env=KEY=VAL`. `crates/rubyrs/src/main.rs`
sidesteps the cache by calling `wasi_snapshot_preview1::
environ_get` directly via FFI (see `collect_wasi_env`),
making env propagation work identically pre- and post-wizer.

Budgets in `perf/wasm_baselines.tsv`. Same absolute-baseline
policy as the host gate: bump a budget with a comment explaining
*what grew*; never silently to make CI green.

Notes:
- The `build.rs` ships a tiny `__wasi_init_tp` no-op stub so Rust std's
  threading init resolves at link time.
- Binary shapes (Rust 1.95 / wasi-sdk 24 / wasmtime 45, Apple
  M-series local):
    - raw `.wasm` (stripped release): ~1.48 MB; wasmtime cold
      start ~200 ms first-run / ~20 ms steady
    - `wasm-opt -Oz` `.wasm`: ~1.17 MB (-21% — the shipping shape
      if you're distributing the .wasm)
    - `wasmtime compile` `.cwasm` (AOT, from optimised .wasm):
      ~4.4 MB on disk (machine code, not bytecode); cold start
      ~7-10 ms with `--allow-precompiled` (no JIT cost per
      invocation — this is what the perf gate measures). NOT a
      portable shipping artifact: wasmtime-version + host-arch
      specific; consumers must regenerate per environment.
  The size has grown vs. earlier PoC numbers as more of the Ruby
  subset landed; the Bignum / require-relative / Symbol features
  each pulled in additional code paths.
- `std::process::id()` panics on wasm32-wasip1 (wasi has no PID
  concept). `crates/rubyrs/src/main.rs` cfg-gates the call so the
  `pid` field is `None` on wasi; the runtime treats that as a
  sentinel and surfaces `$$` as `Int(0)` (see
  `vm/step.rs::"$$"`). The interpreter stays alive — only the
  `$$` value differs from a native host. wasi scripts that
  meaningfully depend on a non-zero PID need to detect the
  zero sentinel themselves.

## Profiling

```bash
# Cycle counts and peak memory:
/usr/bin/time -lp ./target/release/rubyrs script.rb

# Wall-clock comparisons:
hyperfine --warmup 2 \
  './target/release/rubyrs crates/rubyrs/benches/fizzbuzz_1m.rb' \
  'ruby --disable=yjit crates/rubyrs/benches/fizzbuzz_1m.rb' \
  'ruby --yjit crates/rubyrs/benches/fizzbuzz_1m.rb'

# CI-gated workloads:
bash perf/check.sh
```

The checked-in microbench is `crates/rubyrs/benches/fizzbuzz_1m.rb`
(used as the headline arithmetic/dispatch benchmark). See
[BENCHMARKS.md](BENCHMARKS.md) for canonical numbers and
methodology, and `perf/README.md` for the budget format.

## Embedding

rubyrs ships as both a binary and a library. See
[ARCHITECTURE.md § Public embedding API](ARCHITECTURE.md#public-embedding-api)
for the surface, [`crates/rubyrs/examples/embed.rs`](../crates/rubyrs/examples/embed.rs)
for a worked example, and `crates/rubyrs/tests/embed.rs` for the
pinned semantics.

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
├── Cargo.toml                       # workspace root (members + thin LTO)
├── crates/
│   ├── rubyrs/                      # core interpreter crate
│   │   ├── build.rs                 # WASI stub linker shim
│   │   ├── benches/
│   │   │   └── fizzbuzz_1m.rb       # checked-in microbench
│   │   ├── examples/                # brewfile DSL, cext demos, etc.
│   │   ├── src/
│   │   │   ├── lib.rs               # public API (Runtime, Config, ...)
│   │   │   ├── main.rs              # CLI shim around Runtime
│   │   │   ├── ast.rs               # Expr IR + Prism→Expr translation
│   │   │   ├── value.rs             # Value enum + heap-object structs
│   │   │   ├── intern.rs            # SymId + Interner
│   │   │   ├── heap.rs              # mark-sweep GC heap
│   │   │   ├── bytecode.rs          # Op + Proto
│   │   │   ├── compiler.rs          # Expr → bytecode
│   │   │   ├── error.rs             # Span, RubyError, Trap
│   │   │   ├── vm.rs                # Vm struct + shared scaffolding (~380 lines)
│   │   │   └── vm/                  # 17 per-type submodules — see VM_MODULE_MAP.md
│   │   │       ├── dispatch.rs      # do_call / invoke_method ...
│   │   │       ├── step.rs          # opcode interpreter loop
│   │   │       ├── cext.rs          # C ext loader + handle bridge
│   │   │       ├── iter.rs          # block-form Enumerable
│   │   │       ├── string.rs / array.rs / hash.rs / range.rs / numeric.rs
│   │   │       ├── kernel.rs / fileops.rs / raise.rs
│   │   │       ├── lookup.rs        # method resolution + class checks
│   │   │       ├── gc.rs            # GC trigger + resource caps
│   │   │       ├── primitive.rs     # typed fast-path dispatch table
│   │   │       ├── sprintf.rs
│   │   │       └── util.rs          # cross-cutting helpers
│   │   ├── spec/                    # ruby/spec subset runner
│   │   └── tests/
│   │       ├── integration.rs       # golden-stdout fixtures
│   │       ├── embed.rs             # public API smoke tests
│   │       ├── diff_cruby.rs        # 79-fixture byte-compare vs CRuby
│   │       ├── diff/                # diff_cruby fixtures (*.rb)
│   │       └── fixtures/            # legacy .rb + .expected pairs
│   ├── rubyrs-cext/                 # C ABI shims (~40 unsafe extern "C")
│   ├── rubund/                      # Bundler/Gemfile runner
│   └── rubyrs-gapscan/              # subset-coverage scanner
├── perf/
│   ├── baselines.tsv                # CI-enforced perf budget
│   ├── check.sh                     # runs each baseline workload
│   └── README.md
├── docs/
│   ├── ARCHITECTURE.md              # how it works internally
│   ├── VM_MODULE_MAP.md             # per-vm-submodule navigation guide
│   ├── CEXT_SAFETY.md               # FFI safety contracts (3 classes)
│   ├── SECURITY.md                  # trust model
│   ├── ROADMAP.md / SUBSET.md / TESTING.md / BENCHMARKS.md /
│   ├── DEVELOPMENT.md               # this file
│   ├── PANIC_AUDIT.md               # panic-budget breakdown
│   └── adr/                         # ADRs (10+ so far)
├── README.md / CHANGELOG.md / CONTRIBUTING.md / LICENSE-*
└── .github/workflows/               # ci.yml, gapscan-pr.yml, etc.
```

## Common pitfalls

- **`error: ... __wasi_init_tp ...`** when running the WASM binary —
  this is the threading shim. Make sure `build.rs` compiled it. A clean
  rebuild (`cargo clean --target wasm32-wasip1`) typically fixes.
- **Prism build slow on first `cargo build`** — it's vendored C. Subsequent
  builds are cached.
- **`cargo fmt` would touch a lot** — we use a deliberately compact style
  for single-arm matches and short tests. `rustfmt` is not enforced in CI.
