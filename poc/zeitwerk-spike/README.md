# zeitwerk spike — self-test harness on rubyrs

Runs the **zeitwerk 2.8.2 self-test suite** under rubyrs as a parity
measuring stick. zeitwerk is the autoloader under Rails / Hanami /
Bridgetown and most modern gems, so its own test suite is a bounded,
quantifiable target for "does rubyrs run a real metaprogramming-heavy
gem."

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
