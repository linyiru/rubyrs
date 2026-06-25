# sinatra-fast — B1 Phase 1.5 (lean-dispatch shim)

A pure-Ruby shim that speeds up Sinatra request dispatch on rubyrs without
touching observable behaviour, plus a parity test that proves it.

Phase-1.5 deliverable of the B1 "native framework core" lever (see memory
`b1-native-framework-validated`). ~99% of a Sinatra request is framework
plumbing (the route block is ~0µs). An earlier route!-only override
recovered only the mustermann loop (~1.2×); a precise per-component
ablation showed the dispatch generality is much larger, and a *faithful*
reimplementation recovers ~2×.

## What it does

`lean_dispatch.rb` replaces `Sinatra::Base#call!` for an app's own
`static` / `:param` routes (no conditions) with a lean reimplementation
that REUSES Sinatra's own `invoke` / `filter!` / `handle_exception!` /
`error_block!` / `content_type` / `Response#finish` — so behaviour can't
change, only speed. Versus stock `call!` → `dispatch!` → `route!` it:

1. native-segment-matches the one route instead of the mustermann route!
   loop (+ its per-route content-type reset, param-revert, superclass
   recursion, route_missing);
2. collapses call!'s `invoke { dispatch! }` + dispatch!'s inner `invoke`
   into a single `invoke`;
3. skips the `@request.params` parse (~13µs) when there are demonstrably no
   params (empty QUERY_STRING + no body — `@request.params` would be `{}`);
4. skips `filter!` / `error_block!` when the app (walking the superclass
   chain) has zero filters / zero error handlers — both are no-ops then;
5. inlines `Response#finish` for the common non-drop-body case.

Anything it can't match (splat `*`, regexp, conditioned routes, or a
complex route defined before the match) falls back to the real `call!`.

Order safety: routes are captured in definition order; the matcher bails to
the fallback the moment it reaches an ineligible route before a match, so a
later eligible route can never win over an earlier complex one (Sinatra is
first-match-wins). Inherited filters/error handlers are honoured (the no-op
flags walk the superclass chain, like Sinatra's own `filter!`/`error_block!`).

## Results (rubyrs, sinatra-4.2.1)

`parity_test.rb` runs 21 request shapes through the same app with the shim
enabled vs disabled and asserts `[status, headers, body]` are
byte-identical **and** that the fast path was actually taken on eligible
routes / fell back on the rest:

```
PARITY: PASS (21/21)
  static  full=138.3  fast= 71.6  1.93x
  param   full=180.7  fast= 93.0  1.93x
  json    full=265.5  fast=186.6  1.42x
```

Covered: static, `:param` (params-style + block-arg-style), multi-param,
query params, `content_type`, explicit `status`, `redirect`, `halt`
(string and `[status,headers,body]`), bare array return, a raising route
(→500), a raising route with a custom `error` handler (→422), HEAD, 404,
condition-guarded route (provides), splat, regexp, the
ineligible-before-eligible order case, **and inherited before-filter +
inherited error handler on a subclass route**.

## Honest decomposition / where the win is

The ~1.2× figure from the earlier route!-only override was misleading — it
recovered only the mustermann loop. A precise per-component ablation of a
static request (sessionless, `full ≈ 135µs`) shows where the time actually
is, and how much is **faithfully** recoverable in pure Ruby:

| component | cost | recovered by |
|---|---|---|
| route! loop + double-`invoke` nesting | ~42µs | faithful call! replacement (→1.65×) |
| `@request.params` parse (empty query) | ~13µs | skip when no params (→2.05×) |
| `filter!` with zero filters | ~5µs | skip when app has none (chain-aware) |
| `error_block!` probe, no handlers | ~5µs | skip when app has none (chain-aware) |
| `Response#finish` predicates | ~2µs | inline common case |
| Request.new + Response.new + dup | ~7µs | **not recoverable in Ruby** (Phase 2) |
| `invoke` catch(:halt) + route block + finish content-length | ~35µs | **irreducible Ruby method-call cost** |

So the **faithful pure-Ruby ceiling is ~1.9× static/param, ~1.4× json**
(json does real `to_json` work, so less of it is plumbing). NOT 1.2× (the
route!-only undershoot) and NOT 3–4× (finish/dup turned out small). The
remaining ~45µs is genuine Rack object construction + the interpreter's
~5–9×/call floor.

- **~10–20× needs native-backed lazy `Request`/`Response` (Phase 2):** the
  remaining per-request object construction + the per-call interpreter
  floor are recoverable only in Rust. This is the multi-quarter part (big
  Rack::Request API surface) and is the actual B1 lever.
- Floor is ~2.3µs (interpret the route block + wrap), matching bare Rack.

Net: this shim is a faithful ~1.9× foundation + the parity gate. The next
real step is Phase 2 (native Request/Response).

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
