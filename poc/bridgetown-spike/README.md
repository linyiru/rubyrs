# Bridgetown spike workspace (2026-06-14 discovery round)

How far does rubyrs get **booting Bridgetown 2.2.1** (`require
"bridgetown-core"`)? Bridgetown is the modern Jekyll-alternative SSG —
same Liquid/kramdown/rouge stack rubyrs already runs for Jekyll, but
wired through **zeitwerk** autoloading, **roda**, **serbea**, and the
**dry-inflector / signalize / samovar** support cast.

Run: `target/release/rubyrs poc/bridgetown-spike/bt-probe.rb`
(full-feature build: `cargo build --release -p rubyrs --features everything`).
Gem sources read from rbenv 3.4.1's gem dir — see the path constants at
the top of `bt-probe.rb`.

## How far it gets

`require "bridgetown-core"` now advances through, in order:

1. `rubygems` / `bundler/shared_helpers` — **shimmed** (`shim/shims.rb`);
   rubyrs has no gem/bundler runtime, and Bridgetown only touches them
   at boot for plugin discovery.
2. stdlib `find` / `fileutils` — **real 3.4 stdlib**, symlinked into
   `stdlib-subset/` (only the files rubyrs doesn't vendor; the whole
   stdlib dir is NOT exposed so on-disk `rubygems`/`bundler` don't
   shadow rubyrs' own stubs). `time` / `English` are dropped to rubyrs'
   native stubs — real `time.rb` needs `class << Time` + global-var
   `alias` (see walls below), real `English.rb` is all
   `alias $GLOBAL $g`.
3. `csv` (real gem) — needs a working pure-Ruby `StringScanner`; rubyrs'
   vendored `strscan` provides it.
4. `bridgetown-foundation` → `hash_with_dot_access`, `inclusive`,
   `dry-inflector`, **`zeitwerk`** — all load.
5. `securerandom` — dropped to rubyrs' native stub (real gem uses
   `begin/rescue` inside `class << self`, see walls).

**Progress (2026-06-15, round 4):** zeitwerk's implicit-namespace
directory autovivification now works end to end — `require "bridgetown-core"`
boots **all the way through** bridgetown-foundation → liquid → kramdown →
rouge → listen → i18n → concurrent-ruby and into **faraday**. The require
chain runs through zeitwerk's decorated `Kernel#require`
(`core_ext/kernel.rb`) cleanly.

VM fixes that carried it here (all landed with diff_cruby coverage):
- **require honours a user `Kernel#require` override** (the upstream
  campaign): autoload-fired AND explicit requires now route through it,
  so zeitwerk's directory autovivification fires.
- **`const_get(name, false)` fires autoloads through the override** —
  zeitwerk's eager_load descends namespace dirs via `const_get`, a path
  that previously bypassed the override and hit `require "<dir>"`.
- **`Kernel.method_defined?` answers honestly** — was blanket-true, which
  broke the `alias_method … unless method_defined?` guard idiom.
- **`class << self` with if/elsif/case-wrapped defs** → routed to the
  real eigenclass body (listen's MonotonicTime).
- **`super(key, ...)`** argument forwarding (faraday's Headers#fetch).
- `Dir.each_child`, the require-scope fix (round-3), etc.

Shim/probe additions this round: `Gem::Deprecate` no-op (addressable);
drop the bigdecimal gem (use rubyrs' vendored one, the gem wants
`bigdecimal.so`); concurrent-ruby's real `lib/concurrent-ruby` require
path.

**Progress (2026-06-15, round 6):** the user's Struct subclass-as-factory
(double-new) commit cleared the `Options` frontier; faraday now loads
through its `Options`/`memoized` DSL, `rack_builder`, and into the
request middleware. Two more VM fixes this round carried it:
- **string-form `class_eval`/`module_eval` captures the caller's local
  binding** — faraday's `Options.memoized` does
  `class_eval("…remove_method(key)…def #{key}…")` reading the `key`
  method local inside the string.
- **`ruby2_keywords` no-op class-body intrinsic** — faraday's
  `RackBuilder::Handler`.

**Progress (2026-06-15, round 7):** the faraday `register_middleware`
wall is **FIXED** — root cause was that `module Faraday; Request =
Struct.new(…) { extend MiddlewareRegistry }` named/keyed the anon struct
class by the BARE name "Request", so the later `class Request` reopen
(authorization.rb), which keys by the qualified "Faraday::Request",
minted a fresh empty class and dropped the struct members + the extend.
Fixed in `Op::StoreConst` (scope-qualify the anon-class name). faraday
now loads end to end (request middleware, response logging via a `pp`
shim). Shim/probe additions: `pp` → `pretty_inspect` no-op; samovar's
CLI dep chain (mapping, console, fiber-annotation, fiber-local).

**Current wall (frontier):** concurrent-ruby's `SafeInitialization` —
`module SafeInitialization; def new(*a, &b); super(*a, &b); ensure …; end;
end`, `extend`ed onto `Concurrent::Delay`. `Delay.new` → the module's
`new` → `super` should reach the builtin `Class#new`, but rubyrs raises
`super: no superclass method 'new'`. A distinct VM gap (super-to-builtin-
`Class#new` from an extended-module method), not the struct/const bug.

## VM fixes this spike drove (landed, with diff_cruby coverage)

Each is its own atomic commit with a `tests/diff/*.rb` oracle:

- **`feat(parser): Ruby 3.1 hash/keyword value-omission shorthand`** —
  `{x:, y:}` / `foo(x:)`. Prism models the omitted value as an
  `ImplicitNode`; rubyrs now unwraps it. Bridgetown uses this heavily.
- **`feat(reflection): methods/singleton_methods(false)`** — the
  optional regular/all boolean (own methods only). stdlib `fileutils.rb`
  builds its OPT_TABLE with `private_instance_methods & methods(false)`.
- **`feat(parser): unwrap ShareableConstantNode`** — the
  `# shareable_constant_value` magic comment. stdlib `time.rb` uses it.
- **`feat(reflection): native Module#name via instance_method`** —
  `Module.instance_method(:name).bind_call(mod)`, the technique
  zeitwerk's `RealModName` uses. Needed by **both** this spike and the
  hanami one.

## Walls catalogued but not yet bridged

- **`class << <expr>` with a full body** (nested module/class,
  `begin/rescue`, generic statements) — blocks zeitwerk's
  `explicit_namespace` and the real stdlib `securerandom` / `time`.
  The single highest-value remaining wall: it gates zeitwerk, which
  both Bridgetown and Hanami depend on.
- **Global-variable `alias`** (`AliasGlobalVariableNode`) — real
  `English.rb` is nothing but `alias $LOAD_PATH $:`-style lines.

The authoritative ranked wall map lives in session memory.
