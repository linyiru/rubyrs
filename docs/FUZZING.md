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
- Integer / index arithmetic overflow. The fuzz `[profile.release]`
  explicitly enables `debug-assertions` and `overflow-checks`
  despite the release codegen — `cargo fuzz` is otherwise a
  release build, which strips these guards. Without re-enabling
  them, signed-wraparound bugs in primitive ops would slip past
  silently.
- Memory-safety UB caught by AddressSanitizer (libfuzzer-sys
  ships with ASan on by default): use-after-free, heap / stack
  buffer overruns, double free. Most reachable through the
  `unsafe` blocks in `vm/gc.rs` and `vm/dispatch.rs`. The cext
  FFI's `unsafe` surface is **not** in scope — the fuzz crate
  builds rubyrs with `default-features = false`, so the `cext`
  module isn't even compiled into these binaries. Fuzzing that
  boundary would need a separate target with `--features cext`
  (and a corpus shape that crosses the FFI, not raw Ruby
  source).

ASan does **not** model Rust's aliasing rules (Stacked / Tree
Borrows). Those live in the [Miri CI job](../.github/workflows/ci.yml)
which runs a fixed-corpus smoke each PR; fuzz inputs that
exercise an aliasing violation will not crash here, only there.

## Running locally

You need the pinned nightly toolchain (the rest of the project
pins stable 1.95 — the fuzz sub-crate detaches from the workspace
so the nightly install doesn't bleed into normal builds) and
`cargo-fuzz`. The fuzz crate's `rust-toolchain.toml` pins a
specific nightly date; once you `cd` into the sub-crate, rustup
auto-installs that exact version. Bumping the pin is a
deliberate commit (see `crates/rubyrs/fuzz/rust-toolchain.toml`).

```sh
# Triggers rustup to install the date pinned in
# crates/rubyrs/fuzz/rust-toolchain.toml — no `+nightly` needed
# from here on, the file does the channel selection.
cd crates/rubyrs/fuzz
rustup show
# Pin to the same cargo-fuzz version CI uses (see
# `.github/workflows/fuzz.yml`'s `taiki-e/install-action` step).
# Bump both together if cargo-fuzz upstream ships a breaking
# behavioural change.
cargo install cargo-fuzz --locked --version "^0.12"
```

Then from inside `crates/rubyrs/fuzz/`:

```sh
# 1 min smoke. Replace 60 with 300+ to actually soak.
cargo fuzz run parse -- -max_total_time=60

# Same for the full-VM target.
cargo fuzz run eval -- -max_total_time=60
```

> Don't add `+nightly` to those commands. Doing so overrides the
> `rust-toolchain.toml` pin and reintroduces the upstream-nightly
> flakiness the pin exists to prevent. The toolchain file is
> sufficient; rustup walks up from cwd and finds it.

A crash drops a minimised input under
`crates/rubyrs/fuzz/artifacts/<target>/crash-<hash>`. Replay it
without further fuzzing (still inside `crates/rubyrs/fuzz/`):

```sh
cargo fuzz run parse artifacts/parse/crash-abc123
```

## Seeding the corpus

The CI workflow seeds the corpus by walking `tests/diff/` and
`tests/fixtures/` recursively — nested entries like
`tests/diff/require_xpkg/...` or `tests/fixtures/errors/...` are
picked up too, not just top-level `.rb` files. If you're starting
locally with an empty `corpus/<target>/`, mirror that with the
same recursive walk:

```sh
mkdir -p crates/rubyrs/fuzz/corpus/parse crates/rubyrs/fuzz/corpus/eval
for target in parse eval; do
  find crates/rubyrs/tests/diff     -name '*.rb' -exec cp {} "crates/rubyrs/fuzz/corpus/$target/" \;
  find crates/rubyrs/tests/fixtures -name '*.rb' -exec cp {} "crates/rubyrs/fuzz/corpus/$target/" \;
done
```

Every entry under `tests/diff/` and `tests/fixtures/` is real Ruby that rubyrs already
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
- **Not a security audit.** Two layers of "not covered":
  AddressSanitizer catches a useful but bounded subset of UB —
  it doesn't model Rust's aliasing rules (Miri's job) or the
  host-embedder boundary. *On top of that*, this harness
  configures the fuzz binary with `default-features = false`,
  so the `cext` module isn't compiled in at all; the cext FFI's
  `unsafe` blocks aren't reachable from these binaries even
  before ASan's scope question comes up. Those run under their
  own policy ([ADR 0009](adr/0009-cext-panic-policy.md)) and
  would need a separate `--features cext` fuzz target to
  exercise.
- **Not a per-PR gate.** Soak-only by design. Reviewers
  shouldn't wait for a fuzz pass before merging; the panic-budget
  + diff_cruby + miri jobs already guard PRs at the
  short-iteration layer.

## Known limits + future work

One concrete gap remains on the table. The earlier two items in
this list — the preamble-fuel coupling ICE (#3 below) and the
preamble-rebuilt-every-iteration cost (#2 below) — were closed
by PR #204 and PR #212 + the adoption PR respectively; the
sections are kept inline as historical anchors for the
follow-up tracking.

### 1. Filesystem sandbox covers `require` only

`ensure_sandbox_cwd` moves the process cwd into a tempdir, which
makes `require '<relative>'` and `require_relative '...'` look
up against an empty directory. It does **not** intercept:

- `File.open(absolute_path)` / `IO.read(absolute_path)` —
  absolute paths ignore cwd.
- `Dir.glob('/etc/*')` — same.
- `__FILE__` / `__dir__` resolved against the host's repo
  layout.
- `require '<absolute>'` (rare but a real Ruby shape).

Libfuzzer's coverage-guided mutation will eventually walk into
the `vm/fileops.rs` arms via byte sequences shaped like
`File.open(...)`. When that happens, the sandbox is irrelevant.

The right fix is `Config { allow_filesystem_io: bool }` (or
stricter, `allowed_paths: Option<Vec<PathBuf>>`) honoured
uniformly across every I/O sink in `vm/`. Same need lives in
`rubund`, which evaluates untrusted gemspecs; landing the API
there benefits both consumers and removes the cwd-tempdir trick
as a per-consumer rediscovery.

### 2. ~~Runtime preamble rebuilt every iteration~~ — fixed in PR #212 + adoption PR

PR #212 landed `Runtime::reset()` which rewinds a Runtime to
its post-preamble baseline without re-running the preamble.
The harness in `crates/rubyrs/fuzz/src/lib.rs::run_with_caps`
now caches a single `Runtime` in `thread_local!<RefCell<...>>`
and calls `rt.reset()` at the top of each iteration. Measured
~107× speedup over the pre-adoption fresh-Runtime-per-iter
shape on the headline workload (`[1,2,3,4,5].map { |x| x*2 }.length`,
500 iters: 712 µs/iter → 6.6 µs/iter). cargo-fuzz's harness
overhead caps the realized win lower than the Rust-only
benchmark, but the nightly soak now does ~3k iter/sec vs the
previous ~2k — a ~50% boost on the same 5-min budget.

See PR #212's `embed/reset.rs` for the full contract reset
honours (per-class methods/ivars/singleton_methods restored,
heap truncated to post-preamble high-water, interner truncation
respects post-construction host_fn / cext SymIds, etc.).

### 3. ~~Preamble-fuel coupling~~ — fixed in PR #204

`Runtime::with_config` used to consume user-supplied
`Config::fuel` during preamble load — a host setting a tight
fuel budget would panic during construction with
`ICE: failed to load exception preamble` instead of getting a
recoverable Runtime. PR #204 introduced the `CapsGuard` RAII
pattern that lifts all six resource caps (fuel / max_frames /
max_heap_objects / max_symbols / max_value_bytes / deadline)
for the duration of `load_preamble` and restores them after,
so preamble runs unbounded and user evals see the host's caps
as documented.

The workflow's `Preamble-fuel smoke check` step is still in
place as a CI guard against future regressions in this area:
runs the empty input under a tight cap for 5s, fails the job
loudly if the preamble traps. Cheap insurance even with the
coupling fixed.

Historic record: the original harness sized `Caps::tight()`'s
fuel at 50k as an empirical workaround above the observed ~30k
preamble cost. With the coupling fixed, that 50k could be
tightened back to whatever exec/s budget the parser-focused
target actually wants — left at 50k for now to bias mutation
toward the parser surface, not as a workaround.
