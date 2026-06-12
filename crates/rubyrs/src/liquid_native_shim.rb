# _liquid_native shim — injected by rubyrs right after `require
# "jekyll"` completes (see vm/kernel.rs). Routes Liquid template
# rendering through the liquidus native engine when the template AND
# the per-render values fit the supported subset; anything else falls
# back to the pure-Ruby liquid gem:
#   - per TEMPLATE: liquidus compile declines (unsupported tag/filter/
#     construct) → cached false, that template stays pure-liquid;
#   - per RENDER: a value resolves to a shape the model can't carry
#     (or the host declines mid-render) → that one render re-runs
#     through pure liquid;
#   - tag-free templates (markdown post bodies, excerpts — the bulk of
#     per-document renders) short-circuit Ruby-side: pure liquid
#     renders plain text to itself, so the original string IS the
#     output, no parse needed on either engine.
# Jekyll autoloads LiquidRenderer — force it in so the patch target
# exists (idempotent; a LoadError leaves the shim inert).
# RUBYRS_NATIVE_STATS=1: count native-accelerator hits vs declines
# (the 2.8x new-corpus regression hunt — see RUBYRS_REGEX_STATS for
# the pattern). Zero cost when the env var is unset (one nil check
# per counter site).
if ENV["RUBYRS_NATIVE_STATS"] && !$__rubyrs_native_stats
  $__rubyrs_native_stats = Hash.new(0)
  at_exit do
    $stderr.puts "[native-stats] " +
      $__rubyrs_native_stats.sort.map { |k, v| "#{k}=#{v}" }.join(" ")
  end
end

if defined?(__rubyrs_liquid_compile) && defined?(Jekyll)
  begin
    require "jekyll/liquid_renderer"
    require "jekyll/liquid_renderer/file"
  rescue LoadError
    nil
  end
