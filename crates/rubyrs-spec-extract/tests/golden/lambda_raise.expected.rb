describe "raise matchers" do
  it "lowers a simple class name" do
    assert_raises("NoMethodError") do
      1.foo
    end
  end

  it "lowers a constant-path class name" do
    assert_raises("Math::DomainError") do
      1.bar
    end
  end

  it "lowers with a multi-statement lambda body" do
    assert_raises("NoMethodError") do
      x = 1
      x.no_such
    end
  end
end
