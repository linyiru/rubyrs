# BoundMethod &-forwarding via implicit `to_proc`.
# `arr.map(&m)` where `m = obj.method(:foo)` synthesises a
# vararg-lambda forwarder so the BoundMethod can be passed as
# a block.

class Doubler
  def double(x); x * 2; end
  def add(a, b); a + b; end
end

d = Doubler.new

# Stored method, forwarded into Array#map.
m = d.method(:double)
puts [1, 2, 3].map(&m).inspect      # [2, 4, 6]

# Same but with `select` / `reject` semantics — the BoundMethod
# returns truthy/falsy.
class IsEven
  def call(x); x.even?; end
end
ie = IsEven.new
even_check = ie.method(:call)
puts [1, 2, 3, 4, 5, 6].select(&even_check).inspect   # [2, 4, 6]
puts [1, 2, 3, 4, 5, 6].reject(&even_check).inspect   # [1, 3, 5]

# Multi-arg method passed to inject.
adder = d.method(:add)
puts [1, 2, 3, 4].inject(0, &adder)                   # 10

# Primitive receiver — `7.method(:+)` then forward as block.
plus7 = 7.method(:+)
puts [10, 20, 30].map(&plus7).inspect                  # [17, 27, 37]

# Inside a method body — capture self.method(:foo) and use it.
# (Explicit `self.method(:foo)` rather than bare `method(:foo)`;
# bare-form implicit-self for `Object#method` is a deferred case,
# tracked in SUBSET.md.)
class Pipeline
  def transform(arr)
    arr.map(&self.method(:square))
  end
  def square(n); n * n; end
end
puts Pipeline.new.transform([1, 2, 3, 4]).inspect      # [1, 4, 9, 16]
