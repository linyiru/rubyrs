describe "predicate matchers" do
  it "rewrites should.PRED?" do
    [].should.empty?
    "abc".should.frozen?
  end

  it "rewrites should.PRED?(args)" do
    obj.should.equal?(other)
    obj.should.instance_of?(String)
  end

  it "rewrites should_not.PRED?" do
    [1].should_not.empty?
  end

  it "rewrites should_not.PRED?(args)" do
    obj.should_not.equal?(other)
  end
end
