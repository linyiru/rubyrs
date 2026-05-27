# Adapted from ruby/spec core/integer/complement_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished from the
# extractor output: `context` blocks are flattened (the
# micro-runner's spec_helper.rb doesn't define `context` —
# crates/rubyrs-spec-extract/src/lib.rs:901), and
# `bignum_value(N)` is replaced with direct `(2**64 + N)`
# expressions (upstream returns `2**64 + N`, verified by
# round-tripping through `~bignum_value(48) == -18446744073709551665`).
#
# The bignum-context cases are gated with `bignum_it` so they
# only run on `--features bignum`. Under the no-bignum profile
# the literal `(2**64 + N)` saturates via `i64::saturating_pow`
# to `i64::MAX` (see integer_even_spec.rb's header for the same
# saturation reasoning).

describe "Integer#~" do
  it "returns self with each bit flipped (fixnum)" do
    assert_eq((~0), -1)
    assert_eq((~1221), -1222)
    assert_eq((~-2), 1)
    assert_eq((~-599), 598)
  end

  bignum_it "returns self with each bit flipped (bignum)" do
    assert_eq((~(2**64 + 48)), -18446744073709551665)
    assert_eq((~(-(2**64 + 21))), 18446744073709551636)
    assert_eq((~(2**64 + 1)), -18446744073709551618)
  end
end
