# Adapted from ruby/spec core/integer/abs_spec.rb +
# shared/abs.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — the upstream file delegates via
# `it_behaves_like :integer_abs, :abs` against shared/abs.rb,
# whose body uses `value.send(@method)` to dispatch through
# the shared name. We inline the Fixnum case directly and
# drop the Bignum case (uses `bignum_value(N)` fixture; the
# magnitudes are large enough that pinning rubyrs's exact
# behavior is left to a dedicated diff fixture).

describe "Integer#abs" do
  it "returns self's absolute fixnum value" do
    { 0 => [0, -0, +0], 2 => [2, -2, +2], 100 => [100, -100, +100] }.each do |key, values|
      values.each do |value|
        assert_eq(value.abs, key)
      end
    end
  end

  # skipped (fixture): it "returns the absolute bignum value" do
  #   Uses `bignum_value(N)` upstream fixture; specific
  #   magnitudes (18446744073709551655) tested.
end