end
if defined?(__rubyrs_liquid_compile) && defined?(Jekyll::LiquidRenderer::File)
  module Jekyll
    module LiquidusNative
      # filename → [tid, needs] | false (declined)
      @templates = {}

      class << self
        def compile(filename, content, site)
          cached = @templates[filename]
          return cached unless cached.nil?
          includes_dir = site.in_source_dir("_includes")
          baseurl = site.config["baseurl"].to_s
          tid = __rubyrs_liquid_compile(content, baseurl, includes_dir)
          @templates[filename] =
            if tid
              needs = __rubyrs_liquid_needs(tid).split("\n").map do |line|
                path, slice, size, fields = line.split("\t", 4)
                # Pre-split the walk path once at compile time — the
                # render loop walks every need on EVERY render
                # (1371 renders on jekyll-1k), and the per-render
                # `path.split(".")` allocation showed up in the
                # post-dual-engine profile residue.
                [path, path.split("."), slice == "-" ? nil : slice.to_i, size == "1",
                 fields.to_s.empty? ? nil : fields.split(",")]
              end
              [tid, needs]
            else
              false
            end
        end

        # Render via the host. nil = decline (caller falls back to
        # pure liquid for this render).
        #
        # `site.*` values are immutable for the duration of a build
        # (the posts list is fixed before rendering starts), so their
        # resolution — including materializing sliced post fields
        # through to_liquid — is cached once per (path, slice, fields)
        # instead of repeated for all N pages.
        def render(entry, payload)
          tid, needs = entry
          values = {}
          needs.each do |path, segs, slice, need_size, fields|
            if path.start_with?("site.")
              key = "#{path}|#{slice}|#{fields ? fields.join(",") : ""}"
              cached = (@site_values ||= {})[key]
              if cached.nil?
                # ONE walk shared by the value and its #size — the
                # walked target can be expensive to produce anew
                # (SiteDrop#posts sorts all posts per CALL because
                # each render's payload carries a fresh drop, so its
                # @site_posts memo never helps; the old
                # resolve-then-resolve_full_size pair paid that sort
                # twice — measured 2 x 49 ms on liquid-1k).
                v = walk_segs(payload, segs)
                resolved = resolve_walked(v, slice, fields)
                return nil if resolved == :__decline
                full = nil
                if need_size
                  full = walked_size(v)
                  return nil if full == :__decline
                end
                cached = @site_values[key] = [resolved, full]
              end
              values[path] = cached[0]
              values[path + "#size"] = cached[1] if need_size
              next
            end
            v = walk_segs(payload, segs)
            resolved = resolve_walked(v, slice, fields)
            return nil if resolved == :__decline
            values[path] = resolved
            if need_size
              full = walked_size(v)
              return nil if full == :__decline
              values[path + "#size"] = full
            end
          end
          __rubyrs_liquid_render(tid, values)
        rescue StandardError
          nil
        end

        private

        # Walk a dotted path through the Liquid payload (drops answer
        # #[]). nil mid-walk is a legitimate liquid nil.
        def walk_segs(payload, segs)
          cur = payload
          i = 0
          n = segs.length
          while i < n
            return nil if cur.nil?
            cur = cur[segs[i]]
            i += 1
          end
          cur
        end

        # Value-side half of the old `resolve` — operates on an
        # already-walked target so the caller can share one walk
        # between the value and `walked_size`.
        def resolve_walked(v, slice, fields)
          if fields
            # Iterated collection: materialize just the sliced items'
            # fields as plain hashes (the host model can't carry
            # drops/documents).
            return nil if v.nil?
            return :__decline unless v.is_a?(Array)
            items = slice ? v[0, slice] : v
            return items.map do |item|
              liq = item.respond_to?(:to_liquid) ? item.to_liquid : item
              h = {}
              fields.each do |f|
                fv = liq[f]
                return :__decline unless scalar_ok?(fv)
                h[f] = fv
              end
              h
            end
          end
          deep_ok?(v) ? v : :__decline
        end

        def walked_size(v)
          return 0 if v.nil?
          return :__decline unless v.respond_to?(:size)
          v.size
        end

        def scalar_ok?(v)
          case v
          when nil, true, false, Integer, Float, String, Time then true
          else false
          end
        end

        # Values pass to the host as-is; the host's converter is the
        # final gate, but cheap Ruby-side checks avoid the host round
        # trip for obvious drop/object shapes.
        def deep_ok?(v, depth = 0)
          return false if depth > 8
          case v
          when nil, true, false, Integer, Float, String, Time then true
          when Array then v.all? { |e| deep_ok?(e, depth + 1) }
          when Hash
            v.all? { |k, e| k.is_a?(String) && deep_ok?(e, depth + 1) }
          else
            false
          end
        end
      end
    end

    class LiquidRenderer
      class File
        alias_method :__liquidus_orig_parse, :parse
        def parse(content)
          # Tag-free fast path: pure liquid renders plain text to
          # itself — skip BOTH engines (no Liquid::Template.parse per
          # document body/excerpt).
          if !content.include?("{{") && !content.include?("{%")
            @__liquidus_static = content
            @__liquidus_entry = nil
            return self
          end
          @__liquidus_static = nil
          site = @renderer.instance_variable_get(:@site)
          entry = Jekyll::LiquidusNative.compile(@filename, content, site)
          if $__rubyrs_native_stats && entry == false
            $__rubyrs_native_stats[:lq_tpl_decline] += 1
          end
          @__liquidus_entry = entry == false ? nil : entry
          if @__liquidus_entry
            # DEFER the pure-liquid parse: a native-armed template
            # renders through the host, so eagerly building the
            # pure template alongside doubled the parse cost
            # (~1.0B instructions on jekyll-1k, ~6% of the build).
            # The source is kept so a mid-render decline can still
            # parse lazily and fall back (see render!/render).
            @__liquidus_src = content
            return self
          end
          @__liquidus_src = nil
          __liquidus_orig_parse(content)
        end

        # Build the pure-liquid template on demand — only reached
        # when a native-armed template's render declines mid-flight.
        def __liquidus_ensure_parsed
          if (src = @__liquidus_src)
            @__liquidus_src = nil
            __liquidus_orig_parse(src)
          end
        end

        alias_method :__liquidus_orig_render!, :render!
        def render!(*args)
          return @__liquidus_static if @__liquidus_static
          if @__liquidus_entry
            out = Jekyll::LiquidusNative.render(@__liquidus_entry, args[0])
            if $__rubyrs_native_stats
              $__rubyrs_native_stats[out ? :lq_native : :lq_render_decline] += 1
            end
            return out if out
            __liquidus_ensure_parsed
          end
          __liquidus_orig_render!(*args)
        end

        alias_method :__liquidus_orig_render, :render
        def render(*args)
          return @__liquidus_static if @__liquidus_static
          if @__liquidus_entry
            out = Jekyll::LiquidusNative.render(@__liquidus_entry, args[0])
            if $__rubyrs_native_stats
              $__rubyrs_native_stats[out ? :lq_native : :lq_render_decline] += 1
            end
            return out if out
            __liquidus_ensure_parsed
          end
          __liquidus_orig_render(*args)
        end

        alias_method :__liquidus_orig_warnings, :warnings
        def warnings
          return [] if @__liquidus_static
          # Native-armed templates defer the pure parse, so there is
          # no @template to ask. Liquidus's compile whitelist keeps
          # deprecated-syntax shapes OUT of the native subset (they
          # decline), so the honest answer for an armed template is
          # "no warnings" — documented narrowing: a warning-bearing
          # template that still compiles natively would have its
          # warnings suppressed.
          return [] if @__liquidus_entry && @__liquidus_src
          __liquidus_orig_warnings
        end
      end
    end
  end
end
