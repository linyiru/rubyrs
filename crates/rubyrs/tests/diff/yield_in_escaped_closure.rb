# `yield` inside a closure that OUTLIVES its defining method. The
# block executing the yield resolves to the block passed to the
# method that lexically encloses the closure — even after that
# method has returned. CRuby keeps the binding alive via the
# closure's captured cref; rubyrs captures it onto the BlockHandle
# at creation (`captured_yield_block`) and falls back to it in
# Op::Yield when the lexical-owner walk finds no live method frame.
#
# rack's Rack::Lint wraps the downstream app in a lambda and
# `yield`s the env/response from inside it — the shape this exercises.

# lambda yielding from inside a nested block, defining method gone
def stacked(app)
  lambda { |env| app.call(env).tap { |r| r[2] = yield r[2] } }
end
a = ->(e) { [200, { "ct" => "text/plain" }, ["body"]] }
wrapped = stacked(a) { |body| body.map(&:upcase) }
p wrapped.call({})

# escaped proc (not lambda) yields to its enclosing method's block
def make_proc
  proc { |v| yield v }
end
pr = make_proc { |x| x * 3 }
p pr.call(5)
p pr.call(7)

# closure stored in a global, invoked after the method returns
$saved = nil
def capture
  $saved = ->(v) { yield v.upcase }
end
capture { |x| "<#{x}>" }
p $saved.call("hi")

# two levels of escaped nesting: the innermost yield must still
# reach the outermost method's block
def level1(x)
  ->(y) { yield(x + y) }
end
inner = level1(10) { |n| n * 100 }
p inner.call(5)

# lexical (NOT dynamic) resolution still wins when the owner IS live:
# the `{ yield 1 }` block is defined in `outer`, so its yield binds to
# outer's block even though `inner` is the dynamic caller.
def outer
  inner_caller { yield 1 }
end
def inner_caller
  yield
end
p(outer { |x| "outer-#{x}" })
