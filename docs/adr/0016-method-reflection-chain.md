# 0016: Method-object reflection chain — heap layout and design tradeoffs

## Status

Accepted (2026-05).

## Context

A series of ten atomic commits (the "L series", commits `24480aa`
through `33b09b5`) closed out support for the captured-method
reflection family — `Object#method`, `Method#unbind`,
`UnboundMethod#bind`, equality, composition (`>>` / `<<`), curry,
`to_proc`, `Class#instance_method`, `owner` / `receiver`, `hash` /
`source_location`. Each of those was a separate commit with its
own diff_cruby fixture, but several design calls cut across them
and don't fit cleanly in any one commit's message. This ADR
captures the four that would surprise a future reader the most.

The starting point: after K8b (`42830f0`) we had `Value::BoundMethod(ObjId)`
backed by `HeapObj::BoundMethod { recv: Value, name_id: SymId }`
— enough to support `.call` / `.[]` / `.()` and `&m` block
coercion via a lazy forwarder proto. Nothing else.

By the end of the L series we needed all of:

  - A second method-shaped value (`UnboundMethod`) that's
    bind-able into a fresh BoundMethod.
  - A third (`CurriedProc`) that accumulates args across
    successive `.call` invocations.
  - Equality, hash, and identity rules that compose correctly
    across all three shapes.
  - `source_location` returning `[filename, lineno]` for
    user-defined methods — which requires Vm-side access to
    the source text the parser saw.
  - `Method#==` semantics that match CRuby's "same receiver
    identity + same name" rule but also handle inherited
    UnboundMethods (which CRuby treats as equal across
    parent/subclass when they resolve to the same definition).

The four calls below shaped how that landed.

## Decision

### 1. CurriedProc is a heap variant, not a synthesised bytecode proto

`Method#curry` returns a value that, on each `.call`, either
accumulates args (returning a *new* curried value) or invokes
the underlying once the target arity is hit.

The temptation was to encode that state machine in bytecode —
build a synthetic proto whose locals carry `(underlying,
gathered, arity)`, branch on `gathered.len() >= arity`,
either dispatch the underlying or allocate a fresh Block of
the same proto with the new gathered. We already use that
pattern for the `&m → Proc` forwarder (`coerce_bound_method_to_block`)
and for `>>` / `<<` composition.

We didn't:

- The branching is a real one-step state machine: an `if`,
  a conditional alloc, two divergent op streams. A synthetic
  proto would need conditional jumps, `LoadConstInt` for the
  arity comparison, and an `Op::CreateBlock`-style op that
  knows how to splice gathered + new args into a freshly
  allocated `HeapObj::Block`. None of that is currently
  emittable as a bytecode-level primitive; we'd have to
  extend the Op enum.
- The state lives in three values (`underlying`, `gathered`,
  `target_arity`). Stuffing them into a `BlockHandle.captured`
  Vec works but loses the type — every read has to pattern-
  match the captured layout, and a GC walk has to know which
  slot is "gathered" so it can recurse.
- The host-side intercept is small and read-clear:

      if let Value::CurriedProc(cid) = &recv
          && matches!(&*name, "call" | "[]" | "()") {
          let (underlying, gathered, arity) = ...;
          let combined = gathered + args;
          if combined.len() >= arity { invoke } else { new CurriedProc }
      }

  That arm lives in `dispatch.rs` next to the BoundMethod /
  Block call arms; future readers find it where they expect.

So `HeapObj::CurriedProc { underlying: Value, gathered: Vec<Value>, target_arity: u16 }`
joined the heap, with `class_of` reporting it as `Proc` so
script code sees the CRuby-conventional shape. GC walks the
underlying *and* every gathered element (both can hold heap
references).

The cost: a new heap variant means new arms in `Heap::visit_value`,
`class_of`, `type_name`, `to_display`, the lookup `respond_to`
table, and the per-file panic budget for heap.rs (the new
`Heap::curried_proc` accessor). Cumulative diff was small —
each touch was three lines — and the resulting dispatch arm
is the kind of code where "you can read what it does" wins
over "you can read which ops fire".

### 2. `Vm.sources` mirrors Runtime's source map (rather than threading sources through compile)

