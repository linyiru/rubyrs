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

## Results — 2026-05-27 evening (ninth pass), rubyrs at `076c7135`

Ninth pass after pass-8's layer #8 closed:
- PR #196 (`fix(vm): expand bare-call Class bridge whitelist`)
  +3 follow-up commits in the same PR addressing code-review
  findings (`u16::MAX` cache sentinel, tightened Module-allocate
  fence assertion, `do_call_block` parallel bridge).

Same probe driver shape as pass-7 / pass-8 (embedder stubs for
Rack / Gem::Version / ERB / URI / Rack::Utils, then
`require_relative` into sinatra-4.2.1's base.rb).

### What this pass shows

**The pass-8 wall is gone.** Probe now executes past pass-8's
line 265 stop point (and earlier line 974 second stop on the
same pass once the missing `Rack::Utils` constant was stubbed)
and reaches `sinatra/base.rb:1292` — into the `class << self`
body that holds Sinatra::Base's DSL surface. The intermediate
layers between line 265 and 1292 are all embedder-shape
constants (Cat F): adding stubs for them surfaces the next
gap in 4-line increments. The interesting one is the LAST one
hit, which is the actual language-level wall.

### Stacked blockers found this pass

| # | File / line | Symbol | Category | Notes |
|---|---|---|---|---|
| 9 | `sinatra/base.rb:974` (`class Base; include Rack::Utils; ...`) | `Rack::Utils` module + accessor methods (`escape_html` / `escape_path` / `unescape` / `parse_nested_query` / `build_nested_query` / `status_code` / `HTTP_STATUS_CODES` constant) | Project shape | Sinatra's `Base` class mixes in `Rack::Utils`. Embedder must stub the module + the small surface base.rb actually consults. Same shape as the Rack middleware row from pass 8. |
| 10 | `sinatra/base.rb:978` (`URI_INSTANCE = defined?(URI::RFC2396_PARSER) ? URI::RFC2396_PARSER : URI::RFC2396_Parser.new`) | `URI::RFC2396_Parser` class | Project shape | Embedder must stub `URI::RFC2396_Parser` with at least `escape` / `unescape` / `parse`. CRuby ships URI in stdlib; rubyrs doesn't yet. |
| 11 | `sinatra/base.rb:1292` (`class Base; class << self; CALLERS_TO_IGNORE = [...].freeze; attr_reader :routes, ...; def reset!; ...; end; end; end`) | `class << self` body with constant assignment | **Real gap (AST surface)** | rubyrs's spike subset only accepts `def` / `attr_*` / `alias` / `prepend Mod` inside `class << self`. Sinatra's singleton class body opens with `CALLERS_TO_IGNORE = [...]` — a constant assignment — before the `attr_reader` and `def` blocks. The translator raises `NotImplementedError` at compile time. **First Cat D gap surfaced since pass 4.** |

### Minimal repro for layer #11

```ruby
class Foo
  class << self
    BAR = 42                 # NotImplementedError: class << self body: only `def`/...
    def get_bar; BAR; end
  end
end
```

`Foo.get_bar` in CRuby returns 42. In rubyrs, compilation
fails before any code runs. The minimal fix is to extend the
`class << self` body whitelist to accept `ConstantWriteNode`
(constant assignments) and store them on the same singleton
class object that already holds `def self.X` (in its constants
table, separate from the method table where `def self.X`
lives).

### What this tells us

- **Pass 9 surfaces the first Cat D gap since pass 4.** Up to
  pass 8 the wall was always missing runtime methods or
  embedder-shape constants; pass 9 hits a translator-level
  restriction that has stood since the spike. The other two
  rows (layers #9, #10) are pure Cat F — same shape as the
  Rack middleware batch from pass 8.
- **Linear advance shape is holding.** Pass 7 stopped at line
  64, pass 8 at 265 / 974, pass 9 reaches 1292. Each pass
  advances roughly 4× through the file, finding one or two
  Cat F batches plus one language gap.
- **Cat D gap is broader than this one site.** Sinatra has
  THREE `class << self` blocks (lines 1292, 1967, 2122);
  the first one is the cheapest to repro. Whatever fix lands
  for #11 will likely unblock all three at once.

### Cumulative category histogram (sinatra/base.rb body, this pass)

| Category | This pass | Notes |
|---|---:|---|
| Cat B (require) | 0 | Still handled by PR #135 fallback |
| Cat D (AST node) | 1 | `class << self` body with constant assignment (row 11) |
| Cat F (project shape) | 2 batches | Rack::Utils (row 9), URI::RFC2396_Parser (row 10) |
| Cat H (real built-in / runtime gap) | 0 | None this pass |
| Cat I (real bug) | 0 | None this pass |

### Concrete next moves suggested by the data

1. **Extend `class << self` body whitelist to accept
   `ConstantWriteNode`** — closes layer #11 and unblocks the
   other two `class << self` blocks at lines 1967 / 2122
   simultaneously. Translator-side fix, no VM changes required.
   Tier 1.
2. **Continue iterating** if further probes are desired —
   the next stop after layer #11 likely surfaces inside one
   of those subsequent `class << self` bodies, or in the
   `Helpers` / `Templates` module bodies the `include`s on
   lines 975-976 pull in.

---

## Results — 2026-05-27 (eighth pass), rubyrs at `c1605c04`

Eighth pass after the pass-7 Cat H + Cat I gaps were all closed:
PR #144 (`instance_variable_get`/`set`), PR #169 (`alias` nested-
via-path superclass), PR #176 (`Regexp#freeze`/`frozen?`), and
PR #181 (`Class#allocate`) all landed on master between pass-7
and this re-probe. The probe driver is unchanged in shape (same
embedder stubs for Rack / Gem::Version / ERB), so this is a
true apples-to-apples re-run.

### What this pass shows

**The pass-7 wall is gone.** The probe now executes past pass-7's
line 64 stop point and lands at `sinatra/base.rb:260` — a 4×
linear advance through `<class:Sinatra>` body. New layers
surface in the section that runs after
`class Request < Rack::Request` finishes loading: middleware definitions
(`class CommonLogger < Rack::CommonLogger`), more
embedder-shape Rack constants, and one new Cat I bug.

The 4 layers pass-7 documented as Cat H / Cat I
(`instance_variable_get`, `Class#allocate`, `Regexp#freeze`, nested-alias)
are now silent — the probe walks past every one of them
without trace. This is the expected outcome of those PRs but
worth pinning explicitly: zero regressions on the closed
surface.

### Stacked blockers found this pass

| # | File / line | Symbol | Category | Notes |
|---|---|---|---|---|
| 7 | `sinatra/base.rb:260` | `Rack::CommonLogger`, `Rack::NullLogger`, `Rack::Head`, `Rack::MethodOverride`, `Rack::Lint`, `Rack::ConditionalGet`, `Rack::Static`, `Rack::Builder` | Project shape | Sinatra defines middleware subclasses of these — embedder must stub each before the require. Same shape as the pass-7 `Rack::Request` row. |
| 8 | `sinatra/base.rb:265` | bare `superclass` call inside class body | **Real bug** | `class Bar < Foo; superclass.class_eval { ... }; end` raises `NoMethodError: undefined method 'superclass' for Class`, even though `self.superclass` works inside the same body. Bare-call resolution inside a class body apparently routes built-in Class methods through a different path than user `def self.x` methods (the same body resolves user-defined class methods with no `self.` prefix correctly). Minimal repro is 4 lines. |

### Minimal repro for layer #8

```ruby
class Foo; end
class Bar < Foo
  superclass.class_eval do      # NoMethodError: undefined method 'superclass' for Class
    def hi; "from-Foo"; end
  end
end
```

`self.superclass.class_eval` works. User `def self.greet` is also
reachable as a bare call from a subclass body (verified). The
divergence is specific to **built-in `Class` methods** invoked
bare from inside a class body whose implicit receiver is the
class itself — likely a small fix in the bare-call dispatch
path (the same one PR #169 closed for `alias` in nested
contexts, possibly the same code site).

### What this tells us

**Pass-8 found 2 fresh layers** vs pass-7's 6 layers, even
though we now execute through ~4× as many lines of base.rb.
The shape that remains is roughly stable: 1 batch of
embedder-shape Cat F middleware constants, then 1 real bug.
This is consistent with the pass-7 prediction ("each base.rb
section likely exposing 2-3 more layers down the same shape").

If sinatra hosting ever becomes a roadmap item (it currently
isn't, per [ROADMAP.md](../ROADMAP.md)), this pass suggests
~4-6 more rounds of probe-fix-probe would carry the require
chain through the rest of base.rb — comparable in scope to
the pass-5–7 wave for `tilt/string.rb`.

### Cumulative category histogram (sinatra/base.rb body, this pass)

| Category | This pass | Notes |
|---|---:|---|
| Cat B (require) | 0 | Still handled by PR #135 fallback |
| Cat F (project shape) | 1 batch | Rack middleware constants (8 classes in one row, sinatra/base.rb:260) |
| Cat H (real built-in / runtime gap) | 0 | All pass-7 Cat H items closed; none surfaced this pass |
| Cat I (real bug) | 1 | Bare `superclass` inside class body (row 8) |

### Concrete next moves the data suggests

1. **Fix layer #8** — bare-call to built-in `Class` methods from
   inside a class body. Minimal repro above; expected scope is
   small (single dispatch arm or bare-call resolver). Closes the
   only Cat H / Cat I gap this pass surfaced. Tier 1.
2. **Continue iterating** if further pass-8+ probes are desired —
   the next stop after layer #8 will be inside one of the eight
   middleware class bodies the embedder is required to stub.

---

## Results — 2026-05-26 late (seventh pass), rubyrs at `2fd14d7`

Seventh pass after PR #135 (`require` fallback for embedder-pre-
registered namespace constants) and PR #139 (cext_flori_json
regression test) landed. The pass-6 directional read was that
Cat D is empty and the live-fire surface is Cat B (Rack require)
+ Cat F (Gem::Version / ARGV / project helpers). PR #135 closed
the Rack-require half — `require 'rack'` against a pre-stubbed
`module Rack` now no-ops cleanly. This pass tests the obvious
follow-up: **how far does `sinatra/base.rb`'s body actually
execute** once the require chain succeeds?

The point isn't to make a sinatra app work end-to-end (out of
scope per [`docs/ROADMAP.md`](../ROADMAP.md)) — it's to surface
the **stacked-blocker layers behind `require 'sinatra'`**, the
same shape the fifth pass found inside tilt/string.rb. Each
unblock surfaces the next; counting how many layers exist tells
us the order-of-magnitude work that would be required if the
roadmap ever changed its mind.

