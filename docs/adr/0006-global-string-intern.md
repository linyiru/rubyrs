# 0006: Global string interner with SymId

## Status

Accepted (2026-05).

## Context

Before this commit, each Proto carried its own `strings: Vec<String>`
table. Method names, ivar names, class names, and string literals all
became per-Proto u32 indices. The dispatch loop resolved them via
`self.protos[proto_idx].strings[idx].clone()` — a `String::clone` per
Op execution, plus a `HashMap<String, Rc<Method>>` lookup that hashed
the bytes from scratch each time.

For hot paths this is real cost:

- A method call goes through three operations on the name: extract,
  clone, look up. Each is small; called millions of times it adds up.
- `Symbol == Symbol` (`Value::Sym(Rc<String>)`) had to fall back to a
  string-content compare unless the Rcs happened to share a pointer.
- Duplicate names across protos got duplicated `String`s on the heap.

## Decision

A single Vm-global **`Interner`** maps `Rc<str>` to a typed
**`SymId(u32)`**. Every compile-time string — method names, ivar
names, class names, string literals — gets interned, and every Op
that previously held a u32 index into `Proto.strings` now holds a
`SymId` directly.

Consequences for each Value tag:

- `Value::Sym(Rc<String>)` → `Value::Sym(SymId)`. Symbol equality is
  now a single u32 compare.
- `Value::Str(Rc<String>)` → `Value::Str(Rc<str>)`. `LoadConstStr`
  resolves a `SymId` to its `Rc<str>` and clones the Rc (atomic
  inc); no `String::clone` in the hot path.
- `Class.methods: HashMap<SymId, Rc<Method>>`,
  `Instance.ivars: HashMap<SymId, Value>`,
  `Vm.classes: HashMap<SymId, Rc<Class>>`. All keyed on u32.

`Proto.strings` is deleted. `Proto` keeps `name`, `params`,
`n_locals`, `code`, `op_spans`, `filename`.

The compiler threads `&mut Interner` through `compile_expr` and
friends; previously this was `ProtoBuilder.intern()` (per-proto).
`Vm::new(protos, interner)` takes the populated interner so runtime
resolution works.

## Consequences

Wins:

- 1M fizzbuzz: 484 ms → 408 ms (15% faster). Distance to CRuby +
  YJIT 3.44× → 2.82×.
- Symbol equality is u32 vs u32. Hash keying on Symbol is a u32
  hash; the previous `Rc<String>` keyed Hash compared bytes.
- Methods that are defined once but called from N protos now use
  the same `SymId` across all of them; method dispatch tables
  hash on a tighter key.
- `Op` enum size unchanged: `SymId` is a `u32` newtype.

Costs:

- Interned strings live for the lifetime of the Vm. For a
  short-lived CLI invocation this is fine; for a long-lived host
  embedding the SymId space monotonically grows. If we ever support
  dynamic method definition with arbitrary names from untrusted
  input (e.g. via `eval` or DSL macros), we'd need bounded interner
  semantics. Not on the roadmap.
- The `Symbol#to_s` path now needs access to the Interner to
  materialize the string. We pulled the Sym-specific primitives out
  of the pure `primitive_call` and into `Vm::sym_primitive`.
- `to_display` / `to_inspect` on Value now take both `&Heap` and
  `&Interner`. A future refactor may bundle these into a `VmCtx`
  for cleaner call sites.

## Why not split strings and symbols into separate tables?

We considered separate `StrId` (literal strings) and `SymId` (names).
Two tables prevent type-level confusion (can't mix them) and let us
GC literal strings independently. We chose unified because:

1. Symbols and string literals share the property of being immutable
   compile-time content; deduplication helps both.
2. The newtype `SymId` already prevents int/idx confusion in code.
3. Splitting now would require tagged Value variants per kind and
   double the bookkeeping for marginal gain.

If a future use case (dynamic literal strings, hot reload) demands
separation, the split is local to `Interner` and the Op fields —
not a sweeping change.
