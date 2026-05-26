describe "Foo" do
  it "uses both placeholders correctly" do
    assert_eq(obj.send(:alpha), 1)
    assert_eq(obj.send(:beta), 2)
  end
end
