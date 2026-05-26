# rubyrs-spec-extract v0.4: 2 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L8: `mock_int` — only `mock_int(literal_int)` with no receiver is substituted; other forms (explicit receiver, multi-arg, non-int-literal) pass through
#   - L12: `mock_int` — only `mock_int(literal_int)` with no receiver is substituted; other forms (explicit receiver, multi-arg, non-int-literal) pass through

describe "Integer#digits" do
  it "converts the radix with mock_int(2)" do
    assert_eq(12345.digits(2), [1, 0, 0, 1])
  end

  it "leaves dynamic mock_int alone" do
    n = some_int
    assert_eq(12345.digits(mock_int(n)), [1])
  end

  it "leaves multi-arg mock_int alone" do
    assert_eq(12345.digits(mock_int(2, 3)), [1])
  end
end
