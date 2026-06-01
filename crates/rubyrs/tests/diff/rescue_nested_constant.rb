# Regression: `rescue <ShortName>` must resolve a constant nested in an
# enclosing module/class via the lexical scope — the same way the `raise`
# side (and a normal constant read) resolves it.
#
# A class defined as `module M; class Sig` is keyed by its qualified name
# (`M::Sig`). Before the fix, the rescue clause matched only the bare
# source sym `Sig` against the global class table and missed `M::Sig`, so
# the exception escaped uncaught even though a matching `rescue` was in
# lexical scope. CRuby is the oracle.

module M
  class Sig < StandardError
    attr_reader :payload
    def initialize(payload)
      @payload = payload
      super("sig")
    end
  end

  class Service
    # Pure case: nested class, bare rescue, no blocks involved.
    def direct
      begin
        raise Sig.new(:from_direct)
      rescue Sig => e
        "direct: #{e.payload}"
      end
    end

    # Rescue reached across a native iterator + instance_exec (the shape
    # a Sinatra-style route table + halt produces).
    def via_iter
      begin
        [1, 2, 3].each { |x| raise Sig.new("at #{x}") if x == 2 }
        "no-raise"
      rescue Sig => e
        "iter: #{e.payload}"
      end
    end

    # A deeper nesting level resolves too.
    class Inner
      def run
        begin
          raise Sig.new(:from_inner)
        rescue Sig => e
          "inner: #{e.payload}"
        end
      end
    end
  end
end

puts M::Service.new.direct
puts M::Service.new.via_iter
puts M::Service::Inner.new.run

# A sibling-namespace class of the SAME short name must NOT be caught by
# the wrong module's rescue — confirms we resolve to the right qualified
# class, not just any "Sig".
module Other
  class Sig < StandardError; end
end

module M
  class Picky
    def run
      begin
        raise Other::Sig.new("other")
      rescue Sig => e          # M::Sig — should NOT match Other::Sig
        "WRONGLY caught as M::Sig"
      rescue Other::Sig => e   # this one should match
        "correctly caught Other::Sig"
      end
    end
  end
end

puts M::Picky.new.run