`Method#source_location` needs to convert a Proto's first
op_span byte offset into a 1-based line number. Byte offsets
come for free (Prism stamps every node), but line resolution
requires the source text — which lives on `Runtime`, not
`Vm`. Dispatch (`do_call`) runs entirely inside Vm; it has
no callback path to Runtime.

Two alternatives we ruled out:

- **Precompute lines at compile time.** Add `start_line: u32`
  to `Proto`. Requires the compiler to also see source text
  (it currently sees only the SExpr stream from Prism, with
  byte-offset spans). Plumbing source through every
  `compile_proto_kind` call site is a large delta with
  collateral effects on test scaffolding that calls
  `compile_block` / `compile_proto_kind` directly.
- **Lazy callback into Runtime.** Hand Vm a `&Runtime` or a
  closure for line resolution. Hard to express through
  `&mut Vm` everywhere; risks Rc cycles or lifetime knots.

We chose:

    pub(crate) sources: std::collections::HashMap<Rc<str>, Rc<str>>

on `Vm`. Runtime's `eval` does:

    self.sources.insert(filename, source.clone());
    self.vm.sources.insert(filename, source.clone());

— a `Rc<str>` clone per eval, no copy of the bytes. Vm
holds the same `Rc` shared with Runtime. Method#source_location
reads it via `crate::error::line_col(src, byte_offset)`.

Tradeoffs we accept:

- Two HashMap inserts per eval (only one before). At our
  eval rate (CLI-driven, embed-time) the cost is invisible.
- Vm now holds a transitive reference to source text. That
  ties the source's lifetime to Vm — which already outlives
  any single eval, so this is a no-op observability change
  for the host. The byte-level cost is `(Rc<str>, Rc<str>)`
  per script eval'd, ~32 bytes per entry.
- Future Vm-side line-resolution helpers (backtrace builders,
  inspect paths that want a file:line annotation) can now
  use the same map without further plumbing.

### 3. `Method#unbind` captures `class_of(recv)`, not the defining class

CRuby's `Method#unbind` returns an `UnboundMethod` whose
`#owner` is the **defining class** of the underlying method
— the class where the `def` actually appeared. For a
method inherited from a parent, that's the parent, not the
child.

Our `UnboundMethod` heap variant captures `class_of(recv)`
instead:

    HeapObj::UnboundMethod { class: Rc<Class>, name_id: SymId }

— so for a child instance whose method comes from a parent,
the captured `class` is the *child*'s class. This was a
deliberate simplification: `class_of(recv)` is already
computed during the unbind dispatch, and the receiver's
class is sufficient for `UnboundMethod#bind(obj)`'s
`obj.is_a?(class)` check.

The cost shows up in `Method#owner` and
`UnboundMethod#owner`: those *do* need the defining class,
because that's what CRuby exposes. They get it by resolving
the captured `(class, name_id)` through
`lookup_method_uncached` to find the `Rc<Method>`, then
reading the Method's `defining_class.upgrade()` (a `Weak<Class>`
since PR #31's anti-cycle fix). The walk pays for the
simplification at unbind time, not at owner-query time.

This is the same approximation already used by
`Class#instance_method` (L5): capture the class you were
asked about, not the class where the method lives.

### 4. `method_recv_identity` / `method_recv_hash` use `equal?`-style identity, not `==`

`Method#==` requires that BoundMethods compare equal iff
they share a receiver. CRuby's rule is identity-based: two
String literals `"foo"` and `"foo"` produce different
`s.method(:length)` BoundMethods, but the *same* literal
produces equal ones. Likewise `7.method(:+) == 7.method(:+)`
holds (Integer is value-typed).

We added a helper:

    fn method_recv_identity(a: &Value, b: &Value) -> bool

that compares heap-managed receivers (Object / Array / Hash
/ Range / Block / BoundMethod / UnboundMethod / CurriedProc)
by `ObjId` equality, Class / Str by `Rc::ptr_eq`, and Int /
Float / Sym / Bool / Nil by value. This matches CRuby's
`equal?` semantics exactly, narrowed to the value shapes
that can appear in a BoundMethod recv slot.

