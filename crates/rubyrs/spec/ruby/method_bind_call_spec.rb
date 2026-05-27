# Adapted from ruby/spec core/method/bind_call_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — four
# baseline shapes: basic invocation; subclass receiver; cross-
# class TypeError; wrong-arity ArgumentError. The
# private/protected method visibility variants are dropped
# (rubyrs doesn't model the visibility-bypass that bind_call
# allows in CRuby).

describe "Method#bind_call" do
  it "invokes the method on the given receiver" do
    class MBC1
      def add(x); x + 1; end
    end
    assert_eq(MBC1.new.method(:add).bind_call(MBC1.new, 5), 6)
  end

  it "binds onto a subclass instance" do
    class MBC2Base
      def f; self.class.to_s; end
    end
    class MBC2Sub < MBC2Base
    end
    assert_eq(MBC2Base.new.method(:f).bind_call(MBC2Sub.new), "MBC2Sub")
  end

  it "raises TypeError when receiver is incompatible" do
    class MBC3Lhs
      def f; end
    end
    class MBC3Rhs
    end
    m = MBC3Lhs.new.method(:f)
    assert_raises("TypeError") { m.bind_call(MBC3Rhs.new) }
  end

  it "raises ArgumentError when no receiver is given" do
    class MBC4
      def f; end
    end
    m = MBC4.new.method(:f)
    assert_raises("ArgumentError") { m.bind_call }
  end
end
