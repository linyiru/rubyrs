# 0011: CRuby-mirrored `vm.rs` split

## Status

Accepted (2026-05). 17 submodules under `crates/rubyrs/src/vm/`,
each named after its CRuby compilation-unit analogue. `vm.rs`
itself dropped from 6593 → 378 lines.

## Context

By early May 2026 `crates/rubyrs/src/vm.rs` had grown to 6593
lines. The growth wasn't sloppy — every entry was an explicit
feature commit — but the file had become hard to work with:

- **Locating code by intuition** required full-text grep. The
  natural unit for navigation (in a Ruby implementer's head) is
  the Ruby type: "where's String#sub?", "where's the
  rescue-by-class match?". Inside a single 6k-line file none of
  those map to a region.
- **PR conflicts**. Any feature touched the file. Two parallel
  branches working on unrelated arms of `primitive_call` would
  collide. Master had a long history of merge-conflict-only
  commits.
- **Reviewer fatigue**. A diff that's logically small ("add
  `String#sub`") would land in a 6k-line file's context window,
  pushing reviewers into either scrolling far or trusting line
  numbers blindly.
- **Test failure attribution**. A regression in iterator-block
  GC pinning would surface in `vm.rs:3742` or `vm.rs:4516` —
  the line number gave no useful hint about which subsystem
  owned the bug.

The trigger to act was a bug introduced during P3-B-3
(Hash#extras): the `Hash#to_a` arm needed a `PinGuard` that
matched the existing `Array#map` shape, but the two arms sat
~1800 lines apart in the file. The reviewer caught the missing
guard, but it cost a round-trip that would have been a
one-screen scroll inside a focused module.

Three structural options were considered:

1. **Status quo** — leave `vm.rs` as one file, lean on
   `cargo doc` and module-level doc-comments for navigation.
2. **Functional split** — `vm/dispatch.rs`, `vm/exec.rs`,
   `vm/builtins.rs`, etc., grouped by what the code _does_
   rather than what type it serves.
3. **CRuby-mirrored split** — `vm/string.rs`, `vm/array.rs`,
   `vm/hash.rs`, etc., grouped by the Ruby type the code
   _serves_, with each submodule named after the CRuby
   compilation unit a contributor would open in MRI.

## Decision

Option 3. Split `vm.rs` along CRuby's file boundaries.

### Mapping

| `vm/` file | Role | CRuby analogue |
|---|---|---|
| `vm.rs` | `Vm` struct + `Frame` + `PinGuard` + `RescueHandler` + cext re-entrance thread-local. | `vm_core.h` + struct definitions in `vm.c` |
| `vm/dispatch.rs` | `do_call`, `do_call_block`, `invoke_method`, `invoke_block`, `cext_invoke_method`, `try_method_missing`. | `vm_eval.c` + `vm_insnhelper.c` |
| `vm/step.rs` | The per-opcode interpreter loop (`Vm::step`) + `dispatch` / `dispatch_until`. | `vm_exec.c` |
| `vm/cext.rs` | C-extension dispatch, handle ↔ Value translation, `cext_dispatch`, `cext_require`. | `internal/value.h` + `vm_eval.c` callback installation |
| `vm/iter.rs` | Block-form Enumerable (`iter_*_filter`, `collection_call_block`). | `enum.c` |
| `vm/string.rs` | `Value::Str` primitives + Regex shims. | `string.c` |
| `vm/array.rs` | No-block `Array` methods. | `array.c` |
| `vm/hash.rs` | `Hash` primitives. | `hash.c` |
| `vm/range.rs` | `Range` primitives. | `range.c` |
| `vm/numeric.rs` | `Integer` + `Float` primitives. | `numeric.c` |
| `vm/kernel.rs` | `Kernel#puts` / `Integer()` / `raise` / etc. | `object.c` (Kernel arms) |
| `vm/fileops.rs` | `File.read` / `File.exist?` / … | `file.c` |
| `vm/raise.rs` | `normalize_exception`, `trap_to_exception`, `unwind_with_exception`. | `eval.c` + `eval_error.c` |
| `vm/lookup.rs` | Inline method cache + class/ancestor walks (`lookup_method_cached`, `class_is_a`, `responds_to`). | `vm_method.c` + `class.c` |
| `vm/gc.rs` | `Vm::run` entry, resource caps, `trap`, `maybe_gc`. | `gc.c` + `thread.c` + `vm.c` |
| `vm/primitive.rs` | The typed fast-path dispatch table for built-in receiver methods. | (per-class C function tables) |
| `vm/sprintf.rs` | `ruby_sprintf` + width/precision parser. | `sprintf.c` |
| `vm/util.rs` | Cross-cutting helpers too small for a file: `value_cmp_v`, `vec_nil`, `visibility_from_name`. | (no analogue) |

## Why this layout

**Navigation by intuition.** A reviewer or new contributor
asking "where would CRuby put this?" gets the same answer for
rubyrs. The cost of remembering the layout is a single rule:
each submodule mirrors the CRuby filename. We deliberately
sacrificed some Rust idiom (a functional split would be more
Rust-flavoured) for the lower-friction lookup rule.

**Shape over function.** A functional split would pull
`String#sub`, `Array#push`, and `Hash#[]=` together in
`vm/builtins.rs` because they're all "primitive method
implementation" — but the reasons to read them are unrelated
("I'm adding to String", "I'm fixing an Array bug", "I'm
auditing Hash growth"). Sharing a file just shares conflicts.

**PR seam.** Features that touch one Ruby type land in one
submodule. The B-series (`def self.method`, block destructure,
String-endpoint Range, `**kwargs`, case-splat, non-local
return) and F-series (`lambda`, Hash inspect, anon `**`,
mixed/nested destructure, Module include) each touched 1-2
submodules at most. Master's old commit history shows the
counterfactual: dozens of "merge-conflict-only" commits that
disappear under the new layout.

**Behaviour preservation.** Every extraction was its own
atomic commit, gated on the (then 79; now 92) `diff_cruby`
fixtures staying byte-identical to CRuby. Move-only — no logic
moved between sections.

## Trade-offs

### Cost: cross-module inlining loss

The split moved hot code (`Vm::do_call`, `Vm::step`,
`Vm::lookup_method_cached`) into separate compilation units.
Cross-module call edges at `-C opt-level=3` alone can't inline
the way they did when everything lived in one file. The
fizzbuzz 1M microbench measured a 7% slowdown after the
split (349 ms → 372 ms, well outside σ).

We absorbed this by enabling thin LTO in the release profile —
see [ADR 0012](0012-thin-lto-release-profile.md). Dev and test
builds (where LTO is off by default) are unaffected.

### Cost: more `pub(crate)` surface

Items that lived inside `vm.rs` could be `fn foo` (module-
private). After the split, anything used by another submodule
becomes `pub(crate)`. The crate-private surface grew by ~30
items (helpers like `value_cmp_v`, `vec_nil`,
`visibility_from_name`, `with_vm_ptr_set`, etc., plus the
`Frame` / `PinGuard` / `RescueHandler` types).

We accepted this — `pub(crate)` is still invisible to library
consumers, and the alternative (large `mod` blocks inside one
file) gives the worst of both worlds: longer file *and* loss
of file-level navigation.

### Cost: cext-reentrance machinery moves

`CURRENT_VM_PTR` + `VmPtrGuard` + `with_vm_ptr_set` had to
become `pub(crate)` and got hoisted to a re-export from
`vm.rs` so the now-separated `dispatch.rs` could reach them.
Eventually they migrated entirely into `vm/cext.rs` (commit
`1ad96df`) — see [ADR 0013](0013-current-vm-ptr-aliasing.md).

### Benefit: focused module docs

Each `vm/*.rs` opens with a module-level doc-comment naming
its CRuby analogue and listing the public surface. The
`docs/VM_MODULE_MAP.md` reference doc is the per-module
deep-dive; `docs/ARCHITECTURE.md`'s module table is the
quick-reference.

### Benefit: lower review barrier

A reviewer opening "feat(vm): String#sub" only sees the
`vm/string.rs` diff. Anything that needs them to look at
dispatch / step / iter shows up as a separate file in the
diff list — explicit signal that the PR is doing more than
its title claims.

### Benefit: per-file panic-budget granularity

The CI panic-budget (see `docs/PANIC_AUDIT.md`) now has a
per-submodule budget. A regression that adds a `panic!` to
`vm/step.rs` fails the build with an exact file pointer
instead of "vm.rs grew by 1, somewhere".

## Consequences

- Adding a new built-in method has a clear file rule: open the
  per-receiver-type submodule. The decision table at the
  bottom of `docs/VM_MODULE_MAP.md` enumerates it.
- Tests are easier to scope. The unit tests added under G1-G4
  (gc.rs, raise.rs, lookup.rs, iter.rs) sit inside their
  target submodule's `mod tests` block.
- Future restructuring stays cheap. Moving a function between
  submodules is a 3-line edit (delete + insert + maybe
  re-export). The Vm struct itself stays put.

## Alternatives revisited

If the perf cost of LTO had been prohibitive (it wasn't —
~3 s extra release build time), we would have gone with a
finer functional split inside the single file (`vm.rs`
module blocks with `#![allow(clippy::module_inception)]`).
The fact that thin LTO is cheap closed that path off as
unnecessary.

## Related

- [ADR 0012 — Thin LTO in release profile](0012-thin-lto-release-profile.md)
- [ADR 0013 — CURRENT_VM_PTR borrow-aliasing policy](0013-current-vm-ptr-aliasing.md)
- [`docs/VM_MODULE_MAP.md`](../VM_MODULE_MAP.md) — per-submodule
  reference
- [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) — module table +
  rationale (the "Second split" section is the public-facing
  version of this ADR)
- [`CHANGELOG.md`](../../CHANGELOG.md) "Internal: CRuby-mirrored
  vm.rs split" entry
