# Kernel#catch / #throw and bare `raise` (re-raise). CRuby is the oracle.

# Basic: throw unwinds to the matching catch, which returns the value.
puts catch(:done) { throw :done, 42; "unreached" }.inspect      # 42

# No throw: catch returns the block's value.
puts catch(:x) { 7 + 8 }.inspect                                 # 15

# throw across a native iterator (each).
puts catch(:found) {
  [10, 20, 30].each { |n| throw :found, n if n == 20 }
  :none
}.inspect                                                         # 20

# throw from inside a helper method called within the catch block.
def search(list, target)
  list.each { |x| throw :hit, "found #{x}" if x == target }
  "miss"
end
puts catch(:hit) { search([1, 2, 3], 2) }.inspect                # "found 2"

# Nested catches resolve to the right tag (inner throw targets outer).
puts catch(:outer) {
  catch(:inner) { throw :outer, "skip-inner" }
  "inner-returned"
}.inspect                                                         # "skip-inner"

# Uncaught throw -> UncaughtThrowError (an ArgumentError).
begin
  catch(:a) { throw :b, 1 }
rescue UncaughtThrowError => e
  puts "uncaught: tag=#{e.tag.inspect} is_arg_error=#{e.is_a?(ArgumentError)}"
end

# Bare `raise` re-raises the current exception unchanged.
begin
  begin
    raise ArgumentError, "boom"
  rescue
    raise # re-raise
  end
rescue => e
  puts "reraised: #{e.class}: #{e.message}"                       # ArgumentError: boom
end

# NOTE: bare `raise` with NO active exception is intentionally not
# asserted here. rubyrs documents that `$!` is not cleared when a rescue
# body exits (a Tier-1 dynamic-scope divergence — see SUBSET.md), so a
# later context-free `raise` re-raises the last-seen exception instead of
# producing a fresh RuntimeError. The load-bearing case (re-raise *inside*
# a rescue) is covered above and matches CRuby.
