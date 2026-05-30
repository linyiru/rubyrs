# Cross-frame exception propagation through native iter drivers
# (Array#each, Array#map, Hash#any?, etc.) — pins the
# `RubyError::AlreadyCaught` bubble-out protocol introduced to
# stop iter drivers from looping past a rescue handler that's
# already been triggered in a caller frame.
#
# Pre-fix behaviour: `begin; [1,2,3].each { raise }; rescue ...`
# would crash with the uncaught second-iter raise instead of
# catching the first.

describe "Block exception propagation through iter drivers" do
  it "Array#each: caller-frame rescue catches block raise" do
    out = nil
    begin
      [1, 2, 3].each { raise "boom_each" }
    rescue => e
      out = e.message
    end
    assert_eq(out, "boom_each")
  end

  it "Array#map: caller-frame rescue catches block raise" do
    out = nil
    begin
      [1, 2, 3].map { raise "boom_map" }
    rescue => e
      out = e.message
    end
    assert_eq(out, "boom_map")
  end

  it "Hash#any?: caller-frame rescue catches block raise" do
    out = nil
    begin
      {a: 1, b: 2}.any? { raise "boom_any" }
    rescue => e
      out = e.message
    end
    assert_eq(out, "boom_any")
  end

  it "Hash#select: caller-frame rescue catches block raise" do
    out = nil
    begin
      {a: 1, b: 2}.select { raise "boom_select" }
    rescue => e
      out = e.message
    end
    assert_eq(out, "boom_select")
  end

  it "propagates through method boundaries" do
    # Block raise → rescue is in a caller of the method that
    # called the iter. Frame chain: script → method `f` →
    # block → unwind crosses two frames.
    def each_raise
      [1, 2, 3].each { raise "from_method" }
    end
    out = nil
    begin
      each_raise
    rescue => e
      out = e.message
    end
    assert_eq(out, "from_method")
  end

  it "in-block rescue still catches without bubbling out" do
    # When the block has its own rescue, the exception is
    # caught WITHIN the block frame — the iter driver
    # continues normally.
    counter = 0
    [1, 2, 3].each do |x|
      begin
        raise "in_block"
      rescue
        counter += 1
      end
    end
    assert_eq(counter, 3)
  end

  it "catches a primitive error (NoMethodError) raised inside a block" do
    # Mixed path: not a Ruby-level `raise` but a Trap from a
    # primitive that gets routed through dispatch_until's Err
    # handler, which also has the boundary check.
    out = nil
    begin
      [1, 2, 3].each { nil.foo }
    rescue NoMethodError => e
      out = e.class.to_s
    end
    assert_eq(out, "NoMethodError")
  end

  it "still propagates correctly through nested begin/rescue" do
    # Sanity: caller's begin/rescue still catches when an
    # inner begin/rescue re-raises (CRuby parity).
    raised_inner = false
    raised_outer = false
    begin
      begin
        [1].each { raise "from_inner" }
      rescue => e_inner
        raised_inner = true
        raise "rewrapped: #{e_inner.message}"
      end
    rescue => e_outer
      raised_outer = true
      assert_eq(e_outer.message, "rewrapped: from_inner")
    end
    assert_eq(raised_inner, true)
    assert_eq(raised_outer, true)
  end
end
