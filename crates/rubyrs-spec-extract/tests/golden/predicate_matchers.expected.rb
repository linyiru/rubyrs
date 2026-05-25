describe "predicate matchers" do
  it "rewrites should.PRED?" do
    assert([].empty?)
    assert("abc".frozen?)
  end

  it "rewrites should.PRED?(args)" do
    assert(obj.equal?(other))
    assert(obj.instance_of?(String))
  end

  it "rewrites should_not.PRED?" do
    assert(![1].empty?)
  end

  it "rewrites should_not.PRED?(args)" do
    assert(!obj.equal?(other))
  end
end
