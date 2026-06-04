# Minimal `Rack::Request` shim for the rack_protection_smoke
# fixture. The real `rack` gem ships a 500+ LOC Request class
# wrapping env with full URI / params / cookie / session parsing;
# the rack-protection middlewares vendored here use only a
# handful of read-only accessors against the env hash, so the
# shim is just those accessors.
#
# Real Rack::Request stays the parity oracle: on CRuby the
# fixture loads the real rack gem via the `require "sinatra/base"`
# transitive dep tree, and these methods produce the same values
# read from the same env keys. On rubyrs the require resolves
# here.
#
# Methods covered (the union actually called by HttpOrigin /
# JsonCsrf / RemoteReferrer / SessionHijacking):
#   * .host    — from HTTP_HOST or SERVER_NAME, port-stripped
#   * .port    — from HTTP_HOST port suffix, X-Forwarded-Port,
#                or SERVER_PORT, as an Integer
#   * .scheme  — from rack.url_scheme (typically 'http')
#   * .env     — the raw env hash
#   * .xhr?    — true if X-Requested-With: XMLHttpRequest
#
# Adding new methods is cheap; document them up here and add
# the env-key sourcing comment in-line.

module Rack
  class Request
    def initialize(env)
      @env = env
    end

    def env
      @env
    end

    # `host` is HTTP_HOST (which can include a port suffix) with
    # the port stripped, falling back to SERVER_NAME / SERVER_ADDR
    # for clients that omit the Host header. Real Rack also strips
    # IPv6 brackets; the fixture doesn't exercise IPv6 hosts so the
    # `:` split is sufficient.
    def host
      raw = @env["HTTP_HOST"] || @env["SERVER_NAME"] || @env["SERVER_ADDR"] || ""
      raw.to_s.split(":", 2).first
    end

    # `port` resolution order matches real Rack::Request:
    #   1. HTTP_HOST suffix (e.g. "example.com:8080" → 8080)
    #   2. HTTP_X_FORWARDED_PORT (proxy-forwarded port)
    #   3. SERVER_PORT
    #   4. scheme-derived default (80 for http, 443 for https)
    # Returns an Integer.
    def port
      if (raw = @env["HTTP_HOST"]) && raw.include?(":")
        parts = raw.split(":", 2)
        return parts.last.to_i if parts.last && !parts.last.empty?
      end
      if (p = @env["HTTP_X_FORWARDED_PORT"])
        return p.to_i
      end
      if (p = @env["SERVER_PORT"])
        return p.to_i
      end
      scheme == "https" ? 443 : 80
    end

    # `scheme` is the rack.url_scheme env key set by the upstream
    # server. Defaults to 'http' when absent.
    def scheme
      @env["rack.url_scheme"] || "http"
    end

    # XMLHttpRequest detection — the jQuery / fetch convention
    # of setting the X-Requested-With header lets server-side
    # CSRF guards skip the referrer / origin check for genuinely
    # same-origin AJAX requests.
    def xhr?
      @env["HTTP_X_REQUESTED_WITH"] == "XMLHttpRequest"
    end

    # `host_authority` returns the raw Host header value (with
    # port suffix intact, no protocol). HostAuthorization checks
    # both this and `forwarded_authority` against its
    # permitted-hosts list. Real Rack 3 builds this from
    # SERVER_NAME + SERVER_PORT when HTTP_HOST is absent; the
    # fixture always has HTTP_HOST so the simple read suffices.
    def host_authority
      @env["HTTP_HOST"].to_s
    end

    # `forwarded_authority` returns the trusted-proxy-supplied
    # host, used for DNS-rebinding defence behind reverse
    # proxies. Real Rack 3 parses RFC 7239 `Forwarded:` first,
    # falling back to `X-Forwarded-Host`. The fixture only
    # exercises `X-Forwarded-Host` (the older, more common
    # header); a future scenario that needs `Forwarded:`
    # parsing can extend this method.
    def forwarded_authority
      @env["HTTP_X_FORWARDED_HOST"].to_s
    end
  end
end
