# Brewfile demo

A worked example of hosting a Ruby DSL inside a Rust application via
rubyrs. The shape is a (toy) Brewfile: a Ruby script whose top-level
"API" is the four functions `tap`, `brew`, `cask`, `mas`. Each just
records a package the user wants. Homebrew itself does exactly this
under CRuby.

This example does the same thing but **embedded in a Rust app**:

- the host (`../brewfile.rs`) registers those four functions on a
  `Runtime` via `register_fn`,
- evaluates `Brewfile.rb`,
- prints a summary of what the script declared.

## Run it

```bash
cargo run --release --example brewfile
```

## Compare it

Same workload under CRuby (`cruby_runner.rb` defines the four
functions as Ruby methods, then `load`s the same `Brewfile.rb`):

```bash
ruby examples/brewfile/cruby_runner.rb
```

End-to-end wall time on an Apple Silicon M-series mac, release
build, 30 hyperfine runs, including cold start + parse + eval + the
script-defined `each` loops + the small class definition:

| Runtime | Time | vs rubyrs |
|---------|----:|----------:|
| **rubyrs (embedded)** | **1.8 ms** | 1.0× |
| CRuby 3.4 (no YJIT) | 74.7 ms | 42.5× slower |
| CRuby 3.4 + YJIT | 75.5 ms | 42.9× slower |

YJIT doesn't help here — Ruby spends most of its wall time on
process startup and Ruby's own gem loader, not on the script's
arithmetic. This is exactly the embedded-DSL profile rubyrs is
designed for.

## What this isn't

- It is **not** the actual `homebrew` formula. We didn't replicate
  Homebrew's full Brewfile semantics (keyword args, `groups`,
  `link:`, etc.). The script uses our supported subset of Ruby
  (single-arg DSL methods, positional `mas` ID).
- It is **not** a microbenchmark of "rubyrs vs CRuby on raw
  arithmetic" — see [docs/BENCHMARKS.md](../../docs/BENCHMARKS.md)
  for that. On that axis CRuby's interpreter is 1.76× faster than
  us. On *embedded DSL latency* — what an actual DSL-hosting Rust
  app cares about — we're 42× faster end-to-end. Different shape of
  workload, different winner.
