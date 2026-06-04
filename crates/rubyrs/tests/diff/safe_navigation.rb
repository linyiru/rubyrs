# Safe-navigation `recv&.method(args)` — short-circuits to `nil`
# when the receiver IS `nil`, otherwise dispatches normally.
# Used by rack-cors's `vary_resource.vary_headers&.any?` pattern
# and the Ruby ecosystem broadly for nil-tolerant chains.

# Basic shape: nil receiver → nil; non-nil receiver → normal call.
x = nil
puts x&.length.inspect
y = "hi"
puts y&.length

# CRuby's `&.` short-circuits on `nil` ONLY, not every falsy value
# — `false&.to_s` still calls `to_s` and returns "false".
puts false&.to_s

# Chained safe-nav. `nil&.foo&.bar` is nil at the first step.
puts nil&.foo&.bar.inspect

# Safe-nav with args + block.
arr = [1, 2, 3]
puts arr&.map { |i| i * 2 }.inspect
puts (nil&.map { |i| i * 2 }).inspect

# Safe-nav single-eval guarantee: the receiver expression must
# fire exactly once. Use a counter to verify.
class Counter
  attr_reader :count
  def initialize; @count = 0; end
  def tick; @count += 1; self; end
  def value; 42; end
end

c = Counter.new
result = c.tick&.value
puts "count=#{c.count} result=#{result}"

# Nil-returning receiver: the call branch is skipped.
def maybe_nil(flag); flag ? "got" : nil; end
puts maybe_nil(true)&.upcase
puts maybe_nil(false)&.upcase.inspect

# Safe-nav on a method call (no explicit local). The receiver
# expression `expensive` runs once; if nil, the chain is skipped.
def expensive; nil; end
puts expensive&.upcase.inspect
