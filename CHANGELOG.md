# Changelog

All notable changes to rubyrs will be recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/), and this project
follows [Semantic Versioning](https://semver.org/) once we hit 0.1.

## [Unreleased]

### Added
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

### Added
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

### Added
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

### Fixed
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

### Changed
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

### Added
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

### Fixed
- **ADR 0008 retraction**: the earlier draft promised that
  `rescue Exception => e` would catch `ResourceExhausted`
  after P1-10. It doesn't, and shouldn't — the resource trap
  is a host-level `Trap`, not a Ruby-level `raise`, so it
  bypasses `unwind_with_exception` entirely. The ADR and a
  matching test
  (`resource_exhausted_is_uncatchable_even_with_rescue_exception`)
  now lock the actual contract in.

### Added
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

### Fixed
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

### Changed
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

### Changed
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

### Fixed
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

### Added
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

### Fixed
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

### Added
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

### Added
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

### Added
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

### Fixed
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

### Added
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

### Changed
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

### Added
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

### Added
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

### Fixed
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

### Added
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

### Added
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

### Added
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

### Changed
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

### Added
- **CRuby differential testing harness** (P2-B-1). New
  `tests/diff_cruby.rs` runs each `tests/diff/*.rb` under both rubyrs
  and the system `ruby` binary; stdout must match byte-for-byte. CI
  pins Ruby 3.4 via `ruby/setup-ruby@v1` so the comparison is
  reproducible. Seeded with 10 fixtures (integer/string/array/hash/
  block/class/symbol/interpolation/rescue/fizzbuzz). Running the
  fixtures immediately caught a parser gap (`ParenthesesNode` was
  unsupported); fixed in the same commit.

### Added
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

### Added
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

### Changed
- `src/main.rs` shrinks to a 20-line CLI wrapper around `Runtime`.
  Behaviour is identical to before.

### Changed
- **Global string interner** (P1-B). Method names, ivar names, class
  names, and string literals all live in a single Vm-owned `Interner`
  and are referenced by `SymId(u32)`. `Proto.strings` is gone;
  `Value::Sym` carries a `SymId` instead of `Rc<String>`;
  `Value::Str` carries `Rc<str>`. `Class.methods`, `Instance.ivars`,
  and `Vm.classes` are now keyed on `SymId`. Symbol equality is a
  single u32 compare; method dispatch hashes on a tight key.
  Microbench: 1M fizzbuzz 484 ms → **408 ms (1.18× faster)**;
  distance to CRuby + YJIT 3.44× → 2.82×.

### Added
- ADR 0006: Global string interner with SymId.

### Added
- **CRuby-style error format with backtrace** (P0-B-3). Trap output
  now prints `file:line:in 'method': msg (Class)` plus one
  `\tfrom file:line:in 'method'` line per frame, structurally
  matching CRuby. File and line resolve against the source via
  `error::line_col`.
- New `tests/fixtures/errors/` directory + `run_error_fixture()` in
  the integration harness. Each `.rb` has an `.expected_err` golden
  for stderr; the test expects a non-zero exit. Seeded with
  `nomethod`, `wrong_args`, `yield_no_block`.

### Changed
- **User errors no longer panic the host process** (P0-B-2). Undefined
  method, wrong arity, and `yield` outside a block now build a `Trap`
  that bubbles up through every dispatch path (`Result<_, Trap>`
  everywhere), is printed at process exit, and returns a non-zero
  exit code. Internal invariants (heap UAF, empty frame stack while
  dispatching, stack underflow) remain `panic!` but are now marked
  `"ICE: ..."` to make the distinction explicit when one fires.

### Internal
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

### Fixed
- **GC root hole in native-driven iterators** (P0-A). `Array#map`,
  `Array#each`, and `Hash#each` accumulated state in Rust-local `Vec`s
  that weren't visible to the mark phase; a sufficiently large `map`
  could read use-after-free objects. Now uses an explicit `Vm.pinned`
  root list. `STRESS_GC=1 cargo test` exercises this in CI.

### Added
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

### Internal
- Specialised `Op::BinOp(BinOpKind)` for `+ - * / % == != < <= > >=` —
  Int+Int fast path avoids generic method dispatch
- 1M-fizzbuzz: 0.67 s → 0.44 s (2.3× of CRuby's interpreter)

## [0.0.x — development]

Initial PoC and milestones leading up to this point. All work pre-tag is
in the commit log; the changelog is canonical from here forward.
