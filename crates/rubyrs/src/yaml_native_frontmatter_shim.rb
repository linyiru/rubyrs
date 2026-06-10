# rubyrs `_yaml_native` front-matter accelerator shim.
#
# Injected by kernel.rs right after the top-level `require "jekyll"`
# returns true (same hook point as the liquid shim). Replaces the
# per-document read path — `File.read` + YAML_FRONT_MATTER_REGEXP
# match + `Regexp.last_match.post_match` — with one
# `__rubyrs_frontmatter_read` host call that does file read, UTF-8
# BOM strip and front-matter split natively. The YAML text still goes
# through `SafeYAML.load`, which already routes to
# `__rubyrs_yaml_parse`, so YAML semantics live in exactly one place.
#
# Gate: the native path only engages when the effective File.read
# options are Jekyll's defaults (no per-call opts; site encoding
# utf-8, which `Utils.merged_file_read_opts` turns into "bom|utf-8").
# Anything else — custom encodings, host-fn decline (non-UTF-8 file,
# IO error) — falls back to the ORIGINAL implementation below, which
# re-raises real Errno/Psych errors with their CRuby shapes.
#
# Known, accepted divergence: the native path does not set `$~` /
# `Regexp.last_match` as a side effect of the front-matter match.
# Jekyll itself reads `Regexp.last_match` only INSIDE these methods
# (replaced wholesale here); nothing downstream consults it.
# `Jekyll::Document` / `Convertible` are autoload-ed — at this hook
# point (top-level `require "jekyll"` just returned) they are NOT yet
# materialized, and `defined?(Jekyll::Document)` is nil. Force-require
# them, same move as the liquid shim's `require "jekyll/liquid_renderer"`.
if defined?(__rubyrs_frontmatter_read) && defined?(Jekyll)
  require "jekyll/document"
  require "jekyll/convertible"
end
if defined?(__rubyrs_frontmatter_read) && defined?(Jekyll::Document)
  module RubyrsFrontmatterNative
    # True when `Utils.merged_file_read_opts(site, {})` would produce
    # the default "bom|utf-8" read — the only shape the host fn
    # implements.
    def self.native_opts?(site)
      return false unless site
      opts = site.file_read_opts
      return true if opts.nil? || opts.empty?
      return false unless opts.size == 1
      # Symbol key only: CRuby's File.read IGNORES a String "encoding"
      # key, so a string-keyed opts hash must take the pure path
      # (where the BOM stays in the content) to match that behaviour.
      enc = opts[:encoding]
      # Site#initialize already BOM-prefixes the configured encoding,
      # so the default arrives here as "bom|utf-8" (and
      # `merged_file_read_opts` leaves it alone — its `start_with?
      # ("utf-")` check doesn't match the "bom|" prefix). Accept both
      # spellings; the host fn implements exactly this read shape.
      enc.is_a?(String) && ["utf-8", "bom|utf-8"].include?(enc.downcase)
    end

    # One host call: returns [content, yaml_or_nil], or nil when the
    # host fn declines (shim then takes the pure path).
    def self.read(path)
      __rubyrs_frontmatter_read(path)
    rescue StandardError
      nil
    end
  end

  module Jekyll
    class Document
      def read_content(**opts)
        if opts.empty? && RubyrsFrontmatterNative.native_opts?(site) &&
           (pair = RubyrsFrontmatterNative.read(path))
          self.content = pair[0]
          if (yaml_src = pair[1])
            data_file = SafeYAML.load(yaml_src)
            merge_data!(data_file, :source => "YAML front matter") if data_file
          end
          return
        end
        # Pure path — byte-for-byte the original Jekyll 4.4.1 body.
        self.content = File.read(path, **Utils.merged_file_read_opts(site, opts))
        if content =~ YAML_FRONT_MATTER_REGEXP
          self.content = Regexp.last_match.post_match
          data_file = SafeYAML.load(Regexp.last_match(1))
          merge_data!(data_file, :source => "YAML front matter") if data_file
        end
      end
    end

    module Convertible
      def read_yaml(base, name, opts = {})
        filename = @path || site.in_source_dir(base, name)
        Jekyll.logger.debug "Reading:", relative_path

        begin
          if opts.empty? && RubyrsFrontmatterNative.native_opts?(site) &&
             (pair = RubyrsFrontmatterNative.read(filename))
            self.content = pair[0]
            if (yaml_src = pair[1])
              self.data = SafeYAML.load(yaml_src)
            end
          else
            # Pure path — original Jekyll 4.4.1 body.
            self.content = File.read(filename, **Utils.merged_file_read_opts(site, opts))
            if content =~ Document::YAML_FRONT_MATTER_REGEXP
              self.content = Regexp.last_match.post_match
              self.data = SafeYAML.load(Regexp.last_match(1))
            end
          end
        rescue Psych::SyntaxError => e
          Jekyll.logger.warn "YAML Exception reading #{filename}: #{e.message}"
          raise e if site.config["strict_front_matter"]
        rescue StandardError => e
          Jekyll.logger.warn "Error reading file #{filename}: #{e.message}"
          raise e if site.config["strict_front_matter"]
        end

        self.data ||= {}

        validate_data! filename
        validate_permalink! filename

        self.data
      end
    end
  end
end
