describe "Hash#except" do

  it "returns a duplicate without arguments" do
    @hash = { a: 1, b: 2, c: 3 }
    ret = @hash.except
    assert_eq(ret, @hash)
  end

  it "removes the requested keys" do
    @hash = { a: 1, b: 2, c: 3 }
    assert_eq(@hash.except(:c, :a), { b: 2 })
  end
end
