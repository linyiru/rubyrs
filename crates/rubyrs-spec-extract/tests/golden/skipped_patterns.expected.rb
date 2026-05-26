# rubyrs-spec-extract v0.4: 3 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L37: `mock` — no mock library in the micro-runner; hand-translate
#   - L38: `should_receive` — mock expectations; hand-translate
#   - L42: `it_behaves_like` — shared-example name not found in the supplied --shared registry (or none supplied); pass the matching `shared/...` file via `--shared <path>` to inline, or hand-translate

# A spec file mixing recognised and unrecognised patterns.
# After v0.2 the recognised set covers: `expr.should == val`,
# `expr.should_not == val`, predicate matchers
# (`.should.foo?` / `.should_not.foo?`) and lambda-raise
# (`-> { ... }.should.raise(X)`). Anything that still passes
# through is mock-style, shared-examples, or fixtures —
# v0.3+ work.

describe "Mixed" do
  it "covers a recognised pattern" do
    assert_eq([1, 2, 3].length, 3)
  end

  it "v0.2 should_not == val" do
    assert_neq([1, 2, 3].length, 99)
  end

  it "v0.2 lambda-raise" do
    assert_raises("RuntimeError") do
      raise "boom"
    end
  end

  it "v0.2 predicate matcher (should)" do
    assert("abc".frozen?)
  end

  it "v0.2 predicate matcher (should_not)" do
    assert(![].empty?)
  end

  it "v0.2 mixed in one block" do
    val = 1 + 2
    assert_eq(val, 3)
    assert(![].include?(7))
  end

  it "still skips mocks (v0.3 territory)" do
    obj = mock("thing")
    obj.should_receive(:name).and_return("hi")
  end

  it "still skips shared examples (v0.4 territory)" do
    it_behaves_like :some_shared, :method_name
  end
end
