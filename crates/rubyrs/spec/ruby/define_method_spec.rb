# Adapted from ruby/spec core/module/define_method_spec.rb at
# 2026-05 (subset). Sticks to the closure-method form
# `define_method(:name) { ... }`; symbol-of-existing-method
# (`define_method(:new, instance_method(:old))`) needs
# `Module#instance_method` which rubyrs hasn't shipped.

describe "Module#define_method" do
  it "installs the block as a callable method" do
    class DMBasic
      define_method(:greet) { |name| "hello, " + name }
    end
    assert_eq(DMBasic.new.greet("world"), "hello, world")
  end

  it "closes over outer-scope locals — writes propagate across calls" do
    # The closure-method semantic that distinguishes
    # define_method from def: `counter` is captured from the
    # surrounding class-body scope, not a fresh local.
    class DMCounter
      counter = 0
      define_method(:bump) { counter = counter + 1; counter }
    end
    c = DMCounter.new
    assert_eq(c.bump, 1)
    assert_eq(c.bump, 2)
    assert_eq(c.bump, 3)
  end

  it "validates arity against the block's declared params" do
    class DMArity
      define_method(:two) { |a, b| a + b }
    end
    assert_raises("ArgumentError") do
      DMArity.new.two(1)
    end
  end

  it "shares state between two instances of the same class" do
    # All instances see the same captured slot — define_method
    # closes over the class-body scope, not per-instance state.
    class DMShared
      total = 0
      define_method(:tick) { total = total + 1; total }
    end
    a = DMShared.new
    b = DMShared.new
    assert_eq(a.tick, 1)
    assert_eq(b.tick, 2)
    assert_eq(a.tick, 3)
  end
end
