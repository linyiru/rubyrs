
# Comment lines about `require_relative` stay — only the
# actual call form gets stripped.
describe "stripping" do
  it "removes the loader lines but keeps the spec body" do
    assert_eq("hello".length, 5)
    assert_eq([1, 2, 3].length, 3)
  end
end

