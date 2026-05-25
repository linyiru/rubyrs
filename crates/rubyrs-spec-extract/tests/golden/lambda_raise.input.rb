describe "raise matchers" do
  it "lowers a simple class name" do
    -> { 1.foo }.should.raise(NoMethodError)
  end

  it "lowers a constant-path class name" do
    -> { 1.bar }.should.raise(Math::DomainError)
  end

  it "lowers with a multi-statement lambda body" do
    -> {
      x = 1
      x.no_such
    }.should.raise(NoMethodError)
  end
end
