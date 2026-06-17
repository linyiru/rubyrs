# Keyword args in eval'd code. The eval body runs synchronously inside
# the native `eval` dispatch, which had left the "trailing hash is
# positional" flag stale-TRUE — so a kwarg call inside eval bound its
# kwargs POSITIONALLY ("wrong number of arguments (given 1, expected
# 0)" against a kwarg-only method). Surfaced by connection_pool's smoke
# (`ConnectionPool.new(size: 1) { ... }.with { ... }`) run via eval.
def foo(a:, b: 0); [a, b]; end
p eval("foo(a: 2)")           # [2, 0]
p eval("foo(a: 3, b: 4)")     # [3, 4]
p eval("foo(**{a: 5})")       # [5, 0]
p eval("foo(**{a: 6, b: 7})") # [6, 7]

# kwarg-only constructor via eval
class Widget
  def initialize(size:, name: "w"); @size = size; @name = name; end
  def to_a; [@size, @name]; end
end
p eval("Widget.new(size: 1).to_a")            # [1, "w"]
p eval("Widget.new(size: 2, name: 'x').to_a") # [2, "x"]

# block + kwargs combo via eval (the CallBlock path)
class Box
  def initialize(cap:); @cap = cap; @v = yield; end
  def info; [@cap, @v]; end
end
p eval("Box.new(cap: 3) { 99 }.info")         # [3, 99]

# nested: eval inside eval, kwargs at each level
p eval("eval('foo(a: 8)')")                   # [8, 0]

# a positional brace hash to a no-kwarg method stays positional in eval
def bar(h); h[:k]; end
p eval("bar({k: 10})")                        # 10
