# Subset semantics

rubyrs is **not** trying to be CRuby-compatible. It targets the same niche as
**mruby**: a small, memory-safe, embeddable Ruby-flavored runtime — but
written in Rust, with the option of compiling to WebAssembly.

If you need Rails, Sinatra, Bundler, gems, or `eval` — use CRuby.

## Supported today

### Values
- `Integer` (i64), `Float` not yet
- `String` (UTF-8 view) with `+`, `==`, `length`, `to_s`
- `Symbol` (interned via `Rc`) with `to_s`, `to_sym`, `==`, `!=`
- `true`, `false`, `nil`
- `Array` with `length/size`, `push/<<`, `[]`, `[]=`, `first`, `last`, `empty?`,
  `each`, `map`
- `Hash` (insertion-ordered, linear lookup) with `length`, `[]`, `[]=`,
  `empty?`, `keys`, `values`, `each`
- Class instances with instance variables and methods

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
- `begin / rescue => e / end`, nested rescue with rethrow, `raise "msg"`
- Array and hash literals: `[1, 2]`, `{a: 1}`
- Integer arithmetic: `+ - * / %`, comparisons: `== != < <= > >=`

### Built-ins
- `puts`, `print`
- `Integer#times { |i| ... }`

### Runtime
- Mark-sweep GC over `Instance`, `Array`, `Hash` (cycle-safe)
- Class definitions reopenable (`class Foo` twice merges methods)
- Single integer type (`i64`)

## Not supported (today, but candidates for the roadmap)

| Feature | Priority for niche tool? |
|---------|------------------------|
| `Range` (`1..10`) | high |
| More `Enumerable`: `select`, `reject`, `inject`, `find`, `any?`, `all?`, `include?` | high |
| String methods: `split`, `gsub`, `sub`, `chomp`, `strip`, `upcase`, `downcase`, `chars` | high |
| `Module`, `include`, `extend` | high |
| Class inheritance (`class Foo < Bar`), `super` | high |
| `Float`, `Rational`, mixed-numeric arithmetic | medium |
| Exception class hierarchy (`raise SomeError`), `ensure` | medium |
| `attr_reader / attr_writer / attr_accessor` | medium |
| Default args, keyword args, splat, block-arg `&blk` | medium |
| `return`, `break`, `next`, `redo` | medium |
| Inline cache for method dispatch | low (perf-only) |

## Explicitly out of scope

These will not be added unless the project changes direction:

- `eval`, `define_method`, `method_missing`, `instance_eval`, `ObjectSpace`
- `Fiber`, `Thread`, `Mutex`, `Ractor`
- `require / load / autoload`, gems, Bundler
- C extension API
- File / Socket I/O beyond stdout
- Refinements, pattern matching
- Encodings beyond a UTF-8 byte view
- Frozen strings as a language-level constraint

If you need any of these, use CRuby.
