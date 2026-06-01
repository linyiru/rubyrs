# Tier-0 hello smoke — same Rack-shape app.rb runs on both runtimes
# WITHOUT requiring any gems. CRuby uses the stdlib `webrick`; rubyrs
# uses the `_http_server` battery. The runtime-aware shim is in
# `compat.rb` (the ONLY engine-conditional file, per ADR 0026 v2's
# "same code runs on both" headline).
#
# This is intentionally Sinatra-free so the harness's own correctness
# doesn't entangle with the Sinatra gem's availability. The full
# Sinatra parity matrix lives at fixtures/sinatra_hello/.
#
# Routes:
#   GET /                 → "hello root runtime=<engine>"
#   GET /hello/<name>     → "hello <name> runtime=<engine>"
#   POST /echo            → request body echoed back
#   anything else         → 404

require_relative "compat"

APP = ->(env) {
  method = env["REQUEST_METHOD"]
  path   = env["PATH_INFO"]
  if method == "GET" && path == "/"
    [200, {"Content-Type" => "text/plain"},
     ["hello root runtime=#{HARNESS_RUNTIME}\n"]]
  elsif method == "GET" && path.start_with?("/hello/")
    name = path.sub("/hello/", "")
    [200, {"Content-Type" => "text/plain"},
     ["hello #{name} runtime=#{HARNESS_RUNTIME}\n"]]
  elsif method == "POST" && path == "/echo"
    body = env["rack.input"].read
    [200, {"Content-Type" => "text/plain"}, [body + "\n"]]
  else
    [404, {"Content-Type" => "text/plain"}, ["not found\n"]]
  end
}

HARNESS_SERVE.call(APP)
