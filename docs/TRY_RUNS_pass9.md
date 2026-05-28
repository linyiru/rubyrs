# TRY_RUNS Pass-9 — `sinatra/base.rb` end-to-end load

This document records the **pass-9.7d** layer series that drove
`sinatra/base.rb` (Sinatra 4.2.1, 2065 lines) from a load-time
NoMethodError at line 64 all the way to **end-to-end load
completion** (probe prints "REACHED-END" with no errors).

Note: this captures the layers closed during the focused
late-pass-9 session that culminated in PR #270. Earlier
pass-9 layers (#1–#20) landed in prior sessions; their layer
numbers and PRs are referenced here for continuity but not
re-described.

## Methodology — the "probe → layer → fix" loop

1. **Probe**: run `./target/release/rubyrs /tmp/probe_pass8.rb`,
   where the probe script supplies minimal embedder stubs
   (Rack module skeleton, `URI`, etc.) then
   `require_relative` sinatra's `base.rb`.
2. **Read the error**: the first NoMethodError / NameError /
   SyntaxError points at a specific `base.rb` line and a
   specific Ruby semantic gap.
3. **Categorise** (Cat F / H / I / D — see below).
4. **Land the fix as one PR** with:
   - The rubyrs source change(s)
   - A `tests/diff/*.rb` fixture pinning the new behavior
     byte-for-byte against CRuby
   - Documentation update if the fix introduces or exposes a
     Tier-1 divergence (SUBSET.md entry)
5. **Re-probe**: confirm advancement to the next layer.

## Categories

| Cat | Meaning | Example |
|-----|---------|---------|
| **F** | Embedder-stub / probe-shape gap (no rubyrs code change required) | Need `Rack::Session::Cookie` constant; extend probe |
| **H** | Real built-in gap — CRuby has it, rubyrs doesn't | `Proc#arity`, `Array#dup` |
| **I** | Real bug — wrong behavior, wrong shape, or wrong dispatch path | Tier-1 `Class#singleton_class` stub returning the receiver |
| **D** | AST surface gap — parser or compile-time wiring missing | Visibility modifiers inside `class << self` body |

## Layer table — pass-9.7d series

Layers closed during the focused session. The
`base.rb:line` column points at the source where the previous
layer's fix unblocked execution and the next error surfaced.

| Layer | Cat | base.rb line | Blocker | PR | Fix one-liner |
|-------|-----|--------------|---------|----|-----|
| #21 | I | 1735 | `Module#define_method` dispatch missing | [#245] | Runtime dispatch arm in `do_call_block`; explicit-recv + bare-call shapes |
| #22 | F | 1945 | `Rack::Session::Cookie` not in probe stubs | (probe edit) | Added two-line `module Session; class Cookie; end; end` to probe |
| #23 | I | 1735 (revisited via 1953) | `Class#singleton_class` was a Tier-1 stub returning the receiver — `define_singleton` idiom (`singleton_class.class_eval { define_method(name, &content) }`) installed on instance-methods table instead of singleton table | [#253] | Real eigenclass-shell with `singleton_target` weak-ref; install paths (`Op::DefMethod`, `Op::DefMethodBlock`, runtime `define_method`, `Op::AliasMethod`) redirect through `Class::install_method` |
| #24 | H | 1810 | `Proc#arity` missing on `Value::Block` | [#263] | Arity arm in `try_dispatch_callable_intrinsics` derived from `BlockHandle.{n_params, rest_slot}`; `CurriedProc#arity` returns -1 |
| #25 | I | 1404 | `Kernel#Array` not reachable via `method(:Array).call` (with-recv re-dispatch) | [#267] | Kernel module-function fallback in `do_call` tail (after `method_missing`, before NoMethodError) for `Array/Integer/Float/String/sprintf/format`; honours `suppress_call_result_push` |
| #26 | H | 1534 | `Array#dup` / `Array#clone` missing | [#270] | Shallow-copy arm in `array.rs` primitive dispatch (`Vec<Value>::clone` + new heap alloc). **`base.rb` loads end-to-end.** |

[#245]: https://github.com/linyiru/rubyrs/pull/245
[#253]: https://github.com/linyiru/rubyrs/pull/253
[#263]: https://github.com/linyiru/rubyrs/pull/263
[#267]: https://github.com/linyiru/rubyrs/pull/267
[#270]: https://github.com/linyiru/rubyrs/pull/270

## End state

`./target/release/rubyrs /tmp/probe_pass8.rb` runs the
probe's stub-Rack-then-require-sinatra script all the way
through, prints `REACHED-END`, and exits 0. All 2065 lines of
`sinatra/base.rb` evaluate without error.

What still does NOT work (out of pass-9 scope):

- `Sinatra::Application.get('/') { 'hi' }; Sinatra::Application.run!` —
  the DSL itself runs, but `run!` requires Rack server + TCP
  socket support (Cat F at framework boundary, Tier-2 territory).
- Any code that imports the full Rack autoload graph — most of
  Rack's classes are autoloaded lazily; the probe stubs the
  surface sinatra/base.rb touches, not the full Rack API.

## Tier-1 divergences exposed by this pass

Each PR documented its specific divergence in `SUBSET.md`:

- **PR #245** — `Module#define_method` accepts 1-arg + block;
  2-arg Proc form raises `ArgumentError("not yet supported")`.
- **PR #253** — `Class#singleton_class` returns a redirecting
  eigenclass shell. Method installs land on the real class's
  `singleton_methods` table; reflection on the shell itself
  (`instance_methods`, `include?`, `include`, `prepend`)
  operates on the shell's empty tables (documented gap).
- **PR #263** — `Proc#arity` for blocks with keyword params:
  blocks don't support keyword params in Tier-1
  (`compile_block` only accepts `Single`/`Destructure`/`Rest`),
  so the formula is `has_rest ? -(n_required + 1) : n_required`.
- **PR #267** — Kernel module functions reachable on every
  receiver via the `do_call` fallback. CRuby's private-visibility
  check would raise `NoMethodError (private)`; rubyrs silently
  succeeds. `respond_to?` still returns `false` to match CRuby.
  Documented because the visibility-bit model is Tier-2.
- **PR #270** — `Array#freeze` is a no-op (pre-existing
  divergence, line 595 in SUBSET.md). `Array#clone` therefore
  cannot preserve the frozen flag, collapsing into `dup`'s
  shallow-copy semantics. Documented within the existing
  `freeze` entry.

## What the layer numbers don't capture

The pass methodology surfaced ~30 secondary findings during
Copilot code-review on the 5 PRs. Most were adopted; a few
were declined as stale or wrong-premise. Notable
adopt-and-extend cases:

- **#253 round 7** revealed that the `Runtime::reset` snapshot
  must DROP the cached singleton shell (set to None on
  restore) rather than preserve its Rc, because preserving the
  Rc preserves its internal RefCells.
- **#253 round 5** GC walk had to grow to include both
  `cls.singleton_methods` and the eigenclass shell's tables —
  closure-method captures could be swept under STRESS_GC.
- **#267 round 1** caught that the Kernel-fallback's result
  push had to honor `suppress_call_result_push` (the rescue-
  unwind flag used by `eval`); unconditional push corrupted
  the handler's stack under specific eval+rescue shapes.

These cross-cutting concerns (snapshot ownership, GC roots,
stack-flag invariants) consistently surfaced as
second-iteration findings. The methodology of "land the
direct fix → /copilot-loop iterates → /code-review one more
sweep" caught all of them before merge.

## Suggested next probe targets

When pass-10 begins, candidate probes worth attempting:

- **tilt** — template engine, already partially loadable. The
  metaprogramming layer-#23 fix removed a major historical
  blocker.
- **Rack::MockRequest** end-to-end — exercise sinatra's
  `get '/' do ... end` DSL by faking a request via
  MockRequest. Requires full Rack autoload chain stubbing.
- **minitest / Spec runner** — small test framework, broadly
  used. Useful as a Tier-1 acceptance harness.
- **dry-rb / hanami-utils** — pure-Ruby utility libraries
  with no socket / IO requirements; well-suited to Tier-1
  testing.
