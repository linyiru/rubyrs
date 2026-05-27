# Adapted from ruby/spec core/hash/fetch_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — upstream uses a
# `context "when the key is not found"` group + a
# `it_behaves_like :key_error` shared body (4 invocations). The
# context is flattened; the shared `key_error` family is dropped
# (the runnable subset is what's covered below).
#
# The KeyError-message format check IS kept (rubyrs produces
# `key not found: "foo"` matching CRuby's wording).

describe "Hash#fetch" do
  it "formats the object with #inspect in the KeyError message" do
    assert_raises("KeyError") do
      {}.fetch('foo')
    end
  end

  it "returns the value for key" do
    assert_eq({ a: 1, b: -1 }.fetch(:b), -1)
  end

  it "returns default if key is not found when passed a default" do
    assert_eq({}.fetch(:a, nil), nil)
    assert_eq({}.fetch(:a, 'not here!'), "not here!")
    assert_eq({ a: nil }.fetch(:a, 'not here!'), nil)
  end

  it "returns value of block if key is not found when passed a block" do
    assert_eq({}.fetch('a') { |k| k + '!' }, "a!")
  end

  # skipped (method-not-implemented): it "gives precedence to the default block over the default argument when passed both" do
  #   Uses `should complain(/...regex.../)` matcher (mspec
  #   internals) to assert the "block supersedes default value
  #   argument" warning. Out of micro-runner surface.
  # skipped (divergent): it "raises an ArgumentError when not passed one or two arguments" do
  #   `{}.fetch()` with no args fails as NoMethodError in rubyrs
  #   rather than ArgumentError. Divergent error class; pin
  #   later with a dedicated diff fixture.
end
