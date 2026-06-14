# A Hash SUBCLASS that overrides `transform_keys` / `transform_values`
# must run ITS override, not the native arm. The block-driven collection
# path runs BEFORE user-method lookup, so without an explicit defer the
# override would be silently shadowed. (rack's Rack::Headers re-downcases
# keys inside its own transform_keys.)

class DownHash < Hash
  def []=(k, v)
    super(k.to_s.downcase, v)
  end

  def transform_keys(&blk)
    out = DownHash.new
    each { |k, v| out[blk.call(k)] = v }
    out
  end

  def transform_values(&blk)
    out = DownHash.new
    each { |k, v| out[k] = blk.call(v) }
    out
  end
end

h = DownHash.new
h["A"] = 1
h["B"] = 2

# transform_keys override must run: keys get re-downcased by []=
tk = h.transform_keys { |k| "X#{k.upcase}" }
p tk.class.name          # "DownHash"
p tk.keys.sort           # ["xa", "xb"]   (downcased by override's []=)
p tk.values.sort         # [1, 2]

# transform_values override must run
tv = h.transform_values { |v| v * 10 }
p tv.class.name          # "DownHash"
p tv.values.sort         # [10, 20]

# a subclass WITHOUT an override keeps native behaviour
class PlainSub < Hash; end
ps = PlainSub.new
ps["k"] = 5
r = ps.transform_values { |v| v + 1 }
p r["k"]                 # 6

# plain Hash unaffected
p({ "a" => 1 }.transform_keys(&:upcase))   # {"A"=>1}
