# Try-runs: what happens when we actually `rubyrs <file>`

The gap reports under [`docs/gap-reports/`](README.md) measure
**AST-level supportedness** — the syntactic share of a codebase
that the rubyrs translator recognises. That's an *upper bound*
on what will actually execute, because:

- the AST view doesn't see whether the methods called at runtime
  are implemented (e.g. `Object#tap` parses as `CallNode` so the
  scan counts it Supported, but the receiver still needs an
  implementation in `vm.rs`)
- a top-of-file `require "json"` is a single `CallNode` in the
  histogram, but if the require itself fails the entire file
  fails before a single line of the body runs
- DSL-shaped scripts (Brewfile, Gemfile, gemspec) expect the
  embedding host to register methods via
  [`Runtime::register_fn`](../../crates/rubyrs/examples/brewfile.rs);
  running them bare under the CLI doesn't have that wrapper

This document records what happens when we actually feed
highest-AST-Supported files from the 10 scanned codebases to
`./target/release/rubyrs` directly — no host wrapper, no
preloaded environment. Concrete, not theoretical.

## Methodology

For each scanned codebase, pick up to three files at or near
the top of the translatable ratio (per gapscan's `--format json`
per-file output, restricted to ≥50 nodes so we avoid trivial
constants-only files). The variety per codebase is on purpose:
the first 100%-AST-Supported file might happen to run, the
second might trip on a real blocker, the third might surface a
different blocker still — getting a few per codebase exposes
patterns that a single representative would hide. Run each under
the rubyrs CLI with a generous fuel cap and capture the first
failure (if any) plus its category.

```bash
cargo build --release -p rubyrs
RUBYRS_FUEL=2000000 ./target/release/rubyrs <path/to/file.rb>
```

## Results — 2026-05-26 (fifth pass), rubyrs at `cba21b6`

Fifth pass after the session's PR wave landed:

- **PR #102** — class-level `@ivars` + class variables (`@@foo`)
- **PR #104** — Enumerable preamble stub + `require` leniency
  (caller-dir / caller-parent search) + `$LOAD_PATH` as a real
  Array + `__FILE__` / `__dir__` / `File.expand_path`
- **PR #105** — `Module#prepend` (chain walked before class's own
  methods)
- **PR #107** — stdlib `require` lenient pass-through stub
  (no-op for known names like `time`, `date`, `logger`,
  `forwardable`; `loaded_features` tracked so re-require returns
  false) + Object preamble stub + `String#hash`
- **PR #109** — block-arg dispatch: `&nil` is no-block,
  `&curried_proc` is accepted as block, `&` TypeError reports
  CRuby class names, `send(:priv, &nil)` preserves visibility
  bypass

Same 12 standalone files, same pinned target commits.

