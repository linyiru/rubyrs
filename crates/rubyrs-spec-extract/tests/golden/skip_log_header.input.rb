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
