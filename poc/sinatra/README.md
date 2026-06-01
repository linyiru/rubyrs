# PoC: the same Sinatra app on CRuby **and** rubyrs

> **Status (2026-06):** superseded as a CI gate by the M27 D parity
> harness — see [`crates/rubyrs/tests/diff_framework/fixtures/sinatra_hello/`](../../crates/rubyrs/tests/diff_framework/fixtures/sinatra_hello/).
> That fixture runs the same 18-route matrix on every PR via the
> `framework-parity` CI job. This PoC stays in-tree as the readable
> walkthrough + GAPS.md investigation log; new gap discoveries should
> still land here, but the source of truth for "did parity regress?" is
> the diff_framework fixture.


**Goal (from the brief):** one `app.rb` that runs unmodified on CRuby —
backed by the real `sinatra` gem — *and* on rubyrs, backed by a vendored
micro-Sinatra running on rubyrs's native `_http_server` battery. Same
source, two engines, identical HTTP responses.

This is a **proof of concept**, not real Sinatra hosting. rubyrs is not a
gem host (see [`docs/SUBSET.md`](../../docs/SUBSET.md)); the point is to
show that the *language surface and the request→route→response pipeline*
are already good enough to express a Sinatra-shaped framework with the
**same application code** on both runtimes, and to enumerate exactly
what's still missing (see [`GAPS.md`](GAPS.md)).

## Files

| File | Role | Runs on |
|---|---|---|
| `app.rb` | The application. **Byte-identical** on both runtimes; never branches on the engine. | both |
| `sinatra_compat.rb` | The **only** runtime-aware file. Picks who provides `Sinatra::Base`. | both |
| `vendor/sinatra_lite.rb` | A ~120-line `Sinatra::Base` subset on the `_http_server` battery. | rubyrs only |
| `verify.sh` | Runs `app.rb` on both engines, curls the same routes, diffs the responses. | harness |
| `GAPS.md` | Every divergence / unsupported feature found, with analysis. | docs |

How the runtime is selected (the whole trick), in `sinatra_compat.rb`:

```ruby
if defined?(__rubyrs_http_serve_with_app)   # rubyrs-only host fn (ADR 0022)
  require_relative "vendor/sinatra_lite"    # micro-Sinatra
else
  require "sinatra/base"                     # the real gem
end
```

(We cannot use `RUBY_ENGINE` — rubyrs reports `"ruby"`. See GAPS.md #4.)

## Run it

```bash
# CRuby + real Sinatra
ruby poc/sinatra/app.rb
# rubyrs micro-Sinatra (build first)
cargo build -p rubyrs --features _http_server,_fiber
target/debug/rubyrs poc/sinatra/app.rb
```

Then, against either:

```bash
curl -s http://127.0.0.1:9292/                              # before-filter ivar
curl -s http://127.0.0.1:9292/hello/world                   # path param
curl -s 'http://127.0.0.1:9292/hello/%3Cb%3E'               # path param + HTML escape
curl -s 'http://127.0.0.1:9292/search?q=hello+world&limit=5' # query params (decoded)
curl -s http://127.0.0.1:9292/say/cats/to/dogs              # splat params
curl -s -i http://127.0.0.1:9292/admin                      # halt 403
curl -s -i http://127.0.0.1:9292/old                        # redirect 302 + Location
curl -s -i http://127.0.0.1:9292/teapot                     # custom status 418
curl -s -X POST -H 'Content-Type: text/plain' --data ping \
     http://127.0.0.1:9292/echo                             # reads rack.input body
curl -s http://127.0.0.1:9292/no-such-route                 # custom 404
```

## Automated parity check

```bash
poc/sinatra/verify.sh
```

It boots `app.rb` under each interpreter, hits the same five routes, and
diffs. Current result:

```
✅ IDENTICAL (modulo the self-reported runtime name): the same app.rb
   produced byte-identical responses on CRuby+Sinatra and rubyrs.
```

The one intentional difference is `SERVER_BACKEND` — each engine reports
its own name (`CRuby + Sinatra 4.2.1` vs `rubyrs micro-Sinatra …`). That
is the *proof* two different engines ran the same file; the parity check
normalizes it. 404 bodies are compared by status only (real Sinatra ships
styled error pages).

## What this proves — and what it doesn't

**Proves:** a real-shaped Sinatra modular app — `class App <
Sinatra::Base` with `set :environment`, `before` filters, `get/post/put`,
path params, **query params**, **form-body params**, **splat** routes,
the **`request`** object (headers), instance helpers, `halt`, `redirect`
(absolute `Location`), `content_type`/`status`, **request-body reading
via `rack.input`**, **`error <Class>` handlers**, and a custom
`not_found` — 14 routes, runs on the Rack `[status, headers, body]`
contract over rubyrs's hyper-based `_http_server` battery, with the
application source identical to a real-Sinatra deploy.

**Engine fixes this surfaced and drove** (all landed + regression-tested,
see [`GAPS.md`](GAPS.md)):
- #1 non-local `return` from an iterator block across the Rust→Ruby call
- #2 `rack.input` StringIO wiring (request bodies)
- #11 exception unwind to an in-scope `rescue` across that same boundary
  (what makes `halt`/`redirect` work)

**Still open / worked around in the micro-Sinatra:** `throw`/`catch`
(#8), `String#split(sep, limit)` (#9), and the higher-impact
**`rescue <ShortName>` not resolving a module-nested constant (#10)** —
that last one breaks every gem that rescues its own namespaced errors and
is the strongest candidate for the next engine fix.

**Does not attempt:** hosting the *actual* Sinatra gem, sessions,
templates (ERB/Tilt), or middleware.