| File | Was (fourth) | Now | Change |
|---|---|---|---|
| liquid/extensions.rb | B | **A** | `require 'time'` / `require 'date'` now stub-out (PR #107 stdlib require stub); file body runs clean |
| sinatra/middleware/logger.rb | B | **A** | `require 'logger'` / `require 'forwardable'` stubbed by the same PR #107 stub; class body executes — `Logger` itself isn't built in, but the file's class definition no longer touches it |
| rake/linked_list.rb | F | **A** | by the fourth pass this had moved into F (project-helper / undefined module) after #30 and #34 closed the original D+E pieces; the remaining helper hole is now covered by PR #104's Enumerable preamble stub + `require` leniency |
| tilt/string.rb | D | **A** | three-layer unblock: #105 (`Module#prepend`) closed `class << self; prepend(...)` in `tilt.rb`; #107 (Object stub + `String#hash` + stdlib require stub) closed `tilt/template.rb` load; #109 (block-arg `&nil` ICE) closed the remaining downstream `evaluate(..., &block)` call sites. The file's class body executes top-to-bottom |
| (8 others) | — | — | unchanged — same A or same blocker as the fourth pass |

Pass count: **5 → 9** (out of 12). Four files moved from blocked
to clean in one pass — the largest jump across all five passes.

### What this pass shows

The fourth-pass analysis warned that "winning at the AST frontier
now requires multi-step investment for the deeply-stacked files",
and that the C-ext require wall (Cat B) and project-helper holes
(Cat F) were load-bearing. This pass tested both predictions:

- **Multi-step investment paid off**: tilt/string.rb sat behind
  *three* blockers (Object ancestor stub, module `prepend`,
  block-arg ICE). The previous pass moved it from C to D by
  closing the first layer; this pass closes the remaining two
  layers AND a downstream layer that only became visible after
  the first two cleared. The pattern: each multi-step file
  surfaces another layer per unblock, and pass-count movement
  arrives only when the LAST one closes.
- **Cat B partially fell to a stub strategy**: rather than
  building real `Time` / `Logger` modules, PR #107's stdlib
  require stub treats common stdlib `require` calls as no-ops
  (with `loaded_features` tracked so re-require returns false).
  Files that only depend on the *load* succeeding (not on the
  module being functional at runtime) now pass.
  liquid/extensions.rb and sinatra/middleware/logger.rb both
  fit that shape; the remaining Cat B file
  (dry/struct/extensions/pretty_print.rb) doesn't — it
  actually exercises `PP.pp` and needs a real module.

### Cumulative category histogram

After 5 passes:

| Category | First pass | Fourth pass | Now | Notes |
|---|---:|---:|---:|---|
| A (runs clean) | 3 | 5 | 9 | +4 this pass: liquid/extensions, sinatra/middleware/logger, rake/linked_list, tilt/string |
| B (C-ext require) | 2 | 3 | 1 | dry/struct/extensions/pretty_print is the last survivor (needs real `PP`) |
| C (require_relative / load path) | 1 | 0 | 0 | unchanged since #66 |
| D (unsupported AST node at runtime) | 3 | 1 | 0 | tilt/string.rb closed; category empty |
| E (literal-default-arg) | 2 | 0 | 0 | unchanged since #34 |
| F (project helper / undefined module) | 2 | 3 | 2 | rake/linked_list moved to A; jekyll/theme_drop (`delegate_method_as`) and bundler/match_remote_metadata remain |
| G (host DSL — Brewfile, excluded) | 1 | 1 | 1 | unchanged |

Net direction: D and E are both empty; C is empty; the only
remaining categories are A (most of the dataset), B (one C-ext
case), and F (two project-internal helper cases). The next pass
will not move purely by adding AST nodes or `require` stubs —
it needs either (a) a real `PP` for the dry-struct case, or
(b) a way to either implement or stub `delegate_method_as` /
the bundler nil-module path for the two F cases.

## Results — 2026-05-25 (fourth pass), rubyrs at `d151c27`

Fourth pass after PR #66 (`require_relative`) landed plus the
subsequent feature wave (Method-* additions, cext L3 Symbol /
Float, ruby-spec extraction v0.2). Same 12 standalone files,
same pinned target commits. Diff vs the third pass:

| File | Was (third pass) | Now | Change |
|---|---|---|---|
| tilt/string.rb | C | **D** | PR #66's `require_relative` makes line 12 reachable; now hits `SingletonClassNode` (`class << self`) + `GlobalVariableReadNode` — the AST-level next layer down |
| (all 11 other files) | — | — | unchanged — same failure category as the third pass |

Pass count: **5 → 5** (out of 12 — third consecutive pass at
5/12; the only growth across the session was the
first→second-pass jump from 3 to 5, when PR #30's
ConstantWriteNode unlocked rake/scope.rb + bundler/version.rb).

### What the C → D shift means

The third pass concluded "AST-frontier saturated" — adding more
AST nodes wouldn't move the pass count because each unblocked
file has stacked blockers waiting. PR #66 (`require_relative`)
was the **non-AST counter-example**: a runtime/load-path
capability, not a parse node. Its landing here is observable
in the data — Cat C dropped to 0, the file it gated shifted
into Cat D (next blocker is back to AST).

Confirms: **winning at the AST frontier now requires
multi-step investment for the deeply-stacked files**. Other
files in this dataset were one capability away from passing
(bundler/version.rb and rake/scope.rb both went A after #30
alone; sinatra/middleware/logger went E → B after #34's
default-args relaxation). tilt/string.rb is the outlier: it
sat behind three layers (require_relative → class << self
→ global vars) before any pass-count movement is possible.
The fourth pass now visibly puts it at the second of three
remaining tiers (`class << self`).

### Cumulative category histogram

After 4 passes:

| Category | First pass | Now | Notes |
|---|---:|---:|---|
| A (runs clean) | 3 | 5 | rake/scope + bundler/version unblocked by #30 |
| B (C-ext require) | 2 | 3 | sinatra/middleware/logger moved here from E after #34 |
| C (require_relative / load path) | 1 | 0 | PR #66 took the last one to D |
| D (unsupported AST node at runtime) | 3 | 1 | tilt/string.rb is the survivor, now at `class << self` |
| E (literal-default-arg) | 2 | 0 | PR #34 closed the whole category |
| F (project helper / undefined module) | 2 | 3 | rake/linked_list moved here from D+E after #30 and #34 |
| G (host DSL — Brewfile, excluded) | 1 | 1 | host-wrapper-only; AST-irrelevant |

Net direction: the language has absorbed every blocker that
was AST-shaped at the start of the session AND `require_relative`,
but the **C-ext require wall (Cat B)** and the **"project-internal helper / undefined module" issues (Cat F)** are now load-bearing. Both are non-AST and need infrastructure
(Logger built-in / Enumerable register-as-Module / C-ext
require chain) to fix.

### Results — 2026-05-25 (third pass), rubyrs at `402917e`

Third pass after PR #34 (`default args = any expression`) landed
plus the subsequent Method-* / cext / GC-root-hole cleanup wave
(#41 #45 #49 #51). Same 12 standalone files at the same pinned
target commits. Diff vs the second pass (post-PR #30) below:

| File | Was (second pass) | Now | Change |
|---|---|---|---|
| sinatra/middleware/logger.rb | E | **B** | E rule gone, but the file's line 3 `require "logger"` (hidden behind the line-8 literal-default-arg compile error) now fires first |
| rake/linked_list.rb | E | **F** | E rule gone, file now reaches line 7 `include Enumerable` — `Enumerable` isn't registered, trips "wrong argument type NilClass (expected Module)" |
| (all 10 other files) | — | — | unchanged — failure stays in same category |

Pass count: **5 → 5** (out of 12, unchanged). Category E drops
from 2 → 0, but the two E-blocked files BOTH had latent
non-language blockers waiting behind them — sinatra's was a C-ext
require, rake/linked_list's was a Module-missing `include`. The
PR #34 description called this out explicitly as a possibility;
this re-run confirms it.

The optimistic projection from the post-#30 doc ("relaxing E
would push pass to 7/12") was wrong — pass *would* have moved to
7/12 if E had been the only blocker on those files, but in
practice E was the first-line error message that masked deeper
problems. Worth recording: **at the AST-supportedness frontier,
each `.rb` file typically has 2–3 stacked blockers; removing the
visible one usually exposes the next**.

### What this changes about the priority list

The next-cheapest "more files run clean" move is now harder to
identify by AST signal alone:

- B (C-ext `require`): 3/12 files. Implementing a `require "logger"`
  / `require "time"` path that materialises the host-side Ruby
  std stub is non-trivial (would need at minimum a built-in
  `Logger` class + Time epoch). Not "easy win" anymore.
- C (`require_relative`): 1/12 file (tilt). Possible but only
  unblocks one file unless the loaded file then also fails on
  something else (likely given the pattern above).
- F (missing host helper / module): 3/12 files. Each is a
  bespoke fix — `delegate_method_as` is a Jekyll DSL,
  `Enumerable` is stdlib-shaped, the bundler one is project-
  internal. No batch fix.

In other words: the AST-frontier pass-count metric **has flattened**.
Further "AST + 1 fix → more passes" wins require either Tier 3 codebase
expansion (find files where AST coverage IS the bottleneck) or
investment in non-AST features (require chain, Enumerable mixin,
Logger built-in).

### Results — 2026-05-25 (second pass, post-PR #30), rubyrs at `a35348b`

Second pass after PR #30 (`ConstantWriteNode`) landed. Same
pinned target commits and fuel cap as the first pass, re-running
the 12 standalone files (the host-DSL `Brewfile.rb` is excluded
— it needs the embedding wrapper, not a rubyrs change). Diff
vs the first pass:

| File | Was | Now | Change |
|---|---|---|---|
| rake/scope.rb | D | ✅ A | `EMPTY = Class.new` now executes; file runs clean |
| bundler/version.rb | D | ✅ A | `VERSION = "...".freeze` now executes; file runs clean |
| rake/linked_list.rb | D + E | E | `ConstantWriteNode` resolved; remaining blocker is the literal-default-arg rule |
| (all 9 other files) | — | — | unchanged — failure stays in same category |

Pass count: **3 → 5** (out of 12 non-host-DSL files = 42%).
Category D drops from 3 → 0, validating both the gapscan
prioritisation (D was the top "syntactic" blocker) and the
fix itself. The remaining Category E files (`rake/linked_list.rb` —
now E-only after PR #30 — and `sinatra/middleware/logger.rb`,
which was always E-only) are the cleanest next target: a
single documented divergence that, once relaxed, would push
pass to 7/12.

### Results — 2026-05-25 (first pass), rubyrs at `6063af8`

Target-codebase commits scanned (matching the source-tree commits
that the gap reports were generated against):

| Codebase | Commit | Date |
|---|---|---|
| Jekyll | `202df57` | 2026-04-22 |
| Liquid | `742ac3d` | 2026-05-20 |
| Sinatra | `5236d34` | 2026-04-29 |
| dry-struct | `26eb60f` | 2026-05-04 |
| Rake | `5cea175` | 2026-05-25 |
| Bundler (in rubygems) | `5c535b0` | 2026-05-20 |
| Tilt | `6a0dae1` | 2026-03-14 |

| File | AST % Supported | Result | Category |
|---|---:|---|---|
| jekyll/utils/thread_event.rb | 100% | ✅ runs clean (no output, no error) | A |
| jekyll/drops/theme_drop.rb | 100% | ❌ `undefined method 'delegate_method_as'` | F |
| liquid/extensions.rb | 100% | ❌ `cannot find C ext: time` | B |
| liquid/resource_limits.rb | 100% | ✅ runs clean | A |
| sinatra/middleware/logger.rb | 100% | ❌ `default value for parameter must be literal` | E |
| dry/struct/extensions/pretty_print.rb | 100% | ❌ `cannot find C ext: pp` | B |
| rake/scope.rb | 98.6% | ❌ unsupported `ConstantWriteNode` (`EMPTY = Class.new`) | D |
| rake/linked_list.rb | 99.0% | ❌ `ConstantWriteNode` + non-literal default arg | D + E |
| bundler/plugin/installer/git.rb | 100% | ✅ runs clean | A |
| bundler/match_remote_metadata.rb | 100% | ❌ `wrong argument type NilClass (expected Module)` | F |
| bundler/version.rb | 98.2% | ❌ `ConstantWriteNode` (`VERSION = "...".freeze`) | D |
| tilt/string.rb | 100% | ❌ `undefined method 'require_relative'` | C |
| crates/rubyrs/examples/brewfile/Brewfile.rb | 100% | ❌ `undefined method 'tap'` | G |

> The sections below — **Category legend**, **What this tells
> us**, **What "Phase 3" would look like** — were written
> against the first-pass data and are kept as the historical
> record (body unchanged; the legend heading was labelled
> "(first pass)" for clarity). After the **fourth pass** above:
> Category D = 1 (was 3 — the original three D files all moved
> to A or F; tilt/string.rb newly entered D after PR #66's
> `require_relative` exposed its next blocker, `class << self`,
> a different AST node than the original D-class
> ConstantWriteNode). Category E = 0
> (PR #34 default-args-any-expression). Category C = 0
> (PR #66 require_relative). The "Phase 3 step 1"
> ConstantPathWriteNode half of the original plan has also
> landed since the first pass was written. Pass count flat at
> 5/12 across four passes — the stacked-blockers pattern means
> each unlock just exposes the next layer.

### Category legend (first pass)

| Code | Category | Count |
|---:|---|---:|
| A | Runs clean | 3 |
| B | Requires a C extension (`require "time"`, `require "pp"`, etc.) | 2 |
| C | Ruby-source `require_relative` (and `require` with load-path resolution) isn't implemented in rubyrs. `require "literal_path"` for C extensions *does* work — see Category B for what fails next when the .so isn't there | 1 |
| D | Hits a still-Missing AST node at execution time (`ConstantWriteNode`) | 3 |
| E | Default-arg-must-be-literal — documented SUBSET divergence bites | 2 |
| F | Project-internal helper assumed (delegate_method_as, include of undefined module) | 2 |
| G | Host function not registered (Brewfile-style DSL needs the embedding wrapper) | 1 |

Counts sum to >13 because some failures hit two categories (rake/linked_list).

## What this tells us

Things gapscan's AST view *already* knew (now confirmed in
practice):

- **`ConstantWriteNode` is real-world blocking, not just a count
  on the chart** — it crashed 3 of the 13 try-run files,
  including the literal first line of `bundler/version.rb`.
  Implementing top-level `FOO = ...` would immediately unblock
  files that are 98%+ AST-supported, not just shift a number on
  a chart. **This is the cheapest "ship more files that
  actually run" move available.**
- **The block / kwarg parameter family is the next concrete pain
  point** — same story: 98%+ AST-supported files crash on what
  the histograms have been calling out for weeks.

Things gapscan's AST view *couldn't* see (this is the value of
running anything):

- **C-extension `require` is a hard wall** (B, 2/13). `require
  "time"` or `require "pp"` immediately fails — rubyrs doesn't
  have a require chain to traverse, let alone the C extensions
  to find. This is documented as out-of-scope in SUBSET.md, but
  the practical implication is sharper now: any file with a
  C-ext require at the top crashes immediately, regardless of
  what comes after.
- **`require_relative` itself isn't implemented** (C, 1/13). Even
  pure-Ruby internal requires fail. This means almost any file
  that's part of a multi-file project (i.e. most real Ruby) needs
  manual cat-ing or pre-loading via the embedding API.
- **Project-internal helpers are an invisible blocker** (F,
  2/13). `Jekyll::Drops::Drop.delegate_method_as` and
  `Bundler::MatchRemoteMetadata`'s include of an undefined
  constant are both project-private extensions that the host
  framework defines elsewhere — they look fine in the AST, but
  the symbol isn't there at runtime. This is the same shape as
  the C-ext require problem (load-time dependency missing) but
  for pure Ruby; it'll be solved automatically once `require_relative`
  works and the dependent files get loaded.
- **Host-DSL scripts need the host wrapper** (G, 1/1). Brewfile
  at 100% AST-supported still crashes on `tap`. The Brewfile
  `tap` is a Homebrew DSL keyword (a bareword call meaning "add
  this Homebrew tap"), not Ruby's `Object#tap` method — its
  vocabulary lives in `examples/brewfile.rs` and is wired in via
  `Runtime::register_fn`. The CLI doesn't load that wrapper, so
  `tap` resolves against nothing and trips a NoMethodError. This
  is by design — `Runtime::register_fn` is the embedding API —
  but it's worth spelling out that "100% Supported on the chart"
  doesn't mean "you can run it standalone with the CLI".
- **Default args must be literals** (E, 2/13). The divergence
  documented in SUBSET.md (default values restricted to
  `Int/Str/Sym/true/false/nil`) hits real Ruby in the wild — any
  `def initialize(level: Logger::INFO)` rejects compile-time.
  Worth weighing whether to broaden defaults to "any pure
  expression" given the actual hit rate.

## What "Phase 3" would look like

If we wanted to push further than this, the natural next step
matches the original three-phase plan from session start:

1. **Implement `ConstantWriteNode` + `ConstantPathWriteNode`** —
   unlocks bundler/version, rake/scope, rake/linked_list at a
   minimum.
2. **Implement a minimal `require_relative`** that resolves to
   the host's file-system (Embedding API extension), gated by
   a `Runtime::Config` flag for hosts that want to forbid I/O.
   This converts the "3/13 standalone-runnable" rate into
   something much higher.
3. **Standardise a `delegate_method_as`-equivalent** as a
   built-in macro (similar to how `attr_*` are already built-in
   macros) for the cases where projects roll their own. Or
   accept that those projects need a small per-project shim.

Each of these is a real PR, not a docs-only one — but the data
in this file says they'd produce visible "files that now run"
deltas, not just chart movement.
