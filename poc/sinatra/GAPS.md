# Sinatra-on-rubyrs PoC — gap log

Every place where rubyrs did **not** behave like CRuby, or did not yet
support something the PoC needed, recorded with a reproduction, the
probable cause, and a preliminary analysis. Severities are from the
point of view of *running real Sinatra/Rack apps*, not Tier-1 embedding.

Build under test: `cargo build -p rubyrs --features _http_server,_fiber`
(default features: `cext, regex, bignum, std-sink`). CRuby oracle: 3.4.1
with sinatra 4.2.1 / rack 3.1.x / puma 6.6.

---

## Gap #1 — `return` from a native-iterator block fails when the method is reached through a Rust-invoked Rack lambda  **[severity: HIGH] — ✅ FIXED**

> **Status: fixed** in `crates/rubyrs/src/vm/step.rs` (`dispatch_until_inner`).
> Root cause: `dispatch_until_inner` bailed on *any* `method_return`
> (`if self.method_return.is_some() { return Ok(()); }`), assuming the
> return always targets a frame below its scope. That's only true when a
> native iterator is driven from the top-level loop. When the
> returning method lives *inside* the `dispatch_until` scope (the case
> under a Rust-invoked Rack block), the signal escaped to
> `call_ruby_block_sync` and became the error below. The fix mirrors the
> top-level `step` loop's lexical-aware unwind: locate the return's owner
> frame via `method_return_locals`; if `owner_idx >= until_depth`,
> consume it here via `begin_method_break`; otherwise bail as before
> (preserving the legitimate `proc { return }`-from-Rust → RuntimeError
> case). Locked by
> `call_ruby_block_sync_consumes_nonlocal_return_from_in_scope_method`
> (http_server.rs); 173 lib + 291 diff_cruby + ruby_spec all still pass.
> The PoC's `dispatch` now uses the idiomatic `routes.each { … return … }`.

*(Original report, kept for the record:)*

**Repro** (`/tmp/repro.rb`):
```ruby
def dispatch2(env)
  [1,2,3].each { |x| return [200, {}, ["got #{x}\n"]] if x == 2 }
  [404, {}, ["nf\n"]]
end
app = ->(env) { dispatch2(env) }
__rubyrs_http_serve_with_app("127.0.0.1:9362", 4, app)
```
`curl` →
```
Rack app raised: block invoked from Rust raised `return` — no enclosing
Ruby method to unwind to (likely a Rack app misuse; use `next` to return
a value)
```

**Key nuance:** the *same* construct works in a normal call chain —
`def f; [1,2,3].each { |x| return x*100 if x==2 }; end; f` correctly
returns `200`. It only breaks when the enclosing Ruby method is entered
**through a Rust-driven call** (`call_ruby_block_sync`, i.e. the Rack app
invocation).

**Preliminary analysis.** A block `return` must unwind to its lexically
enclosing method frame (`dispatch2`). When the Ruby stack is rooted in a
Rust-invoked lambda, the non-local-return unwinder fails to recognise
`dispatch2` as the target method frame — the message literally says "no
enclosing Ruby method to unwind to". The likely cause is that the
frame-kind bookkeeping treats the Rust entry (the Rack lambda) as the
stack base, and the search for the return target stops at that boundary
instead of finding the Ruby method frame nested above it. Related themes:
ADR 0024 (bytecode iter + block-break), ADR 0005 (pinned stack for
native-driven loops), and the known `times`-inside-Fiber bug referenced
in `examples/sse_server.rb`.

**Why it matters:** `collection.each { ... return ... }` is one of the
most common control-flow shapes in Ruby web code (route tables, filter
chains, `before` hooks). Any Rack handler that early-returns from inside
an iterator hits this.

**Workarounds used in the PoC:** index `while` loop + `break` (see
`vendor/sinatra_lite.rb#dispatch`). `break <value>` out of the `each`
also works; `return` does not.

---

## Gap #2 — `env["rack.input"]` is always `nil`; the request body is never exposed  **[severity: HIGH] — ✅ FIXED**

