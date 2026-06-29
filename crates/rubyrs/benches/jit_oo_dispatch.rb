# NON-ITERATOR north-star (ADR 0034, JIT generalization). The shipped JIT wins the
# iterator family (sum/map/each/group_by/... whole-loop native drivers) and already
# beats YJIT on recursion (fib 2.8×) and on while-loops + self-call chains INSIDE a
# method (3.8× — `compile()` handles those today). The frontier this benchmark tracks
# is the one real OO-dispatch gap that those don't cover:
#
#   an explicit-receiver call to ANOTHER object (`@h.compute(x)`, receiver != self)
#   inside a hot loop.
#
# Measured 2026-06-29 (best-of-3 wall, 30M iterations), jit-native:
#   interp 5.08s   jitN 3.35s   YJIT 0.74s   ->  jitN is 4.5x BEHIND YJIT.
# `run` declines to compile because `compile()` has no Object-receiver Call arm (only
# self-calls, array-element-attr, and getters lower today), so each `@h.compute` falls
# back to interpreter dispatch per iteration while YJIT inlines the whole loop.
#
# TARGET (Step 1: general native->native call + class-guard PIC, generalizing B4 from
# array-elements to any receiver): pull this onto the method-internal-self-chain tier,
# i.e. AHEAD of YJIT. The ultimate north-star is poc/rubocop-spike/bench_walk.rb (the
# rubocop-shaped recursive AST walk: Node objects, 0-arg Bool predicates, symbol/hash
# work) — when THAT goes native, the JIT has truly left the iterator family.
#
# Run: `RUBYRS_JIT_NATIVE=1 rubyrs jit_oo_dispatch.rb` vs `ruby --yjit`.

class Helper
  def initialize(k); @k = k; end
  def compute(x); x * 2 + @k; end
end

class Driver
  def initialize; @h = Helper.new(7); end
  def run(n)
    s = 0
    i = 0
    while i < n
      s += @h.compute(i % 100)   # explicit-recv on another object — the gap
      i += 1
    end
    s
  end
end

p Driver.new.run(30_000_000)
