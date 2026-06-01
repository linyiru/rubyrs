# Sinatra PoC strategy — moved

This note has been promoted to a first-class architectural decision:
[ADR 0026 — Omakase blessed-gem menu](../../docs/adr/0026-omakase-blessed-gem-menu.md).

The ADR captures the strategy in its enforceable form (parity-by-test
gate, honest `LoadError` on miss, named resolution via
`require "rubyrs/<name>"`, the published menu + parity %); the rationale
and the original PoC framing live there.

What still lives in this directory:

- `README.md` — how to run the verify harness.
- `GAPS.md` — the gap log discovered while building the PoC. Engine
  gaps fixed in PR #315 are marked ✅; remaining ones are tracked here
  as the live to-do list.
- `app.rb` + `sinatra_compat.rb` + `vendor/sinatra_lite.rb` — the
  byte-identical Sinatra app + the runtime shim + the vendored
  micro-Sinatra used on rubyrs.
- `verify.sh` — the parity harness CI will eventually wire into
  `diff_cruby` (the ADR's compatibility-contract rule #1).