> **Status: fixed.** `env["rack.input"]` is now a Rack-SPEC `StringIO`
> over the buffered request body. Implementation
> (`crates/rubyrs/src/http_server.rs`):
> - `register_host_fns` loads the vendored `StringIO` via `include_str!`
>   of `stdlib_vendor/stringio.rb` (battery owns the dep; works without
>   the `stdlib` feature).
> - new `install_rack_input` constructs `StringIO.new(body)` by
>   dispatching through `do_call` + `dispatch_until` (the invoke-then-
>   drive pattern; plain `do_call` only pushes the frame) and installs
>   it on the env hash. The `StringIO` class is resolved from
>   `vm.classes` (where classes live — `vm.constants` is checked second).
> - GC: the env hash is pinned during StringIO construction, and the
>   **app block is re-pinned for the request** — building rack.input now
>   runs user Ruby (`StringIO#initialize`) that allocates, and
>   `STRESS_GC=1` caught the app block (held only as a Rust `ObjId`,
>   un-pinned by `reset_between_requests`) being swept. The pin is
>   balanced within the request to satisfy the eval-boundary pinned
>   invariant.
> Locked by `rack_input_exposes_request_body_as_readable_stringio`
> (passes normal **and** `STRESS_GC=1`); 174 lib + 291 diff_cruby green.
> The PoC's `POST /echo` now reads the body via `request_body` and is
> byte-identical to real Sinatra's `request.body.read`.
>
> **Remaining (smaller) follow-ups, not blocking:** `rack.errors` and
> `rack.version` are still `nil` stubs (gap #2b below); non-UTF-8 bodies
> are decoded lossily pending the encoding ADR (0020).

*(Original report, kept for the record:)*

**Repro:** POST a body and read it back:
```ruby
app = ->(env) { [200, {}, ["input=#{env['rack.input'].class} clen=#{env['CONTENT_LENGTH']}\n"]] }
```
`curl -X POST --data 'ping' …` → `input=NilClass clen=4`

**Preliminary analysis.** `build_rack_env` in
`crates/rubyrs/src/http_server.rs:467` hardcodes
`pairs.push((key("rack.input"), Value::Nil))` with an explicit
`// TODO stage 4c.3: StringIO`. The body bytes *are* read and
size-enforced (`Limited`, line ~1545) but are never wrapped into a
StringIO and handed to the app. `rack.errors` (line 468) and
`rack.version` (469) are likewise stubbed `nil`. This is a known,
documented stage in ADR 0022 ("What v1 stubs with TODO") — not yet
landed.

**Why it matters:** without `rack.input` there is no POST/PUT body,
no form parsing, no JSON request bodies — i.e. no write-side web app.
This is the single biggest blocker between the PoC and a "real" Sinatra
app.

**Preliminary fix sketch.** In `build_rack_env`, after collecting
`body_bytes`: ensure the vendored `stringio` source is loaded on the Vm
(it lives in `stdlib_vendor/stringio.rb`, gated by the `stdlib` feature),
construct `StringIO.new(<body as binary String>)`, and store that under
`rack.input`. The non-trivial parts are (a) the `_http_server` build must
pull in the StringIO source even without the full `stdlib` feature, and
(b) GC-rooting the freshly-allocated String/StringIO across the env-hash
allocation (the function's own doc-comment flags this hazard). Estimated
~30-45 LOC of Ruby per ADR 0022 + the Rust wiring.

**PoC accommodation:** the `POST /echo` route proves dispatch works but
deliberately does not read the body, so output stays identical on both
runtimes.

---

## Gap #3 — the Rack app must be a `Proc`/`Lambda`, not an arbitrary `call`-able object  **[severity: MEDIUM]**

**Repro:** passing a Sinatra-style class/instance (responds to `call`)
to `__rubyrs_http_serve_with_app` raises `ArgumentError`; only
`Value::Block` is accepted (`http_server.rs:2094`).

**Preliminary analysis.** The host fn pattern-matches the third argument
as `Value::Block(id)`. Rack's contract is "any object responding to
`#call(env)`", which is how real Sinatra hands its `App` *class* to the
server. The battery currently only understands Procs/lambdas.

**Why it matters:** small, but it leaks into framework design — the
shim's `run!` has to wrap dispatch in `->(env) { call(env) }` instead of
passing the app object directly. A future battery revision should accept
any value responding to `call` (it already does this for response
*bodies* — see `marshal_rack_response` handling `each`/`call`/`to_a`).

**Workaround:** wrap in a lambda (`vendor/sinatra_lite.rb#run!`).

---

## Gap #4 — `RUBY_ENGINE` reports `"ruby"`, so it can't be used to feature-detect rubyrs  **[severity: MEDIUM, by-design but a footgun]**

**Repro:**
```
rubyrs: RUBY_ENGINE=ruby  RUBY_VERSION=3.4.0
CRuby:  RUBY_ENGINE=ruby  RUBY_VERSION=3.4.1
```

**Preliminary analysis.** rubyrs deliberately masquerades as CRuby for
maximum drop-in compatibility. This *helps* the "same code runs" goal but
*breaks* the standard idiom every other alternative implementation
supports: JRuby/TruffleRuby/mruby all set `RUBY_ENGINE` to a distinct
value precisely so libraries can pick a pure-Ruby fallback. Because
rubyrs claims `"ruby"`, a library cannot say "if I'm on rubyrs, avoid the
C-ext path". The PoC had to detect the runtime via
`defined?(__rubyrs_http_serve_with_app)` — a private host-fn name — which
is brittle.

**Recommendation.** Expose an honest discriminator: either set
`RUBY_ENGINE = "rubyrs"` (the conventional approach) plus a
`RUBY_ENGINE_VERSION`, or define a stable public constant like
`RUBYRS`/`RUBYRS_VERSION`. "Looks exactly like CRuby" and "lets code
adapt when it must" are both achievable; today only the former is.

---

## Gap #5 — `respond_to?(:host_fn)` is `false` even though `defined?(host_fn) == "method"`  **[severity: LOW]**

**Repro:**
```ruby
defined?(__rubyrs_http_serve_with_app)      # => "method"
respond_to?(:__rubyrs_http_serve_with_app)  # => false
```

**Preliminary analysis.** Host fns registered via `register_fn` are
callable and visible to `defined?`, but `respond_to?` returns `false`,
i.e. they aren't enrolled in the method table `respond_to?` consults.
This is an internal inconsistency: the two reflection paths disagree.
Low impact for app code, but it means capability detection via
`respond_to?` (a common, more correct idiom than `defined?`) silently
fails for batteries.

---

## Gap #6 — `ENV` is undefined  **[severity: MEDIUM, by-design per ADR 0017]**

**Repro:** `defined?(ENV)` → `nil` on rubyrs (CRuby: `"constant"`).

**Preliminary analysis.** ADR 0017 Tier-1 rule "no script-accessible OS
capabilities by default" excludes `ENV` (host-process bleed). Correct for
the sandbox stance, but note the in-tree `examples/prefork_server.rb`
itself reads `ENV["PORT"]` — so that example only runs under a build/host
that injects `ENV`. Config-by-env-var is ubiquitous in Rack apps
(`PORT`, `RACK_ENV`, `DATABASE_URL`), so Tier-2/3 web use will need an
opt-in `ENV` capability (allowlisted, per the ADR 0019 Rule-4 pattern).

---

## Gap #7 — `__dir__` is undefined (`__FILE__` works)  **[severity: LOW] — ✅ FIXED**

> **Status: fixed.** `__dir__` has a dedicated arm in
> `vm/dispatch.rs::do_call` that recognises the bare-call shape
> AND the explicit-`self` receiver shape (the one private-method
> exception CRuby allows for `Kernel#__dir__`), pulls the current
> frame's proto filename, and returns the dirname. Sandbox-aware:
> when `Config::allow_filesystem_io` is on AND no
> `Config::allowed_paths` allowlist is set, the path is
> canonicalised through `std::fs::canonicalize` to match CRuby's
> documented `File.dirname(File.realpath(__FILE__))` semantics; in
> the default Tier-1 sandbox the FS-touching canonicalize is
> skipped and the lexical `Path::parent` is returned (with `""`
> collapsing to `"."` so the common `$LOAD_PATH.unshift __dir__`
> idiom keeps working under embed-mode filenames like `"test.rb"`
> that have no parent component). `defined?(__dir__)` reports
> `"method"` via a small `ast.rs` reflection arm — the
> `do_call`-arm built-ins don't live in the method table that the
> default `__defined_method?` host fn consults, so this arm
> bridges the reflection gap.
>
> Sentinels: `tests/diff/kernel_dir.rb` (gem-oracle diff against
> CRuby) plus `tests/embed/tier1_capability.rs::dunder_dir_*`
> (explicit-self + third-party-receiver shape) plus
> `tests/embed/filesystem_sandbox.rs::dunder_dir_*` (the
> canonicalize / lexical-parent split).
>
> **Repro at the time the gap was recorded:** `defined?(__dir__)`
> → `nil`. **Current behaviour:** `defined?(__dir__)` →
> `"method"`; `__dir__` returns the dirname matching CRuby's
> output byte-for-byte on absolute-path script invocations and
> the documented sandbox-trimmed shape on embed-mode invocations.

---

## Gap #8 — `Kernel#catch` / `throw` not supported  **[severity: MEDIUM] — ✅ FIXED**

> **Status: fixed.** Implemented `catch`/`throw` as top-level (Kernel-style)
> methods in a new `crates/rubyrs/src/preamble/throw_catch.rb`, on top of
> the exception machinery — the same way CRuby models it: `throw` raises an
> `UncaughtThrowError` carrying the tag + value; the matching `catch`
> rescues it (tags compared by `equal?`) and returns the value; a throw
> with no matching catch propagates as `UncaughtThrowError`. This works
> across native iterators and the Rust-invoked Rack boundary because of the
> #11 fix. Locked by the `throw_catch` diff_cruby fixture. The micro-Sinatra
> now implements `halt`/`redirect` with the **authentic** `throw :halt` /
> `catch(:halt)` — exactly real Sinatra's mechanism (the `HaltSignal`
> exception workaround is gone).

*(Original report:)* `catch(:halt) { throw :halt, 42 }` →
`NoMethodError: undefined method 'catch' for NilClass`. `throw`/`catch` is
the mechanism real Sinatra uses for `halt`/`redirect`/`pass`.

---

## Gap #14 — bare `raise` (re-raise current exception) was broken  **[severity: MEDIUM] — ✅ FIXED**

**Repro (before fix):**
```ruby
begin
  begin; raise ArgumentError, "orig"; rescue; raise; end  # bare re-raise
rescue => e
  p [e.class, e.message]   # CRuby: [ArgumentError, "orig"]; rubyrs: uncaught NilClass
end
```

> **Status: fixed** in `crates/rubyrs/src/vm/step.rs` (`Op::Raise`).
> Root cause: bare `raise` compiles to `LoadNil; Raise`, and `Op::Raise`
> fed the `nil` straight into `normalize_exception`, producing a bogus
> nil-class exception instead of re-raising. Fix: when the operand is
> `nil`, re-raise the current exception from `$!` (already tracked on
> `globals` during a rescue/ensure body); if there is none, raise
> `RuntimeError "unhandled exception"` (CRuby's bare-raise-no-context
> behaviour). Surfaced while building `catch` (whose tag-mismatch path
> re-raises). Locked by the `throw_catch` diff_cruby fixture.

---

## Gap #9 — `String#split(sep, limit)` (the 2-arg limit form) not supported  **[severity: LOW-MEDIUM] — ✅ FIXED**

> **Status: fixed** in `crates/rubyrs/src/vm/string.rs`. Added the
> `("split", [Str(sep), Int(limit)])` arm with CRuby semantics:
> `limit > 0` caps the field count with the remainder as the last field;
> `limit == 0` (and the no-limit form) drops trailing empty fields;
> `limit < 0` keeps them; empty-separator splits per-character with the
> limit joining the remainder. **Also fixed a pre-existing bug the work
> uncovered:** the 1-arg `split(sep)` kept trailing empty fields
> (`"a,,".split(",")` → `["a","",""]`) where CRuby drops them (`["a"]`).
> Locked by the `string_split_limit` diff_cruby fixture; 292→293 fixtures
> green. The micro-Sinatra's query parser now uses the idiomatic
> `pair.split("=", 2)` (the `String#index` workaround is gone).

*(Original report, kept for the record:)*

**Repro:** `"a=b=c".split("=", 2)` → `NoMethodError: undefined method
'split' for String`. The 1-arg form `"a=b=c".split("=")` works fine.

**Preliminary analysis.** Only the limit-less `split` arity is wired; the
`(sep, limit)` overload isn't. `split(sep, 2)` is the idiomatic way to
parse `key=value` pairs (query strings, headers, `key: value`), so it
shows up constantly in web/parsing code.

---

## Gap #10 — `rescue <ShortName>` does not resolve a module-nested constant  **[severity: HIGH] — ✅ FIXED**

> **Status: fixed** in `crates/rubyrs/src/vm/step.rs` (`Op::PushRescue`).
> Root cause: a class defined as `module M; class Sig` is keyed in
> `self.classes` by its QUALIFIED sym (`M::Sig`), but the rescue compiler
> stamped only the bare source sym (`Sig`); `PushRescue` did
> `classes.get("Sig")` (+ constants), which missed `M::Sig`, so the
> handler's `filter_class` was `None` and matched nothing. (The `raise`
> side already resolved via the lexical chain — the two sides disagreed.)
> Fix: `PushRescue` now resolves the filter through the frame proto's
> `lexical_scope`, innermost-first, qualifying the bare name
> (`M::C::Sig`, `M::Sig`, …) before falling back to the bare lookup —
> exactly how `Op::LoadConstChain` resolves a normal constant read. No
> bytecode/op-signature change. Locked by the `rescue_nested_constant`
> diff_cruby fixture (incl. a sibling `Other::Sig` negative case proving
> we resolve to the *right* qualified class). 176 lib + 292 diff_cruby +
> ruby_spec green; the PoC's halt signal is now the idiomatic
> `Sinatra::HaltSignal` (namespaced), no top-level workaround needed.

