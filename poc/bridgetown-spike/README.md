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

**Current wall (frontier):** zeitwerk's
`zeitwerk/explicit_namespace.rb` opens `class << self` with a body that
contains a nested `module Synchronized … end` and `internal def …`
(a method-call-wrapped `def`). rubyrs' `class << self` translator
supports `def` / `attr_*` / `alias` / `prepend` / const + cvar
assignment, but NOT nested module/class definitions or arbitrary
statements run with `self` = the eigenclass. This is a documented
subset limitation, not a quick fix — it needs the singleton-class body
to execute with proper eigenclass `self`.

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
