# Changelog

All notable changes to rubyrs will be recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/), and this project
follows [Semantic Versioning](https://semver.org/) once we hit 0.1.

## [Unreleased]

### Added
- **`Kernel.instance_method(:name)` + `RUBY_VERSION` /
  `RUBY_PLATFORM` constants.** Gemfile-shape prelude scripts
  routinely probe `Kernel.instance_method(:require)` and
  branch on `RUBY_VERSION` / `RUBY_PLATFORM`; both shapes
  now work. `Kernel` is recognised as an intentional extra
  in `is_primitive_class_name` (no user-Method table entry,
  but the synthesised UnboundMethod path applies). The
  two constants are populated at Vm boot and frozen via the
  dsl prelude. New diff fixture
  `kernel_instance_method.rb` pins the surface.
- **`Mutex` single-threaded no-op stub.** `Mutex.new` +
  `m.synchronize { … }` + `m.locked?` ship as a compatibility
  shim so tilt / sinatra / dry-struct-style cache-guard
  patterns (`LOCK.synchronize { @cache[k] ||= … }`) load and
  execute. Because rubyrs is single-threaded the lock surface
  degenerates: `synchronize` just yields the block, `locked?`
  always returns `false`, re-entrant `synchronize` succeeds
  rather than deadlocking (a user-friendly divergence vs
  CRuby). `Mutex.new(...)` with extras raises `ArgumentError`
  via an explicit 0-arity `initialize`. The stub does **not**
  belong in Tier 1 by [ADR 0017](docs/adr/0017-tier1-boundary.md)
  Rule 4 — it's a deliberate compatibility deviation, scoped
  to "the `synchronize { }` shape real gems use" rather than
  the full lock state-machine; direct `lock` / `try_lock` /
  `unlock` queries diverge and are documented out of scope.
  Diff fixture `mutex_stub.rb` pins the observable surface.
- **Global variables — `$foo` read + write.** New AST nodes
  `GlobalVariableReadNode` / `GlobalVariableWriteNode`,
  bytecode `Op::LoadGlobal(SymId)` / `Op::StoreGlobal(SymId)`,
  and a `Vm.globals` map that survives across `eval` calls
  (so DSL host setup can stash state). Uninitialised reads
  default to `Value::Nil` (CRuby's lenient semantics). Special
  globals: `$$` reads host process PID (an [ADR 0017](docs/adr/0017-tier1-boundary.md)
  deviation — Rule 1, target tier 2 / Config-injected) and
  `$0` reads the script name. Globals are a GC root so
  values they hold survive collection.
- **`break` / `next` through `ensure` — proper Ruby semantics.**
  Previously, `break` and `next` inside a `while` body that
  walked through one or more enclosing `ensure` clauses either
  skipped the ensure entirely or trapped. Now the unwinder
  runs every `is_ensure` handler between the `break`/`next`
  site and the loop's target IP before landing — mirroring
  CRuby's structured-jump semantics. New `LoopTransfer` slot
  on `Vm` carries kind / target IP / target rescue depth /
  target stack depth across the ensure walk; the parallel
  `loop_stack_depths` stack on each frame snapshots
  `stack.len()` at every `EnterLoop` so the landing path can
  truncate any operand-stack residue (most importantly the
  exception value the unwinder pushes on entering an ensure).
  New diff fixtures cover `break` from ensure, `next` from
  ensure, `break` through nested ensures, and the same shapes
  composed with `rescue`. Adds `vm/raise.rs` (10 ICE-class
  invariant asserts) and a new `Op::EndEnsure` terminator
  (replacing the prior `Op::Raise` the compiler used to emit
  at the tail of every ensure body).
- **`ConstantPath` op-write family.** `Foo::Bar ||= value`,
  `Foo::Bar &&= value`, and `Foo::Bar += value` (plus the
  full arithmetic op-write set) work end-to-end with a
  fallback path for dynamic-head constant paths. Round-trip
  diff fixtures pin every shape against CRuby.
- **cext spike: BigInt protocol round-trip via msgpack/bigint.rb
  (A5 / A6a-A6d).** End-to-end load a real upstream gem-`lib/`
  Ruby helper and exercise its protocol path byte-identical to
  MRI. Scope is explicitly *Tier 1 protocol-compat* (per
  [ADR 0015](docs/adr/0015-concentric-architecture.md)): inputs
  in i64 range round-trip faithfully; values beyond i64 saturate
  at the parser, BigInt arithmetic remains Tier 2 deferred work.
  - **A5 — `require ".rb"` loads Ruby source files.** `require`
    was an alias for `cext_require` (cext bundles only); now it
    detects `.rb` extension (or auto-appends `.rb` when the input
    has none) and routes through a shared
    `load_ruby_source_from_canon` helper factored out of
    `require_relative`. Cext path stays as the fallback for
    native extensions. Resolved cwd-relative; gem-style
    LOAD_PATH walking still deferred. Acceptance:
    `tests/require_rb.rs` with four cases (explicit `.rb`, auto
    append, dedup, RuntimeError fallback).
  - **A6a — pack/unpack endian modifiers
    (`L>` / `L<` / `S>` / `S<` / `Q>` / `Q<` / `q>` / `q<`).**
    Parse the `>` / `<` suffix and normalise to canonical
    directives (`L>` → `N`, `S<` → `v`, …). `Q>` / `q>` get new
    internal `J` / `j` sentinels for BE 64-bit since CRuby
    doesn't expose a single-char form. New diff fixture
    `pack_endian.rb` byte-identical to MRI across modifiers.
  - **A6b — `Integer#[]` bit access (single + two-arg).**
    `n[i]` returns the bit at position `i` as 0/1; `n[offset,
    length]` extracts a bitfield. Two's-complement semantics
    for negatives. `length == 64` with negative receiver
    saturates to -1 (signed all-ones) where CRuby returns
    unsigned `2^64 - 1` — documented divergence, not in the
    bigint.rb hot path. New diff fixture `integer_bit_index.rb`.
  - **A6c — `Class#instance_method` graceful for primitive
    classes.** `Integer.instance_method(:[])` no longer raises
    NameError (primitives have no entries in the user-Method
    table; dispatch happens through `primitive_call`). Synthesise
    an UnboundMethod for the 14 well-known primitive class names
    (Integer / Float / String / Symbol / Array / Hash / Range /
    Regexp / Proc / Method / UnboundMethod / TrueClass /
    FalseClass / NilClass). Downstream `arity` / `parameters`
    arms already fall back to the builtin sentinel (-1, `[[:rest]]`)
    when the Method record is absent. User classes still raise
    NameError on unknown methods. Diff fixture
    `class_instance_method_primitive.rb`.
  - **A6d — msgpack `lib/msgpack/bigint.rb` round-trip.** Vendor
    upstream's `bigint.rb` at `examples/msgpack-cext/vendor-rb/`
    (Apache-2.0, unmodified) and exercise it via Rust
    integration test `cext_msgpack_bigint`. Eight cases across
    `0` / `±1` / `±i32::MAX` / 64-bit values / `i64::MAX`, all
    byte-identical to MRI. Skipped: `i64::MIN` (magnitude-take
    overflows in the `-bigint` step). Test is Rust integration
    rather than `tests/diff/`-style because CRuby uses proper
    `MessagePack::Bigint` nested-module lookup; rubyrs Tier 1
    flattens nested modules to top-level (separate gap,
    deferred per ADR 0015 Tier 2).
  - **Small dependencies added along the way:**
    `Array#shift` / `Array#pop` / `Array#reverse_each` (used by
    `from_msgpack_ext`); `nil.to_i` / `nil.to_f` (used by
    `parts.pop.to_i` when the unpack result is empty).

  This sub-wave completes the Tier 1 protocol-compat scope for
  msgpack: every i64-range value and every standard frame type
  round-trips byte-identical to MRI, including the BigInt
  protocol path. True BigInt arithmetic, Time class, nested-
  module namespacing, and msgpack-ruby's own minitest suite
  remain Tier 2 work.

- **cext spike: msgpack ext-type chain (L3-J / L3-K / A3 / A4).**
  Four atomic commits that close out the "ship a custom Ruby
  class through msgpack's `register_type` ext-type machinery"
  use case end-to-end. The deliverables:
  - **L3-J — `CValue::Symbol(String)` crosses the cext FFI.**
    Adds the variant and wires the Vm ↔ cext translator both
    directions (`Value::Sym(id) ↔ CValue::Symbol(name)` through
    `vm.interner`). Upgrades the previously-stubbed
    `rb_id2sym` / `rb_sym2id` / `rb_sym2str` from "rubyrs has
    no Symbol CValue" placeholders to real implementations
    over the existing thread-local intern table. `rb_value_type`
    returns `T_SYMBOL` (9). Acceptance test
    `cext_msgpack_symbol` exercises five Symbol literals
    across the fixstr/str8 boundary; all five `Packer#write(:sym)`
    → `Unpacker.read` round-trip byte-identical to MRI's
    no-registration default behaviour.
  - **L3-K — Proc/Block crosses the cext FFI.** Adds
    `CValue::BlockRef(u32)` carrying `Value::Block(ObjId).0`;
    new `rb_proc_call_with_block(proc, argc, argv, block)`
    stub forwards through `rb_funcallv(proc, :call, argv)` so
    msgpack's `protected_proc_call_safe` reaches Vm dispatch's
    Block.call arm. `rb_cProc` sentinel handle (20). Surfaced
    and fixed three pre-existing gaps along the way:
      - `OBJ_FROZEN(v)` was hard-coded to `1` —
        every cext mutation path that gates on `if (OBJ_FROZEN
        (self)) rb_raise(FrozenError, …)` was firing. Flipped
        to `0`.
      - `rb_ary_new3(n, ...)` was returning an empty Array
        because stable Rust can't take extern "C" variadics —
        msgpack's ext-type registries stored
        `[ext_module, proc, flags]` triples that emerged
        empty, so every unpack-time lookup returned `Qnil` and
        the proc never fired. Replaced with a header variadic
        macro that counts `__VA_ARGS__` and dispatches to
        arity-specialised non-variadic helpers
        (`rubyrs_ary_new3_1` / `_2` / `_3`).
      - The new helpers needed `#[used]` static references in
        the rubyrs binary or the linker stripped them — bundle
        dlopen'd cleanly but dlsym returned NULL and the first
        call segfaulted. Added the same `#[used]`-static
        pattern the rest of `crates/rubyrs/src/lib.rs` uses.
    Acceptance test `cext_msgpack_proc` registers a Proc for
    ext-type 0x07, feeds pre-built ext8 bytes, verifies the
    Proc was invoked back through cext → Vm.
  - **A3 — Class handle dedup against sentinels for full
    Symbol round-trip.** `CExtState::intern(CValue::Class(name))`
    now collapses to the seeded sentinel handle when the name
    matches one of the 21-slot prelude (covering `rb_cObject`,
    `rb_cString`, `rb_cSymbol`, `rb_cProc`, etc.). Without
    this, a Vm-side `Value::Class(Symbol)` interned at a fresh
    handle distinct from `rb_cSymbol = 10`, and msgpack's
    `if (ext_module == rb_cSymbol) has_symbol_ext_type = true`
    branch silently didn't fire — Symbol values then packed as
    fixstr instead of ext-type 0x00. With the dedup
    `Packer#write(:foo)` (after `register_type_internal(0x00,
    Symbol, proc)`) emits `c7 03 00 66 6f 6f` matching MRI
    byte-for-byte; `Unpacker.read` restores the Symbol type
    through the registered `to_sym`-shaped proc. Acceptance
    test `cext_msgpack_symbol_ext`. User classes (names not in
    the sentinel set) still intern fresh; the dedup is bounded
    by a new `SENTINEL_COUNT = 21` const with a
    `debug_assert_eq!` keeping the seed list and constant
    aligned.
  - **A4 — application-defined ext-types.** Pins the general
    "register custom ext-type for a user class" mechanism via
    `register_type_internal`. Two cases in
    `cext_msgpack_app_ext`: a `Color` user class round-trips
    through ext-type `0x10` with a 3-byte payload; a `Stamp`
    class round-trips through ext-type `-1` (the same id
    msgpack-ruby's `lib/msgpack/time.rb` reserves for `Time`)
    with an 8-byte `[sec, nsec].pack("NN")` payload. Both
    byte-identical to MRI. Mixed-frame coverage (Int + Color +
    String in the same buffer) verifies ext frames coexist
    with normal frames without disturbing adjacent reads. Real
    `Time` support is a separate subset addition (no `Time.now`
    / `#to_i` / `#nsec` / `Time.at(sec, nsec, :nsec)` yet);
    when it lands, the same Proc shape applies unchanged —
    only the class arg changes.

  Net effect of this cext sub-wave: 20 cext tests across 13
  files all green; perf budgets within 35-50% headroom; Miri
  (SB + TB) clean.

- **Method / UnboundMethod / Proc reflection chain.** Full
  surface for the captured-method object family, in atomic
  commits:
  - **`Method#unbind` / `UnboundMethod#bind(obj)`.** Strip the
    receiver, keep `(class_of(recv), name_id)`; bind checks
    `is_a?(class)` and rebuilds a BoundMethod (TypeError on
    mismatch). Subclass instances bind fine.
  - **`Method#arity` / `#parameters`.** Walks the captured
    class chain to find the user `Method` record; computes
    arity with CRuby's keyword rule (a required keyword bumps
    mandatory count by 1, optional kwargs / kw_rest push to
    negative). `parameters` returns `[[kind, name], ...]`
    pairs with `:req` / `:opt` / `:rest` / `:keyreq` / `:key`
    / `:keyrest`; builtins fall back to `arity == -1` /
    `parameters == [[:rest]]`.
  - **`Method#==` / `UnboundMethod#==`.** Identity on the
    BoundMethod's receiver (Rc-ptr / ObjId / value depending
    on type) plus name_id; UnboundMethod compares resolved
    `Rc<Method>` by pointer so inherited methods across
    parent/subclass compare equal.
  - **`Method#>>` / `#<<` composition.** Shared lazily-built
    forwarder proto that captures (outer, inner) plus a rest
    slot for args; runs the chain in the right order. Both
    BoundMethod and Block are accepted on either side, so
    `f >> g` where one is a `Method` and the other a `Proc`
    just works.
  - **`Method#curry` / `Proc#curry`.** Host-side
    `HeapObj::CurriedProc { underlying, gathered, target_arity }`
    that gathers args across successive `.call` / `.[]` / `.()`
    invocations until the arity is hit, then dispatches the
    underlying. `class_of` reports `Proc`. Explicit arity hint
    (`m.curry(n)`) honoured.
  - **`Method#to_proc`.** Explicit form of the implicit `&m`
    coercion; reuses the existing `coerce_bound_method_to_block`
    forwarder.
  - **`Class#instance_method(:sym)`.** Direct `UnboundMethod`
    construction without the `Object.new.method(:sym).unbind`
    detour. `NameError` if the method isn't anywhere in the
    chain.
  - **`Method#owner` / `#receiver`.** Owner walks
    `Method.defining_class.upgrade()` so an inherited method's
    owner is its defining class (not the receiver's class).
    Receiver returns the captured Value for BoundMethod;
    UnboundMethod#receiver raises NoMethodError to match
    CRuby.
  - **`Method#hash` + `#source_location`.** Hash is derived
    from receiver-identity + name_id, mixed with a
    golden-ratio constant so `==` Methods collide. source_location
    returns `[filename, lineno]` for user methods (resolved
    via a Vm-side mirror of Runtime's source map);
    builtins return `nil`.
  Also surfaced and fixed a pre-existing toplevel-block slot
  collision: `f = ->(a, b) { ... }; x = 99` was clobbering `x`
  through the lambda's `b` slot. `compile_block` now propagates
  the inner builder's `n_locals` back to the parent so outer
  slot allocations don't reuse a block's reserved range.
  New diff fixtures: `method_introspect`, `method_equality`,
  `method_compose`, `method_curry`, `unbound_method`,
  `class_instance_method`, `method_to_proc_explicit`,
  `method_owner_receiver`, `method_hash_source`,
  `proc_curry_compose`. All byte-identical to CRuby.

- **SUBSET-roadmap fill-ins.** A batch of small atomic
  additions plugging high-priority gaps in the SUBSET coverage:
  - **`Integer#digits([base])` / `#bit_length`.** LSB-first
    digit Array (default base 10); custom base must be ≥ 2.
    `bit_length` uses two's-complement semantics for
    negatives, so `-1.bit_length == 0` and
    `-256.bit_length == 8`.
  - **`String#squeeze([charset])`.** Collapse consecutive
    identical chars; with a char-set, only chars in the set
    squeeze. Char-set ranges (`"a-z"`) and `^`-negation are
    NOT expanded — same conservative semantics as `tr`,
    documented in SUBSET.md.
  - **`String#scan` regex + block form.** Extends from
    string-only to also accept Regex patterns with CRuby's
    capture-group rule (groups → Array-of-captures per match;
    no groups → match string). Block form yields each match
    and returns the receiver.
  - **`Enumerable#chunk_while`.** Partition into runs where
    the 2-arg block `{|a, b| ...}` returns truthy for adjacent
    pairs. Returns a materialised Array (no Enumerator type);
    idiomatic `.to_a` use works unchanged.
  - **`Enumerable#min_by(n)` / `#max_by(n)`.** Top-n forms.
    Sort by key + truncate (O(n log n), fine at our sizes).
    Edge cases match CRuby: `n=0 → []`, `n > len → all`,
    `n<0 → ArgumentError`.
  - **`String#center` / `#ljust` / `#rjust`.** All three pad
    to width with an optional pad string (default `" "`); pad
    cycles when multichar. CRuby's odd-total center rule
    (extra char on the right). Empty pad raises ArgumentError.
  - **`Array#bsearch`.** Block-form binary search with
    CRuby's two modes — find-minimum (Bool/nil block return)
    and find-any (Int return: 0 = match, sign drives
    direction). Other block returns raise TypeError.
  - **`Hash#transform_keys` / `#transform_values`.** Both
    block-form, both non-mutating; `transform_keys` collisions
    follow CRuby's later-wins iteration order.
  - **`Hash#except` / `#slice`.** Subset projections. `except`
    drops listed keys in receiver order; `slice` keeps listed
    keys in ARGUMENT order (CRuby semantics — not receiver
    order).
  - **`Array#take_while` / `#drop_while`.** Prefix partitions.
    `drop_while` stops at the first falsy block return and
    keeps every element from that point — the block is not
    re-invoked on the remainder.
  - **`Array#tally`.** Counts each element into a Hash keyed
    by element, ordered by first appearance. (`tally_by` from
    the open Ruby proposal #16504 isn't shipped in MRI yet,
    so the commit covers just `tally`.) Documented divergence:
    CRuby uses `eql?` so `1 == 1.0` distinct; the subset uses
    `==` and collapses.
  - **`Comparable#clamp(Range)`.** Range-arg form for the
    Comparable mixin. Nil bounds are honoured for one-sided
    ranges (`(..max)` / `(min..)`). 2-arg form still works.
    Numeric primitives don't include Comparable in the subset
    yet, so the fixture exercises user-class instances.
  - **`Float#round(n)` / `#truncate(n)`.** Precision-arg
    forms. `n > 0` returns Float; `n == 0` returns Int (same
    as no-arg); `n < 0` zeroes low-order digits and returns
    Int. Lives before the broader `(Float, op, [Int])`
    coercion arm in `numeric_call` — placing it after would
    shadow it (the shadow lesson is logged in the commit).
  - **`Hash#compact` / `#compact!` + `Array#filter_map` /
    `Hash#filter_map`.** `compact!` returns `nil` if there
    were no nils to drop (matches CRuby's "nil = unchanged"
    convention). `filter_map` uses strict truthiness;
    Hash#filter_map collects truthy results into a flat
    Array (not a Hash).
  - **`Array#combination(n)` / `#permutation([n])`.**
    Lexicographic enumeration of n-element subsets and
    n-element ordered arrangements. Permutation defaults to
    full length. Edges: `n=0 → [[]]`, `n > len → []`.
  - **`Array#assoc` / `#rassoc`.** First sub-Array whose `[0]`
    (assoc) or `[1]` (rassoc) equals the needle. Non-Array
    elements in the receiver are silently skipped.
  - **`Range#cover?(Range)` + `Range#step` block form.**
    `cover?(other_range)` is true iff other is fully contained;
    empty sub-ranges (begin ≥ end excl, or begin > end incl)
    do NOT cover, matching CRuby. `step` block-form yields
    each step value and returns the receiver.
  - **`Object#methods` / `#instance_variables`.** Methods
    walks the user-class chain (own → includes → superclass)
    and returns Symbols; primitives currently return `[]` (no
    per-Kernel-method enumeration in the subset — documented
    divergence). instance_variables returns `@`-prefixed
    Symbols for Object instances, `[]` for everything else.
  - **`String#encode` / `#force_encoding` (stubs).** The
    subset has no per-string encoding tag (raw `Vec<u8>`
    backing since PR #53), so both methods are no-ops that
    return the receiver (Rc-shared, no copy). Useful for
    compatibility with library code that defensively calls
    `.force_encoding("UTF-8")` at boundaries. Cross-encoding
    transliteration is explicitly out of scope.
  - **`String#unpack` + `Array#pack` (subset).** Binary
    packing/unpacking for the directives the niche actually
    exercises: `C / c` (8-bit), `n / N` (BE 16/32), `v / V`
    (LE 16/32), `q / Q` (64-bit native LE), `a / A / Z`
    (raw / space-null-trimmed / null-terminated strings).
    Counts (digits or `*`) honoured; whitespace in the format
    silently ignored. Unsupported directives (m, U, w, f/d/e/E,
    etc.) raise ArgumentError. `String#bytes` shipped alongside
    for inspecting packed output without a `unpack("C*")`
    round-trip.

  Net effect of this batch (Method-reflection wave + SUBSET
  fill-ins): ~33 atomic commits, 134 byte-identical fixtures
  in `tests/diff/*.rb`. Each addition shipped as a single
  commit; per-file panic budgets re-verified after each;
  full Miri sweep (Stacked + Tree Borrows) and perf baseline
  ran clean throughout.

- **`String#sub` / `#gsub` / `#tr` (literal forms).**
  Three commonly-needed string transformations. `sub` replaces
  the first occurrence of the literal pattern; `gsub` replaces
  every occurrence; `tr` does character-by-character
  translation with CRuby's "stretch" rule (chars past the end
  of `to` map to its last char; empty `to` deletes). All three
  honour `Config::max_value_bytes` on the result. Regex forms
  (`gsub(/pat/, ...)`) are explicitly out of scope until a
  regex engine lands. Character ranges in `tr` (`"a-z"`) also
  deferred — both gaps flagged in `SUBSET.md`. New diff
  fixture `string_transform.rb` covers happy paths,
  no-match passthrough, empty-pattern edge cases (CRuby's
  `gsub("", "X")` per-boundary insertion), composition with
  default arguments (a `slugify(s, sep = "-")` example), and
  `respond_to?` reachability. Byte-identical to CRuby.
- **`<=>` spaceship operator.** Returns `Integer(-1/0/1)`
  ordering or `nil` when the pair isn't comparable. Per-type
  arms in `primitive_call` cover `Int <=> Int`,
  `Float <=> Float`, `Int <=> Float` / `Float <=> Int`
  (numeric coercion), `String <=> String`; `sym_primitive`
  handles `Symbol <=> Symbol` (lexicographic on interned
  name). `Bool <=> Bool` returns `0` for the same singleton
  and `nil` otherwise — matching CRuby's default
  `Object#<=>` because TrueClass/FalseClass don't override
  it. `nil <=> nil` is `0`. NaN-involved Float comparisons
  return `nil`. Cross-type returns `nil` via per-built-in-lhs
  catch-alls. For `Value::Object` receivers, user-defined
  `<=>` wins via the normal class-method-lookup path; an
  unhandled `Object` lhs falls through to a `do_call`
  universal that returns `0` for identical `ObjId` and
  `nil` otherwise (CRuby's default `Object#<=>`). New diff
  fixture `spaceship.rb` covers every type combination plus
  a user-defined `Version#<=>` for a real sort-key idiom.
  Byte-identical to CRuby.
- **`attr_accessor` / `attr_reader` / `attr_writer`.**
  Compile-time desugar: a no-receiver call to any of these
  with all-Symbol-literal args expands into `Op::DefMethod`
  pairs in place — `attr_accessor :name, :age` becomes
  `def name; @name; end; def name=(val); @name = val; end;
  def age; @age; end; def age=(val); @age = val; end`. Used
  inside a class body, the methods land on the surrounding
  class via the normal `DefMethod` path; the generated
  setter returns the assigned value (`x = (c.v = 99)`
  yields 99) because our `IVarWrite` lowers as `<val>; Dup;
  StoreIvar`. Synthesised getters/setters interact normally
  with inheritance, default-arg methods, and method
  chaining — all covered in the new `attr_accessor.rb` diff
  fixture. Byte-identical to CRuby.
- **Universal `!` (unary not) and `!@`.** `Bool#!`,
  `Nil#!`, and `Foo#!` all needed to work for `!@secret.nil?`-
  shaped predicates to dispatch. Added as a catch-all
  primitive arm — `!recv` returns `true` iff `recv` is `nil`
  or `false`. `!@` is the alternate spelling some metaprogramming
  uses; both names route to the same arm.
- **`Float` type (MVP).** `Value::Float(f64)`, literal parsing
  via Prism's `FloatNode`, new `Op::LoadConstFloat(f64)`. Pure
  Float arithmetic (`+ - * / %` and comparisons), mixed
  Int/Float coercion ("Float wins" — `5 + 0.5 == 5.5`),
  cross-numeric equality (`5 == 5.0` is `true`). Methods:
  `to_i` / `to_f` / `to_s` / `abs` / `-@` / `+@`, predicates
  (`zero?`, `positive?`, `negative?`, `nan?`, `finite?`),
  `infinite?` returning `1` / `-1` / `nil`,
  `floor` / `ceil` / `round` (all Integer-returning).
  Companion conversions: `Integer#to_f`, `String#to_f` (CRuby-
  lenient parse: leading whitespace, optional sign, optional
  exponent, junk-tail → 0.0). `ruby_eq` extended for
  Float-Float and Int-Float coercion. Preamble adds
  `class Float; end` so `1.5.class.name == "Float"`. `class_of`
  and `responds_to` extended.
  Diff fixture `float_basics.rb` (~60 lines) covers literals,
  arithmetic, coercion, comparisons, conversions, predicates,
  rounding, Infinity/NaN sentinels, class identity,
  `respond_to?`. Byte-identical to CRuby in the everyday
  magnitude range; scientific notation (≥ `1e16`, `< 1e-3`)
  diverges from CRuby's formatter and is a documented gap in
  SUBSET.md.
- **`Object#class`, `Class#name` / `#to_s` / `#==` / `#!=`**.
  `obj.class` returns the Class associated with any receiver —
  for user instances it's the instance's stored class; for
  built-in types the preamble now installs stub classes
  (`Integer`, `String`, `Symbol`, `Array`, `Hash`, `Range`,
  `TrueClass`, `FalseClass`, `NilClass`, `Proc`, `Class`)
  that `class_of` looks up via the class table. The stub
  bodies are empty — built-in method dispatch still goes
  through `primitive_call` / `collection_call` before any
  class-table lookup, so re-opening these from user code
  doesn't shadow the primitive arms (documented in
  SUBSET.md, follow-up). `Class#name` and `#to_s` return the
  name as a String. `Class#==` and `#!=` use `Rc::ptr_eq` —
  reopened classes share their `Rc<Class>` via the class
  table, so identity is the right semantics. Unblocks the
  `e.class.name == "MyError"` and `obj.class == MyClass`
  idioms ubiquitous in exception-handling and pattern-matching
  code. New diff fixture `object_class.rb` covers built-in
  types, user-class identity, name lookup, the meta level
  (`Animal.class == Class`), and class-name dispatch inside a
  rescue handler. Byte-identical to CRuby.
  detection. Accepts either a `Symbol` (`:length`) or a
  `String` (`"length"`). For `Value::Object` receivers walks
  the class chain via `lookup_method_uncached` — the precise
  answer for the common case (`spec.respond_to?(:add_dependency)`
  in gemspec eval). For built-in types (Int, Str, Sym, Array,
  Hash, Range, Bool, Nil, Class, Block) it consults an
  enumerated method list; universal methods (`nil?`, `to_s`,
  `respond_to?` itself, `==`, `!=`) are matched first
  regardless of receiver. The enumeration has to stay in sync
  as new built-in methods land — same maintenance shape as
  CRuby's per-class method tables. New diff fixture
  `respond_to.rb` covers universals, built-in positive cases,
  built-in negative cases, both Sym and String args,
  inherited methods on user classes, and the
  feature-detection idiom (`if x.respond_to?(:upcase)`).
  Byte-identical to CRuby.
- **Default method arguments** (literal defaults). `def foo(x,
  y = 1, msg = "hello")` and `def open(path, mode = nil)` now
  compile and run. Defaults are restricted to literal Values —
  `Int`, `String`, `true` / `false`, `nil`. Other expression
  shapes (method calls, references to earlier params) surface
  as a `SyntaxError` Trap at AST translation time instead of
  silently miscompiling. This unblocks `Gemfile`-style DSL
  methods (`gem "rake", "~> 13.0"` etc. via `def gem(name,
  version = nil, **opts)`) and the common `def initialize(x,
  y = nil)` pattern in classes. Implementation: AST translator
  walks Prism's `optionals()` parameters; `Proto` gains a
  `defaults: Vec<Option<Value>>` parallel to `params`;
  `invoke_method_with_block` fills omitted slots from the
  literal at invocation time. New diff fixture
  `default_args.rb` covers all-defaults, mixed required +
  optional, `nil` / `true` literal defaults, and use inside a
  class.
- **`Object#nil?` returns `false` for every non-nil receiver.**
  We previously only had `Value::Nil.nil?` → `true`; calling
  `"abc".nil?` or `5.nil?` raised NoMethodError. Added a
  catch-all `(_, "nil?", [])` arm — matches CRuby's
  `Object#nil?` semantics.
- **`docs/SECURITY.md`** (P2-15): trust model, configuration
  recipe for the semi-trusted profile, known attack surface
  with what each cap defends against, and an explicit
  "rubyrs is a hardening layer, not a sandbox" boundary —
  WebAssembly + `wasmtime` is the answer for untrusted code,
  not the rubyrs caps alone. Catalogues residual risks the
  caps don't cover (host-function blocking, stdout
  back-pressure, HashMap order side channel, the
  `uncaught exception → process::exit(1)` gap). Cross-linked
  from the README.
- **Per-value byte cap** (P2-14c). New `Config::max_value_bytes:
  Option<usize>`. Individual `String` / `Array` / `Hash` values
  can't grow past `n` bytes of content. String size is byte
  length; Array size is `len * size_of::<Value>()`; Hash size
  is `len * size_of::<(Value, Value)>()`. Checked at the
  mutation points the cap exists for: `String#+` and `String#*`
  refuse before allocating the result; `Array#push` / `Array#<<`
  / `Array#[]=` refuse before appending; `Hash#[]=` refuses
  before inserting a new key (existing-key updates pass
  through). Closes the `"a" * 10_000_000` and `arr << i` in a
  loop attack vectors — both used to be single ops or single
  objects (no fuel/heap-cap signal) that hogged RAM. CLI:
  `RUBYRS_MAX_VALUE_BYTES=N` env var. `primitive_call` upgraded
  to `Result<Option<Value>, RubyError>` so its string arms can
  surface the trap; the four callers (do_call, do_call_block,
  the two BinOpInt fallback paths) wrap the error via
  `Vm::trap`.
- **Interner cap** (P2-14b). New `Config::max_symbols:
  Option<usize>`. Runtime intern paths (currently `String#to_sym`)
  check the cap before adding a fresh symbol and trap with
  `ResourceExhausted("interner exhausted: N symbols")` if the
  count would exceed `N`. Re-interning a string that already
  lives in the table re-resolves without growth, so loops like
  `500.times { "foo".to_sym }` stay free; only fresh strings
  count. Compile-time intern (method names, ivar names, source
  string literals) is not capped — it's already bounded by the
  source size the host chose to feed `eval`. CLI exposes the
  knob via `RUBYRS_MAX_SYMBOLS=N`. New `Runtime::symbol_count()`
  lets hosts size the cap relative to the baseline set up by
  the preamble.
- **Wall-clock deadline cap** (P2-14a). New `Config::deadline:
  Option<Duration>`. When set, `eval` traps with
  `ResourceExhausted("wall-clock deadline exceeded")` if it
  runs longer than the budget. The check is amortised — once
  every 1024 ops via a wrapping op counter — so the no-deadline
  case adds one increment plus one bitmask per op and never
  reaches for `Instant::now()`. The deadline is per-`eval`: each
  call re-anchors the clock so a host can reuse a `Runtime`
  across many short evaluations without inheriting a stale
  timer. CLI exposes the knob via `RUBYRS_DEADLINE_MS=N`.
- **`rescue ClassName => e` (class-filtered rescue)** and
  multiple `rescue` clauses per `begin/end` (P1-10). `RescueClause`
  in the AST gains `classes: Vec<String>` and `Expr::Begin.rescue`
  becomes `Vec<RescueClause>` (chained via Prism's `subsequent()`).
  `Op::PushRescue` carries a `SymId` filter; the VM resolves it
  to a class at push-time and `unwind_with_exception` pops past
  handlers whose filter doesn't match the raised exception's
  class chain. Multiple clauses are pushed in REVERSE source
  order so the LIFO unwinder checks them in source order. Bare
  `rescue` (no class) still compiles with `StandardError` as the
  filter — same behaviour P0-1 introduced. `raise SomeError`
  (no message) and `raise SomeError, "msg"` are now supported:
  the latter desugars to `SomeError.new("msg")` at compile time
  so the user's `initialize` runs. New diff fixture
  `rescue_by_class.rb` covers exact-class catch, superclass
  catch, source-order priority on multiple clauses, no-bind
  form, and a `begin/rescue/ensure` combo. Byte-identical to
  CRuby.
- **`docs/SUBSET.md § Divergences`**: documents the cases where
  rubyrs intentionally diverges from CRuby — unresolved class
  in `rescue`, `ResourceExhausted` un-catchability,
  single-class-only in multi-class rescue, `Foo::Bar` falling
  back to the trailing segment. Each pinned by a test.
- **`docs/PANIC_AUDIT.md`** (P0-4): classification of every
  `panic!` / `.unwrap()` / `.expect(...)` in the rubyrs crate.
  Three buckets — 🟢 ICE (compiler-guaranteed invariant), 🟡
  ICE-but-fuzzy (reachable via internal bugs only, exercised
  in P3-17 fuzz target), 🔴 user-reachable (must be converted
  to `Trap`). Current totals: vm.rs 61 / heap.rs 9 / ast.rs 3
  / lib.rs 1 / compiler.rs 1, all 🟢 or 🟡 after this change.
- **CI `panic-budget` job** (P0-5): counts panics per file and
  fails the build if any count rises above the threshold
  recorded in `docs/PANIC_AUDIT.md`. Doc-comment occurrences
  (`///` / `//!` lines) are excluded. Direction is one-way:
  budgets may only ratchet down.
- **Hash extras + short-circuit `||` and `&&`** (P3-B-3). New
  `Hash` methods: `merge` (other's keys overwrite, ordering
  follows CRuby), `to_h` (identity), `to_a` (Array of `[k, v]`
  Arrays), `delete(k)` (returns removed value or nil, mutates),
  `invert` (later-source-key wins on collision), `store(k, v)`
  (alias for `[]=`). `each` now also matches `each_pair`. Adds
  `Expr::Or` and `Expr::And` to the AST, wires Prism's
  `OrNode`/`AndNode`, and compiles them with short-circuit
  semantics using `Dup` + `JumpIfFalse` (`||` keeps `a` if
  truthy; `&&` keeps `a` if falsy). Sort comparator now also
  orders `Symbol`s lexicographically (by interned string), so
  `hash.keys.sort` works on symbol-keyed hashes. New diff
  fixture `hash_extras.rb` (~100 lines) covers each method,
  empty-collection edges, an invert-collision case, a chained
  `merge.each` pattern, and a `Tally` class that uses
  `(@counts[key] || 0) + 1` + `keys.sort` + method chaining.
- **Array combination & iteration extras** (P3-B-2). New
  no-block methods on `Array`: `reverse`, `uniq` (uses Ruby
  equality, preserves first-seen order), `compact` (drops nils),
  `flatten` (depth 1, matching the cases our fixtures exercise),
  `join` (no-arg + explicit separator), `+` (concat to new), `-`
  (set-style difference), `concat` (in-place, returns self),
  `take(n)`, `drop(n)`, `to_a` (identity). New block-taking
  methods in the iterator-driver region: `each_with_index`
  (block receives `|v, i|`) and `sort_by` (compute key per
  element via the block, then sort element/key pairs by key —
  reuses `value_cmp` so block-keys outside Int/Str return
  NoMethodError instead of silent equal-everywhere ordering).
  Also adds the unary-minus method `Integer#-@` (and `+@`) so
  expressions like `arr.sort_by { |n| -n }` work — Prism lowers
  unary minus on a variable as a `-@` method call. New diff
  fixture `array_extras.rb` (~95 lines) covers each method,
  empty-array edges, mutation vs new-allocation contracts for
  `concat` vs `+`, a string-corpus pipeline
  (`text.split.uniq.sort`), and use inside a class. Byte-
  identical to CRuby.
- **Integer predicates + iteration + String basics** (P3-B-1).
  `Integer`: `even?`, `odd?`, `abs`, `zero?`, `positive?`,
  `negative?`, `succ` / `next`, `pred`, plus block-taking `upto`
  and `downto` that mirror `Range#each`'s short-circuit-on-break
  behaviour. `String`: `length` / `size`, `empty?`, `upcase`,
  `downcase`, `reverse`, `strip` / `lstrip` / `rstrip`,
  `include?`, `start_with?`, `end_with?`, `to_i` (CRuby-lenient:
  leading whitespace, optional sign, trailing junk -> 0),
  `*` (repeat), lexicographic `<` / `<=` / `>` / `>=`,
  `chars` (returns an Array of single-char strings), `split`
  (no-arg = whitespace; explicit separator; empty separator =
  per-character), and `to_sym`. New diff fixture
  `int_string_basics.rb` (~90 lines) covers every method plus
  chained idioms (`split.map { ... }`), use inside a class with
  string interpolation, and edge cases like negative `abs`,
  `to_i` on garbage input, and `upto` with start > stop.
  Byte-identical to CRuby.
- **Enumerable aggregation: `inject` / `reduce`, `sum`, `count`,
  `min` / `max`, `sort`** (P3-A-3). `inject`/`reduce` support all
  three CRuby call shapes: block-only (first element seeds),
  block-with-init, and symbol-shorthand (`:+` / `:-` / `:*` etc.,
  dispatched through `BinOpKind::from_op_name`). `sum` accepts an
  optional Int initial value. `count` supports the no-arg form
  (= `size`), the eql-needle form, and the block form (count
  truthy). `min`/`max`/`sort` accept no comparator and work on
  homogeneous arrays of Int or String — block-comparator forms
  are deferred. Range#sum uses the closed-form n(n+1)/2 instead
  of materialising the elements.
- New diff fixture `enumerable_aggregate.rb` covers ~40 cases
  across Array and Range, including method-call inside a class
  (`@values.inject(0) { ... }`), Range#sum on 1..100 = 5050,
  empty-collection edges, and idioms like `select.sum` and
  `map.inject(:+)`. Byte-identical to CRuby.
- **Enumerable filtering: `select` / `reject` / `find` / `any?` /
  `all?` / `none?` / `include?`** across `Array`, `Hash`, and
  `Range` (P3-A-2). Block-taking variants share a single iterator
  driver per receiver type — `iter_array_filter`,
  `iter_hash_filter`, `iter_range_filter` — parameterised by an
  `IterMode` enum so the GC-pinning / break-propagation /
  short-circuit logic only lives in one place per collection.
  `filter` is registered as an alias for `select`; `detect` as an
  alias for `find`; `has_key?` / `key?` / `member?` as aliases
  for `Hash#include?`. `Hash#find` returns a `[k, v]` Array (or
  `nil`) to match CRuby. Empty-collection cases preserve Ruby's
  vacuous-truth semantics (`[].all? → true`, `[].none? → true`,
  `[].any? → false`). New diff fixture `enumerable_filter.rb`
  (~85 lines) covers every method on every receiver, alias
  dispatch, empty-collection edges, chaining (`select.map.each`),
  and use inside a class body. Byte-identical to CRuby.
- **`Range` values + `Range#each` + Range basics** (P3-A-1).
  `1..5` and `1...5` (exclusive) now parse. New `Value::Range`
  (heap-managed; the existing GC walks `begin` and `end`). New
  `Op::NewRange(excl_flag)` pops two values from the stack and
  allocates the Range. Integer-endpoint ranges support
  `.each { |i| ... }`, `.map { |i| ... }`, `.to_a`, `.size` /
  `.length` / `.count`, `.first` / `.begin`, `.last` / `.end`,
  `.min`, `.max` (respects `exclude_end?`), `.include?(n)`, and
  `.exclude_end?`. Non-Int endpoints (e.g. `:a..:z`) are out of
  scope for now and fall through to NoMethodError.
- New diff fixture `range_basics.rb` exercises both inclusive and
  exclusive ranges, iteration, mapping, empty/inverted ranges,
  and Range usage inside a class method. Byte-identical to CRuby.
- **Brewfile DSL demo + benchmark** (P2-A). New
  `examples/brewfile/` directory: a 50-line Brewfile-shaped Ruby
  script (`tap`, `brew`, `cask`, `mas`, plus a class definition
  and a `.each` loop) hosted via four `Runtime::register_fn`
  calls. End-to-end wall time including cold start, parse, and
  eval:
    - rubyrs (embedded):  1.8 ms
    - CRuby 3.4 no-JIT:  74.7 ms
    - CRuby 3.4 + YJIT:  75.5 ms
  **42× faster end-to-end** for this shape of workload (YJIT
  doesn't help because most of CRuby's time goes to startup, not
  arithmetic). The product-niche benchmark.
- README and `docs/BENCHMARKS.md` now lead with this number.
- **`return` / `break` / `next`** (P2-C-4). All three compile to
  the existing Op::Return frame-pop, with `Op::Break` adding a
  `Vm.break_signaled` flag check that iteration drivers
  (Array#each, Array#map, Hash#each, Integer#times) consult after
  each block invocation. When set, the driver clears the flag,
  uses the block's last produced value as the iterator's return
  value, and stops the loop. Without `break`, drivers return their
  documented default (source for #each, accumulator for #map).
- New diff fixture `control_flow.rb` covers `return` mid-method,
  `return` with no arg, `break <val>` from inside a block,
  `break` without arg returning nil, `next` to skip an iteration
  of #each, `break` inside #map returning the break value, and
  `5.times { break i if i == 2 }`. Byte-identical to CRuby.
- **`ensure` clause** (P2-C-3). `begin ... ensure ... end` now runs
  the ensure body on both the normal-exit path and the exception
  path. The compiler emits a `PushEnsure(handler)` before the
  body; on normal completion it emits `PopEnsure` and the ensure
  body inline, then jumps past. On exception, the unwinder treats
  ensure handlers specially: it pushes the exception value onto
  the operand stack and jumps to the handler, which runs the same
  ensure body and ends with `Raise` to rethrow. Compose freely
  with `rescue`: `begin body rescue => e rescue_body ensure
  cleanup end` all work.
- **`raise "msg"` auto-wraps to `RuntimeError.new("msg")`** at the
  Op::Raise site (new `Vm::normalize_exception`). Brings rubyrs
  in line with CRuby's Kernel#raise convention so `e.message`
  works after `raise "..."`. Already-Exception instances pass
  through unchanged.
- New diff fixture `ensure_basics.rb` covers four shapes: normal
  body+ensure, rescue+ensure, ensure-around-uncaught (exception
  still propagates), and multi-statement ensure body. Byte-
  identical to CRuby.
- `tests/fixtures/exception.rb` updated to use `e.message` since
  `raise "x"` now produces an Exception instance, not a String.
- **Built-in exception class hierarchy** (P2-C-2). `Runtime::new`
  now `eval`s a small preamble that defines `Exception`,
  `StandardError`, `RuntimeError`, `NoMethodError`,
  `ArgumentError`, `TypeError`, `NameError`, `ResourceExhausted`,
  with `Exception` providing `initialize`, `message`, `to_s`.
  Each `StandardError` descendant inherits from the level above.
  This is deliberately *Ruby code at the Ruby level* (no special-
  cased C structs), so user-defined `class MyErr < StandardError;
  end` Just Works and `raise MyErr.new("x"); rescue => e;
  puts e.message` produces the same output as CRuby.
- New diff fixture `custom_exception.rb` covers user-defined
  exception subclasses, `e.message` / `e.to_s`, raise-in-method
  + rescue-in-caller, and multiple sequential rescue blocks
  for different classes. Byte-identical to CRuby.

Known divergence: CRuby's `Exception#message` reads an internal
mesg slot set by the C-level Exception.new, not the `@message`
ivar. Our preamble-based implementation reads `@message`, so a
user override of `initialize` that sets `@message` directly is
visible to `message` in rubyrs but not in CRuby. Documented;
won't be fixed until we have a clear use case demanding parity.
- **Class inheritance** (P2-C-1). `class Foo < Bar` is now parsed
  and a `Class` stores its superclass (`Rc<Class>`). Method lookup
  walks the chain via the existing `lookup_method_cached` helper
  (now generalised), and `initialize` lookup during `Class.new` also
  uses the chain so `Dog.new("Rex")` invokes `Animal#initialize`
  when `Dog < Animal`. A `class_is_a` predicate is added for the
  rescue-by-class filter that lands next (kept `#[allow(dead_code)]`
  for now).
- New diff fixture `inheritance.rb` exercises three-level chain
  (Animal → Dog → Puppy) with inherited initialize, inherited
  instance methods, and class-own methods. Byte-identical to CRuby.
- **CRuby differential testing harness** (P2-B-1). New
  `tests/diff_cruby.rs` runs each `tests/diff/*.rb` under both rubyrs
  and the system `ruby` binary; stdout must match byte-for-byte. CI
  pins Ruby 3.4 via `ruby/setup-ruby@v1` so the comparison is
  reproducible. Seeded with 10 fixtures (integer/string/array/hash/
  block/class/symbol/interpolation/rescue/fizzbuzz). Running the
  fixtures immediately caught a parser gap (`ParenthesesNode` was
  unsupported); fixed in the same commit.
- **Resource caps for untrusted scripts** (P1-D). `Config` gains
  three optional knobs: `fuel: Option<u64>` (per-op limit, enforced
  inside `dispatch_until` so block bodies can't bypass it),
  `max_heap_objects: Option<usize>` (live Instance/Array/Hash
  count, checked after `maybe_gc`), and `max_frames: Option<usize>`
  (frame stack depth, checked before every `frames.push`). Hitting
  any returns `RubyError::ResourceExhausted`. Defaults are `None`
  for backward-compat; embedders running untrusted DSLs should set
  all three.
- CLI env vars: `RUBYRS_FUEL`, `RUBYRS_MAX_OBJECTS`,
  `RUBYRS_MAX_FRAMES`.
- 5 new `tests/embed.rs` tests covering fuel exhaustion in a tight
  loop, fuel inside a block, unlimited-fuel happy path, heap-cap
  with retained allocations, and frame-cap with deep recursion.
- **Host embedding API** (P1-C). New `src/lib.rs` exposes `Runtime`,
  `Value`, `Trap`, `RubyError`, etc. as a public Rust API:
  - `Runtime::new()` / `with_config(Config)`
  - `rt.eval(source, filename)` and `rt.eval_file(path)` —
    incremental: class/method defs persist across calls
  - `rt.register_fn(name, |args| ...)` — host functions callable
    from Ruby
  - `rt.set_stdout(Box<dyn Write>)` — capture/redirect `puts`/`print`
  - `rt.format_trap(&trap)` — CRuby-style backtrace formatting
- `examples/embed.rs` demonstrating all four capabilities
- `tests/embed.rs` locking down the API surface (7 tests)
- ADR 0007: Host embedding API design
- ADR 0006: Global string interner with SymId.
- **CRuby-style error format with backtrace** (P0-B-3). Trap output
  now prints `file:line:in 'method': msg (Class)` plus one
  `	from file:line:in 'method'` line per frame, structurally
  matching CRuby. File and line resolve against the source via
  `error::line_col`.
- New `tests/fixtures/errors/` directory + `run_error_fixture()` in
  the integration harness. Each `.rb` has an `.expected_err` golden
  for stderr; the test expects a non-zero exit. Seeded with
  `nomethod`, `wrong_args`, `yield_no_block`.
- `STRESS_GC=1` env flag forces a full collection on every potential
  GC point. Wired into CI as a second job.
- ADR 0005: pinned stack for native-driven loops.
- Symbol literal (`:foo`) and shorthand hash key syntax (`{name: "x"}`)
- String interpolation: `"hello #{name}"`, mixed with method calls and math
- `Nil#to_s` / `inspect` / `nil?`, `Bool#to_s` to round out built-ins
  needed by interpolation
- GitHub Actions CI: Linux + macOS, build + test on every push and PR
- LICENSE files: dual MIT OR Apache-2.0
- Crate metadata in `Cargo.toml` (description, license, repository)
- `docs/` directory with structured project documentation
- Architecture Decision Records (`docs/adr/`)
- `CHANGELOG.md` and `CONTRIBUTING.md`

### Changed
- **`Regexp` / `/pattern/` literals are now opt-in via the
  `regex` Cargo feature** ([ADR 0017](docs/adr/0017-tier1-boundary.md)
  Rule 3, PoC #2). The default `cargo install rubyrs` no
  longer ships the `regex` crate (~300 KB compiled + ReDoS
  attack vector — neither appropriate for the sandbox-host
  niche). `Op::LoadRegex`, `Expr::RegexLit`, and every
  `String#match` / `String#scan` / `String#=~` regex form
  are cfg-gated; with the feature off, regex literals raise
  a parse-time error pointing at the feature flag (same UX
  shape as `require` without `cext`, per PR #75). Embedders
  who need regex either enable the feature or register a
  host fn. Parallels Lua / Wren / rhai / rune / Starlark's
  same call.
- **`[profile.release] lto = "thin"`** in `Cargo.toml`. The
  CRuby-mirrored vm.rs split moved hot dispatch / opcode /
  lookup code into separate compilation units; without LTO,
  cross-module calls couldn't inline, costing ~7% on
  fizzbuzz 1M (349 ms → 372 ms, well outside the ~6 ms σ).
  Thin LTO recovers the regression to within noise (350 ms)
  and modestly improves the metaprog-bench workloads
  (`perf/baselines.tsv`) too. Release-build wall time
  increases by ~3 s; dev and test builds are unaffected.
  Verified 2026-05-25 with hyperfine 15-run on
  `crates/rubyrs/benches/fizzbuzz_1m.rb`.
- **`BlockHandle` now lives in the GC heap** (P2-13). `Value::Block`
  changed from `Rc<BlockHandle>` to `ObjId`, and `HeapObj` gained
  a `Block(BlockHandle)` variant. `Heap::collect` walks
  `BlockHandle.captured` and `self_val` as children, putting
  blocks on the same mark/sweep footing as `Array` / `Hash` /
  `Range`. `Frame.block_arg` and every function that previously
  took `Rc<BlockHandle>` (`invoke_block`, `invoke_method_with_block`,
  the iterator drivers, `collection_call_block`) now take an
  `ObjId`. Inline `block.clone()` Rc-bumps disappear — `ObjId`
  is `Copy`. Iterator drivers (`iter_array_filter` etc.) and
  every inline block-arm in `collection_call_block` now pin the
  block alongside the source receiver so the GC's root walk
  reaches it during the iteration. Without this, the existing
  `pin_guard_balanced_when_block_raises_inside_iterator` test
  would have caught a slot-reuse panic immediately.
  Why this matters: with the `Rc<BlockHandle>` form, a block
  that captured itself (e.g. callback-DSL `proc { p }` patterns
  once `proc` / `lambda` are added — P3+) formed an Rc cycle
  that the mark-sweep collector couldn't reach to break.
  Eliminating that future hazard is the structural payoff.
  Subset doesn't expose `proc` yet, so this is largely
  preventive maintenance, but the iterator paths exercise the
  new heap-block plumbing every test run. `heap.rs` panic
  budget bumped from 9 to 10 (the new `heap.block(id)` accessor).
  New regression test `blocks_are_gc_reclaimed_under_stress`
  loops 200× over `[1,2,3].each { ... }` with stress-GC and a
  `max_heap_objects: 50` cap, proving blocks get reclaimed.
- **GC mark walks children in place instead of cloning** (P0-3).
  `Heap::collect`'s mark phase previously built a fresh
  `Vec<Value>` per popped worklist entry by cloning the entire
  `HeapObj::Array` / `HeapObj::Hash` / `HeapObj::Instance.ivars`
  contents on every visit. On a heap whose largest object is one
  big Array, that turned each full collection into quadratic
  work and pushed stress-GC runs into wall-clock territory the
  test suite would actually notice. Rewrote the loop to
  split-borrow `self.slots` (read) against `self.marks` (write)
  on disjoint fields and iterate children by reference — no
  intermediate allocation, same mark/sweep semantics. The
  external `visit_value` signature is unchanged so the Block
  walk path (which still clones `BlockHandle.captured`) keeps
  working until `BlockHandle` moves into the heap in P2-13.
  Existing 1M-fizzbuzz benchmark is unaffected (~307ms steady)
  because fizzbuzz isn't GC-bound; the win is on workloads with
  many or large container objects.
- **`Vm.pinned` is now managed by a `PinGuard` RAII type** (P0-2).
  Native iterator drivers — Array/Hash/Range `#each` / `#map`,
  `#each_with_index`, the Enumerable filter family
  (`iter_array_filter` etc.), the aggregation family (`#inject`,
  `#count`, `#sort_by`), and the `Class.new` allocator — used to
  do `self.pinned.push(...); ...; ?; ...; self.pinned.pop();` by
  hand. Once those bodies started using `?` for fuel traps and
  host-fn errors, the pop could be skipped, leaving dead values
  pinned. The GC then kept marking those values as live every
  cycle — a slow leak that mostly only showed up under stress-GC.
  Replaced every push/pop pair with `PinGuard::new(self)` plus
  `g.pin(v)`; the guard's `Drop` pops exactly what was pinned, on
  both the success and `?`-unwind paths. Added a `debug_assert!`
  in `Runtime::eval` that the pinned-stack length is unchanged
  across every call — release builds skip the check so a
  regression doesn't crash production hosts. New regression test
  `pin_guard_balanced_when_block_raises_inside_iterator` hammers
  `[1,2,3].map { ... raise ... }` 50× under stress-GC to fire
  the assertion on any leak.
- **Per-call-site method inline cache** (P1-B upgrade). The
  single-slot cache from Tier1-1 is replaced with a per-site
  cache: every `Op::Call` / `Op::CallNoRecv` / `Op::CallBlock` /
  `Op::CallNoRecvBlock` carries a `u16` cache slot id assigned at
  compile time, and `Vm.call_caches: Vec<CallCache>` is sized to
  match. Lookups index directly by site, so call sites that
  dispatch on different classes (polymorphic) no longer thrash
  each other.
  Invalidation: `Op::DefMethod` and `Op::DefClass` bump
  `Vm.method_gen`; cache entries store the gen at fill time and
  miss when it shifts. `lookup_method_uncached` is the fallback
  for paths that shouldn't cache (e.g. `initialize` during
  `Class.new`).
  Microbench (vs the Tier1-1 single-slot baseline):
    - fizzbuzz 1M:        327 ms → **322 ms** (1.76× → 1.72× of CRuby)
    - Counter.inc × 1M:   153 ms → **148 ms** (1.43× → 1.37× of CRuby)
  Monomorphic gains are small (single-slot was already hitting),
  but the structural change matters for polymorphic dispatch (two
  alternating call sites in a loop), which would have made
  single-slot miss on every call.
- **Statement-position avoids redundant `Dup`/`Pop`** (Tier1-5).
  `compile_body` now distinguishes the last expression of a body
  (whose value is the body's value) from intermediate ones (whose
  value is discarded). Intermediate `Expr::LVarWrite` /
  `Expr::IVarWrite` emit `StoreLocal` / `StoreIvar` directly with
  no preceding `Dup`. The `Inc*` ops get matching `NoPush`
  variants emitted in stmt position. Microbench: fizzbuzz 1M 332
  ms → **327 ms**.
- **`Op::BinOpInt` fuses `LoadConstInt + BinOp`** (Tier1-4). For
  any binary op where the right-hand side is a literal integer
  (`n % 15`, `... == 0`, `i <= 1000000`, …), the compiler now
  emits a single `BinOpInt(kind, i64)` op instead of the previous
  pair. Each fused expression saves one dispatch and one stack
  round-trip. Microbench: fizzbuzz 1M 364 ms → **332 ms (~9%)**.
  rubyrs vs CRuby on fizzbuzz: **1.80×** (was 1.94×).
- **`Op::IncIvar` fast path for `@x = @x + 1`** (Tier1-3).
  Symmetric to Tier1-2 but on the receiver's ivar table. Hot
  path: in-place increment of the Int; slow path: synth a `+` call.
  Microbench: Counter.inc × 1M (the `@count = @count + 1` inside
  `inc`): 179 ms → **153 ms (~15%)**. rubyrs is now within
  **1.42×** of CRuby's interpreter on method-dispatch-dominated
  workloads. Fizzbuzz unchanged (no ivars in the hot path).
- **`Op::IncLocal` fast path for `i = i + 1`** (Tier1-2). The
  compiler now recognises the syntactic pattern
  `name = name + 1` (literal `+ 1`) and emits a single
  `IncLocal(slot)` op instead of the previous 5-op sequence
  (LoadLocal, LoadConstInt(1), BinOp::Add, Dup, StoreLocal).
  Read-modify-write happens in place against the slot. On
  non-Int payloads the op falls back to a synthesised
  `+`-call so user-defined types with their own `+` keep
  working (CRuby semantics preserved). Microbench: fizzbuzz
  386 ms → **369 ms**; Counter.inc × 1M loop 203 ms → **179 ms**
  (the outer `i = i + 1` benefits; the inner `@count = @count + 1`
  still goes through the generic path — IncIvar is a follow-up).
- **Single-slot inline method cache** (Tier1-1). `Vm.call_cache`
  holds the (class identity, method name, resolved `Method`) of
  the last successful `Value::Object` dispatch; subsequent calls
  with matching class + name skip the `HashMap<SymId, _>::get`.
  Invalidated conservatively on `Op::DefMethod`. Helper
  `Vm::lookup_method_cached` is shared across all four
  Object-dispatch sites (do_call no_recv, do_call with recv,
  do_call_block no_recv, do_call_block with recv).
  Microbench: fizzbuzz 408 ms → **386 ms** (~5%, BinOp-dominated
  workload limits the cache's reach). Counter.inc × 1M
  (method-dispatch-dominated): 203 ms vs CRuby's 108 ms — we
  are now within **1.87×** of CRuby's interpreter on this shape
  of workload.
- `src/main.rs` shrinks to a 20-line CLI wrapper around `Runtime`.
  Behaviour is identical to before.
- **Global string interner** (P1-B). Method names, ivar names, class
  names, and string literals all live in a single Vm-owned `Interner`
  and are referenced by `SymId(u32)`. `Proto.strings` is gone;
  `Value::Sym` carries a `SymId` instead of `Rc<String>`;
  `Value::Str` carries `Rc<str>`. `Class.methods`, `Instance.ivars`,
  and `Vm.classes` are now keyed on `SymId`. Symbol equality is a
  single u32 compare; method dispatch hashes on a tight key.
  Microbench: 1M fizzbuzz 484 ms → **408 ms (1.18× faster)**;
  distance to CRuby + YJIT 3.44× → 2.82×.
- **User errors no longer panic the host process** (P0-B-2). Undefined
  method, wrong arity, and `yield` outside a block now build a `Trap`
  that bubbles up through every dispatch path (`Result<_, Trap>`
  everywhere), is printed at process exit, and returns a non-zero
  exit code. Internal invariants (heap UAF, empty frame stack while
  dispatching, stack underflow) remain `panic!` but are now marked
  `"ICE: ..."` to make the distinction explicit when one fires.

### Fixed
- **GC rooting holes around `maybe_gc` — 6 latent sites flushed
  out by the first all-green STRESS_GC=1 CI run.** Master CI's
  STRESS_GC=1 step had been blocked by the prior-stage panic-
  budget and clippy red until the CI unbreak below cleared the
  gate; once it could run, three rounds of investigation found
  six sites all sharing one structural bug: a heap-bearing
  `Value` popped from the operand stack lives only in a Rust
  local across an intervening `self.maybe_gc()`, and is not in
  any GC root set (`self.stack`, `self.pinned`, frames'
  `locals`, etc.). Under STRESS_GC=1 the slot gets swept and
  the subsequent `heap.alloc` reuses it, leaving the new heap
  object referencing a dangling ObjId — surfaces as
  `heap.rs ICE: class_of called on non-Object slot` or stack-
  overflow in `to_inspect` (self-referential slot loop). The
  pattern, all known sites, three mitigation options, and the
  recommended next step are written up as a self-contained
  brief in [#90](https://github.com/linyiru/rubyrs/issues/90)
  for systematic follow-up.
  - **`Object#method(:name)` + `invoke_block` rest-slot path**
    (`86db73d`). The first arm holds the recv as a Rust local
    across `maybe_gc` before alloc'ing the BoundMethod; the
    second pops `block_id` off the operand stack before
    pushing the new frame whose locals would have rooted the
    captured Vec. Wrapped both with `PinGuard`. Repro fixture:
    `proc_curry_compose.rb` under `STRESS_GC=1` — fails at
    `(succ >> m).(4)` where `m = Squared.new.method(:call)`.
  - **`Array#combination` / `Array#permutation` / `String#scan`
    capture-group accumulator Vecs** (`f2c3538`). Each builds
    a result via a Rust-local `Vec<Value>` where every
    iteration `heap.alloc`'s a fresh sub-Array and pushes its
    ObjId in; the wrapping result Array isn't allocated until
    the loop finishes, so under STRESS_GC=1 every prior sub-
    Array gets swept on the next iteration. Slot reuse on
    subsequent allocations made the final result self-
    referential, overflowing the stack at `to_inspect`. Fix:
    pin each sub-Array as it's pushed into the accumulator;
    `PinGuard` Drop pops them all on return. Repro fixtures:
    `array_combinatorics.rb` (combination + permutation paths)
    and `string_scan.rb` (capture-group path; no-capture
    branch builds Rc-Strings and was already safe).
  - **`UnboundMethod#bind(receiver)`** (`5946caa`). Same
    pattern as `Object#method`: target value from `args` is a
    Rust local across `maybe_gc`. PR #85's
    `kernel_instance_method.rb` is the first fixture to
    stress this arm under STRESS_GC=1 — went undetected in
    the previous round because the earlier sweep's grep
    filter was too aggressive and dropped the failure line.
    Same `PinGuard`-around-alloc fix.
  - **Companion**: same commit `5946caa` also gates the
    `use super::PinGuard;` import in `vm/string.rs` behind
    `cfg(feature = "regex")`. The PinGuard reference there
    sits entirely inside the `("scan", [Value::Regex(...)])`
    arm; without the gate `--no-default-features` (the
    wasm32-wasip1 build shape) trips `-D warnings` on the
    unused import.
- **CI unbreak — clippy, panic budget, wasm dead_code.**
  Master CI was red on four independent gates after the
  `break/next/ensure`, global-variable, op-write, `require .rb`,
  and bundle-build-helper merges landed without updating the
  gates. (a) Two `doc_lazy_continuation` lint sites
  (`Op::EndEnsure` doc + `tests/common/mod.rs` `+ assertions`
  list-bullet false-positive) regressed after rust-1.95
  sharpened the lint. (b) Four files crossed their per-file
  panic budgets — `vm/step.rs` 52 → 64, `vm/raise.rs` 3 → 10,
  `vm/kernel.rs` 0 → 5, `compiler.rs` 2 → 6 — every new site
  is an `.expect("ICE: …")` invariant assert (ICE-class, not
  user-reachable; budgets ratchet up with annotated rationale
  per the existing convention). (c) `Vm.loaded_features` was
  dead code on `wasm32-wasi` because `require` short-circuits
  to a trap there; gated the field + initializer with
  `cfg(not(target_os = "wasi"))` to match the accessor fns.
  Commit `07d3cd9`.
- **Integer literals no longer truncate to i32.** `ast::tr`
  was reading `IntegerNode::value()` through Prism's
  `TryInto<i32>` and silently defaulting to `0` on overflow,
  so any literal past ~2.1 billion (decimal or hex) became
  `0`. Hex `0x0102030405060708`, decimal `72623859790382856`,
  and similar all parsed as zero — the bug surfaced while
  shipping `Array#pack("Q")`. Fixed by reading through
  Prism's `to_u32_digits()` (LSB-first u32 chunks + sign)
  and rebuilding a full i64. Values beyond i64 saturate to
  `i64::MIN` / `i64::MAX` (the subset doesn't promote to
  BigInt — documented in SUBSET.md). New diff fixture
  `integer_literal_i64.rb` pins the full i64 range plus a
  pack/unpack round-trip on the natural 8-byte demo value.

- **`return` from inside a block now correctly exits the
  enclosing method.** Previously, every `return` in the program
  compiled to `Op::ReturnMethod` (non-local), which broke the
  case where a helper method called from inside a block did
  `return value` — the value escaped out through the block all
  the way to the helper's caller, instead of just exiting the
  helper. The compiler now distinguishes method-body `return`
  (local; `Op::Return`) from block-body `return` (non-local;
  `Op::ReturnMethod`) via a new `is_method_body` flag on
  ProtoBuilder, which `compile_block` deliberately resets to
  `false` even though it inherits the parent's `method_name`
  for `super`'s benefit. New diff fixture
  `nonlocal_return.rb` pins both directions: block-level
  `return` exits the enclosing method (`find_first_even`-style
  short-circuit) and method-local `return` from inside a
  helper called by a block stays local (the block keeps
  iterating). Byte-identical to CRuby. The older "Divergences"
  entry in `docs/SUBSET.md` should be removed in a follow-up.
- **Cross-type `==` / `!=` no longer raises NoMethodError.**
  `"x" == nil`, `nil == :foo`, `5 == "5"`, `[] == ""` — every
  cross-type compare used to crash with `undefined method '==`
  for String` because `primitive_call` only had same-type arms.
  CRuby's `Object#==` defaults to identity (returning false
  for any cross-type pair); we now do the same via a universal
  fallback in `do_call`: after all the type-specific arms
  declined, the dispatcher answers `==` / `!=` via the
  existing `ruby_eq` helper, which returns false for any pair
  whose types don't match. As a side-benefit, `Hash == Hash`
  and `Range == Range` now work — `ruby_eq` gained
  order-insensitive Hash equality (O(n*m); good enough until
  P3-class hash-keying lands) and Range equality
  (begin/end/exclusive triple). New diff fixture
  `cross_type_eq.rb` covers cross-type, same-type
  value-equality, and a `v == "ready"` guard idiom.
  Byte-identical to CRuby.
- **Uncaught Ruby exceptions no longer kill the host process.**
  `Vm::unwind_with_exception` called `std::process::exit(1)` when
  no `rescue` handler matched in any frame — fine for the
  rubyrs CLI, fatal for any embedded host that has work to do
  after `eval` returns. Now uncaught exceptions surface as
  `RubyError::Uncaught { class_name, message }` propagated
  through the normal `Trap` path; embedders can pattern-match,
  log, retry, or carry on. `format_trap` special-cases
  `Uncaught` to print the Ruby exception class (e.g.
  `(MyError)`) instead of the host-side tag, matching CRuby's
  `script.rb:N:in '<main>': msg (ClassName)` format. The
  rubyrs binary still prints + exits on `Trap` so CLI
  behaviour is unchanged. Three new embed.rs tests lock
  in the new contract: round-trip class_name + message, host
  continues after Uncaught, format_trap output. Closes the
  largest residual attack surface called out in
  `docs/SECURITY.md`.
- **`eval` no longer inherits leftover dispatch state from a
  previous Trap.** A previous `eval` that ended in a Trap
  (uncaught exception, fuel exhaustion, deadline hit) left its
  frames, operand-stack residue, and pins on the `Vm`. The next
  call would push a new entry frame on top, run, hit Return,
  and fall back into the abandoned frame from the earlier call
  — at best confusing, at worst running stale bytecode. Class
  definitions and the heap legitimately persist across `eval`
  calls (that's the embedding contract); the dispatch state
  shouldn't. `Runtime::eval` now clears `frames`, `stack`,
  `pinned`, and `break_signaled` at the start of every call.
  Surfaced by the new `deadline_resets_between_eval_calls`
  test — the existing fuel/heap/frame tests each used a single
  eval per Runtime so the bug stayed hidden.
- **ADR 0008 retraction**: the earlier draft promised that
  `rescue Exception => e` would catch `ResourceExhausted`
  after P1-10. It doesn't, and shouldn't — the resource trap
  is a host-level `Trap`, not a Ruby-level `raise`, so it
  bypasses `unwind_with_exception` entirely. The ADR and a
  matching test
  (`resource_exhausted_is_uncatchable_even_with_rescue_exception`)
  now lock the actual contract in.
- **Unsupported AST nodes return `SyntaxError` instead of
  panicking** (P0-4). Any Prism node outside the supported
  subset (e.g. `case/when`, regex literals, lambdas) used to
  hit `panic!("unsupported node: ...")` in `ast::tr`, tearing
  down the host process. AST translation now records the
  message on a thread-local error buffer and returns an
  `Expr::Nil` placeholder; `Runtime::eval` checks the buffer
  after `tr_with_errors` and returns a `Trap` with
  `RubyError::SyntaxError` before compilation runs. With
  rubund eventually evaluating gemspecs from rubygems.org —
  arbitrary third-party Ruby — this was a denial-of-service
  surface that had to close. New `embed.rs` test exercises
  the case statement (currently unsupported) and asserts a
  Trap, not a SIGABRT.
- **`ResourceExhausted` can no longer be swallowed by `rescue => e`**
  (P0-1). The preamble had `class ResourceExhausted < StandardError`,
  which meant a bare `rescue` clause — CRuby-style shorthand for
  `rescue StandardError => e` — could silently catch the resource
  trap and keep burning fuel/heap. Two changes:
  1. Preamble re-roots the kill switch directly under `Exception`,
     alongside CRuby's `SystemExit` and `Interrupt`.
  2. `RescueHandler` gains a `filter_class: Option<Rc<Class>>` field;
     every `Op::PushRescue` populates it with `StandardError` (the
     bare-rescue default), and `unwind_with_exception` now pops past
     handlers whose filter doesn't match the raised exception's
     class chain. `Op::PushEnsure` leaves the filter as `None`, so
     `ensure` runs unconditionally — matching Ruby semantics.
  Three new tests in `tests/embed.rs`: one proves a hostile
  `begin/rescue/end` around `while true` no longer eats the trap,
  one locks in that bare `rescue` still catches `raise "boom"` (i.e.
  RuntimeError under StandardError), and one placeholder reserves
  the contract that explicit `rescue Exception` will work once
  class filtering lands (P1-10). ADR 0008 updated.
- **GC root hole in `Hash#to_a`**. Each `[k, v]` pair is a fresh
  heap Array; the loop accumulated them into a Rust-local Vec
  while calling `maybe_gc` between iterations. Under stress-GC
  that swept earlier pairs, the alloc allocator reused their
  slots for later pairs, and the final outer Array could land
  in a reused slot too — yielding a self-referential Array that
  blew the stack inside `to_display`'s recursion. Fix: pin the
  source Hash and every accumulated pair onto `Vm.pinned` for
  the alloc window, pop on exit. Same pattern the iterator
  drivers use.
- **GC root hole during `Class.new` arg drain**. Allocating an
  Instance from `Class.new(args...)` first popped `args` off the
  operand stack into a Rust local, then called `maybe_gc` before
  allocating the instance. Under stress-GC any heap value in
  `args` (a literal `Array`, `Hash`, etc.) was unreachable from
  GC roots during that window and could be swept; the freshly
  allocated Instance would then reuse the swept slot id, leaving
  the caller's `args` pointing at the new Instance. The bug only
  surfaced once aggregation tests exercised `Stats.new([…])
  -> @values.inject(...)` under stress-GC. Fix: pin `args` onto
  `Vm.pinned` around the alloc window in both `do_call` and
  `do_call_block`. Same pattern as the existing iterator drivers.
- **Block param shadows outer scope correctly**. Discovered by the
  new control_flow fixture: when a block param's name collided
  with a local in the enclosing scope (e.g. an outer `x` then
  `each { |x| ... }`), `compile_block` was reusing the outer
  scope's slot for the block param while still emitting a
  param_start above it, leaving the block reading garbage. Block
  params now always allocate fresh slots via a new
  `define_local_slot` (vs the existing `local_slot` which
  reuses), shadowing the outer binding to match modern CRuby's
  "block local variable" semantics.
- **GC root hole in native-driven iterators** (P0-A). `Array#map`,
  `Array#each`, and `Hash#each` accumulated state in Rust-local `Vec`s
  that weren't visible to the mark phase; a sufficiently large `map`
  could read use-after-free objects. Now uses an explicit `Vm.pinned`
  root list. `STRESS_GC=1 cargo test` exercises this in CI.

### Internal
- **GC rooting lint gate ([issue #90](https://github.com/linyiru/rubyrs/issues/90)).**
  After seven structurally-identical GC-rooting incidents in
  three discovery rounds (`86db73d` / `f2c3538` / `5946caa` /
  the site-#8 fix in this cycle), a static gate now prevents
  the eighth from being written. `scripts/lint-gc-rooting.sh`
  scans every `vm/*.rs` file for the dangerous shape — a
  `self.maybe_gc()` + `self.heap.alloc(` pair preceded by a
  Value drain (`self.stack.pop` / `self.stack.drain` /
  `self.stack.swap_remove` / `args.swap_remove` / `args.drain` /
  `args.pop` / `args[N]`) in the same logical block, without an
  intervening `PinGuard::new(self)`. Sites using the PinGuard
  form (`g.vm.maybe_gc()` / `g.vm.heap.alloc()`) are deliberately
  bypassed — that pattern is the structural fix and the lint
  treats it as the canonical recipe. Genuine false-positives
  carry an inline `// allow: gc-rooting — <reason>` justification.
  Two such annotations sit on the current tree at `dispatch.rs:223`
  (`method(:foo)` implicit-self — `recv` cloned from rooted
  `frames.last().self_val`) and `kernel.rs:278` (`Array(nil)` —
  empty Array, no Value held). New CI step in
  `.github/workflows/ci.yml` runs the gate ahead of clippy.
  Also includes the site-#8 fix in `vm/kernel.rs` (
  `Kernel#Array(other_heap_value)` — `args[0]` was unrooted
  across `maybe_gc`, surfaced under `STRESS_GC=1` as
  `ICE: class_of called on non-Object slot`) and the
  `kernel_array_coerce.rb` diff fixture that reproduces it.
  Heuristic limitations documented in the script header; if a
  future hole slips past, escalate to Option 2 of the issue
  (`Vm::alloc_pinned` helper).
- **[ADR 0017](docs/adr/0017-tier1-boundary.md) — Tier-1 boundary
  specification.** Concrete contract for what is / isn't in
  Tier 1, populating the abstract shape ADR 0015 sketched.
  Four inclusion rules (determinism from script inputs only;
  no script-accessible OS capabilities by default; no regex;
  no OS threads), an OUT-of-Tier-1 table covering 14
  feature families with target tier + rationale, and a
  "Current deviations" table tracking four code paths that
  don't yet match the spec (stdout default sink, `ENV`
  bleed, `$$` PID, `Regexp` in Tier 1 — the regex deviation
  was closed by PR #86). Backed by empirical prior-art
  review of Lua 5.4 / mruby 3.x / rhai 1.25 / rune 0.14.
  SUBSET.md cross-links the ADR as the formal contract this
  doc tracks implementation status against.
- **Centralised cext-bundle build helper.** New
  `crates/rubyrs/tests/common/mod.rs` exposes
  `build_cext_bundle(example_dir_name, bundle_basename)` and
  a `RUBY_DLEXT` const; 12 cext integration tests previously
  inlined their own `bash build.sh` invocation, existence
  checks, and platform-suffix logic. Removed the duplication
  in one pass. `msgpack-cext` and `counter-cext` `build.sh`
  gained `flock` + atomic-tmpfile-rename serialisation so
  parallel `cargo test` runs can't race on a half-written
  `.bundle`/`.so`.
- **`rubyrs-spec-extract` v0.3.** Adds 3 more extractor-
  derived ruby/spec specs (+12 examples) covering
  `Array#compact`, `Array#take`, `Hash#keys`. Tightens the
  extraction context heuristics and skip-log messages after
  multiple rounds of Copilot review.
- **CRuby-mirrored `vm.rs` split.** The 6593-line `vm.rs` is
  split into per-type submodules under `crates/rubyrs/src/vm/`,
  mirroring CRuby's file layout so "where does method X live?"
  follows the same intuition as `string.c` / `array.c` / `hash.c`.
  Behaviour-preserving moves only; every step kept the 79
  `diff_cruby` fixtures byte-identical to CRuby. Modules now in
  place (with their CRuby analogue):
    - `vm/sprintf.rs` — sprintf.c (`ruby_sprintf` + width/prec parser)
    - `vm/numeric.rs` — numeric.c (Int/Float primitives)
    - `vm/string.rs` — string.c (String primitives, Regex shims)
    - `vm/array.rs` — array.c (no-block Array methods)
    - `vm/hash.rs` + `vm/range.rs` — hash.c / range.c
    - `vm/iter.rs` — enum.c (block-form Enumerable filter family,
      `iter_*_filter`, `collection_call_block`)
    - `vm/kernel.rs` — object.c Kernel arms (`puts` / `p` /
      `Integer()` / `Float()` / …)
    - `vm/fileops.rs` — file.c (`File.read` / `File.exist?` …)
    - `vm/raise.rs` — eval.c / eval_error.c (`normalize_exception`,
      `trap_to_exception`, `unwind_with_exception`)
    - `vm/cext.rs` — internal/value.h + vm_eval.c
      (rb_funcallv callback installation, handle ↔ Value
      translation, `cext_dispatch`)
    - `vm/dispatch.rs` — vm_eval.c / vm_insnhelper.c (`do_call`,
      `do_call_block`, `invoke_method`, `invoke_method_with_block`,
      `invoke_block`, `cext_invoke_method`, `try_method_missing`)
    - `vm/step.rs` — vm_exec.c (`dispatch`, `dispatch_until`,
      the per-opcode `step` match)
  Net effect: `vm.rs` from 6593 → ~440 lines after the
  follow-up extractions of `lookup.rs`, `gc.rs`, `primitive.rs`,
  and `util.rs` (Vm struct + Frame/PinGuard/RescueHandler +
  cext-reentrance thread-local). Perf cost: cross-module
  boundaries cost the inlining the single-file version was
  getting for free (~7% on the fizzbuzz 1M microbench);
  recovered by switching `[profile.release]` to `lto = "thin"`
  — see the matching CHANGELOG entry below.
- **Error / Span infrastructure** (P0-B-1): new `src/error.rs` with
  `Span` (byte offset into source), `RubyError` (closed set covering
  SyntaxError / NoMethodError / ArgumentError / TypeError / RuntimeError
  / NameError), `TrapFrame`, and `Trap` (error + backtrace). `Expr`
  is now wrapped as `Spanned<Expr>` (alias `SExpr`); `Proto` gets a
  parallel `op_spans: Vec<Span>` and a `filename: Rc<str>`. The
  panic→Trap migration itself is the next commit; this one just
  wires the plumbing so spans flow from Prism → Expr → Op without
  changing observable behaviour.
- **Module split** (P1-A): `src/main.rs` (1600 lines) split into
  `ast.rs`, `value.rs`, `heap.rs`, `bytecode.rs`, `compiler.rs`,
  `vm.rs`, plus a 55-line CLI `main.rs`. Move-only refactor:
  fixtures emit stdout bit-identical to the pre-split binary. Sets
  up the seam for the upcoming `lib.rs` / embedding API (P1-C).
- `Op` and `BinOpKind` now derive `Copy` (P0-C). The dispatch loop's
  `code[ip].clone()` becomes a plain `Copy`. The previous `clone()` was
  already optimised away by LLVM (all payloads were already POD), so
  this is a structural correctness change rather than a measurable
  speedup. Future Op variants must remain POD or carry a `SymId` /
  similar index instead of an `Rc<str>`.
- Specialised `Op::BinOp(BinOpKind)` for `+ - * / % == != < <= > >=` —
  Int+Int fast path avoids generic method dispatch
- 1M-fizzbuzz: 0.67 s → 0.44 s (2.3× of CRuby's interpreter)
## [0.0.x — development]

Initial PoC and milestones leading up to this point. All work pre-tag is
in the commit log; the changelog is canonical from here forward.
