# Escape-analysed frame locals (Locals::Stack / Vm::locals_arena):
# methods whose body contains no block literal keep their locals in a
# contiguous VM arena instead of an Rc<RefCell<Vec>> cell. This fixture
# pins the behavioural seams between the two representations — every
# case below mixes Stack frames (no block in the body) with Shared
# frames (block-creating methods, blocks themselves, closures).

# 1. Plain Stack methods: positional binding, locals, return values,
#    deep-ish recursion (arena grows and shrinks across the call tree).
def add(a, b)
  c = a + b
  d = c * 2
  d - c
end
puts add(3, 4)

def fib(n)
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end
puts fib(18)

# 2. Optional / rest / kwargs go through the full binder — still
#    Stack-eligible (no block in the body). The optional default
#    expression runs in the method prologue and writes a Stack slot.
def opt(a, b = a + 10, *rest, k: 3, **kw)
  [a, b, rest, k, kw]
end
p opt(1)
p opt(1, 2, 3, 4, k: 9, z: 5)

# 3. rescue => e binds the exception into a Stack slot
#    (RescueHandler.bind_slot writes through the arena).
def catches
  raise ArgumentError, "boom"
rescue => e
  e.message
end
puts catches

# 4. ensure + raise unwinding THROUGH Stack frames (the unwind pop
#    must release each frame's arena segment; later calls reuse it).
$order = []
def inner_raises
  x = 1
  raise "deep"
ensure
  $order << :inner_ensure
end
def mid_calls
  y = 2
  inner_raises
ensure
  $order << :mid_ensure
end
begin
  mid_calls
rescue => e
  $order << e.message
end
p $order
puts add(5, 6) # arena healthy after unwind

# 5. yield / block_given? from a Stack method (no CreateBlock in the
#    METHOD body — the block comes from the caller).
def with_yield
  yield 5
end
puts(with_yield { |v| v + 1 })

def maybe_yield
  block_given? ? yield(1) : :none
end
p maybe_yield
p(maybe_yield { |x| x + 41 })

# 6. Non-local return from a block unwinds through intermediate Stack
#    method frames (each_driver is Shared — it creates the block; the
#    helper it calls is Stack and must be popped + released cleanly).
def stack_helper(x)
  x * 2
end
def ret_from_block
  [1, 2, 3].each do |x|
    return stack_helper(x) if x == 2
  end
  :none
end
p ret_from_block

# 7. super through Stack methods (zero-arg and explicit).
class Base
  def greet(name)
    "hi #{name}"
  end
end
class Sub < Base
  def greet(name)
    "<" + super + ">"
  end
end
puts Sub.new.greet("bob")

# 8. Closure interop: a block-creating method (Shared) whose lambda
#    captures locals, called interleaved with Stack methods — the
#    capture must keep its own values while Stack frames churn the
#    arena around it.
def make_counter
  count = 0
  -> { count += 1 }
end
c1 = make_counter
c1.call
add(1, 1)
fib(8)
c1.call
puts c1.call

# 9. define_method body (closure-shared locals) calling Stack methods.
class Defined
  define_method(:twice) { |v| stack_helper(v) + stack_helper(v) }
end
def stack_helper2(v) = v + 100
puts Defined.new.twice(3)
puts stack_helper2(1)

# 10. Outer-scope writes from blocks still propagate while Stack
#     frames are interleaved inside the block body.
total = 0
[1, 2, 3].each do |i|
  total += stack_helper(i)
end
puts total

# 11. retry through a Stack frame's begin region.
def flaky
  attempts = 0
  begin
    attempts += 1
    raise "again" if attempts < 3
    attempts
  rescue
    retry
  end
end
puts flaky

# 12. send dispatch into Stack methods. (method(:add).call at
#     toplevel is a pre-existing rubyrs gap — by-name re-dispatch on
#     nil self — so it's not pinned here.)
puts send(:add, 10, 20)
puts send(:stack_helper, 21)
