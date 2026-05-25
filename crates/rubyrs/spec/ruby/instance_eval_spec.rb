# Adapted from ruby/spec core/basicobject/instance_eval_spec.rb
# at 2026-05 (subset). Focuses on the "self swap" shape — the
# `def name; end` inside `instance_eval` path needs singleton
# classes and is documented as out of scope in SUBSET.md.

describe "BasicObject#instance_eval" do
  it "yields the receiver as the block argument" do
    class IEReceiver
    end
    obj = IEReceiver.new
    received = nil
    obj.instance_eval do |o|
      received = o
    end
    assert_eq(received, obj)
  end

  it "swaps self for the block body — instance variables read off the receiver" do
    class IEBox
      def initialize(v)
        @v = v
      end
    end
    b = IEBox.new(42)
    seen = nil
    b.instance_eval do
      seen = @v
    end
    assert_eq(seen, 42)
  end

  it "lets the block write instance variables that persist on the receiver" do
    class IEScratch
    end
    s = IEScratch.new
    s.instance_eval do
      @label = "hello"
    end
    # Read back via another instance_eval — same self, same ivars.
    seen = nil
    s.instance_eval do
      seen = @label
    end
    assert_eq(seen, "hello")
  end

  it "returns the value of the block's last expression" do
    class IEReturn
    end
    obj = IEReturn.new
    result = obj.instance_eval do
      1 + 2
    end
    assert_eq(result, 3)
  end
end
