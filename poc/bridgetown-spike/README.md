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

**Progress (2026-06-14, round 2):** after the upstream eigenclass-body
work (`class << expr` now executes for real), zeitwerk fully **loads,
runs `Loader#setup`, and eager-loads** — it walks `bridgetown-foundation`'s
`lib/` tree. Two VM fixes this round carried it there: `Dir.each_child`
(zeitwerk's directory walk) and the require-scope fix (a required file's
top-level `def`s now land on Object, not the enclosing class body).

**Current wall (frontier):** zeitwerk autovivifies implicit-namespace
**directories** by registering `Module#autoload(:CoreExt, "<dir>")` and
intercepting the fired `require "<dir>"` in its own decorated
`Kernel#require` (zeitwerk/core_ext/kernel.rb) — when the path is a
directory it calls `loader.__on_dir_autoloaded` (creates the module)
instead of file-loading. rubyrs' autoload-fired / native `require` does
**not** dispatch through a Ruby-level `Kernel#require` override, so the
raw require hits the directory → `RuntimeError: read … Is a directory`.
Honouring a user-defined `Kernel#require` (at least for the
autoload-trigger path) is the next wall — invasive, since `require` is
perf-critical and deeply wired.

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
