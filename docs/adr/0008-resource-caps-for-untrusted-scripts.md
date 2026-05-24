# 0008: Resource caps for untrusted scripts

## Status

Accepted (2026-05).

## Context

The embedding API ([ADR 0007](0007-host-embedding-api.md)) makes rubyrs
runnable in-process inside a host Rust app. That immediately raises a
question the host has to answer: **what if the script is hostile?**

Without limits, a single line of Ruby can wedge the host:

```ruby
while true; end                  # CPU
[].push(0) while true            # memory (with retained refs)
def f; f; end; f                 # stack
```

A "billion laughs" attack is harder in our subset (no eval, no string
multiply yet) but the three above work in the language as it stands
today.

## Decision

Add three independent caps, all `Option<...>` with `None` meaning
unlimited. Plumb them through the public `Config` struct so embedders
set them before calling `eval`.

| Cap | Type | Where enforced | Trap on hit |
|-----|------|----------------|-------------|
| `fuel`            | `Option<u64>`    | `Vm::check_fuel()` at top of `step()` | `ResourceExhausted("out of fuel")` |
| `max_heap_objects`| `Option<usize>`  | `Vm::check_alloc()` after `maybe_gc`, before `heap.alloc` | `ResourceExhausted("heap exhausted: ...")` |
| `max_frames`      | `Option<usize>`  | `Vm::check_frames()` before every `frames.push` | `ResourceExhausted("stack level too deep: ...")` |

Each is a thin helper on `Vm` that returns `Result<(), Trap>`; sites
that need them just append `?`.

A new `RubyError::ResourceExhausted { msg }` variant covers all three
with a free-form message identifying which limit triggered.

CLI access via env vars `RUBYRS_FUEL`, `RUBYRS_MAX_OBJECTS`,
`RUBYRS_MAX_FRAMES`.

## Why three caps and not one

A single "instructions" counter could subsume the other two by
counting allocations and frame pushes as expensive ops, but:

- **Fuel** measures "time" and is cheap to check (one branch per op).
  Suitable as the primary watchdog.
- **Heap cap** measures "space" and is the real lever against
  memory bombs. Time and space are orthogonal — a long-running
  script with bounded memory is fine; a script that allocates
  100 GB in 1 ms is not.
- **Frame cap** is host-safety, not Ruby-side fairness: without it
  deep recursion in Ruby blows the host's *Rust* stack. The host
  must be able to set it independently.

## Why `Option<>` and unlimited default

Default `None` means existing CLI and library users see no behavioural
change — the CLI keeps running long programs, the library keeps
running test fixtures. Hosts that need limits opt in by setting them
on `Config`.

## Critical invariants

- **Fuel must be enforced inside `dispatch_until`**, not just the
  main `dispatch` loop, otherwise `[1].each { while true; end }`
  bypasses the limit by spending all its time inside the block-
  driver loop. We enforce by putting the check at the top of
  `step()`, which both loops route through.
- **Heap cap is checked *after* `maybe_gc`** so transient garbage
  doesn't count against the steady-state limit. Otherwise hosts
  would have to set unreasonably high caps to avoid spurious
  ResourceExhausted on legitimate workloads.
- **Frame cap is checked *before* the push**, not after, so the
  excess frame never enters the data structure. This keeps the
  invariant `frames.len() <= max_frames` always true and avoids
  one-extra-frame escape hatches.

## What's deliberately not in v0

- Per-object byte cap (e.g. max string size, max array length). We
  could add a billion-laughs guard for `Array#<<`, `String#+`,
  `String#*` once we have a clear test case. Right now our subset
  doesn't admit a clean billion-laughs path that the heap-object
  cap doesn't already catch.
- Wall-clock timeout. Async/threading isn't in the runtime yet;
  fuel is the right primitive until it is.
- Per-op cost weighting (e.g. `NewArray` costs more than `LoadNil`).
  Premature: we don't have benchmarks showing the difference matters.

## `ResourceExhausted` is outside the StandardError subtree

When the caps were first added we put `ResourceExhausted` under
`StandardError` for cheap parity with the rest of the exception
hierarchy. That was a security bug: every Ruby program with a bare
`rescue => e` clause (the conventional shorthand for
`rescue StandardError => e`) could silently swallow the kill switch
and keep burning the host's quota — exactly the scenario this ADR
exists to prevent.

The trap is now rooted **directly under `Exception`**, alongside
CRuby's `SystemExit` and `Interrupt`:

```ruby
class ResourceExhausted < Exception
end
```

This means:

- Bare `rescue` clauses (`rescue => e`) do **not** catch
  `ResourceExhausted`. The default StandardError filter walks past
  it, and the trap propagates to the host as a `Trap` out of
  `Runtime::eval`.
- A script that *deliberately* wants to handle resource exhaustion
  can still write `rescue Exception => e` once explicit class
  filtering lands in P1-10. This is opt-in and explicit — no
  accidental swallowing.
- Hosts that want to retry should construct a fresh
  `Runtime::with_config` and re-evaluate; the trap is not the
  script's responsibility to decide about.

The `unwind_with_exception` path enforces this: every
`Op::PushRescue` attaches `filter_class: Some(StandardError)` to its
handler, and the unwinder discards handlers whose filter doesn't
match the raised exception's class chain. `Op::PushEnsure` keeps
`filter_class: None` — `ensure` always runs regardless of class,
matching Ruby semantics.

## Consequences

Wins:

- The embedding story now has a credible "I'm running untrusted
  code" mode. The Brewfile / Dangerfile demo (P2-A) can lean on
  these to enforce execution budgets without trusting the script.
- 5 new tests in `tests/embed.rs` lock in the semantics. Both
  `cargo test` and `STRESS_GC=1 cargo test` stay green at 23/23.

Costs:

- One branch per op in `step()` (the fuel check) even when fuel
  is `None`. The branch is well-predicted in the common case;
  measured impact on the fizzbuzz microbench is in the noise.
- Heap cap interacts with the GC: with `STRESS_GC=1` every
  allocation collects, so transient allocations never accumulate.
  Tests must use *retained* allocations to demonstrate cap hits.
  Documented in the cap test fixture.
