# `Kernel#Array(obj)` coerces an arbitrary object via `to_ary` then
# `to_a` before wrapping in `[obj]` (CRuby's rb_Array). This also backs
# array-literal splat `[*obj]` / `[a, *obj, b]` and the splat-RHS
# multi-assign `x, y = *obj`, which all desugar through `Array(obj)`.
# rack's Response is destructured `status, headers, body = *response`.

class ToAObj
  def to_a; [1, 2, 3]; end
end
class ToAryObj
  def to_ary; [9, 8]; end
end
class Resp
  def to_a; [200, {}, ["body"]]; end
  alias to_ary to_a
end

p Array(ToAObj.new)        # [1, 2, 3]
p Array(ToAryObj.new)      # [9, 8] (to_ary preferred)
p Array(nil)               # []
p Array([5, 6])            # [5, 6] (Array passthrough)
p Array("x")               # ["x"] (no to_ary/to_a → wrap)
p Array(7)                 # [7]
p Array({a: 1, b: 2})      # [[:a, 1], [:b, 2]]

# Array-literal splat of a coercible object.
p [*ToAObj.new]            # [1, 2, 3]
p [0, *ToAObj.new, 9]      # [0, 1, 2, 3, 9]
p [*"scalar"]              # ["scalar"]
p [*nil]                   # []

# Splat-RHS multi-assign (the Rack::Response destructure shape).
a, b, c = *Resp.new
p [a, b, c]                # [200, {}, ["body"]]

# Bare multi-assign of a to_ary object (the MassignSplat path).
d, e = ToAryObj.new
p [d, e]                   # [9, 8]
