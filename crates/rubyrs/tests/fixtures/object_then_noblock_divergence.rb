# rubyrs-only divergence test: `then` / `yield_self` without a
# block raise LocalJumpError (CRuby returns an Enumerator, but
# rubyrs has no Enumerator type yet so we surface a loud error
# instead of a silent NoMethodError).
#
# Lives outside tests/diff/ because the behavior intentionally
# differs from CRuby; the diff fixture covers everything that
# IS parity.

begin
  5.then
rescue LocalJumpError
  puts "then-noblock-ok"
end

begin
  5.yield_self
rescue LocalJumpError
  puts "yield_self-noblock-ok"
end

# Sanity: with a block, both still work normally.
puts 5.then { |n| n * 2 }
puts 5.yield_self { |n| n + 100 }
