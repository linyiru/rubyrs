# sinatra-fast — B1 Phase 1 (lean-dispatch shim)

A pure-Ruby shim that speeds up Sinatra request dispatch on rubyrs without
touching observable behaviour, plus a parity test that proves it.

This is the **Phase 1** deliverable of the B1 "native framework core" lever
(see memory `b1-native-framework-validated`). The validation gate for B1
PASSED: ~99% of a Sinatra request is framework plumbing (the route block is
~0µs), and a lean dispatch recovers a real chunk of it while staying
byte-identical to Sinatra.

## What it does

`lean_dispatch.rb` overrides **only** `Sinatra::Base#route!` — the
mustermann route-finding loop. For an app's own `static` / `:param` routes
(no conditions) it native-segment-matches the path and runs the matched
route through Sinatra's **unchanged** `route_eval` → `invoke` path. Every
other semantic — `call!`, `dispatch!`, before/after filters, `halt`/`pass`,
`redirect`, explicit status, `[status, headers, body]` returns, error
handling, the response build, sessions/middleware — is Sinatra's own code,
so the shim *cannot* change behaviour, only speed. Anything it can't match
(splat `*`, regexp, conditioned routes, or a complex route defined before
the match) falls back to the real mustermann `route!`.

Order safety: routes are captured in definition order; the matcher bails to
the fallback the moment it reaches an ineligible route before a match, so a
later eligible route can never win over an earlier complex one (Sinatra is
first-match-wins).

## Results (rubyrs, sinatra-4.2.1)

`parity_test.rb` runs 17 request shapes through the same app with the shim
enabled vs disabled and asserts `[status, headers, body]` are
byte-identical **and** that the fast path was actually taken on eligible
routes / fell back on the rest:

```
PARITY: PASS (17/17)
  static  full=135.9  fast=115.0  1.18x
  param   full=173.3  fast=136.6  1.27x
  json    full=260.1  fast=227.7  1.14x
```

Covered: static, `:param` (params-style + block-arg-style), multi-param,
query params, `content_type`, explicit `status`, `redirect`, `halt`
(string and `[status,headers,body]`), bare array return, HEAD, 404,
condition-guarded route (provides), splat, regexp, and the
ineligible-before-eligible order case.

## Honest decomposition / where the win is

- **This shim (route!-override): ~1.2×, fully safe.** It reuses all of
  Sinatra's call!/invoke semantics, so it can't diverge — but it only
  recovers the mustermann route! loop, which is a *small* fraction.
- **~2× needs a `call!` replacement (Phase 1b):** a separate PoC that
  reimplemented the lean call! (skipping dispatch!/invoke nesting, leaner
  param handling) measured **2.0–2.4×** and stayed byte-identical — but it
  reimplements more of Sinatra, so it must be gated by this parity test
  before shipping. Still reuses Ruby `Request`/`Response`.
- **~10–20× needs native-backed lazy `Request`/`Response` (Phase 2):** the
  ~50µs of per-request object construction is recoverable only in Rust.
  This is the multi-quarter part (big Rack::Request API surface).
- Floor is ~2.3µs (interpret the route block + wrap), matching bare Rack.

## Run

```
RUBYRS_NO_PREAMBLE_CACHE=1 rubyrs poc/sinatra-fast/parity_test.rb
```

Needs the real `sinatra` (and `mustermann`, `rack`, `tilt`, …) gems on a
rubygems-free `$LOAD_PATH`; `setup_load_path.rb` wires them from
`$SINATRA_FAST_GEMS` / `$GEM_HOME` / the rbenv default. NB: do **not**
vendor-skip `uri` — mustermann needs the real `uri` gem.

## Notes / known orthogonal gaps (not the shim)

- `enable :sessions` trips rubyrs's `OpenSSL::Cipher#iv_len=` gap in
  rack-session's encryptor; sessions live above `route!` and are unaffected
  by the shim, so the parity app doesn't enable them (to keep the gate's
  exit code clean).
- rubyrs surfaces `exit(0)` itself as an uncaught `SystemExit` and returns
  1; the test therefore only calls `exit`/`abort` on a real failure.