### Probe driver

A standalone Ruby script preloads stub modules for every external
gem `sinatra/base.rb` requires (Rack, Rackup, Tilt, Mustermann,
IPAddr), plus a `Gem::Version` shell with a `<=>` operator, plus
an `ERB` returning a no-op singleton, then `require_relative`s
into the gem's `sinatra/base.rb`. After each new NameError /
NoMethodError, the missing piece is added to the preload and the
probe is re-run.

```ruby
# Embedder pre-stubs — PR #135 fallback resolves `require "rack"`
# against these without going to cext_require.
module Rack
  RELEASE = "3.0.0"
  class ShowExceptions; end
  class Request
    def ssl?; false; end          # used by `alias secure? ssl?`
    # ...other Request methods stubbed similarly
  end
end
module Rackup; end
module Tilt; end
module Mustermann; end
class IPAddr; end

# Cat F closures from prior pass-6 observations.
module Gem
  class Version
    include Comparable
    attr_reader :s
    def initialize(s); @s = s.to_s; end
    def <=>(o); @s <=> o.s; end
    def to_s; @s; end
  end
end

# show_exceptions.rb does `TEMPLATE = ERB.new <<-HTML` at class
# body load. We don't need real templating — return a singleton
# whose `result` produces "".
$erb_singleton = nil
class ERB
  def self.new(_tpl)
    return $erb_singleton if $erb_singleton
    obj = Object.new
    def obj.result(_b=nil); ""; end
    $erb_singleton = obj
  end
end

require_relative '<host-gem-path>/sinatra-4.2.1/lib/sinatra/base'
puts "REACHED-END"
```

