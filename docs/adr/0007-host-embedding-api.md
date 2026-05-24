# 0007: Host embedding API

## Status

Accepted (2026-05).

## Context

So far rubyrs has been a CLI binary. To validate the actual product
niche — Ruby-flavoured scripting embedded in a Rust host (Brewfile /
Dangerfile / sandboxed DSLs / WASM components) — we need an embedding
API.

Constraints we've signed up for elsewhere:

- **No host panics from script errors.** Locked in by P0-B-2 (Trap
  return everywhere).
- **No host stdout takeover.** Embedders need to redirect script output
  to their own sink (a `Vec<u8>` for testing, a tracing span for a
  server, a UI element for a sandbox).
- **Definitions persist across `eval` calls.** Classes and methods
  defined in one chunk must be visible in the next.

We are *not* yet adding fuel/heap caps, host capability injection
(file/network), or a host-callable AST visitor. Those are P1-D and
later — this ADR is about the minimum sufficient surface to start
embedding.

## Decision

A new `src/lib.rs` exposes `Runtime`, the primary embedding type.
`src/main.rs` becomes a thin wrapper that builds a `Runtime` and feeds
it a file argument.

Public surface (initially small on purpose):

```rust
pub struct Runtime { /* ... */ }

impl Runtime {
    pub fn new() -> Self;
    pub fn with_config(cfg: Config) -> Self;

    pub fn eval(&mut self, source: &str, filename: &str) -> Result<Value, Trap>;
    pub fn eval_file(&mut self, path: &Path) -> Result<Value, Trap>;

    pub fn register_fn<F>(&mut self, name: &str, f: F)
    where F: Fn(&[Value]) -> Result<Value, Trap> + 'static;

    pub fn set_stdout(&mut self, w: Box<dyn Write>);

    pub fn format_trap(&self, trap: &Trap) -> String;
}

pub struct Config { pub stress_gc: bool }
pub use { Value, RubyError, Trap, TrapFrame, Span };
```

Implementation details worth noting:

- `Vm` gains `stdout: Box<dyn Write>` (default `io::stdout()`) and
  `host_fns: HashMap<SymId, Rc<HostFn>>`. `builtin_call`'s `puts` /
  `print` now `writeln!(self.stdout, ...)` instead of `println!`.
- `do_call` and `do_call_block` check the host-fns table after the
  built-in path and before falling through to NoMethodError. Host
  fns are visible globally (no receiver), matching the brief.
- `Runtime` caches per-filename source strings so `format_trap` can
  resolve byte offsets to line numbers without re-reading files.
- `eval` is **incremental**: each call appends Protos to the same
  `Vm.protos` and uses the same `Vm.interner`. Class definitions
  from a prior eval persist; the runtime is one long-lived
  conversation, not a series of disposable parses.

## Consequences

Wins:

- The Brewfile/Dangerfile demo (P2-A) is now buildable: host registers
  the DSL's special functions, captures stdout, runs the script.
- Existing CLI behaviour preserved: `main.rs` shrinks from 70 lines to
  20, but `./target/release/rubyrs t.rb` works identically.
- `tests/embed.rs` locks down the surface so accidental breakage
  shows up in CI.

Costs:

- `Value` is now `pub`. Its variants `Object(ObjId)`, `Array(ObjId)`,
  `Hash(ObjId)`, `Block(Rc<BlockHandle>)`, `Class(Rc<Class>)` reference
  types that are also `pub` (the type system requires it) but whose
  fields stay `pub(crate)`. Embedders can pattern-match on these
  variants but can't manufacture instances — heap-managed values must
  come from running a script.
- `HostFn` is `Fn(&[Value]) -> Result<Value, Trap>` with no host
  context. Host code that wants to allocate Arrays/Hashes from inside
  a host fn can't yet. We'll add a `HostCtx` handle when a real use
  case demands it.
- `Trap::new(err)` lets host fns construct Traps without a
  pre-existing backtrace; the dispatch loop fills it from the live
  frame stack. Means `Trap` fields are `pub`, which is a slight
  surface-area cost but the right ergonomic call for now.

## Why not a typed-builder configuration?

`Config { stress_gc: bool }` is the dumbest possible config struct.
A builder would let us add fields without breaking the surface, but
we have one field, and `Config { stress_gc: true }` is fine. We'll
revisit when there are 4+ fields.

## What's deliberately not in v0

- Host can't read globals/locals from the Ruby side
- No `block`-passing from host fn
- Class/method registration from the host (only data-shaped fns)
- No async / coroutine / cancellation
- No fuel limit, heap cap, or stack-depth cap — that's P1-D

Each is solvable; none is needed to validate the embedding thesis.
