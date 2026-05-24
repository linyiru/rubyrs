# Panic audit

This document is the inventory and classification of every
`panic!` / `.unwrap()` / `.expect(...)` call in the rubyrs
crate. It exists for two reasons:

1. **User-reachable panics are bugs.** Embedding APIs that
   panic hand the host a SIGABRT instead of a recoverable
   `Trap`. With rubund evaluating arbitrary `*.gemspec` files
   from rubygems.org, every panic path is one hostile input
   away from killing the host process.
2. **CI guards against regression.** The
   [`panic-budget`](../.github/workflows/ci.yml) workflow job
   counts `panic!` + `.unwrap()` + `.expect(` per file and
   fails the build if any file's count rises above the budget
   recorded below. Audit numbers go down over time, never up.

## Classification

Every site falls into one of three buckets:

| Symbol | Meaning | Action |
|---|---|---|
| 🟢 **ICE** | Compiler-guaranteed invariant. A trip would mean rubyrs itself is wrong, not the script. | Keep as `.expect("ICE: <why>")`. |
| 🟡 **ICE-but-fuzzy** | Theoretically an invariant, but hard to prove. Reachable via internal bugs (e.g. GC slot reuse), not directly by user code. | Keep, but exercise via cargo-fuzz (P3-17). |
| 🔴 **User-reachable** | A specific Ruby program triggers it. Bug-class. | Convert to `Trap`. |

## Current budget (2026-05-24, after P0-4 + P2-13)

| File | Count | All ICE? |
|---|---|---|
| `crates/rubyrs/src/vm.rs` | 61 | 🟢 |
| `crates/rubyrs/src/heap.rs` | 10 | 🟡 |
| `crates/rubyrs/src/ast.rs` | 3 | 🟢 |
| `crates/rubyrs/src/lib.rs` | 1 | 🟢 (bootstrap) |
| `crates/rubyrs/src/compiler.rs` | 1 | 🟢 |
| **Total (excl. doc comments)** | **76** | |

P2-13 bumped heap.rs from 9 to 10 by adding the
`heap.block(id) -> &BlockHandle` accessor (a "heap slot is not
a Block" panic of the same shape as the existing array/hash/range
accessors). Same 🟡 classification — only reachable via a real
GC slot-reuse bug.

CI threshold is set per-file to these exact numbers. Any
increase fails the build.

## vm.rs — 61 sites, all 🟢 ICE

These are all invariants enforced by the dispatch loop and the
compiler's emit order. Categories:

- **`self.frames.last() / last_mut() / pop().expect(...)`**
  inside any `Op::` handler. Ops only run from within
  `Vm::step`, which is only called by `Vm::run` after pushing
  the entry frame. The frame stack can be empty only after the
  final `Op::Return` pops it, at which point dispatch exits
  before another op fires.
- **`self.stack.pop().expect(...)`**: The compiler emits ops
  knowing the exact stack depth at each program point. A
  `BinOp` always follows two pushes; `Dup` follows at least one;
  `StoreLocal` follows one. A trip means the compiler is broken.
- **`acc_id.unwrap()`** in `iter_array_filter` / `iter_hash_filter`
  / `iter_range_filter`. `acc_id` is `Some` iff `mode` is
  `Select | Reject`; the unwraps live inside `match mode {
  IterMode::Select | IterMode::Reject => ... }` arms. The two
  are bound by the function's contract.
- **`panic!("ICE: CallBlock without Block value on stack")`**
  (vm.rs:1038). Compiler emits `CreateBlock` immediately before
  `CallBlock`. Invariant.

If any of these ever trip in practice, the fix is in the
compiler or `Vm::step`, not in the panic site.

## heap.rs — 9 sites, all 🟡 ICE-but-fuzzy

These are slot accessors that panic when an `ObjId` lands on a
slot of the wrong type or a freed slot:

- `get` / `get_mut` — "use-after-free `ObjId(<n>)`"
- `instance` / `instance_mut` — "heap slot is not an Instance"
- `array` / `array_mut` — "heap slot is not an Array"
- `hash` / `hash_mut` — "heap slot is not a Hash"
- `range` — "heap slot is not a Range"

These trip when a `Value::Array(id)` or similar references a
slot that the GC has freed or repurposed. We've fixed two such
bugs already (`Class.new(args)` GC root hole in commit `642857b`
and `Hash#to_a` slot reuse cycle in commit `2c6c8f2`); both
appeared as one of these panics.

Why they stay as panics: they reflect a real corruption of the
VM's internal invariants. If `Value::Array(id)` doesn't point
at an Array, something is *already* deeply wrong — converting
the panic to a Trap would only delay the eventual crash and
make the underlying bug harder to debug. The right fix is
always to chase the GC root hole, not to soften the assertion.

The fuzz target landing in P3-17 specifically exercises these.

## ast.rs — 3 sites, 🟢 ICE

After P0-4, the user-reachable unsupported-node panic is gone
(replaced with a `SyntaxError` Trap surfaced from
`tr_with_errors`). Remaining sites:

- Lines 117, 142, 312: `stmts.into_iter().next().unwrap()`
  guarded by `if stmts.len() == 1`. The next() can never be
  None.

## lib.rs — 1 site, 🟢 ICE

- Line 131: `eval(PREAMBLE, ...).expect("ICE: failed to load
  built-in exception preamble")`. The preamble is a literal
  `&'static str` shipped with the crate. If it fails to parse,
  the crate is broken before any user code runs.

## compiler.rs — 1 site, 🟢 ICE

- Line 63: `panic!("ICE: patch_jump on non-jump op at {}", at)`.
  Internal compiler API contract: only call `patch_jump` on
  positions that hold a `Jump` / `JumpIfFalse` op. Misuse is a
  compiler bug.

## Doc-comment occurrences

`.unwrap()` appearing inside `//!` or `///` doc examples (such
as in `lib.rs:9` and `:13`) is ignored by the budget — they're
text, not code. The CI guard's grep pattern excludes lines that
start with comment markers.

## How to lower the budget

When you add a Trap-returning helper that subsumes a previously-
panicking site:

1. Convert the call site.
2. Update the table above with the new count.
3. Update the CI guard threshold in
   `.github/workflows/ci.yml`.
4. Mention "lowers PANIC_AUDIT budget for `<file>` to N" in
   the commit message.

Direction is always down. Never up.
