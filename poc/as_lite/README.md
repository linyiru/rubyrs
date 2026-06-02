# PoC: ActiveSupport-lite core-ext spike

> **Status (2026-06):** spike only. No `active_support_lite.rb`
> canon ships yet — that lands in a follow-up commit driven by
> the gap inventory below.

This directory mirrors `poc/sinatra/` but for ADR 0026 v2's
menu item 3 (ActiveSupport-lite core-ext slice). The same
"run-on-both-runtimes-and-diff" probe pattern, applied to the
`blank?` / `present?` / `Hash#deep_*` / `String#camelize` family
real Rack apps reach for.

## Files

| File | Role |
|---|---|
| `spike.rb` | The probe. Wraps each ActiveSupport idiom in `probe(label, &block)` so the script runs to completion under both runtimes and prints a labelled `[OK]` / `[GAP]` line per idiom. |
| `compat.rb` | One runtime-aware file. `require "active_support/all"` on CRuby; no-op on rubyrs (detected via the `RUBYRS` sentinel, ADR 0026 v2 M27 B2). |
| `GAPS.md` | Gap inventory grouped by tier (trivial pure-Ruby / Regexp-dependent / Hash transforms / DEFERRED Duration+TZ). Drives the next commits' shape. |

## Run it

```bash
# CRuby + active_support (gem installed)
ruby poc/as_lite/spike.rb

# rubyrs stock (no canon yet — every line should `[GAP]`)
cargo build --release -p rubyrs
target/release/rubyrs poc/as_lite/spike.rb
```

Diff the `[OK]` / `[GAP]` lines side-by-side; the rubyrs-side
gaps are exactly what `src/stdlib_vendor/active_support_lite.rb`
will need to cover.

## Spike outcome (2026-06)

- **Zero VM-level gaps surfaced.** Every miss is a missing
  pure-Ruby method on a built-in class — exactly the shape
  the existing `stdlib_vendor` module is designed to host.
  Contrast M27 D's Sinatra spike, which drove 7 VM batches
  before the canon could ship.
- **Tier-D defer call**: Duration / TimeZone helpers
  (`Numeric#minutes`, `Time.current`, `1.day.from_now`) are
  out of scope for this menu item. They chain through a real
  `ActiveSupport::Duration` value type + tzinfo DB; matching
  the surface without matching the implementation has
  nontrivial parity risk. Defer to a follow-up ADR (provisional
  0028) once a real consumer surfaces.
- **Total practical scope**: ~170 LOC pure-Ruby canon
  (Tier A + B + C) + ~50 LOC parity fixture. Three atomic
  commits, ~1 work day. Significantly under ADR 0026 v2's
  3–5 day estimate (which budgeted for the full slice
  including Duration).

See `GAPS.md` for the per-method inventory + next-commit
ordering.