`Method#hash` (L8) needs a partner: any two receivers that
compare equal under `method_recv_identity` MUST hash equal.
We chose the obvious derivation — `id.0 as i64` for heap
shapes, `Rc::as_ptr` for Class/Str, value for primitives —
combined with `name_id` via a golden-ratio multiplicative
mix. Collisions on distinct receiver/name pairs do happen
(8-byte to 8-byte mixing isn't a hash function), but the
only invariant CRuby promises is "equal ⇒ same hash" and
that's what we provide.

The alternative — reusing `Value::ruby_eq` for `Method#==` —
would have been wrong: `ruby_eq` collapses `1 == 1.0`,
treats two String contents as equal even across different
Rc allocations, etc. Method equality must be stricter.

Together these two helpers (one in `dispatch.rs` for `==`,
one mirroring it for `hash`) are the single source of truth
for "is this the same receiver". Any future arm that needs
receiver identity (like `Method#bind` if it ever grows an
`obj.equal?` check) should reach for them.

## Consequences

What gets easier:

- The state machine for curry is one paragraph in
  `dispatch.rs` instead of a synthetic proto wired through
  the bytecode compiler. Future curry-flavoured features
  (`Method#curry_with`, lazy evaluation, `Proc#curry` already
  shipped because the same arm matched both BoundMethod and
  Block) compose by extending the same heap variant.

- `Method#source_location` shipped without compiler changes.
  The Vm-side sources map is also available for any future
  introspection that wants file/line info — the backtrace
  formatter would be a natural next consumer.

- The recv-identity helpers gate any future receiver-equality
  question through one function. We can't accidentally use
  `ruby_eq` in one Method arm and `equal?` in another.

What gets harder:

- A new heap variant means a new line in every heap-walk arm.
  That's mechanical but mandatory — every `class_of`,
  `type_name`, `respond_to`, GC visit, display has to know
  about CurriedProc. The pattern is already established
  (BoundMethod and UnboundMethod followed the same path);
  adding more "callable shapes" in the future will keep
  paying that cost.

- `UnboundMethod#owner` does an O(ancestor-chain) walk per
  call because we capture the receiver's class, not the
  defining class. In the common case the chain is 1-2 hops
  (Object → user class); deeply nested module hierarchies
  could pay more. Reflection isn't a hot path, so this is
  fine — but if it ever becomes one, the fix is to cache
  the resolved `Rc<Method>` on the UnboundMethod variant
  at unbind time.

- `Vm.sources` keeps source text alive for the lifetime of
  the Vm. Scripts that eval hundreds of distinct files
  accumulate. Real-world: the `Runtime` already holds the
  same map, so this isn't new memory pressure — but it's a
  consideration if we ever want Runtime to drop sources
  after compilation. We'd need to either cache the
  source-derived data we need (currently just `line_col` on
  demand) or accept that `Method#source_location` only
  works for the most recently eval'd file.

What we explicitly trade away:

- Method equality across `alias_method` (the existing
  divergence noted in SUBSET.md). Aliased methods produce
  distinct Method values with different name_ids; CRuby
  looks through the alias and equates them. The
  `method_recv_identity` + name_id check can't recover that
  without resolving both sides through the class chain,
  which would change `Method#==` from O(1) to
  O(ancestor-chain).

- BigInt-style hash spreading. `method_recv_hash` is a
  64-bit mix on 64-bit inputs; collisions are inevitable
  but the only correctness invariant ("equal ⇒ same hash")
  is preserved. If a use case ever needs perfect hashing on
  Methods (a Method-keyed Hash with many entries), we'd
  need to widen this — for now, Hash#hash conventionally
  saturates to i64 anyway, so widening doesn't buy much.

## See also

- [ADR 0007 — Host embedding API](0007-host-embedding-api.md): the
  Runtime / Vm split that motivated the `Vm.sources` mirror.
- [ADR 0010 — Metaprogramming PoC](0010-metaprogramming-poc.md):
  the `Weak<Class>` plumbing that `Method#owner` reads
  through.
- [ADR 0013 — `CURRENT_VM_PTR` aliasing](0013-current-vm-ptr-aliasing.md):
  the prior decision to keep Vm state reachable through a
  raw pointer; the new heap variants follow the same "Vm
  is the canonical store" pattern.
