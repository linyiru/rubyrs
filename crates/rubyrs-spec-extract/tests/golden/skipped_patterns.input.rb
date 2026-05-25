# A spec file mixing recognised and unrecognised patterns.
# After v0.2 the recognised set covers: `expr.should == val`,
# `expr.should_not == val`, predicate matchers
# (`.should.foo?` / `.should_not.foo?`) and lambda-raise
# (`-> { ... }.should.raise(X)`). Anything that still passes
# through is mock-style, shared-examples, or fixtures —
# v0.3+ work.

describe "Mixed" do
  it "covers a recognised pattern" do
    [1, 2, 3].length.should == 3
  end

  it "v0.2 should_not == val" do
    [1, 2, 3].length.should_not == 99
  end

  it "v0.2 lambda-raise" do
    -> { raise "boom" }.should.raise(RuntimeError)
  end

  it "v0.2 predicate matcher (should)" do
    "abc".should.frozen?
  end

  it "v0.2 predicate matcher (should_not)" do
    [].should_not.empty?
  end

  it "v0.2 mixed in one block" do
    val = 1 + 2
    val.should == 3
    [].should_not.include?(7)
  end

  it "still skips mocks (v0.3 territory)" do
    obj = mock("thing")
    obj.should_receive(:name).and_return("hi")
  end

  it "still skips shared examples (v0.4 territory)" do
    it_behaves_like :some_shared, :method_name
  end
end
