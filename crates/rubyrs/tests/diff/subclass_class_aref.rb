# A Hash / Array subclass that REDEFINES the class method `self.[]`
# must reach its own override, not the native `Hash[]` / `Array[]`
# constructor. rack's Rack::Headers does
# `def self.[](*items); new.merge!(items.first); end` to downcase keys
# at construction — previously the native intercept shadowed it, so
# `Headers['AB'=>1]` kept the original-case key.
# A subclass that merely INHERITS Hash.[] (no override) still gets the
# native tagged-instance build (e.g. Jekyll::Configuration[override]).

class DownHash < Hash
  def []=(k, v); super(k.to_s.downcase, v); end
  def update(h); h.each { |k, v| self[k] = v }; self; end
  alias merge! update
  def self.[](*items); new.merge!(items.first); end
end

p DownHash['AB' => 1, 'Cd' => 2]   # {"ab"=>1, "cd"=>2}
p DownHash.[]('EF' => 3)           # {"ef"=>3}
p DownHash['Gh' => 4]['gh']        # 4

# Array subclass overriding self.[].
class SortArr < Array
  def self.[](*items); new.replace(items.sort); end
end
p SortArr[3, 1, 2]                 # [1, 2, 3]
p SortArr["b", "a", "c"]           # ["a", "b", "c"]

# A plain subclass that does NOT override self.[] still builds a tagged
# instance via the inherited native constructor.
class PlainH < Hash; end
ph = PlainH['X' => 1]              # native Hash[] → keys unchanged
p ph                               # {"X"=>1}
p ph.class                         # PlainH
p ph.is_a?(Hash)                   # true

class PlainA < Array; end
pa = PlainA[9, 8]
p pa                               # [9, 8]
p pa.class                         # PlainA

# The literal Hash / Array class still uses the native constructor.
p Hash['k' => 'v']                 # {"k"=>"v"}
p Array[1, 2, 3]                   # [1, 2, 3]
