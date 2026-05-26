# `foo(&nil)` is CRuby's "no-block" shape. Surfaces most often
# in forwarders that re-pass a captured `&block` parameter:
#
#   def render(scope=nil, locals=nil, &block)
#     evaluate(scope, locals, &block)
#   end
#
# When `render` is called with no block, `block` is nil, and the
# inner `evaluate(..., &block)` forwards a nil. CRuby treats that
# as "call without a block"; rubyrs used to ICE in `do_call_block`
# because the `&block` slot wasn't a Proc / Block / BoundMethod.
# tilt's render path was the canonical caller. Fixed in
# vm/dispatch.rs.

# --- Bare &nil ---
def greet(name)
  block_given? ? yield(name) : "hi, #{name}"
end

b = nil
puts greet("world", &b)

# --- Forwarder shape (the tilt-render case) ---
def outer(name, &block)
  inner(name, &block)
end
def inner(name)
  block_given? ? yield(name) : "plain #{name}"
end
puts outer("a")                              # no block → "plain a"
puts outer("b") { |n| "blocked #{n}" }       # block → "blocked b"

# --- Non-Proc / non-Nil &arg is still a TypeError ---
# (Previously also ICE'd; now raises like CRuby.)
begin
  greet("x", &42)
rescue TypeError => e
  puts "caught: #{e.message}"
end
