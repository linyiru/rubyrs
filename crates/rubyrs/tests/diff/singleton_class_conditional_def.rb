# `class << self; if cond; def a; ...; else; def a; ...; end; end` —
# conditional method definitions inside a singleton-class body become
# conditionally-installed CLASS methods. Discovery: P3 Jekyll spike —
# i18n's utils.rb guards `def except(hash, *keys)` on
# `Hash.method_defined?(:except)` to pick native vs. polyfill.

class Probe
  def has_it; end
end

class A
  class << self
    # then-branch taken (condition true)
    if Probe.method_defined?(:has_it)
      def mode; "native"; end
    else
      def mode; "polyfill"; end
    end
    # else-branch taken (condition false)
    if Probe.method_defined?(:absent_method)
      def pick; "native"; end
    else
      def pick; "fallback"; end
    end
    # no-else form, condition false → method not defined
    if false
      def maybe; "yes"; end
    end
    # multiple defs in one branch
    if true
      def one; 1; end
      def two; 2; end
    end
  end
end

p A.mode
p A.pick
p A.respond_to?(:maybe)
p A.one
p A.two
