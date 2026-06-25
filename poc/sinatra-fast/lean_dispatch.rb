# frozen_string_literal: true
#
# B1 Phase 1.5 — pure-Ruby lean-dispatch shim for Sinatra.
#
# Validated lever (memory b1-native-framework-validated): ~99% of a Sinatra
# request is framework plumbing. An earlier route!-only override recovered
# just the mustermann loop (~1.2x); a precise ablation showed the dispatch
# generality is far larger — the double `invoke` nesting, `@params.merge!(
# @request.params)` query-parse, `filter!` even with zero filters, the
# `error_block!` probe, and `Response#finish` are each real µs. This shim
# replaces `Sinatra::Base#call!` for an app's own static/:param routes with
# a lean reimplementation that REUSES Sinatra's own `invoke` / `filter!` /
# `handle_exception!` / `error_block!` / `content_type` / `Response#finish`
# (so behaviour can't change), but skips the route! loop, collapses the
# double-invoke, and elides per-request work that is provably a no-op for
# the current app (no filters / no error handlers / params untouched).
# Anything it can't model (splat / regexp / conditioned routes, a non-
# leading match) falls back to the real `call!`. Every change is gated by
# parity_test.rb (byte-identical [status, headers, body] vs stock Sinatra).

require "sinatra/base"

module Sinatra
  module LeanDispatch
    TABLE = {}            # klass => { "GET" => [entry,...] } in definition order
    @stats = { hit: 0, miss: 0 }
    @enabled = true
    class << self
      attr_reader :stats
      attr_accessor :enabled
      def reset_stats!; @stats = { hit: 0, miss: 0 }; end
    end

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

    # First eligible route matching `path`, or nil. Bails (nil) the moment it
    # reaches an ineligible route before a match — Sinatra is first-match-wins,
    # so a later eligible route must not win over an earlier complex one.
    def self.match(klass, verb, path)
      list = TABLE.dig(klass, verb) or return nil
      segs = path.split("/", -1)
      n = segs.length
      list.each do |e|
        return nil unless e[:pat]
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

      # Per-app no-op flags, computed once after routes/filters are defined.
      # MUST walk the superclass chain — Sinatra's `filter!` / `error_block!`
      # both recurse into ancestors, so an inherited filter / error handler
      # would be silently skipped if we only checked the leaf class. (A
      # dynamically-added-at-runtime filter would need flag busting, but the
      # common case sets them at class-eval time.)
      def __ld_flags
        @__ld_flags ||= begin
          no_filters = true
          no_errors = true
          k = self
          while k.respond_to?(:filters)
            no_filters &&= k.filters[:before].empty? && k.filters[:after].empty?
            errs = k.instance_variable_get(:@errors)
            no_errors &&= errs.nil? || errs.empty?
            k = k.superclass
          end
          { no_filters: no_filters, no_errors: no_errors }
        end
      end
    end

    alias_method :__ld_call, :call
    def call(env)
      if LeanDispatch.enabled
        path = env["PATH_INFO"]
        path = "/" if path.empty? && !settings.empty_path_info?
        path = path[0..-2] if !settings.strict_paths? && path != "/" && path.end_with?("/")
        if (m = LeanDispatch.match(self.class, env["REQUEST_METHOD"], path))
          LeanDispatch.stats[:hit] += 1
          return dup.__ld_lean_call!(env, m[0], m[1])
        end
      end
      LeanDispatch.stats[:miss] += 1
      __ld_call(env)
    end

    # Faithful lean call! — mirrors Sinatra's call!/dispatch! exactly EXCEPT
    # it runs the one native-matched route instead of the mustermann route!
    # loop, collapses call!'s `invoke { dispatch! }` + dispatch!'s inner
    # invoke into one, and elides work that is a no-op for this app.
    def __ld_lean_call!(env, entry, pp)
      flags = self.class.__ld_flags
      @env = env
      @params = Sinatra::IndifferentHash.new
      @request = Sinatra::Request.new(env)
      @response = Sinatra::Response.new
      @pinned_response = nil
      begin
        invoke do
          # query/body params, then path params (override, Sinatra order).
          # Skip the ~13µs parse when there demonstrably are none — empty
          # QUERY_STRING and no body. `@request.params` would be {}, so the
          # merge is a no-op (faithful); parsing it is pure waste.
          if !@env["QUERY_STRING"].empty? || @env["CONTENT_LENGTH"].to_i > 0
            @params.merge!(@request.params)
          end
          if pp
            force_encoding(pp)
            @params = @params.merge(pp) { |_k, v1, v2| v2 || v1 }
          end
          filter!(:before) { @pinned_response = !response["content-type"].nil? } unless flags[:no_filters]
          @env["sinatra.route"] = "#{@request.request_method} #{entry[:src]}"
          # route_eval throws :halt with the block's value on normal return.
          # A `pass` throws :pass — catch it and defer to the real mustermann
          # route! to find the NEXT matching route (it re-tries routes in
          # order, honours pass again, and route_missings to 404). Filters
          # already ran above (route! is after :before filters in Sinatra),
          # so they are not re-run.
          catch(:pass) { route_eval { entry[:wrapper].call(self, pp ? pp.values : []) } }
          route!
        end
      rescue ::Exception => e # rubocop:disable Lint/RescueException
        invoke { handle_exception!(e) }
      ensure
        filter!(:after) if !flags[:no_filters] && !@env["sinatra.static_file"]
      end
      invoke { error_block!(@response.status) } unless flags[:no_errors] || @env["sinatra.error"]
      unless @response["content-type"]
        if Array === body && body[0].respond_to?(:content_type)
          content_type body[0].content_type
        elsif (default = settings.default_content_type)
          content_type default
        end
      end
      __ld_finish
    end

    # Inlined Sinatra::Response#finish for the common (non-drop-body) case:
    # status 200..599 except 204/304/1xx. Replicates calculate_content_length?
    # exactly and returns [status, headers, body]; falls back to the real
    # finish for 1xx/204/304 (informational / drop-body, which delete headers
    # and empty the body). Saves the predicate method-call chain.
    def __ld_finish
      st = @response.status
      return @response.finish if st < 200 || st == 204 || st == 304
      h = @response.headers
      b = @response.body
      if h["content-type"] && !h["content-length"] && Array === b
        h["content-length"] = b.map(&:bytesize).reduce(0, :+).to_s
      end
      [st, h, b]
    end
  end
end
