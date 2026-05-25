# Splat receive in multi-write assignment.

def s(v)
  if v.nil?
    "nil"
  elsif v.class.name == "Array"
    v.inspect
  else
    v.to_s
  end
end

# Trailing splat — splat absorbs everything after the pre-targets.
a, *r = [1, 2, 3, 4]
puts "1.a=#{s(a)}"
puts "1.r=#{s(r)}"

# Leading splat — splat absorbs everything before the post-targets.
*r, b = [1, 2, 3, 4]
puts "2.r=#{s(r)}"
puts "2.b=#{s(b)}"

# Middle splat — pre + splat + post.
a, *m, b = [1, 2, 3, 4, 5]
puts "3.a=#{s(a)}"
puts "3.m=#{s(m)}"
puts "3.b=#{s(b)}"

# Tight middle splat — only one element; pre wins, splat empty,
# post falls to nil (CRuby's "post can be starved" rule).
a, *m, b = [1]
puts "4.a=#{s(a)}"
puts "4.m=#{s(m)}"
puts "4.b=#{s(b)}"

# Source shorter than pre+post — post gets nil.
a, b, *m, c = [10, 20]
puts "5.a=#{s(a)}"
puts "5.b=#{s(b)}"
puts "5.m=#{s(m)}"
puts "5.c=#{s(c)}"

# Empty source array — pre and post both fall to nil.
a, *r = []
puts "6.a=#{s(a)}"
puts "6.r=#{s(r)}"

# Leading splat, multiple post; source shorter than post.
*m, a, b = [1]
puts "7.m=#{s(m)}"
puts "7.a=#{s(a)}"
puts "7.b=#{s(b)}"

# Anonymous splat — `*` with no binding name; discards the slice
# but still anchors the post-target counting.
*, b = [1, 2, 3]
puts "8.b=#{s(b)}"

a, *, b = [1, 2, 3, 4, 5]
puts "9.a=#{s(a)}"
puts "9.b=#{s(b)}"

# Splat into ivars, mixed with regular ivar targets.
class Bag
  attr_reader :head, :tail, :last
  def initialize(arr)
    @head, *@tail, @last = arr
  end
end
g = Bag.new([10, 20, 30, 40])
puts "10.h=#{g.head}"
puts "10.t=#{g.tail.inspect}"
puts "10.l=#{g.last}"

g2 = Bag.new([99])
puts "11.h=#{g2.head}"
puts "11.t=#{g2.tail.inspect}"
puts "11.l=#{s(g2.last)}"

# Splat from a method that returns an Array.
def trio
  [100, 200, 300]
end
x, *y = trio
puts "12.x=#{x}"
puts "12.y=#{y.inspect}"

# Splat with comma-RHS (Prism packs into ArrayNode at value slot).
a, *r = 1, 2, 3
puts "13.a=#{a}"
puts "13.r=#{r.inspect}"

# Splat captures partition output (the original motivating idiom
# now extended — though partition only returns 2 elements, this
# tests that splat works on method-call RHS).
first, *rest = [10, 20, 30, 40].select { |n| n > 15 }
puts "14.first=#{first}"
puts "14.rest=#{rest.inspect}"

# Strings work too (splat doesn't care about element type).
head, *mid, tail = ["a", "b", "c", "d"]
puts "15.h=#{head}"
puts "15.m=#{mid.inspect}"
puts "15.t=#{tail}"