### Stacked blockers found

Each row is "what crashes if you DON'T preload it". `Real gap`
means a genuine rubyrs missing built-in or unimplemented runtime
behavior; `Real bug` is a defect in code that exists today but
behaves incorrectly; `Project shape` means sinatra references
something its embedder is expected to provide.

One observation kept passing through the table without being a
blocker: a `Gem::Version.new(RUBY_VERSION)` call ends up doing
`RUBY_VERSION <=> "3.0"` via String comparison (the actual
literal depends on whichever RUBY_VERSION rubyrs reports —
currently `"3.4.0"` per `crates/rubyrs/src/lib.rs`), which the
Tier-1 String impl gets right. Not in the table because it's
not a blocker; called out here to record that this *did* go
through the probe without surprise.

| # | File / line | Symbol | Category | Notes |
|---|---|---|---|---|
| 1 | `sinatra/indifferent_hash.rb:189` | `Gem::Version` | Project shape | Used to gate `def except` on Ruby ≥3.0 (`Gem::Version.new(RUBY_VERSION) >= Gem::Version.new("3.0")`). Embedder can stub trivially. |
| 2 | first revision of the probe's `Gem::Version#<=>` stub: `def <=>(o); @s <=> o.instance_variable_get(:@s); end` | `Object#instance_variable_get` | **Real gap** | Missing built-in. Surfaced when an embedder-written `<=>` reaches for `instance_variable_get` to compare opaque objects (a common CRuby idiom; the actual call site here was the probe's own first-revision `Gem::Version#<=>`, but the same shape appears in any introspection-heavy gem). The probe driver shown above is the **after-workaround** version (`attr_reader :s` + `o.s`); the call below was what crashed before the workaround was added. |
| 3 | `sinatra/show_exceptions.rb:74` | `ERB` constant | Project shape | `TEMPLATE = ERB.new ...` at class body. Embedder must stub before requiring `show_exceptions`. |
| 4 | first revision of the probe's `ERB.new` stub: `def self.new(*a); self.new; end` → infinite recursion; corrected to `def self.new(_tpl); allocate; end` which then trips this gap | `Class#allocate` | **Real gap** | Missing built-in. CRuby's `Class#allocate` returns a bare instance of the receiver class without calling `initialize`. Note the call site here is `ERB.allocate` (i.e. invoking the `Class#allocate` instance method on the `ERB` class object), not `Class.allocate` (which would be allocating an instance of the Class class itself). Workaround in the probe: build the stub instance via `Object.new` + singleton-class `def`. |
| 5 | `sinatra/base.rb:32` (`class Request < Rack::Request`, line `HEADER_PARAM = /.../.freeze`) | `Regexp#freeze` | **Real gap** | Missing built-in. `Regexp` objects have no mutating instance methods, so freezing is a compatibility shim — `freeze` should exist and be safe to no-op, but rubyrs doesn't currently define it on Regexp at all, which is why the explicit `.freeze` call here raises. (CRuby Regexp literals aren't guaranteed `frozen?` by default in every version; the relevant property is immutability, not literal-time frozen state.) Workaround in the probe: monkey-patch `Regexp#freeze` to return `self`. |
| 6 | `sinatra/base.rb:64` (`alias secure? ssl?`) | `alias` ancestor lookup across nested-via-path superclass | **Real bug** | `class Sinatra::Request < Rack::Request; alias secure? ssl?; end` fails with `undefined method 'ssl?' for class 'Sinatra::Request'` — even though `Rack::Request#ssl?` IS defined. Minimal repro confirms it: top-level `class Child < Parent; alias x y` works, but moving Parent into `module Rack` makes the alias's method-lookup fail to walk the superclass chain. |

