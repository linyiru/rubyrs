# A spec file mixing recognised and unrecognised patterns.
# v0.1 rewrites only `expr.should == val`; everything else
# passes through verbatim — comments included — so a human
# review can see what's still hand-translation territory.

describe "Mixed" do
  it "covers a recognised pattern" do
    assert_eq([1, 2, 3].length, 3)
  end

  it "leaves should_not alone" do
    [].should_not.empty?
  end

  it "leaves raise matchers alone" do
    -> { raise "boom" }.should.raise(RuntimeError)
  end

  it "leaves predicate matchers alone" do
    "abc".should.frozen?
  end

  it "still rewrites the simple cases mixed with skipped ones" do
    val = 1 + 2
    assert_eq(val, 3)
    [].should_not.include?(7)
  end
end
