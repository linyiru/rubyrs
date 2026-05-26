describe "Array#push arg-form classification" do
  it "single-arg push is kept" do
    a = [1, 2]
    a.push(3)
    assert_eq(a, [1, 2, 3])
  end

  it "single-arg with nested call is kept" do
    a = []
    a.push(make_pair(1, 2))
    assert_eq(a.length, 1)
  end

  it "single-arg with hash literal is kept" do
    a = []
    a.push({k: 1, v: 2})
    assert_eq(a.length, 1)
  end

  it "single-arg with array literal is kept" do
    a = []
    a.push([1, 2])
    assert_eq(a.length, 1)
  end

  # skipped (method-not-implemented): it "multi-arg push is dropped" do
end
