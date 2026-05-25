# rubyrs-spec-extract v0.3: 2 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L10: `before` — only `before :each` is lifted in v0.3
#   - L25: `mock_int` — only int-literal arg is substituted in v0.3; dynamic arg passes through

# Guards added in response to the /code-review pass on
# v0.3. Each block exercises a defensive case. None should
# trigger the lift / rewrite — all fall through to the skip
# log so a human reviewer sees them.

describe "before-arg-shape guard" do
  # `before :each, :foo` — additional args after :each are
  # not the v0.3 lift shape (any mspec extension or custom
  # DSL form). Lifter must bail; skip log flags it.
  before :each, :foo do
    @hash = 1
  end

  it "doesn't get the lift" do
    @hash
  end
end

describe "mock_int receiver guard" do
  # `obj.mock_int(2)` — mspec's `mock_int` is top-level; a
  # method named the same on a user class shouldn't get its
  # receiver silently dropped. try_mock_int bails; skip log
  # flags it.
  it "doesn't drop receiver" do
    obj.mock_int(2)
  end
end
