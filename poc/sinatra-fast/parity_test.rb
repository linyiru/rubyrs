# Parity + coverage test for the lean-dispatch shim.
#
# For every request shape below it runs the request twice through the SAME
# app — once with the shim enabled, once disabled (plain Sinatra) — and
# asserts the [status, headers, body] are byte-identical. It also asserts
# the fast path was actually TAKEN on eligible routes and fell back on the
# ineligible ones (a parity check is worthless if BOTH sides quietly fall
# back). Exit code is non-zero on any divergence so it can gate CI.
#
# Run on rubyrs:  RUBYRS_NO_PREAMBLE_CACHE=1 rubyrs poc/sinatra-fast/parity_test.rb
# Run on CRuby :  ruby --disable=gems poc/sinatra-fast/parity_test.rb   (sanity)

require_relative "setup_load_path"
require_relative "lean_dispatch"
require "json"
require "stringio"

# ---- a deliberately broad Sinatra app: filters, sessions, every return
#      shape, and ineligible routes (splat / regexp / conditioned / an
#      ineligible route defined BEFORE an eligible one at the same arity) ----
class App < Sinatra::Base
  set :host_authorization, { permitted_hosts: [] }
  set :protection, false
  set :environment, :production
  # NB: session/middleware live ABOVE Base#route! in the Rack stack, so the
  # shim never sees them — they're unaffected. (We don't enable :sessions
  # here only because rubyrs's rack-session path hits an orthogonal
  # OpenSSL::Cipher#iv_len= gap that would pollute this gate's exit code.)

  before { @before_ran = true }
  after  { headers["X-After"] = "1" }

  get "/" do
    "root #{@before_ran}"
  end
  get "/p/:id" do                       # params-style :param
    "p=#{params['id']}"
  end
  get "/b/:name" do |name|              # block-arg-style :param
    "b=#{name}"
  end
  get "/m/:a/:b" do                     # multi :param
    "m=#{params['a']}-#{params['b']}"
  end
  get "/q" do                           # query params
    "q=#{params['x']}"
  end
  get "/json" do                        # content_type + body
    content_type :json
    { ok: true, n: params['n'] }.to_json
  end
  get "/created" do                     # explicit status
    status 201
    "made"
  end
  get "/go" do                          # redirect (302 + Location)
    redirect "/"
  end
  get "/halt_s" do                      # halt with a string
    halt "halted"
    "unreached"
  end
  get "/halt_code" do                   # halt with [status, headers, body]
    halt 503, { "X-H" => "y" }, "down"
    "unreached"
  end
  get "/array_ret" do                   # bare [status, headers, body] return
    [202, { "X-R" => "z" }, "arr"]
  end
  get "/boom" do                        # raises → error handling (dispatch! rescue)
    raise "boom"
  end
  get "/custom_err" do                  # raises → custom error handler below
    raise ArgumentError, "bad"
  end
  error ArgumentError do                 # custom error block + after-filter-on-error
    status 422
    "handled: #{env['sinatra.error'].message}"
  end
  # order-sensitivity: an ELIGIBLE-looking path that is actually guarded by a
  # condition (ineligible) is defined BEFORE the plain one. The shim must NOT
  # let the plain one win for an Accept: it can't evaluate.
  get "/cond", provides: :json do
    content_type :json
    '{"via":"cond"}'
  end
  get "/cond" do
    "via plain"
  end
  # genuinely complex routes → must fall back
  get "/legacy/*" do
    "splat #{params['splat'].first}"
  end
  get %r{/re/(\d+)} do
    "re #{params['captures'].first}"
  end
end

app = App.new

def env_for(path, qs = "", method: "GET", accept: nil)
  e = {
    "REQUEST_METHOD" => method, "PATH_INFO" => path, "SCRIPT_NAME" => "",
    "QUERY_STRING" => qs, "SERVER_NAME" => "localhost", "SERVER_PORT" => "80",
    "HTTP_HOST" => "localhost", "rack.url_scheme" => "http",
    "rack.input" => StringIO.new(""), "rack.errors" => $stderr
  }
  e["HTTP_ACCEPT"] = accept if accept
  e
end

def drain(body)
  s = +""
  body.each { |c| s << c.to_s }
  body.close if body.respond_to?(:close)
  s
end

# Each case: [label, path, query, opts, expect_fast]
CASES = [
  ["static",        "/",          "",       {},                 true],
  ["param",         "/p/42",      "",       {},                 true],
  ["block-arg",     "/b/sam",     "",       {},                 true],
  ["multi-param",   "/m/x/y",     "",       {},                 true],
  ["query",         "/q",         "x=9",    {},                 true],
  ["content_type",  "/json",      "n=7",    {},                 true],
  ["status",        "/created",   "",       {},                 true],
  ["redirect",      "/go",        "",       {},                 true],
  ["halt-string",   "/halt_s",    "",       {},                 true],
  ["halt-code",     "/halt_code", "",       {},                 true],
  ["array-return",  "/array_ret", "",       {},                 true],
  ["raise-500",     "/boom",      "",       {},                 true],
  ["custom-error",  "/custom_err","",       {},                 true],
  ["HEAD",          "/",          "",       { method: "HEAD" }, true],
  ["404",           "/nope",      "",       {},                 false],
  ["cond-json",     "/cond",      "",       { accept: "application/json" }, false],
  ["cond-plain",    "/cond",      "",       { accept: "text/html" },        false],
  ["splat",         "/legacy/a/b","",       {},                 false],
  ["regexp",        "/re/55",     "",       {},                 false],
]

