# Divergence ratchet: `Hash#fetch` arity error class.
#
# CRuby raises `ArgumentError: wrong number of arguments (given N,
# expected 1..2)` when fetch is called with 0 or >=3 positional
# args. rubyrs surfaces `NoMethodError: undefined method 'fetch'
# for Hash` for the same calls — the dispatcher's arity check
# routes failures through the no-method path.
#
# When fixed (arity gate in dispatch.rs raises ArgumentError for
# Hash#fetch's known 1..2 arity), regen this fixture via
# UPDATE_EXPECTED=1 AND un-skip the
# `# skipped (divergent): "raises an ArgumentError when not passed
# one or two arguments"` block in `spec/ruby/hash_fetch_spec.rb`.

# Zero args.
begin
  {}.fetch()
  puts "fetch(): no error"
rescue => e
  puts "fetch():       #{e.class}"
end

# Three args.
begin
  {}.fetch(1, 2, 3)
  puts "fetch(1,2,3): no error"
rescue => e
  puts "fetch(1,2,3): #{e.class}"
end

# One arg with missing key still raises the expected KeyError
# (this part is NOT divergent — included as a regression guard so
# the fix doesn't accidentally swap KeyError for ArgumentError too).
begin
  {}.fetch(:nope)
  puts "fetch(:nope): no error"
rescue => e
  puts "fetch(:nope): #{e.class}"
end
