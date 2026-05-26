# rubyrs-spec-extract v0.4: 3 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L10: `before` — only the bare `before :each do ... end` form is lifted (no extra args, all sibling `it`s must have bodies); other forms like `before :all` or `before :each, :foo` pass through and need hand polish
#   - L26: `before` — only the bare `before :each do ... end` form is lifted (no extra args, all sibling `it`s must have bodies); other forms like `before :all` or `before :each, :foo` pass through and need hand polish
#   - L43: `mock_int` — only `mock_int(literal_int)` with no receiver is substituted; other forms (explicit receiver, multi-arg, non-int-literal) pass through

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

describe "empty-it bailout guard" do
  # If ANY sibling `it` has an empty body (TODO placeholder
  # `do end`), the whole lift bails — better to leave the
  # `before :each` for the human (with a skip-log entry) than
  # delete it AND lift partially. Without this, the empty it
  # would run without its setup but its non-empty siblings
  # would get it, an asymmetry that's surprising at best.
  before :each do
    @hash = { a: 1 }
  end

  it "with body, would get the lift if not for empty sibling" do
    @hash.length
  end

  it "empty body — pending"
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
