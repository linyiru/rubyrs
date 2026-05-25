# Adapted from ruby/spec core/module/class_eval_spec.rb at
# 2026-05 (subset). class_eval's main DSL use is `def name`
# inside the block landing on the receiver class — that path
# is covered here; the "evaluates a String" form (real eval)
# stays out of scope (SUBSET.md: eval is explicitly excluded).
#
# Known divergence locked in by `returns_the_class_for_now`:
# rubyrs's class_eval returns the class (because we route
# through the existing class-body Return path) where CRuby
# returns the block's last expression. See SUBSET.md and the
# helper comment in vm/dispatch.rs::invoke_block_with_self.

describe "Module#class_eval" do
  it "defines instance methods on the receiver class" do
    class CEHost
    end
    CEHost.class_eval do
      def greet
        "hi from class_eval"
      end
    end
    assert_eq(CEHost.new.greet, "hi from class_eval")
  end

  it "yields the class as the block argument" do
    class CEReceiver
    end
    received = nil
    CEReceiver.class_eval do |k|
      received = k
    end
    assert_eq(received, CEReceiver)
  end

  it "is also reachable via `module_eval` (alias)" do
    class CEAlias
    end
    CEAlias.module_eval do
      def shout
        "loud"
      end
    end
    assert_eq(CEAlias.new.shout, "loud")
  end

  it "rejects a non-class receiver with TypeError" do
    assert_raises("TypeError") do
      "string".class_eval do
        # body never runs
      end
    end
  end

  it "returns_the_class_for_now (rubyrs divergence from CRuby)" do
    # In CRuby this returns 99 (block's last expression).
    # In rubyrs we re-use the class-body Return path, which
    # discards the block value and returns the class itself.
    # Documented in SUBSET.md; lock here so changes notice.
    class CERet
    end
    result = CERet.class_eval do
      99
    end
    assert_eq(result, CERet)
  end
end