failures = []
CASES.each do |label, path, qs, opts, expect_fast|
  Sinatra::LeanDispatch.enabled = true
  Sinatra::LeanDispatch.reset_stats!
  fs, fh, fb = app.call(env_for(path, qs, **opts))
  hit = Sinatra::LeanDispatch.stats[:hit]
  fbody = drain(fb)

  Sinatra::LeanDispatch.enabled = false
  os, oh, ob = app.call(env_for(path, qs, **opts))
  obody = drain(ob)

  # Compare status, body, and a stable subset of headers.
  keys = (fh.keys | oh.keys).reject { |k| k.downcase == "set-cookie" } # session cookie varies
  hdr_same = keys.all? { |k| fh[k] == oh[k] }
  same = (fs == os) && (fbody == obody) && hdr_same
  taken_ok = (hit > 0) == expect_fast

  status = (same && taken_ok) ? "ok  " : "FAIL"
  failures << label unless same && taken_ok
  printf("  %-4s %-13s status=%-3d fast=%-5s%s body=%s\n",
         status, label, fs, hit > 0,
         (taken_ok ? "" : "[EXPECTED fast=#{expect_fast}!] "),
         fbody[0, 26].inspect)
  unless same
    printf("        DIVERGE: status %d/%d  hdr_same=%s\n", fs, os, hdr_same)
    (keys).each { |k| printf("          hdr %-14s fast=%-20s orig=%s\n", k, fh[k].inspect, oh[k].inspect) if fh[k] != oh[k] }
    printf("          body fast=%s\n          body orig=%s\n", fbody.inspect, obody.inspect) if fbody != obody
  end
end

# Inheritance: a before-filter + error handler defined on a BASE class must
# still fire for a route on a SUBCLASS — the shim's no-op flags walk the
# superclass chain, so they must NOT skip inherited filters/handlers.
class BaseApp < Sinatra::Base
  set :host_authorization, { permitted_hosts: [] }
  set :protection, false
  set :environment, :production
  before { @from_base = "base" }
  error RuntimeError do
    status 533
    "base handled: #{env['sinatra.error'].message}"
  end
end
class ChildApp < BaseApp
  get("/inherited") { "child sees #{@from_base}" }     # inherited before-filter must run
  get("/child_boom") { raise "kaboom" }                # inherited error handler must catch
end
child = ChildApp.new
[["/inherited", "child sees base"], ["/child_boom", "base handled: kaboom"]].each do |path, want|
  Sinatra::LeanDispatch.enabled = true;  fs, fh, fb = child.call(env_for(path))
  Sinatra::LeanDispatch.enabled = false; os, oh, ob = child.call(env_for(path))
  fbody = drain(fb); obody = drain(ob)
  same = (fs == os) && (fbody == obody) && (fbody == want)
  failures << "inherit:#{path}" unless same
  printf("  %-4s %-13s status=%-3d body=%s\n", same ? "ok" : "FAIL", "inherit#{path}", fs, fbody[0, 26].inspect)
end

puts(failures.empty? ? "\nPARITY: PASS (#{CASES.length + 2}/#{CASES.length + 2})" : "\nPARITY: FAIL — #{failures.join(', ')}")

# ---- speedup on the eligible routes (fast vs full, same app/middleware) ----
# Fresh env per call: rack-session writes its cookie into env on each
# request, so reusing one env hash accumulates session state and trips
# rubyrs's OpenSSL::Cipher#iv_len= gap (orthogonal to the shim). A fresh
# env mirrors real per-request envs and keeps the ratio honest (both sides
# pay the same env_for cost).
def timeit(app, n)
  best = nil
  5.times do
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    n.times { app.call(yield) }
    dt = Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0
    best = dt if best.nil? || dt < best
  end
  best
end
# Speedup on a MINIMAL app (no sessions/encryptor noise) to isolate the
# dispatch saving the route!-override actually recovers (= skipping the
# mustermann route! loop; call!/Request/Response/invoke are still Sinatra's).
class Speed < Sinatra::Base
  set :host_authorization, { permitted_hosts: [] }
  set :protection, false
  set :environment, :production
  get("/")      { "ok" }
  get("/p/:id") { "p=#{params['id']}" }
  get("/json")  { content_type :json; { n: params['n'] }.to_json }
end
speed = Speed.new
N = (ENV["N"] || "10000").to_i
puts "\nspeedup (us/req, best-of-5, fast vs full; minimal app, fresh env/call):"
[["static", "/", ""], ["param", "/p/42", ""], ["json", "/json", "n=7"]].each do |label, path, qs|
  mk = -> { env_for(path, qs) }
  speed.call(mk.call)
  Sinatra::LeanDispatch.enabled = false; o = timeit(speed, N, &mk)
  Sinatra::LeanDispatch.enabled = true;  f = timeit(speed, N, &mk)
  printf("  %-7s full=%6.1f  fast=%6.1f  %.2fx\n", label, o / N * 1e6, f / N * 1e6, o / f)
end

# Exit non-zero only on a real parity failure; fall off the end on success.
# (rubyrs currently surfaces `exit(0)` itself as an uncaught SystemExit and
# returns 1 — a separate VM gap — so we avoid calling exit on the pass path.)
abort("parity failures: #{failures.join(', ')}") unless failures.empty?
