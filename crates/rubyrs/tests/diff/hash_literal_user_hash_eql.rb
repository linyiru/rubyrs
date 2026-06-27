# A Hash literal computes each key's `hash` (CRuby) and disambiguates collisions
# with `eql?`. rubyrs used to key object keys by identity, skipping both — so a
# key whose `hash` is overridden (here with the wrong arity) didn't raise, and
# equal-by-`eql?` keys didn't collapse. (zeitwerk's Cref::Map non-hashable guard.)
m = Module.new do
  def self.hash(_) = nil          # arity 1 — `{m => 0}` calls hash() → raises
end
result = begin
  h = { m => 0 }
  "no error size=#{h.size}"
rescue ArgumentError => e
  "ArgumentError: #{e.message}"
end
puts result

class KEqlX
  def hash; 7; end
  def eql?(other); other.is_a?(KEqlX); end
end
p({ KEqlX.new => 1, KEqlX.new => 2 }.size)   # 1 — collapse via hash+eql?
p({ a: 1, a: 2 })                            # {a: 2} — primitive fast path intact
p({ 1 => :a, 1.0 => :b }.size)               # 2 — Integer/Float not eql?