*(Original report, kept for the record:)*

**Minimal repro:**
```ruby
module M
  class Sig < StandardError; end
  class C
    def run
      begin
        raise Sig.new          # raise-side resolves Sig -> M::Sig  ✓
      rescue Sig => e          # rescue-side FAILS to resolve Sig -> M::Sig  ✗
        "caught"
      end
    end
  end
end
M::C.new.run    # CRuby => "caught";  rubyrs => exception ESCAPES uncaught
```

No iterators or blocks involved — this is the pure case. The **`raise`
side resolves** the lexically-nested constant `Sig` to `M::Sig`, but the
**`rescue` side does not**: the rescue-clause class match fails to find
`Sig` in the enclosing module's lexical scope, so it doesn't match the
raised `M::Sig` and the exception escapes.

**Preliminary analysis.** The rescue-clause exception-class expression is
evaluated/resolved in a scope that omits the lexical module nesting
(probably only top-level + current class, not the full
`Module.nesting`). Constant resolution for `raise`/normal reads clearly
*does* include the nesting (raise works), so the two paths use different
lookup — the rescue path is the deficient one.

**Why it matters:** every gem defines its errors as
`module Foo; class Error < StandardError; end` and rescues them by short
name (`rescue Error`) from within `module Foo`. This silently breaks all
of them — high impact for real-gem compatibility.

