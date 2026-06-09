# Array.new without a block: size, size+fill (fill SHARED), Array copy,
# negative size → ArgumentError. (Was returning a bare `#<Array>`.)
p Array.new
p Array.new(3)
p Array.new(3, 0)
p Array.new(0, 5)
p Array.new(2, "x")
p Array.new([1, 2, 3])
p Array.new(3, "x").map(&:object_id).uniq.length   # fill shared → 1
begin; Array.new(-1); rescue => e; p [e.class, e.message]; end
begin; Array.new(:sym); rescue => e; p e.class; end
p Array.new(3) { |i| i * i }                        # block form unchanged
