# Small-hash representation battery (2026-07 record-shape campaign:
# SmallVec inline pairs + boxed cold tail). Pins every observable the
# representation change could disturb, with the inline-cap (3) and
# index-threshold (16) boundaries crossed explicitly. CRuby is the
# oracle; each section must stay byte-identical.

puts "== insertion order & delete/reinsert =="
h = { "a" => 1, "b" => 2, "c" => 3, "d" => 4 }
h.delete("b")
h["b"] = 99 # reinsert AFTER delete goes to the END
p h.keys
h2 = { "a" => 1, "b" => 2, "c" => 3 }
h2["b"] = 22 # overwrite while present keeps position
p h2.keys
p h2

puts "== shift then reinsert =="
h3 = { "a" => 1, "b" => 2, "c" => 3 }
p h3.shift
h3["a"] = 11
p h3

puts "== duplicate-key literal: first position, last value =="
h4 = { "x" => 1, "y" => 2, "x" => 3 }
p h4

puts "== inline-cap boundary (3->4) and index threshold (15->16->17) =="
[2, 3, 4, 5, 8, 9, 15, 16, 17, 33].each do |n|
  hb = {}
  (1..n).each { |i| hb["k#{i}"] = i * 10 }
  ok = (1..n).all? { |i| hb["k#{i}"] == i * 10 }
  # order stays insertion order at every size
  puts "n=#{n} size=#{hb.size} lookups=#{ok} first=#{hb.keys.first} last=#{hb.keys.last} miss=#{hb["nope"].inspect}"
  hb.delete("k2") if n >= 3
  hb["k2"] = -2 if n >= 3
  puts "  after del+reinsert: last=#{hb.keys.last} val=#{hb["k2"]}" if n >= 3
end

puts "== iteration matches keys/values/each at boundary sizes =="
[3, 4, 16, 17].each do |n|
  hb = {}
  (1..n).each { |i| hb[i] = i.to_s }
  seen = []
  hb.each { |k, v| seen << k }
  puts "n=#{n} each==keys #{seen == hb.keys} values_join=#{hb.values.join(",")[0, 20]}"
end

puts "== frozen semantics =="
hf = { "a" => 1, "b" => 2 }.freeze
p hf.frozen?
begin
  hf["c"] = 3
rescue FrozenError => e
  puts "aset: FrozenError"
end
begin
  hf.delete("a")
rescue FrozenError
  puts "delete: FrozenError"
end
begin
  hf.rehash
rescue FrozenError
  puts "rehash: FrozenError"
end
hd = hf.dup
hd["c"] = 3 # dup resets frozen
p hd
hc = hf.clone
puts "clone keeps frozen: #{hc.frozen?}"

puts "== defaults untouched by representation =="
hv = Hash.new { |hh, kk| hh[kk] = "auto-#{kk}" }
hv["x"]
p hv
puts "default_proc present: #{!hv.default_proc.nil?}"
hs = Hash.new(:dflt)
puts "#{hs["missing"].inspect} size=#{hs.size}"
hs["k"] = 1
puts "after insert, default still: #{hs["missing2"].inspect}"

puts "== Hash#hash content equality =="
p({ "a" => 1, "b" => 2 }.hash == { "b" => 2, "a" => 1 }.hash)
p({ "a" => 1 }.hash == { "a" => 2 }.hash)

puts "== comparison ops on small hashes =="
p({ "a" => 1 } < { "a" => 1, "b" => 2 })
p({ "a" => 1, "b" => 2 } <= { "a" => 1, "b" => 2 })
p({ "a" => 1, "b" => 2 } > { "a" => 1 })
p({ "a" => 2 } < { "a" => 1, "b" => 2 })

puts "== compare_by_identity flip on populated hash (module keys) =="
ma = Module.new
mb = Module.new
hi = { ma => :a, mb => :b }
hi.compare_by_identity
puts "flagged: #{hi.compare_by_identity?} size=#{hi.size}"
puts "stored-key lookup: #{hi[ma].inspect}"

puts "== subclass with ivars + growth across inline cap =="
class RecordHash < Hash
  def stamp!(v)
    @stamp = v
    self
  end
  def stamp
    @stamp
  end
end
rh = RecordHash.new
rh.stamp!(:s1)
(1..5).each { |i| rh["r#{i}"] = i }
puts "#{rh.class} #{rh.stamp.inspect} #{rh.size} #{rh["r4"]}"

puts "== dup/clone keep pairs + tag across the boundary =="
rd = rh.dup
rd["r6"] = 6
puts "#{rd.class} #{rd.size} #{rh.size}"

puts "== singleton method on a hash =="
hs2 = { "a" => 1 }
def hs2.shout
  "size=#{size}"
end
puts hs2.shout
hs2["b"] = 2
hs2["c"] = 3
hs2["d"] = 4 # crosses inline cap with a singleton installed
puts hs2.shout

puts "== to_a / merge / select keep order at boundaries =="
hm = { "a" => 1, "b" => 2, "c" => 3 }
p hm.merge({ "d" => 4, "a" => 9 })
p hm.select { |k, v| v >= 2 }
p hm.to_a.first
