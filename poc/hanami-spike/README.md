# Hanami spike workspace (2026-06-14 discovery round)

How far does rubyrs get with **Hanami 2.3**? Hanami's full stack
(`require "hanami"`) is built on **dry-rb** (dry-system, dry-configurable,
dry-monitor, …) + **zeitwerk** — a deep metaprogramming stack. This
spike targets the most rubyrs-tractable slice: **`hanami-router` 2.3.1**,
which is `mustermann` + `rack` only (no zeitwerk, no dry), the same
stack the Sinatra spike runs.

Run: `target/release/rubyrs poc/hanami-spike/hr-probe.rb`
(full-feature build). Gem sources read from rbenv 3.4.1's gem dir.

## How far it gets

`require "hanami/router"` loads `rack` + `rack/utils` cleanly, then
descends into `mustermann/rails` → `mustermann/ast/*`.

1. **`DelegateClass(...)` — FIXED (2026-06-14, round 2).** The root
   cause was that mustermann does `require 'delegate'` inside
   `Hanami::Router`'s class body, and rubyrs ran the required file's
   `def DelegateClass` in the caller's class scope (→ defined on Router)
   instead of at top-level. Fixed by `fix(require): run a required
   file's body at top-level lexical nesting` — `DelegateClass` now lands
   globally, and `class NodeTranslator < DelegateClass(Node)` loads.
2. **Current wall: `URI::RFC2396_Parser` surface gaps.** mustermann's
   `Versions` DSL (`version('2.3') { on(?:) { … } }`) routes through
   rubyrs' vendored `forwardable`, and `def_single_delegator` mis-targets
   the `on` DSL call against `URI::RFC2396_Parser` (whose rubyrs stub
   lacks the method). Same mustermann/forwardable/URI depth the Sinatra
   spike bridged with vendored patches.

The full `require "hanami"` path was not probed past these — it needs
the whole dry-rb stack plus zeitwerk, and zeitwerk hits the same
`class << self`-with-full-body wall the Bridgetown spike found.

## Shared VM fix

The **`feat(reflection): native Module#name via instance_method`** commit
(see `../bridgetown-spike/README.md`) is needed here too: zeitwerk —
which the full Hanami boot requires — captures `Module#name` via
`Module.instance_method(:name).bind_call(mod)`.

## Walls catalogued (ranked)

1. **mustermann/forwardable/URI depth** — `DelegateClass` ordering +
   `URI::RFC2396_Parser` method surface. Bridgeable with vendored
   mustermann patches (cf. the Sinatra spike) or by deepening rubyrs'
   `forwardable` + URI-parser stubs.
2. **`class << <expr>` with a full body** — gates zeitwerk, hence the
   full `require "hanami"`. Shared with the Bridgetown spike; the
   highest-value cross-cutting wall.
3. **dry-rb metaprogramming** (dry-configurable / dry-system) — not yet
   reached; the deepest layer, deferred until 1 + 2 clear.

The authoritative ranked wall map lives in session memory.
