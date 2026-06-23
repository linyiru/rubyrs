# Adapted from ruby/spec core/integer/element_reference_spec.rb at
# 2026-05. The Range form of Integer#[] (Ruby 2.7+): `n[i..j]` and
# `n[i...j]` extract a bitfield, an endless range is the full
# arithmetic right shift, and a beginless range raises ArgumentError.
# Covers the native try_push_int_bit_range arm.

describe "Integer#[] with a Range" do
  it "extracts an inclusive bitfield" do
    assert_eq(0b101101[1..3], 6)
    assert_eq(0b101101[2..5], 11)
    assert_eq(0b101101[10..20], 0)
  end

  it "extracts an exclusive bitfield" do
    assert_eq(0b101101[1...4], 6)
    assert_eq(0b101101[2...5], 3)
  end

  it "treats an endless range as the full right shift" do
    assert_eq(0b101101[2..], 11)
    assert_eq(0b101101[0..], 45)
  end

  it "sign-extends negative receivers" do
    assert_eq((-45)[1..3], 1)
    assert_eq((-45)[2..], -12)
  end

  it "leaves the value unmasked when the computed length is <= 0" do
    assert_eq(0b101101[3..1], 5)
    assert_eq(0b101101[1..-1], 22)
  end

  it "left-shifts for a negative begin, growing into a Bignum" do
    assert_eq(0b101101[-1..], 90)
    assert_eq(0b101101[-3..], 360)
    assert_eq(0b101101[-2..3], 52)
    assert_eq((-45)[-1..], -90)
    assert_eq(1[-64..], 18446744073709551616)
  end

  it "raises ArgumentError for a beginless range" do
    assert_raises("ArgumentError") { 0b101101[..3] }
  end
end