### What this tells us

**Six stacked layers before `Sinatra::Base` even finishes loading**, three of which are genuine rubyrs gaps (`instance_variable_get`, `Class#allocate`, `Regexp#freeze`) and one is a real bug (`alias` ancestor lookup with nested-via-path superclass). The Cat F items (Gem::Version, ERB, Rack::Request methods) are embedder-shaped and don't need language work — but they're load-bearing for any pre-loading strategy.

This is the same pattern pass 5's tilt/string.rb showed: a single
file at 100% AST-translatable can stack 4+ runtime-level blockers
behind it. The pass-count metric on the standalone 12-set didn't
move (the dataset was chosen specifically to surface Cat
distinctions, not multi-step depth), but the **first 64 lines
of sinatra/base.rb** revealed more about the surface than the
entire pass-6 sweep across 11 files.

### Concrete next moves the data suggests

In order of "smallest fix that closes the next blocker":

1. **`Object#instance_variable_get` + `Object#instance_variable_set`** — well-defined CRuby built-ins, ~10 lines. Closes layer #2 and likely several others (introspection-heavy gems use these constantly). Tier 1 candidate.
2. **`Class#allocate`** — bypass-initialize allocator. Closes layer #4; also load-bearing for any framework that stores per-instance C state via TypedData-equivalent. Tier 1.
3. **`Regexp#freeze` as no-op** — one line. Closes layer #5. CRuby's `Regexp` values are immutable by construction (no mutating instance methods), so `freeze` on a Regexp has nothing to enforce and is safe to no-op for compatibility. (Distinct from `String#freeze`, which rubyrs implements with real frozen-flag tracking — strings have mutators that need enforcement; regexes don't.) Tier 1.
4. **`alias` ancestor lookup fix** — layer #6 is the only true bug; the minimal repro shows the lookup chain isn't walking nested-via-path superclasses. _Hypothesis (not yet verified against the implementation)_: alias-time method resolution may consult only the current class's method table rather than walking the `Class.superclass` chain. Modest scope expected; the actionable artifact here is the minimal repro itself, plus one regression test pinning the nested case once the cause is confirmed in code.

