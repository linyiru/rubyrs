# rubund

> **A zero-copy `Gemfile.lock` parser in Rust — and the start of a
> Rust implementation of [Bundler](https://bundler.io/).**

Today rubund ships a real, tested `Gemfile.lock` parser as a library
crate, plus two runnable Cargo examples (`lockfile_parser` for
timing/pretty-print, `c_ext_cache` for C-extension caching). The CLI
binary is intentionally a placeholder — none of the user-facing
Bundler commands (`install`, `update`, `exec`, `lock`, `check`) are
implemented yet. `rubund --help` is currently the most useful thing
it does.

Two further proof-of-concept installers (`real_install`,
`manekineko_install`) bridge into an embedded Ruby interpreter
(`rubyrs`) to evaluate real Gemfiles end-to-end. They are
first-class Cargo examples under
[`examples/`](examples/) and run via
`cargo run --release -p rubund --example real_install` /
`--example manekineko_install -- <path/to/Gemfile>`. They depend
on the workspace-internal `rubyrs` path-dep (lifted from
`[dependencies]` so `cargo publish -p rubund` can complete
once rubyrs ships on crates.io; see the comment on rubund's
`Cargo.toml`).

The library half is what to use today. The CLI half is what's
under construction.

---

## Highlights

| What | Why it matters |
| :--- | :--- |
| **Zero-copy lockfile parser** | Parses `Gemfile.lock` by borrowing directly into the input buffer — zero heap allocations for string tokens. A 1,379-line production lockfile is parsed in **~147 µs**. |
| **Bounded parallel installer (PoC)** | The [`manekineko_install`](examples/manekineko_install.rs) example drives a 16-worker thread pool over `std::sync::mpsc` to saturate network I/O without tripping macOS file-descriptor limits. Runs under rubyrs's secure-by-default Runtime (no FS access, no `require`-walking, panic→Trap boundary). Not yet a CLI command. |
| **Single static binary** | No Ruby runtime required at the target machine. Ship one binary. |

---

## Benchmark Snapshot

Measured against the real-world **manekineko** project (196 gems, 1,379-line lockfile):

| Phase | Bundler (Ruby) | rubund (Rust) | Speedup |
| :--- | ---: | ---: | ---: |
| Lockfile parse | ~150–300 ms | **0.147 ms** | **~1,000–2,000×** |
| Hot-cache relink (example) | ~1,200 ms | **382 ms** | **~3×** |
| Cold install (example) | ~40 s | **7.4 s** | **~5×** |

The lockfile parse number is from the `lockfile_parser` integration
test on a 1,379-line production lockfile. The install numbers come
from the [`manekineko_install`](examples/manekineko_install.rs)
PoC fetching the same project's 196 gems; it's runnable via
`cargo run --release -p rubund --example manekineko_install -- <path/to/Gemfile>`
but not yet driven by a `rubund install` command.

---

## Quick Start

```bash
# Build (requires Rust ≥ 1.95)
cargo build -p rubund --release

# CLI today only prints version / help — the real value is the library.
cargo run -p rubund -- --help

# Run the lockfile parser against the bundled test vectors.
cargo run -p rubund --example lockfile_parser
```

---

## Library API — Lockfile Parser

`rubund` exposes a library target (`lib.rs`) in addition to the binary.
The main public API today is the **zero-copy lockfile parser**:

```rust
use rubund::parser::{parse_lockfile, SourceType};

let content = std::fs::read_to_string("Gemfile.lock").unwrap();
let lockfile = parse_lockfile(&content);

// All string tokens are &str borrows into `content` — no allocations.
for spec in &lockfile.specs {
    println!("{} ({})", spec.name, spec.version);
    for (dep, constraint) in &spec.dependencies {
        println!("  → {} {}", dep, constraint.unwrap_or("*"));
    }
}

for src in &lockfile.sources {
    match src.type_ {
        SourceType::Gem => println!("gem remote: {}", src.remote),
        SourceType::Git => println!("git remote: {} @ {:?}", src.remote, src.revision),
        SourceType::Path => println!("path: {}", src.remote),
    }
}
```

### Parsed Sections

The parser handles every standard `Gemfile.lock` section:

- **`GEM`** — remote URL, specs with transitive dependencies
- **`GIT`** — remote, revision, branch, specs
- **`PATH`** — local path, specs
- **`PLATFORMS`** — target platform triples
- **`DEPENDENCIES`** — top-level dependency constraints
- **`CHECKSUMS`** — SHA-256 integrity hashes
- **`RUBY VERSION`** — pinned Ruby version
- **`BUNDLED WITH`** — Bundler version that generated the file

---

## Examples

The [`examples/`](examples/) directory contains runnable demonstrations:

| Example | Description | Command |
| :--- | :--- | :--- |
| [`lockfile_parser`](examples/lockfile_parser.rs) | Parse & pretty-print a production `Gemfile.lock` with timing | `cargo run -p rubund --example lockfile_parser` |
| [`c_ext_cache`](examples/c_ext_cache.rs) | Gem with C extension + binary caching | `cargo run -p rubund --example c_ext_cache` |

Two further examples (`real_install`, `manekineko_install`) bridge
into an embedded Ruby interpreter (`rubyrs`) to evaluate real
`Gemfile` DSLs end-to-end. Both run under rubyrs's secure-by-
default Runtime — the Gemfile DSL eval cannot escape the sandbox
via `File.read` or `require`, and a panicking host_fn callback
surfaces as a recoverable Trap instead of crashing the host
(see rubyrs PRs #268 / #279 / #288 / #302 for the underlying
embed-API hardening).

---

## Testing

```bash
# Run the integration test suite (5 test cases)
cargo test -p rubund

# Test cases include:
#   • Standard GEM source parsing
#   • GIT-pinned dependency parsing
#   • Multi-source (GEM + GIT) with cross-platform platforms
#   • PATH source parsing (local gem)
#   • Official Bundler RSpec test vector (lockfile_parser_spec.rb)
```

The test suite uses the exact same test vectors found in Bundler's
official [`lockfile_parser_spec.rb`](https://github.com/rubygems/rubygems/blob/master/bundler/spec/bundler/lockfile_parser_spec.rb)
to ensure parsing fidelity.

---

## Architecture

```
crates/rubund/
├── src/
│   ├── main.rs       # CLI entry point (--version, --help — placeholder)
│   ├── lib.rs        # Library root — re-exports parser module
│   └── parser.rs     # Zero-copy state-machine Gemfile.lock parser
├── tests/
│   └── lockfile_parser.rs       # Integration tests
└── examples/                    # All Cargo-discovered
    ├── lockfile_parser.rs       # Timing & pretty-print demo
    ├── c_ext_cache.rs           # C extension compilation & caching
    ├── real_install.rs          # Single-gem fetch + extract via rubyrs
    ├── manekineko_install.rs    # 16-worker parallel installer via rubyrs
    └── manekineko_install_fixtures/
        └── minimal_gemfile      # Smoke-test fixture for manekineko_install
```

### Parser Design

The parser is a **line-based state machine** that scans `Gemfile.lock`
in a single pass. Key design decisions:

1. **Zero-copy** — Every string token (`name`, `version`, `remote`, …)
   is a `&'a str` borrowing directly into the input buffer.  No
   `String` allocations occur during parsing.

2. **Indentation-driven** — Section membership is determined by
   leading whitespace depth (0 = header, 2 = metadata, 4 = spec,
   6 = spec dependency).

3. **Streaming-friendly** — The parser emits a `Lockfile<'a>` struct
   with `Vec`-backed collections, so callers can iterate immediately
   without waiting for a tree to be built.

---

## Why In-Tree?

`rubund` is developed in-tree with [`rubyrs`](../rubyrs/), an
embeddable Ruby-subset interpreter. `Gemfile` and `*.gemspec` files
are Ruby DSLs, so `Gemfile` evaluation will eventually be driven by
that interpreter — `rubund` is its first non-test embedder. Keeping
both crates in the same workspace turns every breaking change in the
embedding API into a same-day build failure.

(For this first crates.io release the `rubyrs` dependency is
temporarily lifted — `rubyrs` is not yet published — so the bridge
that the CLI's `--demo` flag previously exercised is on hold. It
returns once `rubyrs`, or its eventual successor name, ships.)

---

## Roadmap

- [x] Zero-copy `Gemfile.lock` parser with full section coverage
- [x] Integration test suite with official Bundler RSpec vectors
- [x] Bounded 16-worker parallel gem installer (example only)
- [ ] `Gemfile` DSL evaluation (returns when `rubyrs` is on crates.io)
- [ ] `bundle check` — read-only lockfile ↔ dependency verification
- [ ] Dependency resolver (version-selection algorithm)
- [ ] `bundle install` — full fetch + install with lockfile generation
- [ ] `bundle exec` — environment setup + command delegation
- [ ] `bundle update` — selective dependency upgrade

---

## License

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE)
at your option.
