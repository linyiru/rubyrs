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

`parity_test.rb` runs 19 request shapes through the same app with the shim
enabled vs disabled and asserts `[status, headers, body]` are
byte-identical **and** that the fast path was actually taken on eligible
routes / fell back on the rest:

```
PARITY: PASS (19/19)
  static  full=135.9  fast=115.0  1.18x
  param   full=173.3  fast=136.6  1.27x
  json    full=260.1  fast=227.7  1.14x
```

Covered: static, `:param` (params-style + block-arg-style), multi-param,
query params, `content_type`, explicit `status`, `redirect`, `halt`
(string and `[status,headers,body]`), bare array return, **a raising route
(→500)**, **a raising route with a custom `error` handler (→422)**, HEAD,
404, condition-guarded route (provides), splat, regexp, and the
ineligible-before-eligible order case.

## Honest decomposition / where the win is

- **This shim (route!-override): ~1.2×, fully safe AND faithful.** It
  reuses all of Sinatra's call!/dispatch!/invoke semantics — including the
  rescue/ensure that handles raised routes, custom `error` blocks, and
  after-filters-on-error (covered: `raise-500`, `custom-error`). It only
  short-circuits the mustermann route! loop, a *small* fraction → ~1.2×.
- **A `call!` replacement does NOT cleanly buy more.** An earlier PoC that
  reimplemented a lean call! measured ~2.0–2.4× — but it was UNFAITHFUL:
  it skipped dispatch!'s rescue/ensure, so it only worked on the happy
  path and would diverge on error / custom-error / after-on-error routes
  (the cases this test now covers). A *faithful* call! replacement has to
  add that machinery back, which collapses the win toward ~1.2×. So **~1.2×
  is the safe Ruby ceiling** for dispatch; pure-Ruby has little more to give.
- **~10–20× needs native-backed lazy `Request`/`Response` (Phase 2):** the
  ~50µs of per-request object construction (Request/Response/IndifferentHash
  + response.finish) is recoverable only in Rust. This is the multi-quarter
  part (big Rack::Request API surface) and is the actual B1 lever.
- Floor is ~2.3µs (interpret the route block + wrap), matching bare Rack.

Net: Phase 1 (this shim) is a safe ~1.2× foundation + the parity gate. The
next real step is Phase 2 (native Request/Response), not a fancier Ruby
dispatch.

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
