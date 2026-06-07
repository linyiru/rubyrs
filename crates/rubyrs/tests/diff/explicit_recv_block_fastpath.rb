# Exercises the do_call_block explicit-receiver inline-cache fast path:
# user Object methods called WITH a literal block, at several arities.

class Widget
  def initialize(label)
    @label = label
  end

  # 0 args, yields once.
  def wrap
    "<#{@label}:#{yield}>"
  end

  # 1 arg, yields the arg.
  def each_char_upper(s)
    out = +""
    s.chars.each { |c| out << yield(c) }
    out
  end

  # 2 args, yields both.
  def combine(a, b)
    yield(a, b)
  end

  # 3 args.
  def triple(a, b, c)
    yield(a, b, c)
  end

  # block_given? must reflect the literal block.
  def maybe(x)
    if block_given?
      yield(x)
    else
      x
    end
  end

  # return value of the method (not the block).
  def tagged
    inner = yield
    "[#{@label}=#{inner}]"
  end

  # nested block calls: method-with-block invoking another
  # method-with-block on a fresh receiver.
  def nest(other)
    other.wrap { yield }
  end

  attr_reader :label
end

w = Widget.new("w")
other = Widget.new("o")

# arity 0
puts w.wrap { "hi" }

# arity 1
puts w.each_char_upper("abc") { |c| c.upcase }

# arity 2
puts w.combine(3, 4) { |a, b| a + b }

# arity 3
puts w.triple(1, 2, 3) { |a, b, c| a * b * c }

# block_given? true / false (regression: same method, with and without block)
puts w.maybe(10) { |x| x * 2 }
puts w.maybe(10)

# method return value distinct from block value
puts w.tagged { 99 }

# closure capture: block reads an outer variable
factor = 7
puts w.combine(5, 6) { |a, b| (a + b) * factor }

# closure mutation: block mutates an outer variable
total = 0
[1, 2, 3, 4].each { |n| total += w.combine(n, n) { |a, b| a + b } }
puts total

# nested block calls
puts w.nest(other) { "deep" }

# A private method called WITH a block must STILL work — it falls
# through to the slow path (the fast path only handles Public).
class Secret
  def expose
    compute { 21 }
  end

  private

  def compute
    yield * 2
  end
end
puts Secret.new.expose

# A method that takes a block but is invoked WITHOUT one, then later
# WITH one — same call site, exercises cache stability.
3.times do |i|
  if i.even?
    puts w.maybe(i) { |x| "even#{x}" }
  else
    puts w.maybe(i)
  end
end
