# Fuzzing

rubyrs ships two `cargo-fuzz` targets that exercise the parser+IR
translation and the full VM eval path against arbitrary input.
They're a soak workload — nightly CI walks the corpus while every
PR runs the deterministic test suite — and a triage tool: any
contributor can rebuild last night's corpus locally and rerun
against their branch in under a minute.

## What the targets cover

| Target | What it stresses | Misses |
|---|---|---|
| `parse` | `prism` parse + `ast.rs` AST→IR translation. Tight fuel (50k ops) biases corpus selection toward parser surface. | Deep VM dispatch — eval body runs but is rate-limited. |
| `eval`  | Full VM dispatch, GC, method lookup, primitive registry, ops. 10× the parse fuel budget. | Cross-iteration state (each input gets a fresh `Runtime`). |

A future `diff` target will run both rubyrs and CRuby and compare
stdout — same shape as `tests/diff_cruby.rs` but with random
inputs. It's deliberately out of scope for the initial harness
because CRuby subprocess overhead drops exec/s by ~50× and the
two in-process targets here are where the cheap-to-find ICEs
live.

Only Rust-level panics fail a fuzz iteration. `Err(Trap)` —
`SyntaxError`, `NoMethodError`, `ResourceExhausted`, anything
else — is by construction correct VM behaviour and is ignored.
What we're hunting:

- `unwrap` / `expect` ICEs that the
  [panic budget](PANIC_AUDIT.md) classifies as 🟢 ICE but a
  reachable input proves was actually 🔴.
- `RefCell` runtime borrow conflicts under unusual call shapes.
- `unreachable!()` arms that a new AST shape can actually reach.
- Integer / index arithmetic that overflows under
  `debug-assertions`.
- Stacked / Tree Borrows violations (libfuzzer-sys runs with
  AddressSanitizer; UB shows up immediately).

## Running locally

You need a nightly toolchain (the rest of the project pins
stable 1.95 — the fuzz sub-crate detaches from the workspace so
the nightly install doesn't bleed into normal builds) and
`cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo +nightly install cargo-fuzz
```

Then from the repo root:

```sh
# 1 min smoke. Replace 60 with 300+ to actually soak.
cd crates/rubyrs/fuzz
cargo +nightly fuzz run parse -- -max_total_time=60

# Same for the full-VM target.
cargo +nightly fuzz run eval -- -max_total_time=60
```

A crash drops a minimised input under
`crates/rubyrs/fuzz/artifacts/<target>/crash-<hash>`. Replay it
without further fuzzing (run from inside `crates/rubyrs/fuzz/` so
the relative `artifacts/...` path resolves):

```sh
cd crates/rubyrs/fuzz
cargo +nightly fuzz run parse artifacts/parse/crash-abc123
```

## Seeding the corpus

The CI workflow seeds from `tests/diff/*.rb` and
`tests/fixtures/*.rb` on first run; if you're starting locally
with an empty `corpus/<target>/`, mirror that:

```sh
mkdir -p crates/rubyrs/fuzz/corpus/parse crates/rubyrs/fuzz/corpus/eval
cp crates/rubyrs/tests/diff/*.rb crates/rubyrs/fuzz/corpus/parse/
cp crates/rubyrs/tests/diff/*.rb crates/rubyrs/fuzz/corpus/eval/
```

Every entry in `tests/diff/` is real Ruby that rubyrs already
handles, so it's the highest-signal starting point we have.

## When a fuzz finding lands

1. The artifact uploaded by the workflow on failure (see
   `.github/workflows/fuzz.yml`) contains the minimised input.
2. Reproduce locally with the `replay` invocation above.
3. Add the input as a `tests/diff/` fixture (or `tests/fixtures/`
   if there's no CRuby oracle) so the regression is locked in
   by the per-PR test suite. The fuzz workflow is a discovery
   tool, not a regression test — once a bug is known, it belongs
   in the deterministic layer.
4. Fix. The diff_cruby + lib + embed suites must stay green
   under the existing CI; the fuzz workflow's success is
   eventual, not immediate.

## What this turned up while it was being written

A first-iteration version of `parse.rs` set `fuel = Some(0)` to
force the VM to trap on the first opcode and isolate the
parse/translate surface. Smoke-testing the harness for 15
seconds surfaced a panic at `lib.rs:460` —
`Runtime::with_config` runs the exception-class preamble during
construction, which consumes the user-provided fuel and trips
`.expect("ICE: failed to load exception preamble")` when fuel
runs out mid-bootstrap. A host that legitimately wants a
fuel-capped sandbox (`Config { fuel: Some(small), .. }`) cannot
construct a Runtime today; the panic is 🔴 user-reachable per
PANIC_AUDIT classification.

The current `parse.rs` works around this by passing
`fuel: Some(50_000)` — enough headroom for the preamble plus
some user code. The underlying ICE is a separate fix (the
preamble loader should be exempt from user-fuel accounting, or
the `.expect` should be downgraded to a `Trap` return); it's
flagged here so the next person to touch `Runtime::with_config`
sees the connection.

## What this isn't

- **Not a coverage tool.** libFuzzer's coverage-guided mutations
  do bias toward unexplored branches, but the corpus growth
  isn't a substitute for `cargo llvm-cov`.
- **Not a security audit.** AddressSanitizer catches a useful
  subset of UB. It doesn't model the host-embedder boundary or
  the cext FFI's `unsafe` blocks (those run with their own
  policy under [ADR 0009](adr/0009-cext-panic-policy.md)).
- **Not a per-PR gate.** Soak-only by design. Reviewers
  shouldn't wait for a fuzz pass before merging; the panic-budget
  + diff_cruby + miri jobs already guard PRs at the
  short-iteration layer.
