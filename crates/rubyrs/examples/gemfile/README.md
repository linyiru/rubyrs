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
| `if RUBY_VERSION >= "3.4.0" ... end` | file-scope conditional + constant comparison |

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
                                                 │ unpacks *splat /
                                                 │ **kwargs / block
                                                 │ scope down to
                                                 │ plain Strings
                                                 ▼
                                    ┌──────────────────────────────┐
                                    │ Rust host (gemfile.rs)       │
                                    │  - __gemfile_source          │
                                    │  - __gemfile_gem (name,      │
                                    │      reqs, req_kw, plat_kw)  │
                                    │  - __gemfile_push_groups …   │
                                    │  - accumulates GemfileState  │
                                    └──────────────────────────────┘
```

The seam between Bundler's public DSL (`gem`, `group`, etc.) and
the host's flat `&[Value]` API lives entirely in the prelude.
The Gemfile never sees it.

## Run it

```bash
cargo run --release --example gemfile
```

Sample output:

```
Collected Gemfile contents:
  source:        https://rubygems.org
  ruby version:  3.4.0
  gem count:     15

  [default] 7 gem(s):
    - rake
    - rails (~> 8.0.0)
    - rack (>= 3.0, < 4.0)
    - puma    [require: false]
    - pry-byebug    [require: pry-byebug, platforms: mri]
    - csv
    - sidekiq (~> 7.3)    [require: sidekiq, platforms: mri]

  [development] 6 gem(s):
    …
  [test] 5 gem(s):
    …

rubyrs ran the unmodified Gemfile in 0.42 ms
```

## Lock-in test

The same prelude + Gemfile + a minimised host run inside
`tests/embed.rs::gemfile_dsl_real_hosting_end_to_end`. Any
regression in `*splat` / `**kwargs` / block-yield scope / file-
scope conditional / String constant comparison fails that test,
not just the unobservable example binary.

## Host-fn API takeaway (for future embed work)

The current `Runtime::register_fn` API hands the closure a
`&[Value]` and no `&Heap` / `&Runtime`. That means heap-y
shapes — `Value::Array` from `*splat`, `Value::Hash` from
`**kwargs` — can't be read from inside the closure (their
inner contents live in the heap, which the closure can't
touch).

The pattern this example uses: **do the unpacking in the Ruby-
side prelude** (one short shim per public DSL entry) and pass
plain positional `String` / `Int` / `Bool` to the host. That
keeps each host fn ~5 lines and avoids needing intimate `Heap`
access. The prelude is the seam — the Gemfile is unmodified.

If the project later wants to support truly-arbitrary Ruby
DSLs without prelude shims, the natural extension is a
`register_fn_v2` that also takes a `&Runtime` (so
`resolve_array` / `resolve_hash` are reachable). Out of scope
for this example, but the gap is concrete and worth noting in
the next embed-API ADR.