**Workaround (PoC):** the micro-Sinatra's halt exception is a TOP-LEVEL
class (`RubyrsSinatraHalt`), since top-level constants resolve in a
rescue clause. Not a fix — gems can't all move their errors to top level.

**Fix sketch (not done):** make the rescue-clause constant lookup use the
same `Module.nesting`-aware resolution as a normal constant read (likely
the compiler emitting the rescue-class as a proper const-path load, or
the matcher consulting the defining frame's lexical scope). Candidate for
the next engine fix.

---

## Gap #11 — exception raised in a native-iterator block escapes an in-scope `rescue` when run under a Rust-invoked Rack block  **[severity: HIGH] — ✅ FIXED**

**Minimal repro (over HTTP):**
```ruby
app = ->(env) {
  begin
    [1].each { raise "boom" }     # raise inside native each…
    "no"
  rescue => e                      # …rescue in the SAME lambda
    [200, {}, ["caught: #{e.message}"]]
  end
}
```
Direct `app.call(env)` works; under `__rubyrs_http_serve_with_app` it
returned `500 "Rack app raised:"` — the exception escaped the in-scope
rescue.

**Root cause & fix.** Exactly the exception analog of gap #1.
`unwind_with_exception` correctly found the in-scope handler, redirected
IP, and emitted the synthetic `AlreadyCaught` "resume here" signal — but
`dispatch_until_inner` (vm/step.rs) **re-emitted `AlreadyCaught`
unconditionally**, on the assumption that the outermost *main-loop*
`dispatch` would consume it. Under the server there is no main loop above
`call_ruby_block_sync` → `step_block` → `dispatch_until`, so the signal
escaped to Rust. Fix: consume `AlreadyCaught` when the redirected handler
frame is within this `dispatch_until`'s scope (`frames.len() >
until_depth`), otherwise bubble out — mirroring the gap #1 fix. Locked by
`call_ruby_block_sync_catches_exception_from_iterator_block_in_scope`
(passes normal + `STRESS_GC=1`); full suite green. This is what makes the
exception-based `halt`/`redirect` workaround for gap #8 actually work.

---

## Gap #12 — `Kernel#load` not supported  **[severity: LOW]**

**Repro:** `load "foo.rb"` → `NoMethodError: undefined method 'load' for
NilClass`. `require` / `require_relative` work. Minor; `load`'s
re-execution semantics are rarely needed by app code.

---

## Gap #13 — `eval` doesn't capture the surrounding binding; `Kernel#binding` absent — blocks template engines (ERB/Tilt)  **[severity: MEDIUM — feature-scoped]**

**Repro:**
```ruby
@name = "World"
eval("1 + 2")   # => 3        (self-contained eval works)
eval("@name")   # => nil      (should be "World" — no binding capture)
binding         # NoMethodError: undefined method 'binding'
```

**Preliminary analysis.** `eval` evaluates a self-contained expression
but does **not** run in the caller's lexical/instance binding, and
`Kernel#binding` isn't available. This is the documented Tier-2
`_full_eval` boundary (ADR 0019: "Full lexical-scope `eval` (binding
capture, locals access)" is Tier 2). 

**Impact on Sinatra:** **templating** is the casualty. `erb`, `haml`,
`slim` (via Tilt) compile a template to Ruby source and `eval` it in the
view's binding so `<%= @name %>` / locals resolve. Without binding-capturing
eval, a real template engine can't render. This is the one major Sinatra
feature the PoC does **not** attempt (see README). A pure-Ruby ERB-lite
that only does string interpolation is possible, but it would NOT be
real-ERB-compatible, so it's intentionally left out rather than fake it.

**Not worked around** — documented as the boundary of the PoC.

---

## Sinatra compatibility notes (real-gem semantics, NOT rubyrs gaps)

Surfaced while widening the PoC to `error` handlers, `redirect`, form
params, and the `request` object. These are things the micro-Sinatra had
to match in *real Sinatra's* behaviour — not interpreter deficiencies:

- **`redirect` Location is absolute.** Real Sinatra expands `redirect
  "/new"` to `http://<host>/new` using the request host. The micro-Sinatra
  now does the same (from `HTTP_HOST`).
- **`error` handlers are environment-dependent.** Under `App.run!`, real
  Sinatra defaults to `:development`, where the `show_exceptions` debug
  page intercepts a raised exception (500 + backtrace) *before* a
  registered `error MyError do … end` handler can answer. Setting
  `:environment` to `:production` makes the handler authoritative — which
  is how you'd actually deploy. The shared app sets this; the micro-Sinatra
  treats `set`/`enable`/`disable` as compatibility no-ops.
- **Form bodies are consumed by param parsing.** A
  `application/x-www-form-urlencoded` POST body is parsed into `params` by
  Rack/Sinatra, so `request.body.read` is then empty. The PoC's `/echo`
  uses `text/plain` to read the raw body; `/form` uses form-encoding to
  read `params`.

## Non-gaps (verified to work — recorded so they aren't re-investigated)

These all behaved identically to CRuby and are load-bearing for the PoC:

- Singleton-method DSL inheritance: `class App < Sinatra::Base; get "/" do … end`
  resolves `get` up the singleton chain (locked by `tests/diff/sinatra_dsl_shape.rb`).
- `class << self` + `attr`-style class-ivar route tables (`@routes` per subclass).
- `instance_exec(&block)` running a route block in instance context.
- Block array-destructuring `[[a,b]].each { |a,b| … }`; multiple assignment.
- `return` from an `each` block in a **normal** (non-Rust-rooted) call chain.
- `String#[](range)` (`seg[1..-1]`), `String#to_i(16)`, `Integer#chr`,
  `String#gsub`, `split`, `reject`, `start_with?`.
- `require_relative` (relative to caller dir) and `$LOAD_PATH`.
- `File.expand_path`, `__FILE__`.
- Exception raised inside `each` and rescued OUTSIDE the block in a
  normal call chain (only the Rust-rooted variant was broken — gap #11).
- Custom exception classes, `attr_reader` payloads, `e.message`,
  `rescue ClassName => e` (for top-level classes).
- `String#index`, range slicing (`s[0...i]`, `s[i..-1]`), `gsub("+", " ")`,
  `Integer#chr`, `to_i(16)`, `format`/`%`.
- `Hash#merge` / `merge!`, `Array` destructuring in block params
  (`each { |a, b| }`), `send`.
- The whole `_http_server` request→route→`[status, headers, body]`→wire
  pipeline: HTTP status codes, custom headers (incl. `Location` for
  redirects), query strings, splat/path params, `before` filters,
  `rack.input` body reads, and custom 404 handlers.
