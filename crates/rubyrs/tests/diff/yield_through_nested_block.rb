# `yield` resolves to the enclosing METHOD's block, not the
# current block frame's. CRuby walks the cfp chain looking for
# the nearest method-context frame; rubyrs previously stopped
# at `frames.last().block_arg`, which for a yield inside a
# nested block is the BLOCK's own (absent) block_arg → spurious
# "no block given (yield)" RuntimeError.
#
# Motivating use: MRI's `lib/erb/compiler.rb:166-169` —
#   def scan_line(line)
#     line.scan(@scan_reg) do |tokens|
#       tokens.each do |token|
#         yield(token)           # ← must reach scan_line's caller's block
#       end
#     end
#   end
# Without the walk, every ERB compile call raises.

# --- Basic: yield inside a single nested block ---
class C
  def each_doubled(arr)
    arr.each do |x|
      yield(x * 2)
    end
  end
end
out = []
C.new.each_doubled([1, 2, 3]) { |y| out << y }
puts out.inspect                                # [2, 4, 6]

# --- Two levels deep ---
class D
  def deep(arr)
    arr.each do |group|
      group.each do |x|
        yield(x)
      end
    end
  end
end
out = []
D.new.deep([[1, 2], [3, 4]]) { |x| out << x }
puts out.inspect                                # [1, 2, 3, 4]

# --- Three levels deep with a Method#call entry ---
# Combines yield-through-nesting with Method#call's block
# forwarding (the exact ERB shape).
class E
  def initialize
    @inner = self.method(:do_scan)
  end
  def do_scan(items)
    items.each do |group|
      group.each do |x|
        yield(x.upcase) if !x.empty?
      end
    end
  end
  def scan(items, &block)
    @inner.call(items, &block)
  end
end
out = []
E.new.scan([%w[a b], %w[] , %w[c]]) { |s| out << s }
puts out.inspect                                # ["A", "B", "C"]

# --- yield from inside a block while the method ALSO has &block ---
# Both the explicit-named block and bare yield should resolve to
# the SAME enclosing-method block.
class F
  def both(arr, &blk)
    arr.each do |x|
      # Even with &blk in scope, plain yield should match.
      yield(x)
      blk.call(x * 10)
    end
  end
end
out = []
F.new.both([1, 2]) { |x| out << x }
puts out.inspect                                # [1, 10, 2, 20]

# --- block_given? inside a nested block ---
# Same lookup rule — `block_given?` reports the enclosing
# method's block state, not the nested block frame's.
class G
  def check(arr)
    arr.each do |_x|
      return block_given?
    end
  end
end
puts G.new.check([1]) { }                       # true
puts G.new.check([1])                           # false

# --- No-block call still raises with the expected message ---
class H
  def yield_from_nest
    [1].each do |_x|
      yield
    end
  end
end
begin
  H.new.yield_from_nest
rescue LocalJumpError, RuntimeError => e
  puts e.message                                # no block given (yield)
end
