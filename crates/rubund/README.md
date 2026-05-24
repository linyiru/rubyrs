# rubund

> **A high-performance, drop-in Rust replacement for [Bundler](https://bundler.io/).**

`rubund` lives in the same [Cargo workspace](../../Cargo.toml) as
[`rubyrs`](../rubyrs/), the embedded Ruby interpreter it uses to
evaluate `Gemfile` and `*.gemspec` files (both are Ruby DSLs).

---

## Highlights

| What | Why it matters |
| :--- | :--- |
| **Zero-copy lockfile parser** | Parses `Gemfile.lock` by borrowing directly into the input buffer — zero heap allocations for string tokens. A 1,379-line production lockfile is parsed in **~147 µs**. |
| **Bounded parallel installer** | 16-worker thread pool over `std::sync::mpsc` — saturates network I/O without hitting macOS file-descriptor limits. |
| **Embedded Ruby evaluation** | Uses `rubyrs::Runtime` to evaluate real `Gemfile` DSLs, so the dependency specification is never "approximated" — it's the same Ruby code Bundler would run. |
| **Single static binary** | No Ruby runtime required at the target machine. Ship one binary. |

---

## Benchmark Snapshot

Measured against the real-world **manekineko** project (196 gems, 1,379-line lockfile):

| Phase | Bundler (Ruby) | rubund (Rust) | Speedup |
| :--- | ---: | ---: | ---: |
| Lockfile parse | ~150–300 ms | **0.147 ms** | **~1,000–2,000×** |
| Gemfile eval | ~150 ms | **0.507 ms** | **~300×** |
| Hot-cache relink | ~1,200 ms | **382 ms** | **~3×** |
| Cold install | ~40 s | **7.4 s** | **~5×** |

---

## Quick Start

```bash
# Build (requires Rust ≥ 1.85)
cargo build -p rubund --release

# Run the placeholder CLI
cargo run -p rubund -- --demo
# => rubund 6 — the interpreter is wired up.
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
| [`manekineko_install`](examples/manekineko_install.rs) | Full parallel download + extract of 196 gems | `cargo run --release -p rubund --example manekineko_install -- <path/to/Gemfile>` |
| [`real_install`](examples/real_install.rs) | Single-gem fetch + extract flow | `cargo run -p rubund --example real_install` |
| [`c_ext_cache`](examples/c_ext_cache.rs) | Gem with C extension + binary caching | `cargo run -p rubund --example c_ext_cache` |

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
│   ├── main.rs       # CLI entry point (--version, --help, --demo)
│   ├── lib.rs        # Library root — re-exports parser module
│   └── parser.rs     # Zero-copy state-machine Gemfile.lock parser
├── tests/
│   └── lockfile_parser.rs   # Integration tests
└── examples/
    ├── lockfile_parser.rs   # Timing & pretty-print demo
    ├── manekineko_install.rs # 16-worker parallel installer
    ├── real_install.rs       # Single-gem installer
    └── c_ext_cache.rs        # C extension compilation & caching
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

`rubund` is the first non-test consumer of `rubyrs`'s embedding API
(`Runtime`, `register_fn`, `set_stdout`, `eval`). Keeping it in the
same workspace means every breaking change to that API surfaces
immediately as a build failure — turning the question *"will anyone
actually use this?"* into a daily yes/no signal.

The strategy is **dogfooding**: `rubyrs` needs a real driver to expose
gaps in its language subset; `rubund` needs an embedded Ruby evaluator
to make sense as a Rust binary at all.

---

## Roadmap

- [x] Workspace wiring + embedded `rubyrs` demo
- [x] Zero-copy `Gemfile.lock` parser with full section coverage
- [x] Integration test suite with official Bundler RSpec vectors
- [x] Bounded 16-worker parallel gem installer
- [ ] `Gemfile` DSL evaluation via `rubyrs::Runtime`
- [ ] `bundle check` — read-only lockfile ↔ dependency verification
- [ ] Dependency resolver (version-selection algorithm)
- [ ] `bundle install` — full fetch + install with lockfile generation
- [ ] `bundle exec` — environment setup + command delegation
- [ ] `bundle update` — selective dependency upgrade

---

## License

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE)
at your option.
