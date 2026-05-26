describe "Array#push string with end keyword" do
  it "appends a string containing 'end'" do
    arr = []
    arr.push("end")
    assert_eq(arr, ["end"])
  end

  it "another assertion after" do
    assert(true)
  end
end
