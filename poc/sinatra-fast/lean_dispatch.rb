# frozen_string_literal: true
#
# B1 Phase 1 — pure-Ruby lean-dispatch shim for Sinatra.
#
# Validated lever (see memory b1-native-framework-validated): ~99% of a
# Sinatra request is framework plumbing, and the route-finding generality
# (mustermann + the route! loop) is a recoverable ~half of it. This shim
# overrides ONLY `Sinatra::Base#route!` to native-segment-match the app's
# own static / `:param` routes, then runs the matched route through
# Sinatra's UNCHANGED `route_eval` → `invoke` path. Everything else —
# call!, dispatch!, before/after filters, halt/pass, error handling, the
# response build — is Sinatra's own code, so the shim cannot change
# observable behaviour, only speed. Anything it doesn't handle (splat /
# regexp / conditioned routes, or a non-leading match) falls back to the
# real mustermann `route!`.
#
# Zero Rust. Gated by simply requiring this file after sinatra/base.

require "sinatra/base"

module Sinatra
  module LeanDispatch
    # klass => { "GET" => [entry, ...] } in DEFINITION ORDER.
    # An entry is either an eligible route { pat:, names:, nseg:, src:, wrapper: }
    # or an ineligible marker { eligible: false } — the marker is kept so the
    # matcher can preserve Sinatra's first-match-wins order (see `match`).
    TABLE = {}
    @stats = { hit: 0, miss: 0 }
    @enabled = true
    class << self
      attr_reader :stats
      attr_accessor :enabled
      def reset_stats!; @stats = { hit: 0, miss: 0 }; end
    end

    # A path is fast-eligible iff it is a sequence of literal / `:param`
    # segments — no splat (`*`), optional (`?`), group (`(`), or regexp.
    def self.classify(path)
      return nil unless path.is_a?(String)
      return nil if path =~ /[*?(\[]/
      names = []
      segs = path.split("/", -1)
      pat = segs.map do |s|
        if s.start_with?(":") && s.length > 1
          names << s[1..]
          :p
        else
          s
        end
      end
      { pat: pat, names: names, nseg: segs.length, src: path }
    end

    # Returns [entry, path_params_or_nil] for the FIRST eligible route that
    # matches `path`, or nil to fall back. Crucially, it returns nil (fall
    # back) the moment it reaches an INELIGIBLE route before a match — that
    # route might match under mustermann and Sinatra tries routes in order,
    # so we must not let a later eligible route win over an earlier complex one.
    def self.match(klass, verb, path)
      list = TABLE.dig(klass, verb) or return nil
      segs = path.split("/", -1)
      n = segs.length
      list.each do |e|
        return nil unless e[:pat] # ineligible reached first → preserve order
        next unless e[:nseg] == n
        pp = nil
        ok = true
        pi = 0
        e[:pat].each_with_index do |seg, i|
          if seg == :p
            (pp ||= {})[e[:names][pi]] = segs[i]
            pi += 1
          elsif seg != segs[i]
            ok = false
            break
          end
        end
        return [e, pp] if ok
      end
      nil
    end
  end

  class Base
    class << self
      alias_method :__ld_route, :route
      def route(verb, path, options = {}, &block)
        sig = __ld_route(verb, path, options, &block)
        entry =
          if options.empty? && (c = LeanDispatch.classify(path))
            c.merge(wrapper: sig[2]) # sig == [pattern, conditions, wrapper]
          else
            { eligible: false }
          end
        ((LeanDispatch::TABLE[self] ||= {})[verb] ||= []) << entry
        sig
      end
    end

    alias_method :__ld_route!, :route!
    def route!(base = settings, pass_block = nil)
      # Only short-circuit the TOP-LEVEL match against THIS app's own routes.
      # Superclass recursion (base != settings) and pass-block re-entry keep
      # the original path.
      if LeanDispatch.enabled && pass_block.nil? && base.equal?(settings)
        path = @request.path_info
        path = "/" if path.empty? && !settings.empty_path_info?
        path = path[0..-2] if !settings.strict_paths? && path != "/" && path.end_with?("/")
        if (m = LeanDispatch.match(self.class, @request.request_method, path))
          LeanDispatch.stats[:hit] += 1
          entry, pp = m
          # Mirror process_route's per-route prelude for the matched route.
          @response.delete_header("content-type") unless @pinned_response
          values = []
          if pp
            force_encoding(pp)
            @params = @params.merge(pp) { |_k, v1, v2| v2 || v1 }
            values = pp.values
          end
          @env["sinatra.route"] = "#{@request.request_method} #{entry[:src]}"
          # route_eval throws :halt with the block's value on normal return
          # (→ invoke builds the response). A `pass` throws :pass, which we
          # catch and fall through to the full mustermann route! so the next
          # matching route runs.
          catch(:pass) { route_eval { entry[:wrapper].call(self, values) } }
        else
          LeanDispatch.stats[:miss] += 1
        end
      end
      __ld_route!(base, pass_block)
    end
  end
end
