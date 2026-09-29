# Returns that take the plain-return fast path (no ensure, rescue, loop
# or class-body state in the frame) next to the shapes that must not.

class O
  def n; end
  def v; 42; end
  def early(x)
    return :neg if x < 0
    :pos
  end
  def mid_expr(x)
    [1, 2, (x > 0 ? (return x * 10) : 3)]
  end
  def matcher(s)
    s =~ /(\d+)/
    $1
  end
  def with_ensure
    return :body
  ensure
    puts "ensure ran"
  end
  def with_loop
    i = 0
    while i < 3
      return i if i == 2
      i += 1
    end
  end
  def yielder
    yield + 1
  end
end

o = O.new
p o.n, o.v, o.early(-1), o.early(1), o.mid_expr(4), o.mid_expr(-4)

# `$~` is method-local: a callee's match must not leak into the caller.
"abc" =~ /(b)/
p o.matcher("x 123 y")
p $1

p o.with_ensure
p o.with_loop
p o.yielder { 41 }
p [1, 2, 3].map { |x| o.early(x - 2) }

# Deep chain of plain returns, then the stack must be balanced.
def fib(n) = n < 2 ? n : fib(n - 1) + fib(n - 2)
p fib(15)
acc = 0
10_000.times { acc += o.v }
p acc

# A class body takes the full return path; the caller must still work.
class Foo; def self.k = o_k; end
def o_k = :k
p Foo.k
