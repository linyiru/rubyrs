# String#gsub with a lone regex pattern (no replacement, no block)
# returns an Enumerator. Driving it with a block re-runs gsub with that
# block, so the result is the substituted String; without a block it
# enumerates the matches.
p "hello".gsub(/l/).count
p "aaa".gsub(/a/).with_index { |m, i| i.to_s }
p "aaa".gsub(/a/).with_index(1) { |m, i| i.to_s }
p "a1b2c3".gsub(/\d/).map(&:to_i)
p "hello world".gsub(/o/).to_a
p "hello".gsub(/[aeiou]/).to_a

# The plain replacement / block forms are unaffected.
p "hello".gsub(/l/, "L")
p "hello".gsub(/l/) { |m| m.upcase }

# sub has NO enumerator form — a lone pattern is an arity error.
def rescued
  yield
rescue => e
  e.class
end
p rescued { "hello".sub(/l/) }
p rescued { "hello".sub("l") }
