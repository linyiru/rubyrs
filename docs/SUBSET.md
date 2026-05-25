# Subset semantics

rubyrs is **not** trying to be CRuby-compatible. It targets the same niche as
**mruby**: a small, memory-safe, embeddable Ruby-flavored runtime — but
written in Rust, with the option of compiling to WebAssembly.

If you need Rails, Sinatra, Bundler, gems, or `eval` — use CRuby.

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
- `Array` with `length`/`size`, `push`/`<<`, `[]`, `[]=`, `first`,
  `last`, `empty?`, `each`, `map`
- `Hash` (insertion-ordered, linear lookup) with `length`/`size`, `[]`,
  `[]=`, `empty?`, `keys`, `values`, `each`
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

Covered: `length` / `size`, `+`, `==`, `empty?`, `reverse`,
`upcase`, `downcase`, `include?(String)`, `equal?` (identity),
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
`UnboundMethod#==`, `&proc`-style forwarding
(`arr.map(&doubler_method)`).

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

### Multi-class `rescue A, B => e`

```ruby
begin ...
rescue A, B => e
  ...
end
```

- CRuby matches A or B.
- rubyrs honours only the **first** class (`A`). The remaining
  classes are silently ignored at compile time. Document the
  gap as a P1-10 follow-up.

### `Foo::Bar` constant-path in `rescue`

- We extract only the trailing segment (`Bar`) and look it up
  at the top level. If `Foo::Bar` shadows a top-level `Bar`
  with different semantics, the rescue may behave unexpectedly.
- Most real Bundler / Gemfile uses (`rescue Gem::LoadError`) work
  if the trailing name is defined at the top level.

## Not supported (today, but candidates for the roadmap)

| Feature | Priority for niche tool? |
|---------|------------------------|
| `Range` (`1..10`) | high |
| More `Enumerable`: `select`, `reject`, `inject`, `find`, `any?`, `all?`, `include?` | high |
| Additional String methods: `split`, `chomp`, `strip`, `chars` (see "String built-in methods" above for what ships today, including `sub` / `gsub` / `upcase` / `downcase` / `reverse` / `include?` / `empty?`) | high |
| `Module`, `include`, `extend` | high |
| Class inheritance (`class Foo < Bar`), `super` | high |
| `Rational`, `Complex`, big-Integer overflow promotion | low |
| Exception class hierarchy (`raise SomeError`), `ensure` | medium |
| `attr_reader / attr_writer / attr_accessor` | medium |
| Default args, keyword args, splat, block-arg `&blk` | medium |
| `return`, `break`, `next`, `redo` | medium |
| Inline cache for method dispatch | low (perf-only) |

## Explicitly out of scope

These will not be added unless the project changes direction:

- `eval` (string form), `binding`, `ObjectSpace`
- (`define_method` / `method_missing` / `alias_method` /
  `instance_eval` / `class_eval` / `def obj.foo` /
  `define_singleton_method` are now in the supported set as a
  PoC — see above. The remaining items stay out of scope.)
- `Fiber`, `Thread`, `Mutex`, `Ractor`
- `require / load / autoload`, gems, Bundler
- C extension API
- File / Socket I/O beyond stdout
- Refinements, pattern matching
- Encodings beyond a UTF-8 byte view
- Frozen strings as a language-level constraint

If you need any of these, use CRuby.
