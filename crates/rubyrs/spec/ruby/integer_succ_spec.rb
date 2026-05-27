# Adapted from ruby/spec core/integer/succ_spec.rb +
# core/integer/next_spec.rb + shared/next.rb at upstream commit
# 448cb340 (2026-05). Hand-translated — the upstream files
# delegate to `it_behaves_like :integer_next, :succ` / `:next`
# against shared/next.rb. We inline the runnable Fixnum cases
# for both method names and drop the four Bignum/overflow
# blocks that depend on `bignum_value` / `fixnum_max` /
# `fixnum_min` fixtures.

describe "Integer#succ / Integer#next" do
  it "returns the next larger positive Fixnum (succ)" do
    assert_eq(2.succ, 3)
  end

  it "returns the next larger negative Fixnum (succ)" do
    assert_eq((-2).succ, -1)
  end

  it "returns the next larger positive Fixnum (next)" do
    assert_eq(2.next, 3)
  end

  it "returns the next larger negative Fixnum (next)" do
    assert_eq((-2).next, -1)
  end

  it "succ and next return the same value" do
    assert_eq(5.succ, 5.next)
    assert_eq(0.succ, 0.next)
    assert_eq((-100).succ, (-100).next)
  end

  # skipped (fixture): it "returns the next larger positive Bignum" do
  #   Uses `bignum_value` upstream fixture.
  # skipped (fixture): it "returns the next larger negative Bignum" do
  # skipped (fixture): it "overflows a Fixnum to a Bignum" do
  #   Uses `fixnum_max` upstream fixture.
  # skipped (fixture): it "underflows a Bignum to a Fixnum" do
  #   Uses `fixnum_min` upstream fixture.
end
