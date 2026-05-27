# Adapted from ruby/spec core/method/source_location_spec.rb
# at upstream commit 448cb340 (2026-05). Hand-translated — the
# baseline "returns [path, line]" shape is inlined, plus the
# "returns nil for a C-defined/builtin method" assertion (which
# rubyrs honours — primitives return nil since they have no
# Method record with source coordinates). Mock-based locator
# tests + alias / define_method variants dropped.

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

  it "returns nil for a C-defined / builtin method" do
    # `String#upcase` and `Integer#to_s` are dispatched via the
    # primitive table, no Method record with source coordinates
    # is attached.
    assert_eq("x".method(:upcase).source_location, nil)
    assert_eq(1.method(:to_s).source_location, nil)
  end
end
