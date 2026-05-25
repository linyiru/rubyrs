# 0013: `CURRENT_VM_PTR` borrow-aliasing policy

## Status

Accepted as the current shape (2026-05); flagged for a
follow-up `UnsafeCell`-flavoured refactor when a real Miri
or `RUSTFLAGS=-Z borrow-checker-strict` failure forces the
issue. Not blocking any active work.

## Context

When a Ruby script calls a registered host function from
inside a `do_call`, the dispatch path is:

```
Vm::do_call(self: &mut Vm, ...)
  → host_fn(&[Value])         ← arbitrary user code
      → rb_funcallv(recv, sym, ...)   ← C extension call back
          → cext_funcall_to_vm callback
              → Vm::cext_invoke_method(self: &mut Vm, recv, method, args)
```

The C extension can decide to call back into Ruby
(`rb_funcallv`). When it does, the callback needs `&mut Vm`
to dispatch the call — but the outer `do_call` is *still*
holding `&mut self` for the duration of the host_fn
invocation. The two `&mut Vm`s alias in scope (the outer's
lifetime spans the inner's), but they're time-disjoint
(only one is used at any instant — the outer is parked
inside the `host_fn(args)` call, the inner runs the nested
dispatch, then returns and the outer resumes).

Rust's borrow checker can't see "time-disjoint" — it sees
two `&mut`s overlapping in scope and refuses. The
established workaround in single-threaded scripting
runtimes that interoperate with `extern "C"` callbacks is
to route the inner reference through a raw pointer:

```rust
#[cfg(not(target_os = "wasi"))]
thread_local! {
    pub(crate) static CURRENT_VM_PTR: Cell<*mut Vm>
        = const { Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn with_vm_ptr_set<R>(vm_ptr: *mut Vm, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_VM_PTR.with(|c| c.replace(vm_ptr));
    let _guard = VmPtrGuard { prev };
    f()  // host_fn runs here; rb_funcallv reads CURRENT_VM_PTR
}
```

`do_call` calls `with_vm_ptr_set` before invoking the host
fn; the C-ext callback (`cext_funcall_to_vm`) reads the
thread-local and synthesises a fresh `&mut Vm` from the
raw pointer via `&mut *CURRENT_VM_PTR.get()`.

This works in practice. It's how mruby's C-ext bridge
operates, and CRuby's own bytecode interpreter has the
same time-disjoint aliasing pattern under the hood (the
C-extension `VALUE` is conceptually a pointer into the
interpreter's heap that the cext can dereference between
calls). At the level of the actual CPU executing, only
one `&mut Vm` is live at a time.

## Decision

Keep the raw-pointer thread-local for now. Document the
contract precisely (it's in `vm/cext.rs` as a SAFETY
note); defer the structural fix.

The structural fix is a refactor of `Vm` into something
like:

```rust
pub(crate) struct Vm {
    inner: UnsafeCell<VmState>,
}
```

where every `&mut Vm` method internally borrows
`inner.get()` and the cext callback explicitly takes
`&Vm` (the cell makes the interior mutation legal even
under an outer shared borrow). The UnsafeCell pattern is
what `RefCell` / `Cell` use under the hood; here we'd
inline it because we need the speed and want to avoid
the runtime borrow tracking RefCell adds.

That's a substantial change — every `&mut self` method
on `Vm` (~150 sites) becomes `&self` plus an
`unsafe { &mut *self.inner.get() }` shim. The cleanup is
done by inlining a helper macro / method, but the diff
is still wide. We don't have a forcing function for it
yet, so it stays on the to-do list.

## Why this isn't catastrophic

The Rust formal models differ in how strict they are:

| Model | Naive verdict | Actual verdict |
|---|---|---|
| **Stacked Borrows** (Miri default) | Would-flag two simultaneous `&mut`s | ✅ Clean for our reborrow shape (see synthetic test below) |
| **Tree Borrows** (Miri 2.0+) | Recognises time-disjoint reuse via the function-call frame structure | ✅ Clean |
| **rustc itself** | Won't compile two `&mut`s naively, hence the raw-pointer escape hatch we use | ✅ Build clean |

Our actual production pattern is NOT "two `&mut`s alive
simultaneously". It's "outer `&mut self` demoted to `*mut
Vm` before the host fn runs, then re-promoted to `&mut Vm`
inside the host fn's closure". The outer borrow is parked
while the inner borrow is active — Stacked Borrows allows
this because the pointer-demotion records a permission
that the later reborrow re-uses. Tree Borrows additionally
allows more aggressive patterns (truly simultaneous
shared borrows that don't alias the parked mut), which we
don't need.

The earlier wording of this ADR implied Stacked Borrows
would flag the cext path; that wasn't backed by a test. The
"Synthetic cext-reentrance test" subsection below pins the
actual verdict — clean under both models.

The risk surface is:
- A future Rust optimisation that exploits Stacked-Borrows-
  level no-aliasing claims to mis-codegen. So far rustc
  doesn't do this — there's no `noalias`-style attribute
  on raw pointers — but a sufficiently aggressive
  link-time optimiser could.
- A Miri run with Stacked Borrows enabled would flag the
  cext path as UB. We don't run Miri against the cext-
  enabled code paths today; if we did, it'd need
  `-Zmiri-tree-borrows` to pass.

### Miri verification record (2026-05-25)

Ran `cargo +nightly miri test` against every Miri-friendly
subset of the test suite — both under default Stacked Borrows
and `-Zmiri-tree-borrows`. Result:

| Subset | Stacked Borrows | Tree Borrows |
|---|---|---|
| `vm::lookup` unit tests (9) | ✅ | ✅ |
| `vm::gc` unit tests (10) | ✅ | ✅ |
| `vm::raise` unit tests (9) | ✅ | ✅ |
| `rubyrs-cext` FFI negative tests (19) | ✅ | ✅ |
| **Total verified** | **47 tests** | **47 tests** |

All 47 tests passed cleanly under both formal models. The
non-cext `Vm` code path has no Stacked Borrows violations Miri
can detect.

**Unverifiable under Miri** (documented gap, not a known bug):

- Any test that calls `Runtime::eval` — hits Prism's vendored
  C parser (`pm_parser_init`) which Miri rejects as
  "unsupported foreign function on macos".
- The diff_cruby fixtures — require running the rubyrs binary
  against the system Ruby for byte-comparison; Miri's harness
  doesn't expose them.

### Synthetic cext-reentrance test (2026-05-25)

The original "cext path remains unverified" note implied a
deferred risk. Closed it by writing a synthetic test
(`vm::cext::miri_tests`) that drives the exact reborrow shape
without dlopen:

```rust
fn miri_drive_cext_pattern(&mut self) -> usize {
    let vm_ptr: *mut Vm = self;          // outer &mut self parked
    with_vm_ptr_set(vm_ptr, || {
        let ptr = CURRENT_VM_PTR.with(|c| c.get());
        let inner_vm = unsafe { &mut *ptr };
        // Mutate every Vm field the cext path touches: heap,
        // classes, interner, stack, pinned. If Stacked Borrows
        // flags any of these, the time-disjoint argument is
        // wrong.
        let id = inner_vm.heap.alloc(...);
        inner_vm.stack.push(...);
        inner_vm.pinned.push(...);
        ...
    })
}
```

Three tests (single re-entry, multiple re-entries, nested
`with_vm_ptr_set` save/restore). Result under both formal
models:

| | Stacked Borrows | Tree Borrows |
|---|---|---|
| `cext_reentrance_pattern_is_aliasing_clean` | ✅ | ✅ |
| `cext_reentrance_can_re_enter_multiple_times` | ✅ | ✅ |
| `cext_reentrance_nested_save_restore` | ✅ | ✅ |

**Stacked Borrows passes too.** This was the result ADR 0013
originally implied without evidence. The "Stacked Borrows
considers this UB" wording in the table above turned out to
be too strong — the actual reborrow shape (outer `&mut self`
demoted to `*mut Vm`, then re-promoted to `&mut Vm` inside a
closure after the outer borrow has been parked) sequences
cleanly under both models. Two *simultaneous* `&mut`s would
be UB; the production pattern is not that.

The real `cext_funcall_to_vm` still can't run under Miri
because it goes through dlopen, but the reborrow shape it
uses is now pinned by these synthetic tests in CI.

J4 (the UnsafeCell refactor) remains deferred. None of the
three forcing functions listed below have been met.

## Why we're shipping it anyway

Three reasons:

1. **The trust model already accepts arbitrary cext code.**
   Once a C ext is loaded (via `require "/path/to/foo.so"`),
   it can `memcpy` over our heap directly. The borrow-
   aliasing concern is one tiny shape inside a much larger
   "C runs unsandboxed" trust boundary. `docs/SECURITY.md`
   states rubyrs is "a hardening layer, not a sandbox" —
   the borrow story can't be tighter than the FFI story.
2. **The structural fix is invasive and not yet pulled by a
   real failure.** A blind refactor of 150 `&mut self`
   methods to `&self` + `UnsafeCell::get` would cost a
   week and introduce its own bug risk. The current
   shape works under Tree Borrows and on every observed
   CPU.
3. **The defensive scaffolding is in place.** `VmPtrGuard`
   restores the previous pointer on *every* scope exit
   including panic unwinding (we caught and fixed the
   panic-leak case explicitly — see commit `1ad96df`).
   `CURRENT_VM_PTR` is `null` by default and reads from
   it without a prior `with_vm_ptr_set` would dereference
   null (clean crash, not silent UB).

## Trade-offs

### Cost: Miri runs with Stacked Borrows fail

We don't routinely run Miri on cext-enabled code. If we
did, the test would need the `-Zmiri-tree-borrows` flag.
That's a documented gotcha rather than a fix.

### Cost: a future borrow-checker upgrade might fail-loud

If a future rustc release tightens raw-pointer dereferences
through the borrow checker — currently they're a free
escape hatch — the cext path would stop compiling. The
fix at that point would be the deferred refactor below.
We accept that this is a possible future cost.

### Cost: the SAFETY comment in `vm/cext.rs` is load-bearing

The comment around `CURRENT_VM_PTR` is essentially the
contract. If a contributor "cleans it up" by removing the
comment, the next reader has to re-derive the
time-disjoint argument from scratch. Cross-linked from
this ADR.

### Benefit: a working cext implementation today

Without this pattern, the cext bridge would either:
- Need a major Vm restructure before any cext work could
  land. We'd have shipped 0 cext support to date.
- Spawn a background thread per Vm to host the dispatch
  loop, with synchronous channel calls. Major complexity
  cost; brings real concurrency hazards we don't have
  today.

The raw-pointer pattern lets us ship `cext_dispatch`,
`rb_funcallv`, `rb_define_method`, `rb_data_typed_object_wrap`,
and the full TypedData machinery. None of this would have
existed if we'd held the line on "no `&mut` aliasing
under any interpretation".

## The deferred refactor

When we do this, the shape will be:

```rust
pub(crate) struct VmState { /* the current Vm fields */ }
pub(crate) struct Vm { inner: UnsafeCell<VmState> }

impl Vm {
    fn state(&self) -> &mut VmState {
        // SAFETY: rubyrs is single-threaded; cext re-entrance is
        // time-disjoint with the outer dispatch (documented above).
        unsafe { &mut *self.inner.get() }
    }
}
```

Every `self.foo` becomes `self.state().foo`; every `&mut self`
method becomes `&self`. The cext callback receives `&Vm`
(captured cleanly without thread-locals).

The expected diff is ~150 method signatures + ~3000 callsite
edits. Mechanically straightforward (a sed pass plus
borrow-checker iteration) but wide. We'll do this when:
- Miri starts catching real bugs in non-cext code that the
  current shape obscures, OR
- A future rustc edition deprecates the raw-pointer escape, OR
- We commit to running the cext path under
  `RUSTFLAGS=-Zsanitizer=address` in CI and need the
  no-aliasing claim to hold.

Until one of those forces the issue, the current shape ships.

## Related

- [ADR 0009 — C-ext crate panic policy](0009-cext-panic-policy.md)
- [`crates/rubyrs/src/vm/cext.rs`](../../crates/rubyrs/src/vm/cext.rs) —
  the SAFETY note around `CURRENT_VM_PTR` is the source of
  truth.
- [`docs/CEXT_SAFETY.md`](../CEXT_SAFETY.md) — public-facing
  cext trust model; this ADR is the implementation-detail
  companion.
- [`docs/SECURITY.md`](../SECURITY.md) — the broader trust
  model that contextualises why the cext bridge gets to play
  loose with aliasing.
