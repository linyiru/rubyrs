# Divergence ratchet: `Array#first(bignum)` exception class.
#
# CRuby raises `RangeError: bignum too big to convert into `long``.
# rubyrs's `Array#first(n)` arm only matches `Value::Int(n)`, so a
# true BigInt arg falls through `do_call` to dispatcher fallback
# and surfaces as `NoMethodError: undefined method 'first' for Array`.
#
# When the divergence is fixed (BigInt arm added in vm/array.rs that
# raises RangeError to match CRuby), this fixture's .expected will
# need regeneration via `UPDATE_EXPECTED=1` AND the fix PR should
# un-skip the `# skipped (divergent): "raises a RangeError when count
# is a Bignum"` block in `spec/ruby/array_first_spec.rb`.

big = 99_999_999_999_999_999_999  # ≈ 1.0×10^20, between 2^66 and 2^67 — well past i64::MAX ⇒ BigInt
begin
  [].first(big)
  puts "first(bignum): no error"
rescue => e
  puts "first(bignum): #{e.class}"
end

# Same shape for Array#last(n).
begin
  [].last(big)
  puts "last(bignum): no error"
rescue => e
  puts "last(bignum): #{e.class}"
end
