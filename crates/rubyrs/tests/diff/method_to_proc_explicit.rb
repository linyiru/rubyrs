# Method#to_proc — explicit conversion of a BoundMethod to a Proc.
# Routes through the same forwarder as the implicit `&m` coercion;
# the resulting Proc splats its args back into the underlying
# BoundMethod's `.call(...)`.

class C
  def dbl(x); x * 2; end
  def add(a, b); a + b; end
end

c = C.new

# Basic conversion + class reporting.
p = c.method(:dbl).to_proc
puts p.class.name                   # Proc
puts p.(5)                          # 10
puts p.call(7)                      # 14
puts p[6]                           # 12

# Stored and reused — same Proc instance can be invoked many times.
puts p.(8)                          # 16

# Used as a block via &-forwarding. Both `&m` (implicit) and
# `&m.to_proc` (explicit) reach the same place.
puts [1, 2, 3].map(&c.method(:dbl).to_proc).inspect   # [2, 4, 6]
puts [1, 2, 3].map(&c.method(:dbl)).inspect           # [2, 4, 6]

# Multi-arg: to_proc works with arbitrary arity.
add_p = c.method(:add).to_proc
puts add_p.(3, 4)                   # 7
puts [1, 2, 3].inject(0, &add_p)    # 6
