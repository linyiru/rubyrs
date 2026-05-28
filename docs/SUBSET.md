# Subset semantics

rubyrs is **not** trying to be CRuby-compatible at the language level today.
It targets the same niche as **mruby**: a small, memory-safe, embeddable
Ruby-flavored runtime — but written in Rust, with the option of compiling
to WebAssembly.

If you need Rails, Sinatra, Bundler, gems, or `eval` today — use CRuby.

## Tier framing — what this document defines

[ADR 0015](adr/0015-concentric-architecture.md) lays out a concentric
multi-tier architecture: a tight Tier 1 core (the embeddable subset
described here), with strictly opt-in outer tiers
(`language` → `stdlib` → `mri-compat`) for everything from Sinatra-class
Ruby semantics up to eventual CRuby-shaped binary compatibility.

[ADR 0017](adr/0017-tier1-boundary.md) is the formal Tier 1 boundary
specification — the four inclusion rules (deterministic from script
inputs, no script-accessible OS capabilities by default, no regex,
no OS threads) and the OUT-of-Tier-1 table that pin down exactly
which feature lands in which tier. This document tracks *implementation
status* against that contract; ADR 0017's "Current deviations" table
is the authoritative list of code paths that don't yet match the spec
(stdout sink default, `ENV` host-process bleed, `$$` PID exposure —
the `regex` deviation was closed by PR #86, the `regex` Cargo feature).

**This document defines Tier 1 only.** Everything below "Supported today"
or "Divergences from CRuby" is a Tier 1 statement. Items labeled
"explicitly out of scope" are out of Tier 1; they may land in a higher
tier later, but committing here is a Tier 1 design statement, not a
permanent rubyrs-wide "never".

Concretely, the divergences in this file fall into three categories:

1. **Documented Tier 1 semantics.** e.g. integer literals saturate to
   `i64::MIN` / `i64::MAX` rather than promoting to BigInt; nested
   `module Foo; module Bar; …; end; end` flattens `Bar` to top-level;
   `nil.to_i == 0` matches CRuby. These are deliberate Tier 1 choices,
   not bugs.
2. **Tier 2 deferred.** Features where the cext / wire-format surface
   is shipped but the underlying language semantics aren't — e.g.
   `rb_big2ll` works for any value that fits in i64 (cext ABI is real);
   true arbitrary-precision arithmetic on `Value::BigInt` is Tier 2
   work. Same shape for `Time` class (no `Time.now` yet — but
   user-class ext-type frames work today via `register_type_internal`).
3. **Out of scope today, candidate later.** Listed at the bottom under
   "Not supported (today, but candidates for the roadmap)".

When in doubt, the rule from ADR 0015 applies: **"Does this serve
Tier 1?"** If a proposed change costs Tier 1 size, cold-start, or
sandbox guarantees, it doesn't go here — it's a Tier 2+ proposal.

## Supported today

### Values
- `Integer` (i64) and `Float` (f64). Float literals (`3.14`,
  `1e6`), Float arithmetic, mixed Int/Float coercion (CRuby's
  "Float wins on mix" rule), `5 == 5.0` cross-numeric equality.
  Float methods: `to_i` / `to_f` / `to_s` / `abs`, predicates
  (`zero?` / `positive?` / `negative?` / `nan?` / `finite?`),
  `infinite?` (returns `1` / `-1` / `nil`), and
  `floor` / `ceil` / `round` (Integer results). Scientific
  notation diverges from CRuby for very large or very small
  magnitudes (Rust prints `1e16`, CRuby prints `1.0e+16`) —
  documented divergence; restrict diff fixtures to the
  everyday range.
- `String` (`Rc<str>`, UTF-8 view) with `+`, `==`, `length`, `to_s`.
  String literals share storage via the global interner.
- `Symbol` (`u32` index into the interner) with `to_s`, `to_sym`, `==`,
  `!=`. Symbol equality is a single integer compare.
- `true`, `false`, `nil` — including `nil.to_s` (`""`), `nil.inspect`
  (`"nil"`), `nil.nil?`, and `Bool#to_s`
- `Array` — see "Array built-in methods" below for the
  full method list (~50 methods covering iteration,
  filtering, combinatorics, pack/unpack, bsearch, etc.).
- `Hash` — insertion-ordered, linear lookup; see "Hash
  built-in methods" below for the full method list
  (~25 methods including transform_keys/values, except/slice,
  compact, filter_map, etc.).
- Class instances with instance variables and methods
- `Proc` (Block value) — opaque; created by `arr.each { ... }` and
  consumed by built-in iterators or `yield`

### Syntax
- Local and instance variables (`x`, `@x`)
- `if / elsif / else`, `while`
- `def` (top-level and inside `class`)
- `class Foo ... end`, `Foo.new(args)`, `initialize`, instance methods,
  implicit-self method calls
- `self`
- `String` interpolation: `"hello #{name}"`
- `Symbol` literal: `:foo`; shorthand hash key `{name: "x"}`
- Block syntax: `arr.each { |x| ... }` and `arr.each do |x| ... end`
- `yield`
- `begin / rescue => e / end`, nested rescue with rethrow,
  `raise "msg"`, `raise SomeError`, `raise SomeError, "msg"`
- `rescue ClassName => e` (class-filtered) and multiple `rescue`
  clauses in source order — see Divergences below
- Array and hash literals: `[1, 2]`, `{a: 1}`
- Integer arithmetic: `+ - * / %`, comparisons: `== != < <= > >=`

### Built-ins
- `puts`, `print`
- `Integer#times { |i| ... }`

### String built-in methods

Covered: `length` / `size`, `bytesize`, `bytes`, `chars`,
`+`, `==`, `<=>` (lexicographic), `empty?`, `reverse`,
`upcase`, `downcase`, `strip` / `lstrip` / `rstrip`,
`squeeze` / `squeeze(charset)` (literal char-set, no range
expansion — same conservative semantics as `tr`),
`center(width[, pad])` / `ljust(width[, pad])` /
`rjust(width[, pad])` (pad cycles when multichar; empty pad
raises ArgumentError),
`include?(String)`, `start_with?` / `end_with?`,
`equal?` (identity), `match?` / `match`, `scan` (String and
Regex patterns; both non-block and block forms — capture
groups in the regex make `scan` return Array-of-captures
per match, no groups → match-string), `tr(from, to)`,
`sub` / `gsub` (see below), `to_i` / `to_f` / `to_sym`,
`encode` / `force_encoding` (no-op stubs — the subset has no
encoding tag; see "String encoding stubs" below),
`valid_encoding?` (always true), `encoding` (returns the
String name `"UTF-8"`; see "String encoding stubs" below),
`unpack(format)` (subset — see "Pack/Unpack"),
interpolation `"... #{expr} ..."`.

Not yet covered on String identity / inspection:

- `object_id` — use `equal?` instead for identity assertions.

`String#sub` and `String#gsub` cover the patterns shipped so
far:

- **Regex pattern, String replacement**:
  `"hello".sub(/[aeiou]/, '*')` → `"h*llo"`
- **Regex pattern, block replacement**:
  `"hello".gsub(/[aeiou]/) { |m| m.upcase }` → `"hEllO"`
- **String pattern, String replacement** (literal, no regex
  metacharacter interpretation): `"\\d".sub('\\d', 'a')` → `"a"`

Not yet shipped on `sub` / `gsub` (each will land in its own
PR; specs in
[`crates/rubyrs/spec/ruby/string_*_spec.rb`](../crates/rubyrs/spec/ruby/)
skip the upstream blocks that exercise them, with a `#`
comment naming the gap):

- `/i` case-insensitive Regex flag. `"Hello".sub(/h/i, "j")`
  currently returns `"Hello"` (no match), upstream expects
  `"jello"`.
- Backref replacement strings: `'\1'`, `'\&'`, `'\k<name>'`.
  The substitution String is taken verbatim — captures aren't
  expanded.
- String pattern under block form: `"hi".sub("hi") { "bye" }`.
  rubyrs routes block-form sub through the Regex path only;
  String-pattern + block currently raises NoMethodError.

### Regex literals

Covered: `/.../` literal, single-character classes (`/[aeiou]/`),
the empty regex `//` (matches between every character),
anchors `\A` (string start) and `\z` (string end).

Divergence from CRuby on the `^` anchor: rubyrs's regex engine
fires `^` only at the string start, not at every line start in
multi-line input. `"Text\nFoo".gsub(/^/, ' ')` returns
`" Text\nFoo"` (one anchor); CRuby returns `" Text\n Foo"` (one
per line). Tracked as a separate engine upgrade. The upstream
ruby/spec `it` block covering this is skipped in
[`crates/rubyrs/spec/ruby/string_gsub_spec.rb`](../crates/rubyrs/spec/ruby/string_gsub_spec.rb)
with a comment naming the upstream expectation and pointing
back at this section; the skip un-resolves once per-line
anchoring lands.

### Method objects

Covered: `obj.method(:name)` capture (including implicit-self
`method(:foo)` inside a class body), `Class#instance_method`,
`Method#call` / `Method#()` shorthand (with ArgumentError on
arity mismatch), `Method#curry` (including the explicit-arity
form), `Method#>>` / `Method#<<` composition (with another
Method or any Proc on the RHS), `Method#==` /
`UnboundMethod#==`, `Method#owner` (the class/module that
defined the method, walking through inheritance and through
`alias_method`), `Method#receiver` (the bound object, by
identity), `Method#to_proc` (both explicit `.to_proc` and the
implicit `&meth` forwarding path).

Divergence from CRuby on `Method#==` for aliased methods:

```ruby
class Foo
  def bar; 1; end
  alias_method :baz, :bar
end
f = Foo.new
f.method(:bar) == f.method(:baz)  # CRuby: true; rubyrs: false
```

rubyrs's `Method#==` compares both the underlying Method
pointer and the call name; an `alias_method :baz, :bar`
produces a Method whose call name is `:baz`, so the equality
check returns false. CRuby looks through the alias. Tracked
as a separate engine upgrade. The upstream ruby/spec `it`
block covering aliased equality is skipped in
[`crates/rubyrs/spec/ruby/method_equal_spec.rb`](../crates/rubyrs/spec/ruby/method_equal_spec.rb).

### Integer built-in methods

Covered: arithmetic (`+ - * / %`), comparisons (`== != < <= > >=`),
predicates (`zero?` / `positive?` / `negative?` / `odd?` /
`even?`), `abs`, `to_s` / `to_s(base)`, `times { |i| ... }`,
`digits` / `digits(base)`, `bit_length`, `succ` / `pred`,
`Comparable#clamp` (including Range form).

Divergence from CRuby on `Integer#digits` for negative receivers:

```ruby
-12345.digits(7)  # CRuby: raises Math::DomainError
                  # rubyrs: raises ArgumentError
```

CRuby distinguishes "the radix is bad" (`ArgumentError`) from
"the input violates the math domain" (`Math::DomainError`);
rubyrs collapses both to `ArgumentError`. The
`Math::DomainError` class isn't yet exposed in the subset, so
the distinction has no place to land. The upstream ruby/spec
`it` block covering this is skipped in
[`crates/rubyrs/spec/ruby/integer_digits_spec.rb`](../crates/rubyrs/spec/ruby/integer_digits_spec.rb).

### Float built-in methods

Covered: arithmetic (`+ - * / % **`), comparisons
(`< <= > >=` via spaceship), mixed-numeric coercion
(`5 + 5.0`, `5 == 5.0`), `to_s` / `inspect`, `to_i` / `to_f`,
`abs`, predicates (`zero?` / `positive?` / `negative?` /
`nan?` / `infinite?` / `finite?`), `floor` / `ceil` (Int
result), `round` / `truncate` — both nullary (Int) and the
precision-arg form (`round(n)` returns Float for `n > 0`,
Int for `n == 0`, Int with low-order digits zeroed for
`n < 0`; `truncate(n)` analogous).

### Array built-in methods

Covered (no-block): `length` / `size`, `push` / `<<`, `[]`,
`[]=`, `first` / `last`, `empty?`, `include?`, `count` /
`count(needle)`, `sum` (Int-only), `min` / `max`, `sort`,
`reverse`, `uniq`, `compact`, `flatten` (depth 1), `join` /
`join(sep)`, `+` / `-`, `concat`, `replace(other)`, `take(n)` / `drop(n)`,
`to_a`, `tally`, `combination(n)` / `permutation([n])` (both
return materialised Arrays — Enumerator isn't modelled),
`assoc` / `rassoc`, `pack(format)` (subset of CRuby's
directives — see Pack/Unpack below), `dig(*keys)`, `inject` /
`reduce` with Symbol or block.

Covered (block): `each`, `map` / `collect`, `select` /
`filter`, `reject`, `find` / `detect`, `any?` / `all?` /
`none?`, `each_with_index`, `each_with_object`, `sort_by`,
`min_by` / `max_by` (both single-element and `min_by(n)` /
`max_by(n)` top-n forms), `group_by`, `partition`,
`chunk_while`, `take_while` / `drop_while`, `flat_map` /
`collect_concat`, `each_slice` / `each_cons`, `bsearch`
(two CRuby modes — Bool-block for find-minimum, Int-block
for find-any), `filter_map`, `chunk`, `zip`.

Mutating bang forms: `sort!`, `uniq!`, `compact!`, `flatten!`,
`reverse!`.

Divergence from CRuby on `Array#take(negative)`:

```ruby
[1].take(-3)  # CRuby: raises ArgumentError
              # rubyrs: returns [] (silent)
```

Similar in spirit to the `Integer#digits` divergence above
(`-12345.digits(7)` on a negative RECEIVER raises
ArgumentError rather than Math::DomainError) — both are
cases where rubyrs returns a less specific signal than
CRuby, just via different mechanisms. The upstream
ruby/spec `it` block covering this is skipped in
[`crates/rubyrs/spec/ruby/array_take_spec.rb`](../crates/rubyrs/spec/ruby/array_take_spec.rb)
with a comment naming the upstream expectation and pointing
back here.

### Hash built-in methods

Insertion-ordered with linear lookup (O(n) on n keys —
acceptable for the niche).

Covered (no-block): `length` / `size`, `[]` / `[]=`,
`empty?`, `include?` / `has_key?` / `key?` / `member?`,
`keys`, `values`, `to_h` / `to_a`, `merge` (other Hash),
`delete`, `invert`, `store`, `except(*keys)`, `slice(*keys)`
(argument-order result), `compact` / `compact!` (the
mutating form returns `nil` when nothing changed, matching
CRuby's "nil = unchanged" convention), `dig(*keys)`,
`fetch(key)` / `fetch(key, default)` / `fetch(key) { ... }`,
`inspect`.

Covered (block): `each` / `each_pair` (yields `|k, v|`),
`each_with_index`, `map` / `collect` (returns Array of block
results), `select` / `filter`, `reject`, `find` / `detect`,
`any?` / `all?` / `none?`, `sort` / `sort_by`, `min_by` /
`max_by`, `group_by`, `transform_keys` / `transform_values`
(both non-mutating; collisions in `transform_keys` follow
CRuby's later-wins iteration order), `filter_map` (collects
truthy block returns into a flat Array — not a Hash —
matching CRuby).

Divergence — `Hash.new` with a default value / default proc
isn't supported: `Hash.new(5)` and `Hash.new { ... }` both
return an `Object`, not a Hash. Calling any Hash method on
the result (`.keys`, `.[]`, `.empty?`) raises NoMethodError.
For the niche the runtime targets (small embedded DSLs),
the default-value behaviour rarely shows up; full
constructor support is a separate engine task. The upstream
ruby/spec `it` blocks that touch this form are skipped
inline in
[`crates/rubyrs/spec/ruby/hash_keys_spec.rb`](../crates/rubyrs/spec/ruby/hash_keys_spec.rb)
with `# Skipped` comments naming the upstream line.

### Range built-in methods

Closed Int–Int ranges (and the partial-range cases listed
below) are the primary subset. String–String ranges drive
`('a'..'z').each` / `.to_a` / `.size` / `.include?(String)`.

Covered (no-block): `begin` / `first` / `min`, `end` /
`last` / `max`, `size` / `length` / `count`, `exclude_end?`,
`include?(Int)`, `cover?(Int)`, `cover?(Range)` (true iff
the other range is fully contained; empty sub-ranges —
`begin ≥ end` excl or `begin > end` incl — do NOT cover,
matching CRuby), `to_a` / `sort`, `sum` / `sum(init)`,
`step(n)` (Array result), `inject` / `reduce`. Endless
ranges (`(1..)`) support `first(n)`; beginless (`..n`) only
the methods that don't need an anchor.

Covered (block): `each`, `step(n) { |i| ... }` (yields step
values, returns the receiver), `map`, `select` / `filter`,
`reject`, `find` / `detect`, `any?` / `all?` / `none?`,
`each_with_index`, `each_with_object`, `min_by` / `max_by`,
`group_by`, `sort_by`, `partition`, `inject` / `reduce`
(with block, optional init).

### String pack/unpack (subset)

`String#unpack(format)` and `Array#pack(format)` support the
directives the niche actually uses:

  `C` / `c`       8-bit unsigned / signed
  `n` / `N`       16-bit / 32-bit big-endian unsigned
  `v` / `V`       16-bit / 32-bit little-endian unsigned
  `q` / `Q`       64-bit signed / unsigned (native LE)
  `a` / `A` / `Z` raw / space-null-trimmed / null-terminated
                  strings

Counts (digits or `*`) are honoured. Whitespace inside the
format string is silently ignored (CRuby behaviour).
Unsupported directives (`m`, `U`, `w`, `f` / `d` / `e` / `E`
/ `g` / `G`, etc.) raise `ArgumentError`. `String#bytes`
ships alongside for inspecting packed output without a
`unpack("C*")` round-trip.

### String encoding stubs

`String#encode(target)` and `String#force_encoding(target)`
are no-ops that return the receiver (Rc-shared, no copy).
The subset stores raw bytes with no per-string encoding tag
(`Vec<u8>`-backed `RStr`); cross-encoding transliteration is
explicitly out of scope. The methods exist for compatibility
with library code that defensively normalises at boundaries
(`s.force_encoding("UTF-8")`).

Query-side stubs:

- `String#valid_encoding?` — always returns `true`. The
  receiver is viewed via `String::from_utf8_lossy`, so the
  observable character stream is well-formed UTF-8 by
  construction. CRuby can return `false` for malformed
  byte sequences in encoding-tagged strings; we can't
  model that.
- `String#encoding` — returns the encoding NAME as a
  `String` (`"UTF-8"`). CRuby returns an `Encoding`
  object. The portable usage shape is `.encoding.to_s`
  or `.encoding.to_s == "UTF-8"`. Direct
  `str.encoding == Encoding::UTF_8` does NOT work — even
  if `Encoding::UTF_8` were added later, the comparison
  would be String-vs-Encoding-object and diverge from
  CRuby.

### Object reflection

Covered: `obj.class`, `obj.is_a?(C)` / `kind_of?` /
`instance_of?`, `obj.respond_to?(sym)`, `obj.equal?(other)`
(identity), `obj.methods` (Array of Symbols of every method
the receiver can dispatch — for user-class instances walks
the class chain; primitives currently return `[]`, see
divergence below), `obj.instance_variables` (Array of
`@`-prefixed Symbols for Object instances; `[]` for
everything else), `obj.tap` / `obj.then` / `obj.yield_self`,
`obj.frozen?`, plus the Method-object capture chain
described in the "Method objects" section above.

Divergence from CRuby on `obj.methods` for primitives:

```ruby
5.methods         # CRuby: ~150-method Array (Kernel, Numeric,
                  #        Comparable, Integer)
                  # rubyrs: []
```

The subset doesn't enumerate Kernel methods individually —
they're handled as universal arms in `primitive_call` /
`responds_to`. User-class instances enumerate fine.

### Metaprogramming (PoC)
- `alias_method :new, :old` — resolves the source method by walking
  the surrounding class's ancestor chain (so inherited methods can
  be aliased) and installs the same `Rc<Method>` under the new
  SymId on the *current* class. Alias shares the original's
  `defining_class`, so `super` from the aliased name walks the
  original's superclass chain, matching CRuby's "module of
  definition" rule. A missing source name raises `NameError`.
  Compile-time desugar; both args must be Symbol literals (dynamic
  `alias_method(*syms)` falls through).
- `method_missing(name)` — on an Object receiver whose class chain
  defines `method_missing`, missed calls route there with the
  missed name passed as a Symbol. Inherited through the superclass
  chain. Primitives (Int, Str, Sym, …) skip the lookup and raise
  NoMethodError as before. See [ADR 0010](adr/0010-metaprogramming-poc.md).
- `define_method(:name) { |args| ... }` — installs the block as a
  method on the surrounding class. The Method shares the block's
  captured-locals `Rc<RefCell<Vec<Value>>>`, so closure semantics
  hold: writes to outer-scope locals from inside the method body
  are visible to the lexical scope (and to other invocations of
  the same method). Compile-time desugar; arg must be a Symbol
  literal. GC walks all installed closure-methods as roots — see
  [ADR 0010](adr/0010-metaprogramming-poc.md).
- `obj.instance_eval { |o| ... }` — runs the block with `self`
  swapped to `obj`. Inside, `@ivar` reads/writes go to `obj`'s
  ivars, and method calls use `obj` as the receiver. The block's
  last expression is the return value (matches CRuby).
- `def obj.name; ...; end` — singleton method install. Allocates
  an eigenclass for `obj` on first install (a synthetic
  `Rc<Class>` whose `superclass` is `obj.class`, so method
  lookup walks the singleton first then falls through to the
  real class). `obj.class` still reports the user-declared
  class — CRuby skips the eigenclass when reporting. Only
  user-class instances (`Value::Object`) are supported;
  `def 1.foo` / `def "x".foo` raise `TypeError`. `super`
  inside walks the original class chain. Same allocation /
  GC plumbing is reachable via `obj.define_singleton_method`.
- `obj.define_singleton_method(:name) { |args| ... }` —
  closure-method form of the above, with the same
  block-captures-outer-locals semantic as `define_method`.
- `cls.class_eval { |c| ... }` (alias `module_eval`) — runs the
  block with `self = cls` AND with the class-body machinery
  active, so `def name; ...; end` inside lands on `cls`'s method
  table. This is the dominant DSL use of `class_eval`. Non-class
  receivers raise `TypeError`. **Divergence**: rubyrs returns
  the class (re-using the class-body Return path), CRuby returns
  the block's last expression — see the lock-in test
  `returns_the_class_for_now` in
  [`spec/ruby/class_eval_spec.rb`](../crates/rubyrs/spec/ruby/class_eval_spec.rb).

**Caveats for the PoC**
- No `*args` splat — `method_missing(name, *args)` and arity-flexible
  `define_method` aren't expressible yet. Tracking item on the
  "Not supported" list.
- `method_missing` is only invoked when the receiver is a user-class
  instance (`Value::Object`). Adding per-primitive class chains is
  a follow-up.
- `alias_method` / `define_method` outside a class body (called at
  the toplevel or, more surprisingly, inside an instance method
  with implicit self) install into `toplevel_methods` instead of
  raising — the compile-time intercept doesn't track class-body
  context. Same divergence already in effect for
  `attr_accessor` / `attr_reader` / `attr_writer`
  (see [`compiler.rs:319-326`](../crates/rubyrs/src/compiler.rs)
  for the rationale). Fix is a single shared piece of context
  tracking for all four; treated as a follow-up so the existing
  `attr_*` semantics aren't changed in isolation.
- Per-iteration dispatch is ~3× CRuby's. See
  [`examples/metaprog_bench/README.md`](../crates/rubyrs/examples/metaprog_bench/README.md)
  — peak memory is still ~5× lighter than CRuby on the same workload.

### Runtime
- Mark-sweep GC over `Instance`, `Array`, `Hash` (cycle-safe). See
  [ADR 0003](adr/0003-rc-plus-mark-sweep-hybrid-gc.md) and
  [ADR 0005](adr/0005-pinned-stack-for-native-driven-loops.md).
- Class definitions reopenable (`class Foo` twice merges methods)
- Single integer type (`i64`); wrapping arithmetic
- Global string interner with `SymId`-keyed method/ivar lookup, so
  method dispatch hashes on `u32` not bytes. See
  [ADR 0006](adr/0006-global-string-intern.md).
- All user-facing errors flow as `Trap` values; the host never sees a
  Rust panic from script-level mistakes. See
  [ADR 0007](adr/0007-host-embedding-api.md) and
  [ADR 0008](adr/0008-resource-caps-for-untrusted-scripts.md) for
  the embedding API and per-runtime resource caps (`fuel`,
  `max_heap_objects`, `max_frames`).

## Divergences from CRuby

Deliberate behavioural differences. Each is locked in by a
test so it stays a choice, not drift.

### Anonymous block forwarding outside `def foo(&)`

```ruby
def bar
  inner(&)   # no enclosing `def bar(&)`
end
```

- CRuby raises `SyntaxError: no anonymous block parameter` at
  parse time (unconditional, regardless of how the callee uses
  the block).
- rubyrs translates `inner(&)` to `inner(&local["&"])`, and the
  read auto-creates the slot as `nil`. The call degenerates to
  `inner(&nil)` — i.e. proceeds without a block. The observable
  runtime outcome depends on the callee:
  - Callee ignores the block → call succeeds silently (no error).
  - Callee invokes `blk.call` on its `&blk` parameter →
    `NoMethodError: undefined method 'call' for NilClass`.
  - Callee uses `yield` → `RuntimeError: no block given (yield)`.
- Why: pushing this diagnostic up to parse time would require
  threading anonymous-block-availability through the AST
  translator and SExpr context. The silent-success case for
  block-ignoring callees IS a behavioral divergence — a typo
  like `def bar; inner(&); end` where `inner` happens not to
  use the block won't be caught. Accepted as a known cost of
  the simpler implementation, since the common pass-through
  wrapper pattern (the use case that motivates anonymous
  forwarding in the first place) does call the block and
  surfaces an error.
- Test: `anon_block_forward` in `crates/rubyrs/tests/diff/`.

### `rescue` with an unresolved class name

```ruby
begin
  raise SomeError
rescue NeverDefined => e   # NeverDefined isn't loaded
  puts "won't reach"
end
```

- CRuby raises `NameError: uninitialized constant NeverDefined`
  eagerly when the rescue clause would fire.
- rubyrs silently skips the clause. The `PushRescue` op resolves
  the class via `Vm.classes` at push-time; if the lookup misses,
  the handler stores `filter_class: None` and the unwinder
  treats it as "matches nothing", so the original exception
  continues unwinding.
- Why: chasing CRuby's eager-NameError semantics for the
  rescue-class case would require a separate per-handler check
  in the unwinder and an extra error path that would itself
  need a class lookup. Not worth the complexity for the
  embedding use-cases we serve.
- Test: `rescue_with_unresolved_class_does_not_catch` in
  `crates/rubyrs/tests/embed.rs`.

### `class MyHash < Hash` returns Value::Object, not Value::Hash

```ruby
class MyHash < Hash
end
h = MyHash.new
h[:k] = 1  # NoMethodError in rubyrs; works in CRuby
```

- The `Hash.new` intercept (`vm/dispatch.rs`) matches solely on
  `cls.name == "Hash"`, so user subclasses fall through to the
  generic `Class.new` allocator and return a bare
  `Value::Object` instance whose methods table doesn't include
  the Hash primitives (`[]`, `keys`, `each`, ...).
- Switching the intercept to a `Hash`-in-`cls.ancestors` walk
  would fix the primitive-method side but BREAK any custom
  instance methods defined on the subclass: `Value::Hash`
  dispatches only the hardcoded primitives, so
  `class MyHash < Hash; def my_helper; ...; end; end` would
  see `my_helper` become NoMethodError too.
- The proper fix is a structural one: give `Value::Hash` a
  class-of slot AND route primitive Hash methods through that
  slot rather than the fixed primitive table. Sizeable
  refactor; deferred until a real caller needs Hash
  subclassing.
- No test pin (would lock in divergence); this entry is the
  contract.

### `freeze` doesn't actually freeze — `frozen?` always false

```ruby
EMPTY = [].freeze
EMPTY << :x         # CRuby: FrozenError; rubyrs: silently mutates
puts EMPTY.inspect  # rubyrs: [:x] — shared constant corrupted
puts EMPTY.frozen?  # rubyrs: false (never tracked)
```

- CRuby tracks an immutability bit per object; `freeze` flips
  it on, subsequent mutation methods raise `FrozenError`.
  rubyrs doesn't model the bit at all.
- `freeze` returns the receiver (so chainable patterns like
  `EMPTY_HASH = {}.freeze` compile cleanly) and `frozen?`
  returns `false`. Mutation methods (`<<`, `[]=`, `push`,
  `delete`, ...) don't check.
- Real risk: code that uses `EMPTY = [].freeze` as a shared
  immutable sentinel will see that sentinel mutated by any
  later mutation call. CRuby would have raised; rubyrs
  silently corrupts.
- Why we stub instead of implementing: real freeze tracking
  needs an immutability bit on every heap-managed `HeapObj`
  variant + a check in every mutator. Embeddable use cases
  the host VM serves (template engines, config loaders,
  short-lived scripts) generally don't rely on freeze as a
  correctness mechanism; the stub unblocks common idioms
  like `EMPTY_HASH = {}.freeze` without the per-mutator
  enforcement cost.
- Tests: `tilt_load_capabilities.rb` pins the chainable
  return shape; the corruption divergence is NOT diff-pinned
  (would lock in divergence) but is part of this contract.

### `ResourceExhausted` is host-only, not script-visible

```ruby
begin
  while true; end             # exhausts fuel
rescue Exception => e         # explicit Exception filter
  puts "won't reach"
end
```

- The resource trap (fuel / heap-cap / frame-cap) propagates as
  a host-level `Trap` directly out of `Runtime::eval`. It does
  not go through `unwind_with_exception`, so no `rescue` clause
  — bare or class-filtered, even `rescue Exception` — can
  intercept it.
- See [ADR 0008](adr/0008-resource-caps-for-untrusted-scripts.md).
  The earlier promise in that ADR that `rescue Exception` could
  catch the trap was aspirational; it's been retracted.
- Tests: `resource_exhausted_cannot_be_swallowed_by_bare_rescue`
  and `resource_exhausted_is_uncatchable_even_with_rescue_exception`.

### `deprecate_constant` is accepted but silent

```ruby
class Foo
  OLD = 1
  deprecate_constant :OLD
end
Foo::OLD                       # CRuby: warns; rubyrs: 1 (no warning)
```

- `Module#deprecate_constant` accepts any number of Symbol args
  and returns the receiver (chainable form), matching CRuby's
  call-site contract.
- Reading a deprecated constant does NOT emit a warning in
  rubyrs — there's no warning subsystem to route it through.
  The constant value is returned silently, identical to a
  non-deprecated read.
- Why: MRI's `lib/erb.rb:264` calls
  `deprecate_constant :Revision` at the class body's top
  level; without the call-shape acceptance, ERB fails to load
  (this is the motivating consumer behind the stub).
- Test: extends `crates/rubyrs/tests/diff/tilt_load_capabilities.rb`
  (locks both forms — class-body and explicit-receiver — plus
  `respond_to?` parity).

### `private_constant` / `public_constant` are accepted but not enforced

```ruby
class Foo
  BAR = 1
  private_constant :BAR
end
Foo::BAR                       # CRuby: NameError; rubyrs: 1
```

- `Module#private_constant` and `Module#public_constant` accept
  Symbol / String args (or no args) and return the receiver,
  matching CRuby's call-site contract. Internal references to the
  named constants work in both runtimes.
- External access to a private constant (`Klass::Foo` from outside
  the class body) does NOT raise NameError in rubyrs — the
  visibility flag isn't tracked on the per-class constants table.
  CRuby raises.
- Why: tilt's `lib/tilt/mapping.rb` calls
  `private_constant :BaseMapping` from a class body; without the
  call-shape acceptance, tilt fails to load. Enforcement on the
  lookup side is a separable change that can land later without
  breaking this contract.
- Test: `crates/rubyrs/tests/diff/private_constant.rb` (locks the
  call shapes and the receiver return value; the no-enforcement
  divergence is documented here rather than encoded as a passing
  diff).

### `Class#singleton_class` returns a redirecting eigenclass shell

```ruby
class Foo; end
Foo.singleton_class.equal?(Foo.singleton_class)  # CRuby: true; rubyrs: true
Foo.singleton_class.class                        # CRuby: Class; rubyrs: Class
Foo.singleton_class.name                         # CRuby: nil;   rubyrs: nil

# Method installs redirect to Foo's singleton_methods:
Foo.singleton_class.class_eval do
  define_method(:greet) { "hi" }       # lands on Foo.singleton_methods
  def shout; "HI"; end                 # ditto, via Op::DefMethod redirect
  alias_method :hello, :greet          # ditto, source resolved via real class
end
Foo.greet   # => "hi"
Foo.shout   # => "HI"
```

- **Real eigenclass shell since PR #253 (layer #23).** Previously a
  Tier-1 stub that returned the receiver itself. The shell is a
  separate `Class` object carrying a `singleton_target` weak ref back
  to the real class; the three method-install paths (`Op::DefMethod`,
  `Op::DefMethodBlock`, runtime `Module#define_method` arm) plus
  `Op::AliasMethod` (source lookup + install) detect the shell and
  redirect into the real class's `singleton_methods`. This makes
  sinatra's `define_singleton` idiom (`singleton_class.class_eval do
  define_method(name, &content) end`) work end-to-end.
- Identity invariant
  `X.singleton_class.equal?(X.singleton_class)` still holds via a
  cached shell.
- Cross-CRuby alignment: `X.singleton_class.name` now returns `nil`
  (CRuby shape); `to_s` / `inspect` render `"#<Class:X>"`.
- **Remaining divergence** — reflection on the shell itself: methods
  installed via the shell are visible through `X`'s singleton dispatch
  (`X.method_name`) but the shell's own `instance_methods` /
  `method_defined?` / `include?` / `include` / `prepend` operate on the
  shell's empty tables, NOT on the redirected installs. Sinatra and
  the mainstream `singleton_class.class_eval` idiom don't probe the
  shell reflectively, so this is documented divergence rather than a
  bug. A future PR can mirror writes into the shell's tables (or
  proxy the reflection methods) without breaking the redirect.
- `Object#singleton_class` for non-Class receivers is not implemented
  in this arm and will raise NoMethodError.
- `Runtime::reset()` drops the cached shell so any session-time
  installs disappear; the shell rebuilds lazily on the next call.
- Tests:
  - `crates/rubyrs/tests/diff/class_singleton_class.rb` (idempotency,
    `class is Class`, the ERB-shape `@_init` cache invariant,
    `respond_to?` parity).
  - `crates/rubyrs/tests/diff/singleton_class_class_eval.rb` (PR #253:
    `define_method` install, parsed `def` install, identity, mutation
    persistence in the sinatra `add_charset << x` shape, visibility
    leak fix, `alias_method`, the `nil` name pin).

### `Kernel#eval` / `Class#class_eval(string)` skip caller scope

```ruby
x = 99
puts eval("x")                 # CRuby: 99; rubyrs: NameError
```

- `Kernel#eval(string)` parses, compiles, and runs the source at
  top level. Returns the final expression's value (matches CRuby).
- `Kernel#eval` accepts up to 4 args matching CRuby's signature
  (`eval(src, binding, file, line)`), but the `binding` arg is
  silently ignored — rubyrs doesn't model `Binding`, so eval'd
  code sees only top-level scope, not the caller's locals.
  `file` is wired through to source registration so backtraces
  and `Method#source_location` for methods defined inside the
  eval'd source resolve.
- `Class#class_eval(string [, file, line])` (and `module_eval`
  alias) does NOT switch to the receiver class's class-body
  context. Bare `Foo.class_eval("def bar; end")` lands `bar` at
  top level, not on `Foo`. The block-form
  `Foo.class_eval { def bar; end }` continues to work as in
  CRuby (intercepted separately and routed through the existing
  `invoke_block_with_self` machinery — `bar` lands on `Foo`).
- Why: completes the tilt full-render chain (tilt's
  `eval_compiled_method` calls `Object.class_eval(method_source)`
  where `method_source` is itself a `Tilt::TOPOBJECT.class_eval
  do def ... end end` block, so the inner block-form handles the
  actual class context switching — top-level eval is enough).
  Implementing real class-body switching requires plumbing
  `class_stack` and `class_visibility_stack` for the duration of
  the eval; deferred until a non-self-wrapping consumer needs it.
- Test: `crates/rubyrs/tests/diff/eval_basics.rb` (locks Kernel#eval
  shapes, the tilt-shape `class_eval(string)` with self-wrap,
  the file+line signature, and `module_eval` alias parity).

### `Kernel#Array` / `Integer` / `Float` / `String` / `sprintf` / `format` reachable on every receiver

```ruby
class Plain; end
Plain.new.Array([1, 2])   # CRuby: NoMethodError (private); rubyrs: [1, 2]
Plain.new.Integer("42")   # CRuby: NoMethodError (private); rubyrs: 42
Plain.new.respond_to?(:Array) # CRuby: false; rubyrs: false (matches)
```

- CRuby's `Kernel#Array` / `Integer` / `Float` / `String` /
  `sprintf` / `format` are **private** instance methods on the
  `Kernel` module (which is mixed into `Object`). The standard
  ancestor chain finds them, but the private-visibility check
  raises `NoMethodError (private method called)` when invoked
  via an explicit receiver.
- rubyrs's `do_call` has a Kernel-fallback that routes these
  six names to `builtin_call` when normal lookup AND
  `method_missing` both miss. The fallback ignores
  private-visibility — `obj.Array(...)` silently succeeds where
  CRuby raises.
- `respond_to?` still returns `false` for these names on a
  non-Kernel-mixin receiver (CRuby parity); the divergence is
  the call shape, not feature detection.
- A user `method_missing` correctly wins over the fallback
  (matches CRuby's "private NoMethodError → method_missing
  intercepts" path); fixture `kernel_array_via_method.rb`
  Shape 6 pins this.
- `Kernel#eval` is intentionally NOT in the fallback set —
  with-recv `obj.eval(...)` would silently discard the
  receiver, which is surprise-driven; rubyrs raises
  NoMethodError there.
- Why: sinatra's `codes.flat_map(&method(:Array))`
  (`sinatra/base.rb:1404`) captures `method(:Array)` from an
  explicit receiver and re-dispatches through `BoundMethod#call`,
  which lands in the with-recv path. Without the fallback the
  framework load fails. Implementing real private-visibility
  for Kernel methods uniformly would require a private-bit on
  every method entry plus an explicit-receiver gate in
  dispatch; the fallback is the pragmatic Tier-1 stub.
- Tests: `crates/rubyrs/tests/diff/kernel_array_via_method.rb`
  (8 shapes: direct, capture, &-conversion, sinatra-shape,
  method_missing-wins, `obj.eval` NoMethodError,
  `respond_to?` false).

### `Module#define_method` 2-arg Proc form not implemented

```ruby
p = proc { |x| x * 2 }
Foo.define_method(:double, p)   # CRuby: installs :double; rubyrs: NotImplemented
Foo.define_method(:double) { |x| x * 2 }  # both: installs :double
Foo.define_method               # both: ArgumentError (wrong arity)
Foo.define_method(:x)           # both: ArgumentError (tried to create Proc without block)
```

- CRuby's `Module#define_method` accepts EITHER a block
  (`define_method(:name) { ... }`) OR a Proc/Method/UnboundMethod
  as a second positional argument
  (`define_method(:name, proc_obj)`).
- rubyrs Tier-1 only implements the block form. The 2-arg Proc
  form raises a CRuby-shape `ArgumentError` ("the 2-arg
  Proc/UnboundMethod form of Module#define_method is not yet
  supported by rubyrs Tier-1"). The 0-arg and 3+-arg cases
  raise standard wrong-arity ArgumentError matching CRuby.
- Why: no current consumer surfaced the 2-arg form (sinatra's
  `define_singleton` shape (`base.rb:1735`) uses
  `define_method(name, &content)`, the `&`-conversion form
  which IS the block path with a Proc as the block_arg).
  Implementing the 2-arg form requires extracting the Proc's
  proto/captures into an installable Method, parallel to the
  existing closure-method install in `Op::DefMethodBlock`.
- Tests: `crates/rubyrs/tests/diff/module_define_method.rb`
  (CRuby-shape arity errors pinned; 2-arg NotImplemented
  surface tested via `rescue ArgumentError` with the
  not-yet-supported message).

### Kernel module functions reachable via `method(:name).call(...)` round-trip

```ruby
m = method(:Array)
m.class                    # both: Method
m.call([1, 2])             # both: [1, 2]
m.call(nil)                # both: []
[[1], [2]].flat_map(&m)    # both: [1, 2]
```

- `method(:Array)` (and similar for Integer/Float/String/sprintf/
  format) produces a `BoundMethod` whose internal `snapshot` is
  `None` (the regular class lookup misses the Kernel module
  function). Subsequent `.call(args)` re-dispatches through
  `BoundMethod#call`'s fallback path, which lands in
  `do_call` with `no_recv=false` — where the Kernel-fallback
  (documented above) routes to `builtin_call`.
- The `&method(:Array)` block-arg form (sinatra's
  `flat_map(&method(:Array))` shape) works through the same
  pipeline — `&` builds a `<callable-forwarder>` synth block
  that calls `m.call(elem)` per element.
- `method(:eval)` resolves and `.call(src)` works (via the
  no_recv path), but the with-recv form `obj.eval(...)` does
  not (see Kernel#eval entry above).

## Deferred to outer tiers

Features whose absence is a tier-assignment decision per
[ADR 0015](adr/0015-concentric-architecture.md), not a "we'll never do
this". The table below records *where* each item is expected to land
and *what's already in place* to make that future work tractable.

| Feature | Target tier | Current Tier 1 state |
|---------|-------------|----------------------|
| Arbitrary-precision Integer arithmetic (`2**100`, true Bignum) | Tier 1 (`bignum` Cargo feature, default ON) | Phase A shipped: `Value::BigInt` + `HeapObj::BigInt`, integer-literal overflow promotes to BigInt at AST time, `+ - * / %` + comparisons + `to_s` / `inspect` / `class` work via `try_bigint_binop`, Float×BigInt coerces with Float-wins-on-mix. Build without `--no-default-features` to drop the `num-bigint` dep and fall back to wrapping i64 arithmetic. Phase B (`**`, bit ops, unary, `abs`) still on the bench. Earlier "i64 saturates at parser" wording belongs to the pre-Phase-A era and only applies under `--no-default-features`. |
| `Rational`, `Complex`, `BigDecimal` | Tier 2 / Tier 3 | None. |
| Real nested-module namespacing (`Foo::Bar` after `module Foo; class Bar; end; end`) | Tier 1 (shipped) | Class table now keyed by qualified SymId, so top-level `Bar` and `Foo::Bar` are independent `Class` objects with separate method / ivar / superclass tables (was a `class_qualified_separates` divergence, closed). Bare-name reads inside a class/module body walk a precomputed cref chain (`Op::LoadConstChain`) before falling back to the top-level bare slot — matches CRuby's "innermost-scope wins" behaviour. `Module.nesting` reflection API is still deferred (the cref chain exists at compile time but isn't exposed yet); two top-level modules that DON'T collide via the qualified-key story still don't get a real `Module` shape distinct from `Class` — see the `Module` semantics row below. |
| `Time` class (`Time.now`, `#to_i`, `#nsec`, `Time.at(sec, nsec, …)`) | Tier 2 | None as a primitive value type. User classes carrying `(sec, nsec)` plus `register_type_internal` already round-trip Time-shaped ext-type frames byte-identical to MRI (see `tests/cext_msgpack_app_ext.rs`). |
| `Fiber`, `Thread`, `Mutex`, `Ractor` | Tier 2 (`_fiber` / `_thread` / `_ractor` feature gates in ADR 0015) | None — single-threaded at the language level by design. |
| Full `Module` semantics (real Module type distinct from Class, `include` chain with method-lookup ordering matching CRuby exactly) | Tier 2 | PoC: `include Mod` works via method-table copy; ancestry walks via `class_is_a` + `includes` list. Strict CRuby `ancestors` compatibility deferred. |
| `eval` (string form), `binding`, `ObjectSpace` | Tier 4 (`mri-compat`) | None — explicitly out of scope for Tier 1's sandbox guarantees. |
| `require / load / autoload` from LOAD_PATH | Tier 1 (partial) | `require "/abs/path.rb"`, auto-`.rb`, cwd-relative, caller-source-dir + caller-source-parent hops, AND `$LOAD_PATH` walking all work (covered by `tests/diff/require_xpkg.rb`). CRuby's auto-populated stdlib/gem `$LOAD_PATH` entries are NOT pre-seeded — scripts opt in via `$LOAD_PATH.unshift(dir)`. `load` and `autoload` are still deferred. |
| Pure-Ruby stdlib subset (`Pathname`, … future names) | Tier 3 (`stdlib` Cargo feature) | Default Tier 1 build keeps the lenient stub "feature-absent surface": `require 'pathname'` materialises the constant shell, calls raise NoMethodError. With `--features stdlib` the same require path loads `crates/rubyrs/src/stdlib_vendor/<name>.rb` (deterministic, fs-free subset) and the module behaves CRuby-compatibly. Pilot: `Pathname` path-string manipulation methods, covered by `tests/diff/stdlib_pathname.rb` (the test is `#[cfg(feature = "stdlib")]`-gated). |
| C extension API (CRuby ABI compatibility) | Tier 4 (`mri-compat`) per ADR 0015 | A working partial implementation lives in `crates/rubyrs-cext` as a spike, not as a covenant — see ADR 0015's "C-ext ABI stays out of v1 and v2" rule. Specifically the L3-J/K + A3/A4 work shipped msgpack-shaped FFI that's "real enough to round-trip the wire protocol" but doesn't promise full CRuby C-API equivalence. |
| Refinements, full pattern matching, full encoding model, `Marshal`, `IO` beyond stdout | Tier 3 / Tier 4 | None. |
| Inline cache for method dispatch | Tier 1 (shipped) | 5-way polymorphic per-call-site IC (`IC_WAYS = 5`, widened from 4 in PR #185 after PR #175 measured a cliff at 5 shapes) with round-robin eviction; each way carries `(class_ptr, method_gen, method)` and a `method_gen` bump invalidates every entry. Megamorphic case (> 5 distinct receiver classes at one call site) degenerates to the same uncached walk the original single-slot cache did — worst case unchanged, common polymorphic dispatch no longer thrashes. |

### What ships today: i64-range BigInt protocol

The msgpack BigInt wire protocol round-trips byte-identical to MRI for
any value in `i64` range:

```ruby
require ".../msgpack/bigint.rb"
n = 0x123456789ABCDEF0
bytes = Bigint.to_msgpack_ext(n)
# => [0, 154, 188, 222, 240, 18, 52, 86, 120]   ← matches MRI byte-for-byte
Bigint.from_msgpack_ext(bytes) == n  # => true
```

For embedded host scenarios — passing 64-bit timestamps, counters,
hash-low-words, or any value the host produced as `i64` — this is
the contract. Inputs whose Ruby literal exceeds `i64::MAX` /
`i64::MIN` (e.g. `2**100`) saturate at the parser before bigint.rb
sees them; that's a Tier 2 boundary, not a Tier 1 bug.

See [`tests/diff/cext_msgpack_bigint.rb`](../crates/rubyrs/tests/diff/cext_msgpack_bigint.rb)
for the eight-case acceptance suite (a regular diff_cruby
fixture: each value's pack bytes and round-trip equality are
asserted byte-for-byte against CRuby on the same `bigint.rb`).

## Permanently out of scope at every tier

Nothing in rubyrs is "permanently out of scope" by design — ADR 0015's
tier system explicitly leaves the door open through Tier 4
(`mri-compat`) as a research bet. The items below are *not currently
planned* for any tier:

- Replacing the parser (Prism is fixed per [ADR 0001](adr/0001-prism-as-parser.md))
- Adding a JIT (the bytecode VM is fixed per [ADR 0002](adr/0002-bytecode-vm-not-jit.md))
- Pluggable VM backends (no mruby-fallback, no Truffle interop, one core layered outward — see ADR 0015's "What this is not")

If you need a JIT or a parser-pluggable Ruby today, use CRuby (for
YJIT) or TruffleRuby.
