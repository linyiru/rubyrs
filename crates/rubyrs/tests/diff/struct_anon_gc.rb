# An anonymous Struct class (Struct.new(...).new(...)) is reachable only
# through its instances; the GC must keep its @__struct_attrs members
# Array alive across the new/initialize sequence. Heavy allocation here
# forces real GC cycles mid-construction.
results = []
500.times do |i|
  s = Struct.new(:a, :b, :c).new(i, "name#{i}", [i, i * 2])
  results << [s.a, s.b, s.c]
  junk = (0..20).map { |k| "garbage-#{i}-#{k}" }
end
p results.length
p results.first
p results.last
# subclass-of-anonymous-Struct factory shape
Point = Struct.new(:x, :y)
pts = (0...300).map { |i| Point.new(i, i + 1) }
junk2 = (0..5000).map { |k| k.to_s }
p pts.map { |pt| pt.x + pt.y }.sum
