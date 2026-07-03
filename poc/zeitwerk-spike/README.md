# zeitwerk spike — self-test harness on rubyrs

Runs the **zeitwerk 2.8.2 self-test suite** under rubyrs as a parity
measuring stick. zeitwerk is the autoloader under Rails / Hanami /
Bridgetown and most modern gems, so its own test suite is a bounded,
quantifiable target for "does rubyrs run a real metaprogramming-heavy
gem."

## Status (2026-07-01b): full self-test = 520/520, DETERMINISTIC

The zeitwerk 2.8.2 self-test now passes **520/520** (48/48 files
fully green) deterministically — `run_all.sh` is a stable 520/520.

The previously-noted "intermittent 518/520" was **NOT** minitest
seed flakiness (the earlier diagnosis in this README was wrong —
rubyrs runs minitest at a fixed `seed 0`, so the two failures were
100% reproducible). They were two real, independent
constant-resolution bugs, both fixed this round:

1. **own-scope autoload lost to a superclass const**
   (`test_explicit_namespace` "same cname in the superclass"). For
   `Sub < Base` where `Base::X` is defined and `Sub::X` is autoloaded,
   `Sub::X` wrongly resolved to `Base::X`. `resolve_const_path`'s
   direct lookup for the start scope probed only *loaded* consts and
   skipped the pending `Sub::X` autoload, so the ancestor walk fired
   `Base`'s `X` first. Fix: fire the start scope's own pending scoped
   autoload before the ancestor-chain fallback (the start-scope twin of
   the existing nearer-ancestor firing). Also fixes `Sub.const_get(:X)`.

2. **`const_defined?` accepted a malformed uppercase name**
   (`test_cpath_expected_at` "does not yield a constant name", via
   zeitwerk's `ConstantPathValidator#validate!` →
   `Module.new.const_defined?(cname, false)`). The never-interned
   fast-undefined path gated only on `starts_with(uppercase)`, so an
   invalid name like `"Foo-bar"` returned `false` instead of raising
   `NameError: wrong constant name`. Fix: gate that path on the full
   `is_valid_const_name` check.

Regression tests (both fail without the fix, verified):
`crates/rubyrs/tests/diff/{autoload_own_scope_beats_superclass,const_defined_invalid_uppercase_name}.rb`.

(Also fixed same turn, surfaced while debugging #2 but not on the
zeitwerk path: `Module#freeze` / `Class#freeze` were a no-op — a
Class/Module IS an object, so `mod.freeze` must make `mod.frozen?`
report true. Added a `frozen: Cell<bool>` to `struct Class` and wired
the freeze/frozen? dispatch arms (dup drops the flag, clone keeps it,
matching CRuby). Regression test `crates/rubyrs/tests/diff/module_class_freeze.rb`.
Mutation-guard — FrozenError on `def`/const-set to a frozen module —
is deferred, matching the existing flag-only Object freeze.)

## Status (2026-07-01): full self-test = 520/520 (per-file)

The zeitwerk 2.8.2 self-test passes **520/520** when files are run
independently. Two real rubyrs constant-resolution bugs were fixed this
round (commit `5be79f47`): a subclass's own `autoload(:X)` being shadowed by
a parent const propagated onto its flat `Sub::X` key, and `const_defined?`
not raising `NameError` on syntactically-invalid names. Regression tests live
in `crates/rubyrs/tests/diff/{autoload_subclass_same_cname,const_defined_invalid_name}.rb`.

## Status (2026-06-15)

`require "zeitwerk"` loads end to end, and **basic file autoload works**
(`loader.setup` + referencing an autoloaded constant loads its file):

```
loader = Zeitwerk::Loader.new
loader.push_dir("lib"); loader.setup
MyThing.new          # => autoloads lib/my_thing.rb
```

What made this possible (committed to master, each with a diff_cruby
test):

- **real eigenclass-body execution** for `class << expr` (the campaign
  crux — `Op::OpenSingletonClass`, self = the metaclass): unblocked
  zeitwerk's `internal def` / `include`-into-metaclass shapes.
- **Hash#compare_by_identity** — zeitwerk's `Cref::Map`.
- **singleton-alias of a Kernel builtin** (`class << self; alias_method
  :zeitwerk_original_require, :require`) — let core_ext/kernel.rb load.
- **`$LOADED_FEATURES` / `$"` as a real Array** — zeitwerk's require
  wrapper reads `.last`; its unload path calls `.reject!` (this fired
  in every test's teardown before the fix).

### Self-test tally

`bash run_all.sh`:

```
test files: 48 (ran 48, fully-green 5)
520 runs, 373 assertions, 34 failures, 382 errors
passing: 104 / 520
```

All 48 files **load and run** (the harness is complete). The ~382
errors are dominated by a small number of real walls, not harness
noise:

| wall | ~weight | owner |
|------|---------|-------|
| **require respects user `Kernel#require` override** (directory autovivification: `Module#autoload(:Ns, "<dir>")` → native require hits a dir → `Is a directory`; and `uninitialized constant Ns::X`) | the bulk | the require-override campaign (`fire_pending_autoload`, dispatch.rs) |
| `TracePoint` absent (explicit-namespace descent) | medium | separate |
| `Object#const_source_location`, `Kernel#caller_locations` | ~50 | small contained gaps |
| `no block given (yield)` cluster | ~46 | unanalyzed |
| `File.split`, `FileUtils.ln_s`, `Set#replace` | ~20 | small stdlib gaps |

The dominant wall (require-override) is the shared blocker with
Bridgetown's zeitwerk usage; once it lands, the autovivification /
eager-load / reloading families should swing green.

## Layout

- `harness/` — minimal stubs for test-helper gems rubyrs/this-env
  don't provide (`minitest/focus`, `minitest/proveit`,
  `minitest/reporters` → no-op, `warning`, `pp` → `pretty_inspect`).
  minitest itself runs as the real 5.25.4 gem, zero-shim.
- `run.rb` — per-file runner (rubyrs has no `-I`; load paths arrive via
  the `ZW_LOADPATH` env var).
- `run_all.sh` — clones the zeitwerk source repo (the gem ships no
  tests), builds the load path, runs every `test_*.rb`, tallies.

## Running

```sh
# rubyrs MUST be built with the stdlib feature — zeitwerk needs the
# real Set / ERB / etc., which are feature-gated (ADR 0017).
cargo build --release -p rubyrs --bin rubyrs --features stdlib
bash poc/zeitwerk-spike/run_all.sh
```

Paths (rubyrs binary, zeitwerk gem lib, minitest 5.25.4 lib, source
repo) auto-detect; override via `RUBYRS` / `ZWLIB` / `MT` / `ZW_SRC`.
