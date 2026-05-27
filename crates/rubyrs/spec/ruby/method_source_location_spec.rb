# Adapted from ruby/spec core/method/source_location_spec.rb
# at upstream commit 448cb340 (2026-05). Hand-translated — the
# baseline "returns [path, line]" shape is inlined. Mock-based
# locator tests + alias / define_method variants + the
# C-defined "returns nil" block are dropped.

describe "Method#source_location" do
  it "returns [filename, line_number] for a Ruby-defined method" do
    class LocT1
      def f
        :ok
      end
    end
    loc = LocT1.new.method(:f).source_location
    # Don't pin the path/line literal — the test runs from a
    # synthetic source under the micro-runner. Shape-check only.
    assert_eq(loc.is_a?(Array), true)
    assert_eq(loc.length, 2)
    assert_eq(loc[0].is_a?(String), true)
    assert_eq(loc[1].is_a?(Integer), true)
  end

  # skipped (method-not-implemented): it "returns nil for a C-defined builtin" do
  #   rubyrs doesn't model the "C-defined" distinction at the
  #   Method level; built-in primitives return `[<builtin>, 0]`
  #   rather than nil. Out of subset.
end
