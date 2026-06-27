# Minimal `Gem::Version` shim. CRuby has `Gem` always loaded
# because RubyGems auto-runs at interpreter startup; rubyrs
# has no RubyGems, but ecosystem code (Sinatra 4's
# `sinatra/indifferent_hash.rb:189`) uses `Gem::Version.new(...)`
# at class-body load time for version gating:
#
#   def except(*keys) ... end if Gem::Version.new(RUBY_VERSION) >= Gem::Version.new("3.0")
#
# Without `Gem::Version` the require raises NameError and the
# whole gem fails to load. The shim covers the load-time
# surface (`.new(str)` and `<=>` / `>=` comparison); the full
# RubyGems API stays out of scope.
#
# Comparison is lexicographic on the dotted-int parts cast to
# integers — close enough to RubyGems::Version semantics for
# the load-time gating that Sinatra / Mustermann / Rack do.
# Pre-release suffixes (e.g. "3.0.0.beta1") aren't modelled;
# they compare lexically as strings within their position,
# matching CRuby for the simple "X.Y" shapes ecosystem code
# actually compares against.

module Gem
  class Version
    include Comparable

    attr_reader :parts

    def initialize(str)
      @str = str.to_s
      # Avoid regex literals so this preamble loads in
      # no-regex builds (wasm32-wasip1 / `--no-default-features`).
      # All-digits check is byte-level: every char must be 0..9.
      # Equivalent to the original `=~ /\A\d+\z/` branch shape
      # — version strings don't carry signs in practice
      # (RubyGems sources are positive dotted ints).
      @parts = @str.split(".").map do |p|
        all_digits = !p.empty? && p.chars.all? { |c| c >= "0" && c <= "9" }
        all_digits ? p.to_i : p
      end
    end

    def <=>(other)
      return nil unless other.is_a?(Version)
      a = @parts
      b = other.parts
      len = a.length > b.length ? a.length : b.length
      i = 0
      while i < len
        ap = i < a.length ? a[i] : 0
        bp = i < b.length ? b[i] : 0
        # Coerce to int when comparing across kinds so
        # 0 <=> "0" doesn't blow up; preserves int ordering.
        if ap.is_a?(Integer) && bp.is_a?(Integer)
          c = ap <=> bp
        else
          c = ap.to_s <=> bp.to_s
        end
        return c unless c == 0
        i += 1
      end
      0
    end

    def to_s
      @str
    end
  end
end

module Gem
  # Plugin discovery (`Gem.find_files("minitest/*_plugin.rb")`).
  # rubyrs has no gem installation database to search — requireable
  # code comes from explicit `$LOAD_PATH` entries — so the answer
  # is always "no plugin files found". Returning [] (not raising)
  # matches how minitest/rake degrade when no plugins exist.
  def self.find_files(_glob)
    []
  end

  # Gem install locations. rubyrs has no gem database; derive a best-effort
  # answer from GEM_HOME / GEM_PATH (set by rbenv/bundler) so consumers like
  # ActiveSupport::BacktraceCleaner#add_gem_filter (which only builds a
  # cosmetic backtrace regexp and guards the empty case) work without raising.
  def self.default_dir
    ENV["GEM_HOME"] || ""
  end

  def self.dir
    default_dir
  end

  def self.path
    gp = ENV["GEM_PATH"]
    list = (gp.nil? || gp.empty?) ? [default_dir] : gp.split(":")
    list.reject { |p| p.nil? || p.empty? }
  end
end
