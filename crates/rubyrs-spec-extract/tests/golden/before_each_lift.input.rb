describe "Hash#except" do
  before :each do
    @hash = { a: 1, b: 2, c: 3 }
  end

  it "returns a duplicate without arguments" do
    ret = @hash.except
    assert_eq(ret, @hash)
  end

  it "removes the requested keys" do
    assert_eq(@hash.except(:c, :a), { b: 2 })
  end
end