### Cumulative category histogram (sinatra/base.rb body itself, this pass)

Two new categories are introduced in this pass on top of the
original A–G legend (preserved as historical record at the
bottom of this file). They are scoped to pass 7 and forward —
older passes don't use them:

- **Cat H** — real gap, fixable with a small Tier-1 PR. A genuine missing built-in or runtime feature; the codebase doesn't yet implement what CRuby's semantics require.
- **Cat I** — real bug, needs investigation + regression test. A defect in code that already exists but behaves incorrectly under some shape (here: nested-via-path alias-time lookup).

Counted against the 6 table rows above, **plus** one additional
"Project shape" observation that didn't earn its own row: the
embedder must define enough `Rack::Request` method surface
(`ssl?` / `request_method` / etc.) for the alias targets to
resolve. That's a third Cat F item without being a distinct
blocker row, because the probe driver hit it as part of
preparing for row 6 rather than as a fresh post-row-6 layer.
So the histogram total is 7 (6 rows + 1 in-prose Cat F), not 6:

| Category | This pass | Notes |
|---|---:|---|
| Cat B (require) | 0 | All resolved by PR #135 fallback against pre-stubs |
| Cat D (AST node) | 0 | Pass-6 confirmed sinatra/lib AST-clean; body inherits that |
| Cat F (project shape) | 3 | Gem::Version (row 1), ERB (row 3), Rack::Request method surface (in-prose, not a row) |
| Cat H (real built-in / runtime gap) | 3 | `instance_variable_get` (row 2), `Class#allocate` (row 4), `Regexp#freeze` (row 5) |
| Cat I (real bug) | 1 | `alias` ancestor lookup w/ nested-via-path (row 6) |

If the goal ever becomes "host real sinatra," it's not a one-PR effort — it's a roadmap shift, with each base.rb section likely exposing 2-3 more layers down the same shape this pass mapped for the first 64 lines.

---

## Results — 2026-05-26 evening (sixth pass), rubyrs at `ad0a6ba`

