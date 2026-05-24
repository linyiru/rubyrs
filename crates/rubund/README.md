# rubund

A Rust implementation of [Bundler](https://bundler.io/) — the
Ruby dependency manager. Lives in the same workspace as
[`rubyrs`](../rubyrs/), which it uses to evaluate `Gemfile` and
`*.gemspec` files (both are Ruby DSLs).

## Status

**Placeholder.** This crate exists today only to lock in the
workspace wiring. The binary understands `--version`, `--help`,
and `--demo` (which evaluates a one-liner through the embedded
`rubyrs::Runtime` to prove the dependency is alive). No actual
Bundler features are implemented yet.

```sh
cargo run -p rubund -- --demo
# rubund 6 — the interpreter is wired up.
```

## Why in-tree

rubund is the first non-test consumer of rubyrs's embedding API
(`Runtime`, `register_fn`, `set_stdout`, `eval`). Keeping it in
the same workspace means every breaking change to that API
surfaces immediately as a rubund build failure — turning the
"will anyone actually use this?" question into a daily
yes/no signal instead of a hypothetical.

The strategy is dogfooding: rubyrs needs a real driver to expose
the gaps in its language subset and host API; rubund needs a
small embedded Ruby evaluator to make sense as a Rust binary at
all.

## Planned milestones

These are stubs — the actual scope and order will firm up
as work begins.

1. **Gemfile parser** — register Bundler DSL methods (`source`,
   `gem`, `group`, `gemspec`, `git`, `path`) on a rubyrs
   Runtime, evaluate `Gemfile`, collect the dependency list.
2. **`Gemfile.lock` reader** — parse the lockfile format
   (deterministic; not Ruby).
3. **`bundle check`** — verify the lockfile against the
   declared dependencies. Read-only; no network, no fetch.
4. **Dependency resolver** — port the version-selection algorithm
   off of Bundler's resolver behaviour.
5. **`bundle install`** — fetch gems and stage them. This is
   where the Rust-side work pays off: parallel fetch + verified
   downloads in a single static binary.

See [`docs/ROADMAP.md`](../../docs/ROADMAP.md) at the workspace
root for the integrated rubyrs + rubund plan.
