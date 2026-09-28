# ADR 0038 — `Value` representation: keep the 16-byte enum, inline its `Clone`

Date: 2026-09-27
Status: accepted. B2 of #384. Options 1 and 2 are **deferred by measurement**; only Option 0
ships.
Follows: ADR 0035 (the `#[repr(u8)]` layout contract), ADR 0036 (measure before rewriting).

## Context

B2 in #384 proposed an 8-byte `Copy` `Value`. The premise came from an empty-loop profile taken
before the A items and B3 landed. In that profile `drop_glue`, `Value::clone` and `Vec::push`
were about 25% of samples, and they were named "the root of the ~4× base step cost".

Today `Value` is a 16-byte `#[repr(u8)]` enum:
- Three variants hold an `Rc`: `Str(Rc<RStr>)`, `Class(Rc<Class>)` and `Regex(Rc<CompiledRegex>)`.
- Every other variant is plain data: i64, f64, `ObjId`, `SymId`, bool or nil.

Because of the three `Rc` variants, `Value` is not `Copy`. Every clone and every drop runs glue
that switches on the tag.

Following ADR 0036, the rewrite was costed and measured before any commitment. Both profiles are
from `sample(1)` on the Studio with `everything,jit-native`:
- the first is master fd2c3561
- the second is master plus the Option 0 PoC

The Rails hello
bench is `poc/rails-spike/bench.rb`, which drives GET / through the full middleware stack in
process.

## Measurements

**Rails hello, leaf samples (5087 total):**

| frame | share |
|---|---|
| `dispatch_until_inner` (the interpreter loop, with ops inlined) | 17.8% |
| `step_cold` | 9.0% |
| `do_call` | 7.2% |
| `stat` syscall (ActionDispatch::Static and I18n `File.file?` / `exist?`; CRuby makes the same calls) | 5.5% |
| `Value::clone` (out of line) | 2.5% |
| `drop_in_place<[Value]>` + `drop_in_place<Value>` | 3.3% |
| `memmove` | 2.4% |
| `Heap::visit_value` (GC mark) | 2.4% |

After Option 0 (below), the same profile has 5725 samples:
- The out-of-line `Rc` half of clone (Str, Class, Regex) is **0.6%**.
- Drop glue is 3.9%.
- `memmove` is 3.1%.
- `visit_value` is 2.5%.

Everything that a `Copy` or 8-byte `Value` could remove adds up to at most about **7–10%** of a
Rails request, and that ceiling assumes the new representation costs nothing anywhere else.

On the same machine, CRuby 3.4.8 serves **3470 req/s** and rubyrs **~990 req/s**, a 3.5× gap.
Removing all `Value` glue cannot close more than a tenth of it.

## Options

**Option 0: hand-written `#[inline(always)] Clone`.** A derived `Clone` over 20 variants
compiles to an out-of-line jump table, so cloning an `Int` costs a call. The hand-written
version has two paths:
- For the three `Rc` variants it calls an out-of-line helper (`clone_str` / `clone_class` /
  `clone_regex`). Each helper takes the `Rc` itself, so it has no wildcard arm.
- For everything else it does a 16-byte bitwise copy (`ptr::read`).

The match is exhaustive and there is no `_` arm anywhere, so a new variant fails to compile
until it is classified as either `Rc` or plain. That way a payload with a `Drop` impl can never
fall into the copy arm by default. The JIT contract is unchanged: layout, size and tags stay the same.

**Option 1: `Copy` `Value`, 16 bytes, with Str/Class/Regex behind heap ids.** This removes the
drop glue (about 4%), but it costs more than that:
- **Strings move to the GC heap.** Today a string is freed as soon as its refcount reaches zero.
  After the move it lives until the next sweep. Rails allocates many short-lived strings per
  request, so there would be more live heap, more `visit_value` work and more frequent GCs.
- **Every string access pays a slab lookup** (`oid → slots[oid]`), and so does every class
  access: `class_of`, method lookup and constant lookup, the hottest paths in `do_call`.
- **Class identity is built on the `Rc`.** There are 90 `Rc::ptr_eq` and 151 `Rc::as_ptr`
  sites, mostly method-cache and inline-cache keys, `Heap::class_ptrs`, and the class pointers
  the native JIT bakes into guards. There are also 9 `Weak<Class>` back-edges (the singleton
  target, `defining_class`, the global method cache).
- **Size of the change:** `Value::Str(` occurs at 733 sites and `Value::Class(` at 502.
- **Public API break.** `Value` is public (`lib.rs:235`), and embedders pattern-match
  `Value::Str` (`tests/embed/*`, rubund's `real_install` example).

**Option 2: 8-byte tagged / NaN-boxed word.** This needs everything in Option 1, plus:
- boxed or flonum-style Floats
- 62-bit fixnums, with a new promotion edge to BigInt
- a rewrite of the native JIT's 16-byte slot stride and tag-byte loads, and tier 2's two-word
  model (`jit_tier2.rs:188-240`)

It halves `memmove` and operand-stack traffic, which is at most about 1.5% of a Rails request.
C extensions are not affected, because `rubyrs_cext::Value` is its own `u64` handle.

## Decision

- **Ship Option 0.** It removes the out-of-line clone call and adds no risk to the layout
  contract.
- **Defer Options 1 and 2.** Their best case is a single-digit share of a Rails request, and in
  the realistic case the slab indirection on strings and classes plus the extra GC pressure
  eat most of it. The 3.5× Rails gap lives in the interpreter loop and the call path
  (`dispatch_until_inner`, `step_cold`, `do_call`: 34% combined), not in how a `Value` is
  stored.

**Option 0 results** (5 interleaved rounds of the PoC build on the Studio, ns per call unless
noted; 3 more rounds of the final exhaustive-match build landed in the same ranges):

| | base | Option 0 |
|---|---|---|
| `o.r` (attr_reader) | 51.7–55.3 | 48.2–50.6 (about −7%) |
| `while` loop | 24.8–25.7 | 24.5–25.2 |
| `o.n` | flat | flat |
| `o.n1` | flat | flat |
| `o.n2` | flat | flat |
| Rails hello median | 991 req/s | 993 req/s (flat) |

## Consequences

- The ADR 0035 layout contract stands: 16 bytes, `#[repr(u8)]`, the oid at offset 4, the i64
  payload at offset 8.
- The next B item is **B1 (kwargs without a Hash)**: each kwargs call costs 370 ns, against
  19 ns on CRuby. After that come the call-path costs this profile names:
  - the ops that still fall to `step_cold` in hot loops
  - `do_call`
- **Revisit Options 1 and 2 only if a profile shows `Value` glue above about 15%.** That could
  happen on a numeric-heavy workload, where boxed Floats never appear and operand traffic
  dominates. Any revisit starts from a PoC that measures the string and class indirection cost,
  not only the glue removed.
