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

End-to-end wall time on an Apple Silicon M-series mac (M2 Max),
release build, hyperfine `--warmup 3`, 25 runs, repeated in reverse
order — including cold start + parse + eval + the script-defined
`each` loops + the small class definition. Re-measured 2026-07-06
(CRuby 3.4.8 via rbenv, invoked directly — the rbenv shim itself
adds ~38 ms and is excluded):

| Runtime | Time | vs rubyrs |
|---------|----:|----------:|
| **rubyrs (embedded)** | **9.5 ms** | 1.0× |
| CRuby 3.4.8 (no YJIT) | 30.4 ms | 3.2× slower |
| CRuby 3.4.8 + YJIT | 30.6 ms | 3.2× slower |

(Earlier eras looked very different — 1.8 ms vs 74.7 ms ("42×") on
the small pre-Jekyll-era binary against a slow-starting CRuby
install. Since then rubyrs's always-on preamble grew — the embedded
`Runtime::new` compiles it live, which dominates the 9.5 ms — and
this CRuby installation starts in ~30 ms, not ~75 ms. See the
matching table in
[docs/BENCHMARKS.md](../../../../docs/BENCHMARKS.md).)

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
  arithmetic" — see [docs/BENCHMARKS.md](../../../../docs/BENCHMARKS.md)
  for that. On that axis CRuby's interpreter is ~2.2× faster than
  us. On *embedded DSL latency* — what an actual DSL-hosting Rust
  app cares about — we're ~3.2× faster end-to-end. Different shape
  of workload, different winner.
