# rubyrs-spec-extract v0.4: 1 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L6: `before` — curly-brace hook form, not lifted

describe "Array#example with curly hook" do
  before(:each) { @setup = [1, 2] }

  it "uses fixture" do
    assert_eq(@setup, [1, 2])
  end

  it "uses mock" do
    m = mock("test")
    assert(m)
  end
end
