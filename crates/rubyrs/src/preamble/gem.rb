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
      @parts = @str.split(".").map { |p| p =~ /\A\d+\z/ ? p.to_i : p }
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
