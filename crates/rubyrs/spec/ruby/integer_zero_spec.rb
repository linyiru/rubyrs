# Adapted from ruby/spec core/integer/zero_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# upstream first block uses predicate matchers
# (`.should.zero?` / `.should_not.zero?`) translated to plain
# `assert_eq(...,true/false)` for the micro-runner.

describe "Integer#zero?" do
  it "returns true if self is 0" do
    assert_eq(0.zero?, true)
    assert_eq(1.zero?, false)
    assert_eq((-1).zero?, false)
  end

  it "Integer#zero? overrides Numeric#zero?" do
    assert_eq(42.method(:zero?).owner, Integer)
  end
end
