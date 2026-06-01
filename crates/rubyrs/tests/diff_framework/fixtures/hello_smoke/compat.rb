# Runtime-aware shim — the ONLY engine-conditional file in this
# fixture (per ADR 0026 v2's "same code runs on both" rule; the
# anti-pattern §"no engine-branching in blessed reimpls" doesn't
# apply here because this file IS the user-side adapter shim that
# the anti-pattern explicitly carves out).
#
# Detects rubyrs via the `RUBYRS` sentinel constant (ADR 0026 v2 /
# GAP #4, shipped in PR #315's M27 B2 commit). CRuby leaves the
# constant undefined; `defined?(RUBYRS)` is the canonical idiom.
#
# Exports two top-level constants the app.rb leans on:
#   HARNESS_RUNTIME   — human-readable runtime tag for /-route output
#   HARNESS_SERVE     — Proc that takes the Rack app and boots the
#                       per-runtime server loop. The framework's
#                       PORT env var picks the bind port; duration
#                       is short so the test exits cleanly.

PORT = (ENV["HARNESS_PORT"] || "8080").to_i
SECS = (ENV["HARNESS_SECS"] || "6").to_i

if defined?(RUBYRS)
  HARNESS_RUNTIME = "rubyrs"
  HARNESS_SERVE = ->(app) {
    __rubyrs_http_serve_with_app(
      "127.0.0.1:#{PORT}", SECS, app,
      { per_request_fuel: 1_000_000 },
    )
  }
else
  # CRuby — stdlib webrick (no gem required since Ruby 3.0+, gem'd
  # but pre-installed on the standard CI Ruby image). Rack-style
  # input mapping done by hand: webrick gives `req.body` which we
  # wrap as a StringIO under the `rack.input` env key the app
  # reads. Other env keys are constructed minimally — just what
  # the smoke routes need.
  require "webrick"
  require "stringio"
  HARNESS_RUNTIME = "cruby"
  HARNESS_SERVE = ->(app) {
    server = WEBrick::HTTPServer.new(
      Port: PORT,
      BindAddress: "127.0.0.1",
      Logger: WEBrick::Log.new(File::NULL),
      AccessLog: [],
    )
    server.mount_proc "/" do |req, res|
      body = req.body ? req.body : ""
      env = {
        "REQUEST_METHOD" => req.request_method,
        "PATH_INFO"      => req.path,
        "rack.input"     => StringIO.new(body),
      }
      status, headers, body_chunks = app.call(env)
      res.status = status
      headers.each { |k, v| res[k] = v }
      res.body = body_chunks.join
    end
    trap("INT") { server.shutdown }
    # Auto-stop after SECS so the test harness's wait-and-collect
    # cycle has a known upper bound. WEBrick has no per-call
    # duration knob; we register a Thread that calls shutdown.
    Thread.new {
      sleep SECS
      server.shutdown
    }
    server.start
  }
end
