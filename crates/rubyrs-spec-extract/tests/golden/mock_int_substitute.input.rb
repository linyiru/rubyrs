describe "Integer#digits" do
  it "converts the radix with mock_int(2)" do
    assert_eq(12345.digits(mock_int(2)), [1, 0, 0, 1])
  end

  it "leaves dynamic mock_int alone" do
    n = some_int
    assert_eq(12345.digits(mock_int(n)), [1])
  end

  it "leaves multi-arg mock_int alone" do
    assert_eq(12345.digits(mock_int(2, 3)), [1])
  end
end
