# Subset semantics

rubyrs targets CRuby 3.4 semantics on the surface it covers, and
pins that surface with differential testing (1,100+ fixtures whose
oracle is CRuby itself — stdout compared byte-for-byte, including
under GC stress; 1112 passing / 0 failed as of 2026-07). This
document is the honest catalogue of where the boundary lies: what
works, what diverges (every divergence documented with its
trigger and trade-off), and what's absent.

If you need Rails (ActiveRecord 7.0 loads, but no database
adapter can run), OS-thread parallelism (Thread is a cooperative
green-thread subset), or the full 100+-encoding registry
(rubyrs ships ~20 encodings, with real transcoding behind
`_encoding_full`) today — use CRuby.

## At a glance

| Area | Status |
|---|---|
| **Syntax** | ~Complete: 149/150 Prism AST node kinds translate (incl. pattern matching `case/in`, refinements, `BEGIN/END`, anonymous arg forwarding, numbered/`it` params, flip-flops) |
| **Core types** | Integer (i64 + BigInt), Float, String (bytes), Symbol, Array, Hash, Range, Regexp, Proc/Lambda/Method, Struct, Set, Time, Complex/Rational (constructors + arithmetic; `String#to_c`/`to_r` absent), Comparable/Enumerable/Enumerator (+ Lazy) |
| **Metaprogramming** | `define_method`, `method_missing`, `send`, singleton classes, `instance_variable_*`, `const_*`, hooks (`inherited`/`included`/…), `alias`, `prepend`/`include`/`extend`, refinements |
| **Exceptions** | Full begin/rescue/ensure/retry, custom hierarchies, `$!` dynamic scoping, catchable `SystemStackError` |
| **Regexp** | Dual engine: linear-time `regex` (default, ReDoS-immune) + `fancy-regex` fallback for lookaround/backrefs; Onigmo ASCII classes (`\s\d\w\h`), Unicode `\b`; named captures, `$~` frame-local |
| **stdlib** | ~39 vendored modules (`json`, `yaml`, `set`, `pathname`, `stringio`, `strscan`, `digest`, `logger`, `cgi`, `bigdecimal`, `date`, `ipaddr`, `erb`, `optparse`, `psych`, `active_support`-lite, …) behind `--features stdlib` |
| **Real gems** | Jekyll 4.4.1 + rouge 4.7.0 + kramdown + Liquid build **byte-identical to CRuby**; RuboCop 1.88.0 runs end-to-end (needs `_prism_native`, in `cli-defaults`; full default cop set, byte-identical offense output on probe files; a handful of documented cop-level gaps on wider corpora); Bridgetown 2.2.1 4-phase probe (require → configuration → `Site.new` → `site.process`) byte-identical (needs `_socket`/`_openssl`); ActiveModel 7.0.10 boots + validates, `require "active_record"` (7.0.10) loads — but no DB adapter can run; Rack 3.1.10 upstream specs at CRuby parity (2026-06 validation); Sinatra 4.2.1 at 17/24 core spec files parity (2026-06 validation); msgpack/bcrypt via the C-ext ABI |
| **Accelerators** | `_json_native`, `_rouge_native`, `_kramdown_native`, `_yaml_native`, `_liquid_native` — Rust engines, byte-identical-or-decline contract |
| **Embedding** | `Runtime` API: fuel/heap/frame caps, capability sandbox (FS/env gated), host fns, captured stdout, incremental eval, wasm32-wasip1 target |
| **Concurrency** | Fiber subset (`_fiber` feature); **cooperative Thread subset on green threads** — `Thread.new`/`join`/`value`/`alive?`/`kill`, thread-locals + thread-variables, `Mutex`, `Queue` (blocking pop), `ConditionVariable`, `Thread.pass` interleaving — no OS threads, no preemption (gaps listed in the [tier table](#deferred-to-outer-tiers)) |
| **Marshal** | Real CRuby `\x04\x08` binary format for the common-tag subset — nil/bool/Integer/Bignum/Float/String/Symbol/Array/Hash/Range/Struct/user objects (+ ivars, links, `marshal_dump`/`marshal_load` hooks); `load(dump(x))` is a genuine deep copy; Proc raises CRuby's TypeError; registry-token fallback + framed IO ports for shapes outside the byte subset ([tier table](#deferred-to-outer-tiers)) |
| **Encoding** | **Partial** — every build ships a ~20-encoding registry (26 `Encoding` constants: GB18030/Big5/GBK/EUC-JP/Shift_JIS/UTF-16/32 families, Latin-1/15, KOI8-R, …) with CRuby-exact `find`/`aliases`/`dummy?`/`ascii_compatible?`/`inspect`, per-string tags for UTF-8/US-ASCII/BINARY, `CompatibilityError`/`UndefinedConversionError`/`ConverterNotFoundError`; **real transcoding + registry-encoding string tags behind `_encoding_full`** (encoding_rs) ([details](#string-encoding)) |
| **Object model gaps** | `Method#==` doesn't look through aliases; `defined?` on a private constant says `"constant"`; `include` into a singleton-class shell records ancestry but doesn't route dispatch ([divergences](#divergences-from-cruby)) |
| **Absent** | Ractor; `ObjectSpace` beyond finalizers (`each_object`/`count_objects`/`_id2ref` missing); implicit `eval` caller-scope capture (explicit `eval(src, binding)` **does** capture locals/self/ivars); `Binding` reflection API (`local_variable_get`/`local_variables`/`receiver`) |

Every "Status" cell above is expanded in the body of this document;
the 19 known behavioural divergences each get their own section
under [Divergences from CRuby](#divergences-from-cruby) (21
sections; 2 are struck through as since-FIXED).

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
- `class Foo < ParentExpr ... end` — the parent slot accepts any
  expression (constant name, local variable, method call), not
  just a constant reference. Mainline cases like
  `class Sub < SomeConstant` and `class Sub < ::Foo::Bar` use
  the fast-path Const opcode; dynamic shapes like
  `class Sub < some_local_var` or
  `class Sub < DelegateClass(Hash)` (factory-method-returns-Class
  pattern) route through the generic `compile_expr` path and
  evaluate the expression to a `Value::Class` before
  `Op::DefClass` consumes it.
- `Class.new { ... }` / `Class.new(SuperClass) { ... }` — anonymous
  Class with the block evaluated as the class body
  (`class_eval`-style: `def name; ... end` inside lands on the
  new class's instance-method table). Superclass defaults to
  Object; an explicit `Class` arg overrides. The block also
  receives the new class as its sole positional arg (CRuby
  parity for the `Class.new { |k| k.foo }` shape that
  `delegate.rb` uses).
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
`encode` / `force_encoding` (real per-string tag semantics;
transcoding behind `_encoding_full` — see "String
encoding" below), `valid_encoding?`, `encoding`,
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
anchors `\A` (string start) and `\z` (string end). Interpolation
(`/^#{var}$/`) compiles at runtime via `Op::CompileRegex`.

Class methods: `Regexp.compile(str)` / `Regexp.new(str)` build a
Regexp from a String pattern (same code path as the literal,
including the Onigmo→Rust `\G` preprocess). `Regexp.escape(str)` /
`Regexp.quote(str)` produce a metachar-escaped String. ASCII
metachars (`. * + ? | ( ) [ ] { } \ ^ $`) match CRuby byte-for-byte;
the only documented divergence is whitespace — Rust's
`regex::escape` doesn't backslash spaces or tabs, CRuby does.
The escape→interpolate→compile pipeline used by gems for
turning untrusted strings into safe patterns still works
identically as long as the input avoids whitespace.

`String#match` (regex OR String argument) returns a full
**MatchData** instance with `[N]` (positional), `[:name]` /
`["name"]` (named groups), `captures`, `named_captures`, `to_a`,
`size` / `length`, `to_s`, `inspect`, `pre_match`, `post_match`,
`string`, `regexp`. Unknown named-capture references raise
`IndexError` matching CRuby's `"undefined group name reference:
<name>"` message. Non-participating named groups (alternation
arms that didn't match) appear in `named_captures` with a nil
value, distinct from missing-name lookups.

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
`none?` / `one?`, `each_with_index`, `each_with_object`,
`sort_by`, `min_by` / `max_by` (both single-element and
`min_by(n)` / `max_by(n)` top-n forms), `group_by`,
`partition`, `chunk_while`, `take_while` / `drop_while`,
`flat_map` / `collect_concat`, `each_slice` / `each_cons`,
`bsearch` (two CRuby modes — Bool-block for find-minimum,
Int-block for find-any), `filter_map`, `chunk`, `zip`.

No-block predicate forms: `any?` / `all?` / `none?` / `one?`
all support the zero-arg form (`arr.any?` tests element
truthiness, no block needed). `any?` is true iff at least one
element is truthy; `all?` iff every element is truthy;
`none?` iff no element is; `one?` iff exactly one is.

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

Covered (block): `each` / `each_pair`, `each_with_index`,
`map` / `collect` (returns Array of block results), `select`
/ `filter`, `reject`, `find` / `detect`, `any?` / `all?` /
`none?` / `one?`, `sort` / `sort_by`, `min_by` / `max_by`,
`group_by`, `transform_keys` / `transform_values` (both
non-mutating; collisions in `transform_keys` follow CRuby's
later-wins iteration order), `filter_map` (collects truthy
block returns into a flat Array — not a Hash — matching
CRuby).

Pair-yield contract (CRuby parity): `each` / `map` /
`collect` / `find` / `any?` / `all?` / `none?` / `one?` /
`sort_by` / `min_by` / `max_by` / `group_by` / `filter_map`
all yield each entry as a single `[k, v]` Array. Two-param
blocks (`|k, v|`) auto-destructure via the F4 block
prologue; single-param blocks (`|pair|`) receive the pair
Array directly. `Hash#select` / `Hash#reject` / `Hash#filter`
override Enumerable here — they yield `(k, v)` as TWO
separate args (so a single-arg block binds to just the key,
matching CRuby's documented divergence for the filter
shapes).

### `Hash#rehash` — two documented divergences

`Hash#rehash` (re-index after mutating keys in place) follows
CRuby's collapse rule — when rehashing reveals duplicate keys,
the FIRST key object keeps its position and the LAST value
wins — and raises FrozenError on frozen receivers. Two
divergences, both verified against CRuby 3.4.1:

- **No iteration guard**: `h.each { h.rehash }` silently
  rehashes; CRuby raises `RuntimeError` ("rehash during
  iteration"). Same family as the pre-existing missing
  insert-during-each guard.
- **`rehash` never calls user `hash`**: duplicate detection
  runs on `eql?` alone, so a key class whose `hash` raises
  completes silently where CRuby propagates the error, and
  the `eql?` receiver/argument direction is reversed vs
  CRuby. The observable collapse OUTCOME (position, value,
  survivor identity) matches CRuby on consistent keys.

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

### String encoding

ADR 0020, phases E1–E3 shipped. Every `RStr` carries an
`EncodingTag`; the default build's tag set is UTF-8 / US-ASCII /
ASCII-8BIT a.k.a. BINARY, and the `_encoding_full` feature extends
per-string tags (and `force_encoding`) to the full registry.
Every build ships the registry *reflection* surface: 26
`Encoding` constants naming ~20 encodings (GB18030 / Big5 / GBK /
EUC-JP / Shift_JIS / ISO-2022-JP / UTF-16 / UTF-32 families,
ISO-8859-1/-15, KOI8-R, Windows-1252/31J), `Encoding.list` /
`find` / `aliases` / `name` / `names`, and CRuby-exact `dummy?` /
`ascii_compatible?` / `inspect` answers. Within the default tag
set, behaviour is pinned to CRuby 3.4 by the
`string_encoding_e1` / `string_encoding_compat` diff fixtures:

- `String#encoding` returns the real `Encoding` singleton
  (`#<Encoding:BINARY (ASCII-8BIT)>` dual-name inspect included);
  `force_encoding` flips the tag in place (names case-insensitive
  with CRuby's fold set, `Encoding` objects accepted,
  `ArgumentError` on unknown names, `FrozenError` on frozen
  receivers); `valid_encoding?` judges the bytes against the tag.
- `==`/`eql?`/`hash` are tag-compatible: pure-ASCII content is
  encoding-blind, non-ASCII content with different tags compares
  unequal and hashes apart (Hash keys follow).
- `+` / `<<` / `concat` / interpolation apply CRuby's
  compatibility rule (ASCII-only side defers; `<<`/`concat`
  upgrade the receiver's tag) and raise
  `Encoding::CompatibilityError` for non-ASCII mixes; `<=>`
  breaks byte-equal ties by encoding index.
- `encode`: the default build covers the no-conversion subset
  within its tag set (same encoding, or ASCII-only bytes across
  UTF-8 / US-ASCII / BINARY) and raises
  `Encoding::UndefinedConversionError` (CRuby's class and message
  shape) for ASCII-incompatible content, or
  `Encoding::ConverterNotFoundError` for registry-encoding
  targets. **Real transcoding shipped behind `_encoding_full`**
  (`encoding_rs`, per the amended ADR 0020): probe-verified
  byte-exact against CRuby for UTF-8 ↔ UTF-16LE / ISO-8859-1 /
  Shift_JIS / EUC-JP round trips, with the same error classes for
  unconvertible input and unknown targets. In the default build
  `force_encoding` accepts only the three-tag set (registry names
  raise `ArgumentError: unknown encoding name`); under
  `_encoding_full` it accepts registry names and `Encoding`
  objects.
- Producers tag correctly: `String#b`, `Array#pack`,
  `Integer#chr` (US-ASCII < 0x80, BINARY above), `File.binread`
  (+ `binwrite`), cext `rb_str_new`, SQLite BLOBs;
  `dup`/`clone`/`+@`/`-@`/`*`/`strip` family/`chomp` propagate
  the receiver's tag; BINARY strings `inspect` with CRuby's
  `\xNN` byte escapes.

Remaining boundaries after E2/E3 (documented): case ops on
registry-tagged strings are real (Unicode mapping, tag-carrying —
see `encoding_full_v3`), but OTHER derived strings
(`slice`/`reverse`) still produce UTF-8-tagged results, and
char-level operations on BINARY strings use the UTF-8-lossy view
rather than CRuby's byte-indexed semantics.

#### Regexp over non-UTF-8 strings — settled boundary (E3)

CRuby compiles a regexp PER ENCODING and matches in the string's
own coding; rubyrs's engines are UTF-8-only and match through the
lossy view. The practical contract:

- Read-only matching with ASCII patterns (`=~`, `match`, `scan`
  positions/captures over the ASCII portion) agrees with CRuby —
  byte offsets line up because the lossy view only rewrites
  high bytes.
- DERIVED strings from regex/substring ops (`gsub`/`sub`/`split`/
  capture strings) on a registry- or BINARY-tagged receiver are
  rebuilt through the lossy view: non-ASCII bytes come back as
  U+FFFD and the tag resets to UTF-8. This is the same E1-era
  boundary, now PERMANENT for patterns-in-foreign-encodings
  (a per-encoding regex engine is out of scope — ADR 0020).
- Workaround that preserves fidelity: transcode first, operate,
  transcode back — `s.encode("UTF-8").gsub(...).encode(
  "ISO-8859-1")` round-trips losslessly for every registry
  encoding.
- High-byte patterns targeting a foreign coding (`/\xE9/n`-style)
  are out of subset on both sides of the divergence (CRuby's `n`
  flag has its own restrictions); spell the pattern in UTF-8 and
  use the workaround above.

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
  Two dispatch paths: the compile-time desugar (both args are
  Symbol literals) emits `Op::AliasMethod` directly; the
  runtime path (one or both args is a method parameter / local
  / String) lands via `Klass.alias_method(new, old)` or bareword
  `alias_method(new, old)` inside a class singleton method
  body. The runtime form returns the new name as a Symbol
  (CRuby's Ruby 3.x contract) and accepts Symbol OR String args.
  Motivating case: rack-protection's
  `def self.default_reaction(reaction); alias_method(:default_reaction,
  reaction); end` — `reaction` is a parameter, not a literal.
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
- `method_missing` is invoked when the receiver is a user-class
  instance (`Value::Object`) OR a Class / Module — the latter
  routes through `lookup_class_singleton_method` so a
  `method_missing` defined in a module extended into the receiver
  (the canonical sinatra-contrib/Extension recorder pattern)
  fires. Adding per-primitive class chains (Int / Str / etc.) is
  the remaining follow-up.
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
- Per-iteration dispatch is ~1.3-3× CRuby's depending on shape
  (fizzbuzz 1.31× as of 2026-06-11; metaprog shapes nearer 3×). See
  [`examples/metaprog_bench/README.md`](../crates/rubyrs/examples/metaprog_bench/README.md)
  — peak memory is still ~2× lighter than CRuby on the same
  workload (the earlier ~5× predates rubyrs's Jekyll-era preamble
  growth and a leaner current CRuby build; see docs/BENCHMARKS.md
  "Memory").

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
- **Default stack-depth ceiling of 10,000 frames** (matches CRuby
  parity) before raising `SystemStackError`. The check runs in
  `Vm::check_frames` at every method/block invocation entry,
  always on (no opt-in). Embedders sandboxing untrusted scripts
  set `max_frames` to a smaller value, which trips with
  `ResourceExhausted` (outside the StandardError subtree, can't
  be swallowed by bare `rescue`). `SystemStackError` itself lives
  under `Exception` (NOT StandardError), so bare `rescue` clauses
  can't silently swallow runaway recursion either — same
  placement and rationale as CRuby's `SystemExit` / `Interrupt`.
  Before this default ceiling, infinite recursion allocated
  frames unboundedly and OOM-killed the host process (observed
  at >90 GB resident in one terminal session); the cap turns it
  into a normal, rescue-able Ruby exception.

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

### `retry` from inside a `begin/ensure` inside a rescue body skips the ensure

```ruby
$ensure_count = 0
counter = 0
begin
  counter += 1
  raise "boom" if counter < 2
rescue
  begin
    retry if counter < 2      # jumps backwards BEFORE the ensure body runs
  ensure
    $ensure_count += 1
  end
end
puts $ensure_count            # CRuby: 1   rubyrs: 0
```

- CRuby's `retry` is a non-local control transfer (RUBY_TAG_RETRY)
  that walks back through any active `ensure` scopes on its way
  to the begin-block start, running each `ensure` body in order
  before re-executing the begin body.
- rubyrs's `retry` compiles to `Op::TruncateRescuesToBeginBaseline`
  + `Op::Jump(begin_top)` — a direct backward jump that bypasses
  any `PushEnsure` handlers active in the rescue body. The
  truncation cleans up the `frame.rescues` Vec so the bypassed
  ensure handler doesn't leak to catch later exceptions, but the
  ensure body itself never executes for that aborted iteration.
- Why: a proper TAG_RETRY-style transfer needs to share the
  existing `pending_loop_transfer` walker (`break`/`next` already
  go through it for the same reason) AND a new
  `EnsureTransferKind::Retry` variant. The fixture surface that
  motivated `retry` support (rackup-2.2.1/lib/rackup/server.rb:439's
  EADDRINUSE loop) puts `retry` at the top of a flat rescue body
  with no nested ensure, so this gap doesn't gate sinatra-4
  loading. Deferred to a follow-up if a real consumer needs it.
- Test: not pinned in `diff_cruby` — pinning the CRuby behavior
  would lock the harness to expect rubyrs's divergent output,
  blocking a future fix from landing cleanly. The bypass
  shape is reproducible via the snippet above.

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

### Hash / Array subclasses are supported; String subclasses are not (yet)

```ruby
class Conf < Hash; end
Conf.new[:k] = 1            # works — tagged Value::Hash
class StringRegister < Array; end
StringRegister.new << "x"   # works — tagged Value::Array
class MyStr < String; end
MyStr.new("hi").upcase      # NoMethodError — still a bare Instance
```

- A user subclass of Hash or Array allocates a REAL tagged
  primitive (`HashObj.class_tag` / `ArrayObj.class_tag` carry the
  subclass), so every primitive method dispatches on instances,
  user overrides win over the primitives (`def push(x); super(...)
  ; end` works), instance variables / `dup` / `clone` / `class` /
  `is_a?` all see the subclass, and `Subclass[...]` /
  `Subclass.new(n, fill)` construct tagged instances. CRuby
  semantics pinned by the `hash_subclass` / `array_subclass`
  diff fixtures (derived results like `map` are plain Array,
  `==` compares content across the boundary).
- The Array side was driven by rouge's python lexer
  (`StringRegister < Array`) — its official sample now renders
  byte-identical to CRuby+rouge.
- `class MyStr < String` (and Integer/Float/Symbol, which CRuby
  forbids subclass instantiation of anyway) still falls through
  to a bare `Value::Object` Instance without the String
  primitives — the remaining documented gap of this family.
- PER-INSTANCE String singletons (`def s.foo` /
  `s.singleton_class` / stub-style define/alias/undef through the
  eigenclass) ARE modelled, via the `Vm::str_singletons`
  side-table keyed on the `Rc<RStr>` pointer identity — `RStr`
  itself stays pointer-free (strings are the hottest heap shape).
  The table holds a strong Rc per singleton-bearing string, so
  those strings live for the VM's lifetime (test-harness-rare;
  bounded by their count — documented trade-off). Pinned by the
  `string_singleton_methods` fixture; minitest's
  `test_stub_yield_self` was the motivating consumer.

### `Hash#transform_keys!` / `transform_values!` on `break` leave the receiver untouched

```ruby
h = {a: 1, b: 2, c: 3}
h.transform_keys! { |k| break if k == :b; k.to_s }
h   # rubyrs: {a:1, b:2, c:3}; CRuby: {b:2, c:3, "a"=>1}
```

- The bang transforms build the new pairs in a scratch Vec and
  commit them to the receiver only on normal completion, so a
  `break` mid-iteration discards the whole transform. CRuby
  mutates incrementally — entries processed before the `break`
  are already committed.
- Why: incremental in-place key rewriting while iterating the
  same hash is hazardous (the scratch-Vec approach is the safe
  one); `break` inside a `transform_*!` block is rare. No test
  pin; this entry is the contract.

### ~~`freeze` doesn't actually freeze — `frozen?` always false~~ (FIXED)

```ruby
EMPTY = [].freeze
EMPTY << :x         # both: FrozenError (was: rubyrs silently mutated)
puts EMPTY.frozen?  # both: true       (was: rubyrs false)
```

- FIXED — freeze is now real. Probe-verified against CRuby 3.4.1
  (2026-07): `freeze` / `frozen?` on String / Array / Hash /
  Object; mutation after freeze (`<<`, `[]=`, `push`, `delete`,
  `sort!`, `instance_variable_set`) raises `FrozenError` with
  CRuby's message shape (`can't modify frozen Array: []`);
  Symbol / Integer / `nil` report always-frozen; bare String
  literals report unfrozen; `dup` returns an unfrozen copy; the
  `# frozen_string_literal: true` magic comment freezes literals
  (and mutation of one raises).
- Tests: `tilt_load_capabilities.rb` pins the chainable return
  shape; frozen-receiver checks are exercised by fixtures like
  `Hash#rehash`'s FrozenError arm and the `force_encoding`
  frozen-receiver contract in the encoding fixtures.

### `ResourceExhausted` is host-only, not script-visible

```ruby
begin
  while true; end             # exhausts fuel
rescue Exception => e         # explicit Exception filter
  puts "won't reach"
end
```

- The resource trap (fuel / heap-cap / frame-cap **when set via
  the embedder-configurable `max_frames`**) propagates as a
  host-level `Trap` directly out of `Runtime::eval`. It does not
  go through `unwind_with_exception`, so no `rescue` clause —
  bare or class-filtered, even `rescue Exception` — can
  intercept it.
- See [ADR 0008](adr/0008-resource-caps-for-untrusted-scripts.md).
  The earlier promise in that ADR that `rescue Exception` could
  catch the trap was aspirational; it's been retracted.
- The **default 10,000-frame ceiling**
  (`SystemStackError`, always on) is the parity sibling and IS
  rescue-able — see the Runtime bullet above for placement
  and rationale. The split: `ResourceExhausted` is the
  uncatchable embedder cap, `SystemStackError` is the
  CRuby-parity ceiling that scripts handle normally.
- Tests: `resource_exhausted_cannot_be_swallowed_by_bare_rescue`
  and `resource_exhausted_is_uncatchable_even_with_rescue_exception`;
  `diff/system_stack_error.rb` covers the SystemStackError parity.

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

### `private_constant` is enforced; `defined?` on a private constant diverges

```ruby
class Foo
  BAR = 1
  private_constant :BAR
end
Foo::BAR              # both: NameError "private constant Foo::BAR referenced"
defined?(Foo::BAR)    # CRuby: nil; rubyrs: "constant"
```

- Enforcement shipped (probe-verified against CRuby 3.4.1,
  2026-07): external access to a private constant raises
  CRuby's `NameError` ("private constant Foo::BAR referenced");
  internal references keep working; `const_get` still returns
  the value (CRuby parity — `const_get` bypasses privacy);
  `public_constant` re-exposes the name.
- Remaining divergence: `defined?(Foo::PRIVATE)` returns
  `"constant"` where CRuby returns `nil` — the `defined?` walk
  doesn't consult the visibility flag.
- Test: `crates/rubyrs/tests/diff/private_constant.rb` (call
  shapes and receiver return value).

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
- ~~Reflection on the shell operates on empty tables~~ (FIXED) —
  probe-verified (2026-07): methods installed through the shell
  (both parsed `def` and `define_method`) are now visible to the
  shell's own `instance_methods(false)` and `method_defined?`,
  and `Klass.singleton_class.ancestors` includes modules
  extended via `Klass.extend(M)` — reflection matches CRuby on
  these probes.
- **Remaining divergence — `include` INTO the shell doesn't route
  dispatch.** `Klass.singleton_class.include(M)` records the
  ancestry (`ancestors` / `include?` report `M`, matching CRuby)
  but `Klass.some_method_from_M` still raises NoMethodError where
  CRuby dispatches. The reverse of the old gap: reflection now
  leads, dispatch lags — but only for this include-into-shell
  direction (`Klass.extend(M)` dispatch is correct).
- `Object#singleton_class` works for plain-object receivers too
  (probe-verified: `class << obj` bodies, `def obj.name` installs,
  and `obj.singleton_class.instance_methods(false)` reflection all
  match CRuby).
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

### Implicit (binding-less) `eval` skips caller locals; explicit `binding` capture works

```ruby
x = 99
puts eval("x")           # CRuby: 99; rubyrs: NoMethodError (no implicit caller scope)
puts eval("x", binding)  # both: 99  (explicit binding captures locals)
```

- `Kernel#eval(string)` parses, compiles, and runs the source at
  top level. Returns the final expression's value (matches CRuby).
- `Kernel#eval` accepts up to 4 args matching CRuby's signature
  (`eval(src, binding, file, line)`). An explicit `binding`
  argument **is honoured** (probe-verified 2026-07): `Kernel#binding`
  is a native builtin that captures the live frame's self, lexical
  class, ivars AND locals, so `eval("local", some_binding)`
  resolves method-scope locals, `@ivars`, and `self` exactly as
  CRuby does (rack's `Builder.new_from_string` shape). What's
  missing is the *implicit* capture: `eval("x")` WITHOUT a binding
  runs at top level and can't see the caller's locals (CRuby's
  binding-less eval can). `file` is wired through to source
  registration so backtraces and `Method#source_location` for
  methods defined inside the eval'd source resolve.
- The `Binding` reflection API is absent: `Binding#local_variable_get`
  / `local_variables` / `receiver` / `Binding#eval` all raise
  NoMethodError — a `Binding` is only useful as an `eval` argument
  today.
- `Class#class_eval(string [, file, line])` (and `module_eval`
  alias) now DOES switch to the receiver class's class-body
  context — `Foo.class_eval("def bar; :x; end")` lands `bar` on
  `Foo` (probe-verified 2026-07; the earlier "lands at top level"
  divergence is closed). The block form continues to work as in
  CRuby.
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
- rubyrs `do_call` has a Kernel-fallback that routes these
  six names to `builtin_call` when normal lookup AND
  `method_missing` both miss. The fallback ignores
  private-visibility — `obj.Array(...)` silently succeeds where
  CRuby raises.
- `respond_to?` still returns `false` for these names — CRuby
  reaches the methods via the Kernel include (every Object is
  Kernel-mixed), but `respond_to?` defaults to
  `include_private: false`, which hides private methods. rubyrs
  matches by reporting `false`. The divergence is the call
  shape, not feature detection.
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
class Foo; end
p = proc { |x| x * 2 }
Foo.define_method(:double, p)
# CRuby: installs :double; rubyrs: ArgumentError
#   ("the 2-arg Proc/UnboundMethod form of `Module#define_method`
#    is not yet supported by rubyrs Tier-1")
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
  pins the CRuby-aligned arity errors (0-arg, 3+-arg wrong-arity;
  1-arg "tried to create Proc object without a block"). The
  2-arg form itself can't be pinned via a diff fixture — CRuby
  installs the method and rubyrs raises ArgumentError, so the
  outputs would diverge byte-for-byte by design. The Tier-1
  divergence is verified via manual probe; the ArgumentError
  message string is constructed in
  `crates/rubyrs/src/vm/dispatch.rs` — `grep` for
  `"the 2-arg Proc/UnboundMethod form of \`Module#define_method\`"`
  to find the two emission sites (no-block arm in
  `try_dispatch_class_intrinsics`; block-form arm in
  `do_call_block`'s `define_method` intrinsic).

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

### Bare-call dispatch from inside reopened-NilClass instance methods

```ruby
class NilClass
  def helper; "from helper"; end
  def caller
    helper            # ← bare call
  end
end
nil.caller            # rubyrs: NoMethodError; CRuby: "from helper"
nil.send(:helper)     # both: "from helper"
class NilClass
  def caller_ok
    self.helper       # ← explicit self
  end
end
nil.caller_ok         # both: "from helper"
```

- Bare method calls inside reopened-class instance method bodies
  reach a primitive-class lookup arm in `vm/dispatch.rs` that
  finds the sibling method on the receiver's class. The arm is
  gated on `!matches!(self_val, Value::Nil)` because rubyrs uses
  `Value::Nil` as the toplevel `main` self too: bridging from
  a Nil receiver to NilClass-method lookup would turn the
  CRuby-correct `ArgumentError` from a toplevel arity mismatch
  (`def one(a); end; one(1, 2)`) into `NoMethodError for NilClass`,
  breaking the toplevel-method dispatch contract.
- The two cases (toplevel-main vs real-nil receiver) are
  indistinguishable at the dispatch site under the current
  `Value::Nil`-shared representation, so the Nil exclusion
  is preserved and the reopened-NilClass-bare-call case takes
  the documented divergence.
- Workaround: write `self.helper` (or `send(:helper)`) instead
  of bare `helper`. The primitive-reopen bridge covers Hash /
  Array / String / Integer / Symbol / Float etc.; only NilClass
  needs the explicit receiver.
- Why not fixed at the structural level: closing this would
  need either (a) lifting toplevel-main to its own `Value`
  variant — a workspace-wide change touching every dispatch arm
  and `class_of` site — or (b) frame-context-aware dispatch
  checking `is_toplevel_frame` on every Nil-receiver call. Both
  trade real complexity / runtime cost for a rarely-used
  pattern (`class NilClass` reopens with bare-call siblings).
  The 1-line `self.` workaround is the right cost-benefit point.
- Surfaces visible in-tree: `src/stdlib_vendor/active_support_lite.rb`'s
  `NilClass#present?` / `presence` overrides (rather than
  inheriting from `Object#present?` which calls bare `blank?`).
  Test: `crates/rubyrs/tests/diff/reopen_primitive_bare_call.rb`
  pins the toplevel-arity ArgumentError surface this exclusion
  preserves.

### `super(*args, **kwargs)` doesn't split the kwargs into the super-target's kwarg channel

```ruby
class P
  def m(x, **opts)
    "p: x=#{x} opts=#{opts}"
  end
end
class C < P
  def m(x, **opts)
    super
  end
end
puts C.new.m(1, a: 2)  # rubyrs: "p: x=1 opts={}"; CRuby: "p: x=1 opts={a: 2}"
```

- `Expr::SuperApply` lacks the `kwargs_trailing: bool` flag
  that `Expr::Call` carries; the AST translator's super-arg
  walk routes a trailing `KeywordHashNode` through
  `tr_kwhash` AS A POSITIONAL `HashLit`, so the parent
  method's `**opts` binder sees it as a positional Hash and
  the kwargs slot stays empty.
- Bare `super` (no parens) forwards positional locals only —
  same defect for kwargs declared as `**kw` on the calling
  method.
- The Sinatra spike doesn't trip this — parent methods in the
  vendored stdlib chain accept kwargs positionally or omit
  them. Hit by any gem whose super-target has an explicit
  `**kw` signature with caller-side kwargs.
- Proper fix: add `kwargs_trailing` flag on `Expr::SuperApply`,
  introduce `Op::ApplySuperKw` + `Op::ApplySuperKwBlock`
  variants, route the trailing-kwhash split into the kwargs
  channel at dispatch time. Significant opcode-level work;
  deferred until a real caller surfaces.

### Block keyword params: kwargs-vs-positional-Hash recovery is a heuristic

```ruby
proc { |a, k: 5| [a, k] }.call({k: 9})
# rubyrs: [nil, 9]   (the Hash is peeled as kwargs)
#  CRuby: [{k: 9}, 5] (a positional Hash literal stays positional)
```

- `|k1:, k2: default|` block/lambda keyword params bind, raise
  CRuby-worded `missing keyword:`/`unknown keyword:` ArgumentErrors
  (missing reported first), and split leftovers into `**rest` —
  see `tests/diff/block_kw_params.rb`. But our block call sites
  flatten kwargs into a trailing positional Hash before
  `invoke_block`, so the caller's kwargs-vs-brace-hash bit is
  gone. Recovery heuristic: the trailing Hash is treated as
  kwargs only when at least one key names a declared keyword
  param (or the block has `**rest`, which peels any trailing
  Hash — pre-existing behaviour). A passed brace-Hash whose keys
  happen to overlap a declared keyword is therefore consumed as
  kwargs; zero-overlap Hashes stay positional (so iteration
  drivers yielding Hash elements bind them positionally, like
  CRuby).
- `|k: default|` with an EXPLICIT `k: nil` argument re-evaluates
  the default (CRuby keeps the nil): the default is a
  translation-time desugar to `k = default if k.nil?` at the
  head of the block body (ast.rs `prepend_kw_default_prologue`),
  not a caller-supplied bit.
- A kw-param block installed AS A METHOD via `define_method`
  keeps the pre-existing method-binder arity behaviour (kw names
  stay out of `proto.params`); only ordinary block invocation
  (`call`/`yield`/iterators) binds `Proto::block_kw_params`.

### `DelegateClass(SuperKlass)` collapses to one class for all callers

```ruby
require 'delegate'
A = DelegateClass(Hash)
B = DelegateClass(Array)
puts A.equal?(B)      # rubyrs: true; CRuby: false
puts A.superclass     # rubyrs: Delegator; CRuby: <anon>
```

- `DelegateClass(X)` in our shim returns `Delegator` itself
  (vs CRuby's `Class.new(Delegator)` with method delegation
  enumerated from `X.public_instance_methods`). Each call
  produces the same class identity, so any gem that
  introspects via `subclass.superclass.equal?(
  DelegateClass(X))` or compares two DelegateClass results
  for inequality diverges.
- The collapse was a workaround for a separate rubyrs gap:
  dynamic-superclass dispatch (`class C < SomeExpr()`) doesn't
  walk the result class's `method_missing` chain for subclass
  instances. Switching the shim to `Class.new(Delegator)`
  would break the Sinatra spike's load surface.
- Proper fix: close the dynamic-superclass dispatch gap (give
  `Class.new(Delegator)` a `Value::Class` whose method-
  lookup walks Delegator's `method_missing` correctly), then
  drop the shim's collapse. Substantial — touches the
  method-lookup walker.

### `Struct.new`-created classes have a GC root hole under STRESS_GC

```
rubyrs (STRESS_GC=1): ICE: heap slot is not an Instance
                      / not a Block at <preamble:struct>:N
```

- The `Struct` preamble uses `define_method`-with-class-ivars-
  closure shapes (the captured `attrs` Array is stored on the
  Class via an ivar and read at every invocation). Under
  STRESS_GC the per-Instance heap slot can be swept and
  rebound mid-dispatch — `class_of` then ICEs.
- Normal-mode runs are unaffected; the Sinatra spike load
  surface doesn't intersect a sweep window.
- The `diff/struct_factory.rb` fixture sentinel-skips under
  `STRESS_GC=1` via an empty `else` branch (NOT `exit 0`,
  which would diverge from CRuby's silent exit via rubyrs's
  "exit (SystemExit)" tail-line).
- Proper fix lives at the GC root-set side — pin captured
  block-locals across heap allocs during define_method-
  emitted method invocations.

### ~~Detached inner closures don't write-through to outer-method locals~~ (FIXED)

```ruby
total = 0
adders = []
[1, 2, 3].each { |x| adders << -> { total += x } }
adders.each(&:call)
puts total      # rubyrs: 6; CRuby: 6 (was: rubyrs 0)
```

- FIXED by the outer-chain capture-routing model: every
  `BlockHandle` records the canonical owner of each captured
  slot region (`captured` + `creator_start` for the creating
  scope, `outer_chain` for ancestor scopes), and slot accesses
  below a frame's `own_start` route STRAIGHT to the original
  binding cell (`Frame::outer_cell_for`) instead of the
  frame's per-invocation snapshot. A captured local is now one
  shared binding across the defining scope and every closure,
  for the lifetime of any capturing closure — including after
  intermediate frames pop (stored procs, deferred Thread
  bodies, suspended Fibers, `define_method` bodies,
  `instance_eval` blocks). Per-iteration capture isolation
  (`[:a,:b,:c].map { |s| -> { s } }`) is preserved: a block's
  OWN params/body-locals stay per-invocation; only the
  captured outer region is shared.
- Regression fixtures: `tests/diff/closure_capture_nested.rb`
  and `tests/diff/closure_define_method_binding.rb` (plus the
  earlier `tests/diff/closure_in_iter_capture.rb`).

### define_method bodies don't bind named keyword params

```ruby
class C
  define_method(:m) { |a, k: 1| [a, k] }
end
C.new.m(1, k: 2)   # rubyrs: [1, {k: 2}] via rest / k default; CRuby: [1, 2]
```

- The `define_method`-installed closure binder handles
  positional, optional (via the nil-keyed default prologue),
  `*rest`, `**kwrest` and `&blk` params, but does NOT peel a
  trailing kwargs Hash into NAMED keyword slots the way
  `invoke_block` does — the Hash stays positional (flowing
  into `*rest` when present) and the keyword takes its
  default. Separate binder gap, unrelated to the capture
  representation; fix belongs in the `m.closure` arm of the
  method dispatch (mirror `invoke_block`'s kw peel + bind).

## Deferred to outer tiers

Features whose absence is a tier-assignment decision per
[ADR 0015](adr/0015-concentric-architecture.md), not a "we'll never do
this". The table below records *where* each item is expected to land
and *what's already in place* to make that future work tractable.

| Feature | Target tier | Current Tier 1 state |
|---------|-------------|----------------------|
| Arbitrary-precision Integer arithmetic (`2**100`, true Bignum) | Tier 1 (`bignum` Cargo feature, default ON) | Phase A shipped: `Value::BigInt` + `HeapObj::BigInt`, integer-literal overflow promotes to BigInt at AST time, `+ - * / %` + comparisons + `to_s` / `inspect` / `class` work via `try_bigint_binop`, Float×BigInt coerces with Float-wins-on-mix. Build without `--no-default-features` to drop the `num-bigint` dep and fall back to wrapping i64 arithmetic. Phase B (`**`, bit ops, unary, `abs`) still on the bench. Earlier "i64 saturates at parser" wording belongs to the pre-Phase-A era and only applies under `--no-default-features`. |
| `Rational`, `Complex`, `BigDecimal` | Tier 2 / Tier 3 | `Rational` and `Complex` shipped in the always-on preamble (probe-verified 2026-07): `Rational()` / `Complex()` constructors, arithmetic with reduction (`Rational(4, 8)` → `(1/2)`), comparisons, `to_f`, `Complex#real` / `imaginary` / `abs`, `Integer#to_c`, `1 / Rational(3)` coercion. Still absent: `String#to_c` / `String#to_r`, imaginary literals (`3i`). `BigDecimal` is vendored behind `--features stdlib`. |
| Real nested-module namespacing (`Foo::Bar` after `module Foo; class Bar; end; end`) | Tier 1 (shipped) | Class table now keyed by qualified SymId, so top-level `Bar` and `Foo::Bar` are independent `Class` objects with separate method / ivar / superclass tables (was a `class_qualified_separates` divergence, closed). Bare-name reads inside a class/module body walk a precomputed cref chain (`Op::LoadConstChain`) before falling back to the top-level bare slot — matches CRuby's "innermost-scope wins" behaviour. `Module.nesting` reflection API is still deferred (the cref chain exists at compile time but isn't exposed yet); two top-level modules that DON'T collide via the qualified-key story still don't get a real `Module` shape distinct from `Class` — see the `Module` semantics row below. |
| `Time` class (`Time.now`, `#to_i`, `#nsec`, `Time.at(sec, nsec, …)`) | Tier 2 | None as a primitive value type. User classes carrying `(sec, nsec)` plus `register_type_internal` already round-trip Time-shaped ext-type frames byte-identical to MRI (see `tests/cext_msgpack_app_ext.rs`). |
| `Fiber`, `Thread`, `Mutex`, `Ractor` | Tier 2 (`_fiber` / `_thread` / `_ractor` feature gates in ADR 0015) | The VM itself stays single-threaded by design — there is no OS-thread parallelism and no preemption. On top of that: `Fiber` subset behind `_fiber`; a **cooperative green-thread `Thread` subset** (~1,100-line preamble) is always on. Works (probe-verified 2026-07): `Thread.new` / `join` / `join(timeout)` / `value` / `alive?` / `status` / `kill`, exception propagation through `value`, thread-locals (`Thread#[]`) and `thread_variable_get/set` (correctly isolated per thread), `Thread.pass` interleaving, `sleep` in threads, `Mutex` (`lock` / `unlock` / `synchronize` / `owned?`), `Queue` incl. cross-thread blocking `pop`, `ConditionVariable`, `Thread.list`. Gaps (probed): `Thread.main`, `Thread#priority`, `abort_on_exception`, `SizedQueue` absent; `Thread.current` at top level returns the `Thread` class itself (not a main-Thread instance; inside spawned threads it is a real `Thread`); `Thread.stop` / `wakeup` raise; `Thread#raise` propagates into the calling thread; **`Thread.pass` inside a NATIVE iterator block (`3.times { … }`) truncates the loop** — use `while` loops in thread bodies that yield. `Ractor` absent. |
| Full `Module` semantics (real Module type distinct from Class, `include` chain with method-lookup ordering matching CRuby exactly) | Tier 2 | PoC: `include Mod` works via method-table copy; ancestry walks via `class_is_a` + `includes` list. Strict CRuby `ancestors` compatibility deferred. |
| `eval` (string form), `binding`, `ObjectSpace` | Tier 4 (`mri-compat`) | `Kernel#eval(string)` shipped, and `eval(src, binding)` captures the binding's locals / self / ivars (see the [eval divergence section](#implicit-binding-less-eval-skips-caller-locals-explicit-binding-capture-works)); the `Binding` reflection API is still absent. `ObjectSpace` exists as a module with `define_finalizer` / `undefine_finalizer` only — `each_object` / `count_objects` / `_id2ref` / `garbage_collect` are absent (probe-verified 2026-07). |
| `require / load / autoload` from LOAD_PATH | Tier 1 (partial) | `require "/abs/path.rb"`, auto-`.rb`, cwd-relative, caller-source-dir + caller-source-parent hops, AND `$LOAD_PATH` walking all work (covered by `tests/diff/require_xpkg.rb`). CRuby's auto-populated stdlib/gem `$LOAD_PATH` entries are NOT pre-seeded — scripts opt in via `$LOAD_PATH.unshift(dir)`. `load` and `autoload` shipped too (probe-verified 2026-07 for the basic forms; the Bridgetown 4-phase probe exercises the zeitwerk autoloader on top of them). |
| Pure-Ruby stdlib subset (`Pathname`, … future names) | Tier 3 (`stdlib` Cargo feature) | Default Tier 1 build keeps the lenient stub "feature-absent surface": `require 'pathname'` materialises the constant shell, calls raise NoMethodError. With `--features stdlib` the same require path loads `crates/rubyrs/src/stdlib_vendor/<name>.rb` (deterministic, fs-free subset) and the module behaves CRuby-compatibly. Pilot: `Pathname` path-string manipulation methods, covered by `tests/diff/stdlib_pathname.rb` (the test is `#[cfg(feature = "stdlib")]`-gated). |
| C extension API (CRuby ABI compatibility) | Tier 4 (`mri-compat`) per ADR 0015 | A working partial implementation lives in `crates/rubyrs-cext` as a spike, not as a covenant — see ADR 0015's "C-ext ABI stays out of v1 and v2" rule. Specifically the L3-J/K + A3/A4 work shipped msgpack-shaped FFI that's "real enough to round-trip the wire protocol" but doesn't promise full CRuby C-API equivalence. |
| Refinements, full pattern matching, full encoding model, `Marshal`, `IO` beyond stdout | Tier 3 / Tier 4 | `Marshal` shipped for the common-tag subset (probe-verified 2026-07, byte-format `\x04\x08` matching CRuby): nil / bool / Integer / Bignum / Float / String (incl. non-ASCII) / Symbol / Array / Hash (nested) / Range / Struct subclasses / user objects with ivars, link-aware, honouring `marshal_dump` / `marshal_load` hooks; `load(dump(x))` is a genuine deep copy; `Marshal.dump(proc)` raises CRuby's `TypeError` ("no _dump_data is defined for class Proc"). Divergences: `Marshal.dump(STDOUT)` serialises a plain-object stand-in where CRuby raises "can't dump IO"; graphs outside the byte subset fall back to a same-process registry token (shallow, process-local, capped at 1024); dump-to-IO writes a rubyrs-only `RMF1` length frame, so IO-port streams are self-consistent but not CRuby-wire-compatible. Encoding model: partial, see [String encoding](#string-encoding). |
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
