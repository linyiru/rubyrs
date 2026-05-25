# Gemfile demo

A real-shape Gemfile, hosted by rubyrs, **unmodified**. Same byte
content you'd check into a Ruby project. End-to-end runtime is in
the low milliseconds — same headline number `examples/brewfile`
has held since the README's first table.

## What this demonstrates

The Brewfile demo proved rubyrs can host a four-method
`tap` / `brew` / `cask` / `mas` DSL. Gemfile demo is the next
step up: every shape a typical Rails-style Gemfile uses, with no
script-side tweaks.

| Gemfile shape | What lights up |
|---|---|
| `gem "rake"` | bare host fn call |
| `gem "rails", "~> 8.0.0"` | single positional + version constraint |
| `gem "rack", ">= 3.0", "< 4.0"` | `*splat` receive on the Ruby side, joined for the host |
| `gem "puma", require: false` | `**kwargs` Hash receive |
| `gem "x", "~> 7.3", require: "x", platforms: :mri` | full mix |
| `group :development, :test do ... end` | multi-symbol block, `ensure`-balanced scope pop |
| `platforms :mri do ... end` | same block shape as `group`, different scope stack |
| `git "url" do ... end`, `path "vendor/cache" do ... end` | source-override scope; push-order wins on nesting |
| `if RUBY_VERSION >= "3.4.0" ... end` | file-scope conditional + constant comparison (positive branch) |
| `if RUBY_VERSION >= "99.0.0" ... end` | same conditional, falsy branch (the gem must NOT appear) |

The Gemfile is at `Gemfile` (no `.rb` extension — Bundler's
convention). The host-side DSL hooks are in `../gemfile.rs`.
The Ruby-side prelude that bridges Bundler's surface to the
host is in `dsl_prelude.rb`.

## Architecture

```
                                    ┌──────────────────────────────┐
   Bundler-shape Gemfile  ─eval─►   │ dsl_prelude.rb (preloaded)   │
   (unmodified)                     │  - source / ruby / gem       │
                                    │  - group / platforms /       │
                                    │    git / path block helpers  │
                                    │  - RUBY_VERSION constant     │
                                    └────────────┬─────────────────┘
                                                 │ rebuilds **opts
                                                 │ with String keys
                                                 │ + values; joins
                                                 │ block-scope args
                                                 ▼
                                    ┌──────────────────────────────┐
                                    │ Rust host (gemfile.rs)       │
                                    │  - __gemfile_gem_v2 (v2):    │
                                    │      name, *reqs Array,      │
                                    │      **opts Hash             │
                                    │  - __gemfile_source / _ruby  │
                                    │      / push_* (v1, String)   │
                                    │  - accumulates GemfileState  │
                                    └──────────────────────────────┘
```

The seam between Bundler's public DSL (`gem`, `group`, etc.) and
the host lives entirely in the prelude. The `gem` shim uses v2
(Array + Hash flow through unwrapped); the scope-stack shims use
v1 (single String per push). The Gemfile itself never sees the
seam.

## Run it

```bash
cargo run --release --example gemfile
```

Sample output:

```
Collected Gemfile contents:
  source:        https://rubygems.org
  ruby version:  3.4.0
  gem count (unique): 18

  [default] 10 gem(s):
    - rake
    - rails (~> 8.0.0)
    - rack (>= 3.0, < 4.0)
    - puma    [require: false]
    - pry-byebug    [require: pry-byebug, platforms: mri]
    - csv
    - sidekiq (~> 7.3)    [require: sidekiq, platforms: mri]
    - rb-readline    [platforms-scope: mri]
    - forked-gem    [git: https://github.com/example/forked-gem.git]
    - vendored-gem    [path: vendor/cache]

  [development] 6 gem(s):
    …
  [test] 5 gem(s):
    …

rubyrs ran the unmodified Gemfile in 0.20 ms
```

`gem count (unique)` counts each gem once; the per-group
sub-headers sum to more than that whenever a gem belongs to
multiple groups (e.g. `rspec-rails` is in both `:development`
and `:test`).

## Lock-in test

The same prelude + Gemfile + a minimised host run inside
`tests/embed.rs::gemfile_dsl_real_hosting_end_to_end`. Any
regression in `*splat` / `**kwargs` / block-yield scope / file-
scope conditional / String constant comparison fails that test,
not just the unobservable example binary.

## Host-fn API: v1 vs v2 in this demo

This example mixes both flavours of the host-fn API. The
`__gemfile_gem_v2` host fn uses **v2**
([`register_fn_v2`](../../src/lib.rs) + `HostCtx`), receiving
the `*splat` as a real Array and `**kwargs` as a real Hash.
`HostCtx::resolve_array` / `resolve_hash` borrow directly
from the VM heap with no clone. All splat / kwarg unpacking,
per-key filtering, value typing, and requirement parsing
lives in typed Rust:

```rust
rt.register_fn_v2("__gemfile_gem_v2", move |ctx: &HostCtx, args| {
    let [name, requirements, opts] = args else {
        return Err(arg_err("expected 3 args"));
    };
    // Fail fast on wrong shapes — never copy-paste `unwrap_or(&[])`
    // here. A prelude regression sending a non-Array/Hash should
    // surface as ArgumentError immediately, not as silent partial
    // state collected several gem decls later.
    let reqs_slice = ctx.resolve_array(requirements)
        .ok_or_else(|| arg_err("requirements must be an Array"))?;
    let opts_slice = ctx.resolve_hash(opts)
        .ok_or_else(|| arg_err("opts must be a Hash"))?;
    // ... iterate, validate every element/entry is Value::Str,
    //     populate GemDecl. See `gemfile.rs` for the full pattern.
});
```

The scope-stack helpers (`group` / `platforms` / `git` /
`path`) stay on **v1** ([`register_fn`](../../src/lib.rs))
because their natural input is already a single String — the
prelude joins multi-symbol args (`group :a, :b`) into one
String before pushing, and the host pops one String off the
stack. No heap shape, no v2 benefit.

### One remaining prelude transform

The prelude still does ONE Ruby-side rebuild:

```ruby
def gem(name, *requirements, **opts)
  stringified = {}
  opts.each { |k, v| stringified[k.to_s] = v.to_s }
  __gemfile_gem_v2(name, requirements, stringified)
end
```

The `opts.each` loop rebuilds the kwargs hash with String
keys + String values. `HostCtx` borrows the heap but NOT the
interner, so a `:require` Symbol key has no host-side path to
its name. Closing this fully would require a
`HostCtx::resolve_sym(&Value) -> Option<&str>` (interner
widening); deferred. For most DSLs the one-line `.each`
rebuild is acceptable.
