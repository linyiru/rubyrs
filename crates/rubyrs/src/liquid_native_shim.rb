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
                [path, slice == "-" ? nil : slice.to_i, size == "1",
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
          needs.each do |path, slice, need_size, fields|
            if path.start_with?("site.")
              key = "#{path}|#{slice}|#{fields ? fields.join(",") : ""}"
              cached = (@site_values ||= {})[key]
              if cached.nil?
                resolved = resolve(payload, path, slice, fields)
                return nil if resolved == :__decline
                full = nil
                if need_size
                  full = resolve_full_size(payload, path)
                  return nil if full == :__decline
                end
                cached = @site_values[key] = [resolved, full]
              end
              values[path] = cached[0]
              values[path + "#size"] = cached[1] if need_size
              next
            end
            resolved = resolve(payload, path, slice, fields)
            return nil if resolved == :__decline
            values[path] = resolved
            if need_size
              full = resolve_full_size(payload, path)
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
        def walk(payload, path)
          cur = payload
          path.split(".").each do |seg|
            return nil if cur.nil?
            cur = cur[seg]
          end
          cur
        end

        def resolve(payload, path, slice, fields)
          v = walk(payload, path)
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

        def resolve_full_size(payload, path)
          v = walk(payload, path)
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
          @__liquidus_entry = entry == false ? nil : entry
          __liquidus_orig_parse(content)
        end

        alias_method :__liquidus_orig_render!, :render!
        def render!(*args)
          return @__liquidus_static if @__liquidus_static
          if @__liquidus_entry
            out = Jekyll::LiquidusNative.render(@__liquidus_entry, args[0])
            return out if out
          end
          __liquidus_orig_render!(*args)
        end

        alias_method :__liquidus_orig_render, :render
        def render(*args)
          return @__liquidus_static if @__liquidus_static
          if @__liquidus_entry
            out = Jekyll::LiquidusNative.render(@__liquidus_entry, args[0])
            return out if out
          end
          __liquidus_orig_render(*args)
        end

        alias_method :__liquidus_orig_warnings, :warnings
        def warnings
          return [] if @__liquidus_static
          # Native-rendered templates also parsed through pure liquid
          # in #parse, so @template exists and reports real warnings.
          __liquidus_orig_warnings
        end
      end
    end
  end
end