Sixth pass after the post-#107/#109 wave (PR #124 BackReferenceRead,
PR #127 StringScanner vendor, PR #128 Module.deprecate_constant stub,
plus the BigInt / Module.new / Kernel#sprintf / Integer(str,radix)
landings that arrived alongside them). Host is the same Mac;
re-probed against the sinatra/rake/tilt/bundler files installed on
host gem path (jekyll/liquid/dry-struct not installed — those 5
files of the original 12 are SKIPPED below, not regressed).

### What this pass shows

**The Sinatra `lib/` AST frontier is gone.** Re-running
`rubyrs-gapscan scan sinatra/lib` against the same Sinatra commit
(`5236d34`) returns **0 Missing node classes** — down from 17 the
day before (`BlockParameterNode` ×58, `RestParameterNode` ×44,
`RegularExpressionNode` ×39, `AliasMethodNode` ×16,
`KeywordHashNode` ×12, `ConstantWriteNode` ×10, `AssocSplatNode` ×6,
`DefinedNode` ×5, `GlobalVariableReadNode` ×5,
`KeywordRestParameterNode` ×5, `NumberedReferenceReadNode` ×5,
`ClassVariableReadNode` ×4, `SingletonClassNode` ×3,
`ClassVariableWriteNode` ×2, `InterpolatedRegularExpressionNode` ×2,
`BackReferenceReadNode` ×1, `LambdaNode` ×1; full list in
[sinatra.md](sinatra.md)). All 5 non-trivial files in sinatra/lib are
now **100% AST-translatable**, including `sinatra/base.rb` (7113
nodes, previously 97.48%).

Observable in this pass's try-runs:

| File | Pass 5 | Now | Change |
|---|---|---|---|
| sinatra/middleware/logger.rb | A | **A** | unchanged |
| sinatra/base.rb | (out of 12-set; would have been D — parse blocked on `BackReferenceReadNode`) | **B** | parse now completes; first runtime statement is `require 'rack'` → C-ext require wall (Cat B) |
| rake/scope.rb | A | **F** | `Rake::LinkedList` undefined — rake/scope.rb references the helper without requiring it; the helper file itself is still Cat A |
| rake/linked_list.rb | A | **A** | unchanged |
| tilt/string.rb | A | **A** | unchanged |
| bundler/version.rb | A | **A** | unchanged |
| bundler/plugin/installer/git.rb | A | **A** | unchanged |
| bundler/match_remote_metadata.rb | F | **F** | unchanged |

Pass count (host-installable subset, 7 files — original 12 minus
the 5 gems not on host minus Brewfile): **6/7 → 5/7** at the
file-roster level — `rake/scope.rb` downgraded A → F this pass
(detailed in the table above; cause is load-order, not language
regression). `sinatra/base.rb` *would* be a fresh Cat B file if we add it,
but the original 12-set kept Sinatra represented by
`middleware/logger.rb` (already Cat A) so the roster's pass count
doesn't move. The five other files from the original 12-set
(jekyll/utils/thread_event, jekyll/drops/theme_drop,
liquid/extensions, liquid/resource_limits, dry/struct/extensions/pretty_print)
are skipped this pass because their gems aren't installed on the
current host. Treat their pass-5 category (3×A, 1×F, 1×B) as
carried forward; future passes that re-fetch those gems should
re-confirm.

### Additional sinatra/lib files probed this pass (not in original 12)

Since the gapscan flip surfaced four more sinatra files at 100%
translatable, ran each standalone to see what category they fall
into at runtime:

| File | Nodes | Category | Trigger |
|---|---:|---|---|
| `sinatra/main.rb` | 216 | F | `uninitialized constant Sinatra::ARGV` — Ruby's top-level `ARGV` global isn't exposed inside class/module bodies |
| `sinatra/indifferent_hash.rb` | 483 | F | `uninitialized constant Gem::Version` — uses Rubygems' `Gem::Version` for a version check |
| `sinatra/show_exceptions.rb` | 175 | B | `require 'rack/show_exceptions'` C-ext wall |
| `sinatra/base.rb` | 7113 | B | `require 'rack'` C-ext wall |

