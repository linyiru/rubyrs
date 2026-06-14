# A Hash SUBCLASS constructed with a default value or default block
# (`Sub.new(d)` / `Sub.new { |h,k| ... }`) gets the default, just like
# `Hash.new`. Previously the `Hash.new`-default intercept only matched
# the exact `Hash` class, so subclass instances returned nil for
# missing keys. (rack's Rack::Headers.new('1') relies on this.)

class HS < Hash; end

# scalar default
h = HS.new("dflt")
p h.class.name           # "HS"
p h["missing"]           # "dflt"
p h.default              # "dflt"
h["a"] = 1
p h["a"]                 # 1 (present key unaffected)
p h["other"]             # "dflt"

# default block
hb = HS.new { |hash, k| "blk:#{k}" }
p hb.class.name          # "HS"
p hb["x"]                # "blk:x"
p hb.key?("x")           # false (block doesn't store by default)

# no default → nil
hn = HS.new
p hn["z"]                # nil

# plain Hash unaffected
p Hash.new("p")["m"]     # "p"

# a subclass with its OWN initialize runs it (not the native default)
class HI < Hash
  def initialize
    super
    self["seeded"] = "yes"
  end
end
hi = HI.new
p hi["seeded"]           # "yes"
p hi["missing"]          # nil (no default set)
