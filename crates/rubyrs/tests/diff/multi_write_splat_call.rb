# Multi-write with splat onto an attribute writer
# (`a, *b.attr = expr` shape). Pre-fix this tripped
# `unsupported splat target: CallTargetNode(...)` at AST
# lowering — only Local/Ivar/Global splat targets were
# wired despite the non-splat `MWT::Call` path being
# present.
#
# P3 Sinatra-spike gap: Mustermann's
# mustermann/ast/node.rb:216 reads
#   self.head, *self.payload = Array(payload)
# inside a class initializer; this was the final AST gap
# blocking `require 'sinatra/base'` from parsing through.

class Box
  attr_accessor :head, :payload
end

# 1. Basic splat into recv.attr from an Array literal.
b = Box.new
b.head, *b.payload = [1, 2, 3, 4]
puts "lit_head=#{b.head} lit_payload=#{b.payload.inspect}"

# 2. Single-element RHS — splat captures empty.
b.head, *b.payload = ["only"]
puts "single_head=#{b.head} single_payload=#{b.payload.inspect}"

# 3. Empty RHS — head goes nil, splat empty.
b.head, *b.payload = []
puts "empty_head=#{b.head.inspect} empty_payload=#{b.payload.inspect}"

# 4. The exact Mustermann shape — splat target on
# `self.attr` inside a method body.
class Mustermann
  attr_accessor :head, :payload
  def init(payload)
    self.head, *self.payload = Array(payload)
  end
end
m = Mustermann.new
m.init([10, 20, 30])
puts "m_arr_head=#{m.head} m_arr_payload=#{m.payload.inspect}"

# 5. `Array(non_array)` wraps a scalar — splat sees a
# single-element rest.
m.init(99)
puts "m_scalar_head=#{m.head} m_scalar_payload=#{m.payload.inspect}"

# 6. Mixed: positional + splat on attr + trailing
# positional. Splat captures the MIDDLE slice.
class Tri
  attr_accessor :a, :b, :c
end
t = Tri.new
t.a, *t.b, t.c = [1, 2, 3, 4, 5]
puts "tri_a=#{t.a} tri_b=#{t.b.inspect} tri_c=#{t.c}"

# 7. Splat target with NESTED receiver (`outer.inner.attr`).
class Outer
  attr_accessor :inner
end
class Inner
  attr_accessor :head, :rest
end
ow = Outer.new
ow.inner = Inner.new
ow.inner.head, *ow.inner.rest = [100, 200, 300, 400]
puts "nest_head=#{ow.inner.head} nest_rest=#{ow.inner.rest.inspect}"