Notably, **none of the four falls into Cat D** — every AST node
they reach for at runtime is implemented. They split between
"first executable statement is `require <C-ext>`" (Cat B) and
"references a constant not in scope" (Cat F).

### What the gapscan flip means for what to do next

Pre-pass-6, the standing read was "AST frontier saturated; further
movement needs non-AST work (require chain, Enumerable, Logger
built-in, etc.)" — that was empirically true *for the original 12
files*. The flip to zero-Missing on sinatra/lib doesn't change that
read; it confirms it. Specifically:

- **The dataset to look at next is sinatra-shaped, not AST-shaped.**
  Every blocker in the four new sinatra files above is either Cat B
  (C-ext require chain — Rack) or Cat F (project-helper / Rubygems /
  toplevel-ARGV). None are about Prism nodes anymore.
- **`require 'rack'` is the highest-leverage Cat B in the wild.**
  Solving it (even via a "register a host-provided Rack module and
  let `require` no-op when the constant is already defined" stub)
  unblocks sinatra/base.rb's body. That body in turn is gated by
  many of the same hidden gaps that the AST view classifies
  Supported (e.g. `route`, `halt`, `set` as bareword DSL calls).
  Expect each unblock to surface the next.
- **Cat F now divides into two flavours.** Project-internal helpers
  (jekyll's `delegate_method_as`, bundler's `MatchMetadata`,
  rake/scope's missing `LinkedList`) are one shape; well-known
  built-ins absent from rubyrs (`Gem::Version`, top-level `ARGV`)
  are another. The latter are individually small wins and worth
  considering as "low-hanging Cat F → Cat A" moves.

### Cumulative category histogram

Counted against the host-installable subset (7 files of the original
12 + 4 new sinatra files = 11 unique files):

| Category | Pass 5 (7-file subset) | Now (11 with sinatra additions) | Notes |
|---|---:|---:|---|
| A (runs clean) | 6 | 5 | down by one — `rake/scope.rb` moved A → F this pass (Rake::LinkedList load-order). The 5 remaining: middleware/logger, rake/linked_list, tilt/string, bundler/version, bundler/plugin/installer/git |
| B (C-ext require) | 0 | 2 | sinatra/base, sinatra/show_exceptions |
| D (unsupported AST node at runtime) | 0 | 0 | unchanged at empty |
| F (project-helper / undefined constant) | 1 | 4 | match_remote_metadata, rake/scope (new this pass — Rake::LinkedList load-order), sinatra/main (ARGV), sinatra/indifferent_hash (Gem::Version) |

Net direction: Cat B grew (the Rack require is now a real,
sinatra-side blocker, not just an abstract C-ext wall), and Cat F
became the live-fire surface — the *specific* gaps in Cat F now
include things rubyrs could plausibly stub (`Gem::Version`,
top-level `ARGV` exposure), which is a more actionable shape than
pass 5's "Cat F = unfixable per-project quirks".

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
  `forwardable`; a separate `loaded_stdlib_stubs` set
  tracks first-load so re-require returns `false`, matching
  CRuby's idempotency without sharing the `loaded_features`
  path-keyed set used for real Ruby-source loads) + Object
  preamble stub + `String#hash`
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
  a *load-path* layer (require_relative — closed in PR #66, the
  C→D move recorded in the fourth pass) and then *three more
  AST/VM layers* after that — `class << self; prepend(...)`,
  Object as an ancestor in the lookup chain, and the block-arg
  ICE on the inner `evaluate(..., &block)` forwarding. This pass
  closes the three post-require_relative layers (PR #105, #107,
  #109) AND a fourth that only became visible after the first
  two cleared. The pattern: each multi-step file surfaces
  another layer per unblock, and pass-count movement arrives
  only when the LAST one closes.
- **Cat B partially fell to a stub strategy**: rather than
  building real `Time` / `Logger` modules, PR #107's stdlib
  require stub treats common stdlib `require` calls as no-ops
  (with a separate `loaded_stdlib_stubs` set tracking first
  load so re-require returns `false`, matching CRuby
  idempotency). Files that only depend on the *load*
  succeeding (not on the module being functional at runtime)
  now pass.
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
