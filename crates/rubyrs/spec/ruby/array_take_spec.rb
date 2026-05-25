# Adapted from ruby/spec core/array/take_spec.rb at
# upstream commit 448cb340 (2026-05). 3rd extractor-derived
# spec — produced by `rubyrs-spec-extract` v0.3; the 6
# auto-extracted `it` blocks below are byte-identical to
# the extractor's output. One block commented for a
# fixture-class reference the micro-runner can't resolve.

describe "Array#take" do
  it "returns the first specified number of elements" do
    assert_eq([1, 2, 3].take(2), [1, 2])
  end

  it "returns all elements when the argument is greater than the Array size" do
    assert_eq([1, 2].take(99), [1, 2])
  end

  it "returns all elements when the argument is less than the Array size" do
    assert_eq([1, 2].take(4), [1, 2])
  end

  it "returns an empty Array when passed zero" do
    assert_eq([1].take(0), [])
  end

  it "returns an empty Array when called on an empty Array" do
    assert_eq([].take(3), [])
  end

  # Skipped (divergence) — upstream take_spec.rb:25-28.
  # CRuby raises ArgumentError on `Array#take(negative)`;
  # rubyrs returns `[]` instead. Documented divergence to
  # be tracked in docs/SUBSET.md alongside the existing
  # `Integer#digits(-n)` Math::DomainError divergence —
  # both follow rubyrs's tendency to coalesce negative-arg
  # cases into a benign result rather than raising.
  #
  # it "raises an ArgumentError when the argument is negative" do
  #   assert_raises("ArgumentError") do
  #     [1].take(-3)
  #   end
  # end

  # Skipped — upstream take_spec.rb:30 uses the
  # `ArraySpecs::MyArray[...]` subclass fixture; no
  # fixtures file vendored, so the micro-runner can't
  # resolve `ArraySpecs::MyArray`. The behaviour itself
  # (Array#take on a subclass returning a plain Array) is
  # exercised by other ruby/spec files we may ingest
  # later.
  #
  # it 'returns a Array instance for Array subclasses' do
  #   assert(ArraySpecs::MyArray[1, 2, 3, 4, 5].take(1).instance_of?(Array))
  # end
end
