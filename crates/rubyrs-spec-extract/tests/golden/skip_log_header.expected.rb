# rubyrs-spec-extract v0.4: 6 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L5: `before` — only the bare `before :each do ... end` form is lifted (no extra args, all sibling `it`s must have bodies); other forms like `before :all` or `before :each, :foo` pass through and need hand polish
#   - L9: `after` — not lifted; inline cleanup into each `it` or comment the block out
#   - L13: `context` — the micro-runner's spec_helper.rb doesn't define `context` — rename to `describe` (or remove) before running, or the file crashes with NoMethodError on `context`
#   - L15: `it_behaves_like` — shared-example name not found in the supplied --shared registry (or none supplied); pass the matching `shared/...` file via `--shared <path>` to inline, or hand-translate
#   - L20: `mock` — no mock library in the micro-runner; hand-translate
#   - L21: `should_receive` — mock expectations; hand-translate

# A spec file exercising multiple skip-loggable patterns
# so the v0.3 header lists everything in one place.

describe "Mixed v0.3 skips" do
  before :all do
    @cached = compute_once
  end

  after :each do
    cleanup
  end

  context "nested context" do
    it "uses it_behaves_like" do
      it_behaves_like :shared, :method
    end
  end

  it "uses mock" do
    obj = mock("x")
    obj.should_receive(:m)
  end
end
