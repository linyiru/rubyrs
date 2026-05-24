# Security & trust model

rubyrs is an embedding API. The host process gives a script
some compute, some memory, and (optionally) some host
functions. What the script can do with those resources is what
this document is about.

The shortest summary: **rubyrs is not a sandbox. It is a
hardening layer for one component of a sandbox.** The cap
machinery described here defends against accidental and
moderately-adversarial scripts. For seriously hostile input
(`rubygems.org` gemspecs, paid tenants, network-sourced
plugins), you must combine rubyrs with OS-level sandboxing or
run the WebAssembly build inside `wasmtime`. The boundaries
below say exactly when each level applies.

## Trust levels

| Level | Example workloads | Required configuration |
|---|---|---|
| **Trusted** | Internal scripts shipped by your team. CI tooling. A developer's own DSL files. | `Config::default()` is fine. |
| **Semi-trusted** | DSL files from a user you have a relationship with — `Brewfile`, `Dangerfile`, Bundler `Gemfile`, configuration shipped by a paying customer. Buggy more often than malicious, but a runaway loop is a real failure mode. | Set every cap. fuel + heap-objects + frames + deadline + symbols + value-bytes. Keep `ResourceExhausted` uncatchable (the default). |
| **Untrusted** | Code that arrived over the wire. `*.gemspec` files from `rubygems.org`. Multi-tenant SaaS where one tenant's script must not affect others. | Same caps as semi-trusted, **plus** an OS-level boundary: build rubyrs for `wasm32-wasip1` and run inside `wasmtime` with `--fuel` and a deny-by-default filesystem. **Do not** rely on the rubyrs caps alone — they are not a substitute for an isolated address space. |

The intent of "semi-trusted" is "won't take down the host
process when the script misbehaves." The intent of "untrusted"
is "the host process is intact even if the script is actively
hostile." Those are different commitments.

## Configuration

Every cap is `Option<T>` on `Config`; `None` (the default) means
unlimited. For the **semi-trusted** profile, set them like this:

```rust
use std::time::Duration;
use rubyrs::{Config, Runtime};

let mut rt = Runtime::with_config(Config {
    // Bounded execution cost. ~1M ops finishes a small Brewfile
    // with headroom. Adjust to your workload.
    fuel: Some(1_000_000),
    // Defends "build a giant data structure" attacks.
    max_heap_objects: Some(10_000),
    // Defends `def f; f; end; f` recursion before our Rust
    // stack overflows.
    max_frames: Some(256),
    // Wall-clock budget — fuel measures ops, this catches
    // "slow per op" cases like a host function that blocks.
    deadline: Some(Duration::from_secs(2)),
    // Cap the interner so `to_sym` in a loop can't grow the
    // symbol table without bound. The preamble + your DSL's
    // own method names already consume ~50–100 symbols, so
    // size relative to `rt.symbol_count()` after a warmup eval.
    max_symbols: Some(10_000),
    // Per-value byte cap. Catches `"a" * 10_000_000` and
    // friends. 1 MB is generous; tune to the maximum string /
    // array you actually expect.
    max_value_bytes: Some(1_048_576),
    ..Default::default()
});
```

Each cap can be set independently. They compose: a script that
trips any of them gets a `ResourceExhausted` Trap and `eval`
returns.

## How `ResourceExhausted` works

`ResourceExhausted` is the trap raised by every cap. It is
deliberately rooted under `Exception`, **not** under
`StandardError`. That placement means:

- Bare `rescue => e` (CRuby's default `rescue StandardError =>
  e`) does **not** catch it. A hostile script can't loop in a
  rescue clause to burn through fuel.
- Even an explicit `rescue Exception => e` does **not** catch
  it. The trap is a host-level `Result::Err(Trap)` raised
  directly out of `Vm::run` — it bypasses `unwind_with_exception`
  entirely and never appears as a Ruby exception to the
  script. The trap is *not* recoverable from inside the Ruby
  layer by design.
- The host can choose to retry — construct a fresh
  `Runtime::with_config(...)` and re-evaluate. That's a
  *host-side* decision, where it belongs.

This is the same shape CRuby uses for `SystemExit` and
`Interrupt`. See [ADR 0008](adr/0008-resource-caps-for-untrusted-scripts.md)
for the full reasoning.

## Known attack surface

The caps cover what they cover, but there are residual risks
the host should be aware of.

| Attack | Defence in rubyrs | Host's responsibility |
|---|---|---|
| **CPU monopolisation** (`while true`) | `fuel` + `deadline` | Use both. fuel is precise per-op; deadline catches a host-fn blocking on something else. |
| **RAM monopolisation** (one big string / array) | `max_value_bytes` | Set it. The default-unlimited cap is convenient, not safe. |
| **RAM monopolisation** (many small objects) | `max_heap_objects` | Set it. Combined with `max_value_bytes` covers both shapes. |
| **Symbol table growth** (`to_sym` in a loop) | `max_symbols` | Set it. The interner is a global per-Runtime resource and never shrinks. |
| **Stack growth** (deep recursion) | `max_frames` | Set it. Without this a runaway recursion overflows the *host's Rust* stack, which is fatal. |
| **Slow per-op host functions** | Not directly. `deadline` catches it eventually. | If your `register_fn` callbacks make network calls or grab locks, they can stall the whole runtime. Either set short internal timeouts in the callback or use Tokio + a separate executor. |
| **stdout blocking** | None | If the script `puts`-floods to a `set_stdout(Box::new(slow_writer))`, the writer call blocks `eval`. Use a buffered or back-pressured writer. |
| **HashMap iteration-order side channel** | None | Our `Hash` iterates in insertion order (matching CRuby). A script can therefore learn the host's `register_fn` registration order via the global function namespace. Treat host-function names as semi-public. |
| **Symbol name leak via backtraces** | None | `Trap::backtrace` includes user-supplied filenames and method names. Sanitise before logging if those are sensitive. |
| **`uncaught exception → process::exit(1)`** | **Partial** — see below. | An exception that no rescue clause catches currently exits the host process. **Do not run untrusted scripts without OS-level isolation** until this becomes a `Trap`. Tracked. |

The last row is the most consequential gap. The current
`unwind_with_exception` calls `std::process::exit(1)` when no
rescue handler is found in any frame. That's acceptable for the
CLI but unacceptable for an embedded host that has work to do
after `eval` returns. Until that path becomes a `Trap`, even
"trusted" embedding hosts should wrap `eval` in a child
process for any script they don't fully control.

## Defaults reflect the trusted-script case

`Runtime::new()` constructs a runtime with **no caps**. That's
the right default for the most common case — a developer
running a script they wrote themselves on a workstation they
own. The semi-trusted and untrusted profiles are explicit
opt-in: a host that takes scripts from less-than-fully-trusted
sources has to think about it.

If you find yourself reaching for "I want safe defaults", you
probably want the semi-trusted profile copy-pasted above.

## Reporting issues

Security-relevant bugs (anything in the table above that you
can reproduce, plus any panic reachable from a script the host
fed `eval`) should be filed on the rubyrs issue tracker or
emailed to the maintainer. Please don't disclose publicly until
a fix has shipped.

The current panic audit lives in
[`PANIC_AUDIT.md`](PANIC_AUDIT.md). A user-reachable panic is
treated as a security bug — see also
[ADR 0008](adr/0008-resource-caps-for-untrusted-scripts.md).
