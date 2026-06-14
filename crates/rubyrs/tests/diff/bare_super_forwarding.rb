# Bare `super` (no parens) forwards the enclosing method's args AS
# RECEIVED: positionals with the `*rest` SPLATTED (not passed as one
# array), and the `&block` forwarded as a block (not a positional).
# Previously only a lone `*rest` was handled; `def m(a, *rest, &blk);
# super; end` over-counted args (e.g. broke Rack::Headers#fetch).

# (a) pre-rest + rest → user parent
class P1
  def g(a, *rest); "P1:#{a}/#{rest.inspect}"; end
end
class C1 < P1
  def g(a, *rest); a *= 2; super; end
end
p C1.new.g(5, 6, 7)        # "P1:10/[6, 7]"
p C1.new.g(5)              # "P1:10/[]"

# (b) pre-rest + rest + &block → user parent (block forwarded)
class P2
  def g(a, *rest, &blk); "P2:#{a}/#{rest.inspect}/#{blk ? blk.call : 'noblk'}"; end
end
class C2 < P2
  def g(a, *rest, &blk); super; end
end
p C2.new.g(1, 2) { "BLK" }  # "P2:1/[2]/BLK"
p C2.new.g(1, 2)            # "P2:1/[2]/noblk"

# (c) no rest, but &block → forwarded as block, not positional
class P3
  def g(a, b, &blk); "P3:#{a},#{b}/#{blk ? blk.call : 'noblk'}"; end
end
class C3 < P3
  def g(a, b, &blk); super; end
end
p C3.new.g(1, 2) { "Y" }    # "P3:1,2/Y"
p C3.new.g(3, 4)            # "P3:3,4/noblk"

# (d) lone *rest still works (the old fast path)
class P5
  def g(a, b = 0); "P5:#{a},#{b}"; end
end
class C5 < P5
  def g(*); super; end
end
p C5.new.g(7, 8)            # "P5:7,8"

# (f) pure positional super unchanged
class P6
  def g(a, b); "P6:#{a},#{b}"; end
end
class C6 < P6
  def g(a, b); super; end
end
p C6.new.g(1, 2)            # "P6:1,2"

# (g) bare super to a NATIVE parent with rest+block (the Hash#fetch shape)
class MyH < Hash
  def fetch(key, *default, &block)
    key = key.to_s.downcase
    super
  end
end
mh = MyH.new
mh["foo"] = "1"
p mh.fetch("FOO")               # "1"
p mh.fetch("MISS", "d")         # "d"
p mh.fetch("MISS") { "blk" }    # "blk"
