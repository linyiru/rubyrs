# Exercises two substitution-overlap edge cases from
# /code-review on PR #74:
#
# 1. A `should ==` rewrite INSIDE a `before :each` block —
#    the recogniser would want to rewrite it, but the lifter
#    is about to delete the entire `before` call. The filter
#    should drop the recogniser sub so apply_substitutions
#    doesn't get an out-of-bounds range.
#
# 2. An `it` block whose VERY FIRST statement is a
#    `should ==` rewrite. The lifter inserts the lifted body
#    at the same offset where the recogniser substitution
#    starts. Sort tiebreaker must apply the longer range
#    (recogniser) first, then the zero-length insertion.

describe "overlap edges" do

  it "first stmt is should ==" do
    @hash = assert_eq({ a: 1 }.length, 1)
    assert_eq([1].length, 1)   # ← recogniser bait at it body start;
                              #   lifter inserts before it
  end
end
