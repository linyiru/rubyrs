# HOT-LOOP twin of super_forward_kwargs.rb: a bare-`super` method that
# forwards KEYWORD args and tier-2-COMPILES under the DEFAULT threshold.
# The warm loops below trip the tier-2 compile counter, so `calc`/`init`
# run from their native (tier-2) bodies — NO `RUBYRS_JIT_TIER2_THRESHOLD=1`
# needed. Parity here therefore pins the tier-2 inline-body path ==
# interp == CRuby without a special threshold, so the regression can't hide
# CI-invisibly again (given a tier-2 diff leg).
#
# Root cause (see super_forward_kwargs.rb): the tier-2 body runs INLINE
# inside the caller's `do_call`, BEFORE the `Op::Call` arm resets
# `trailing_hash_positional`. The read-only fast-path binder
# (`try_invoke_nfa_method_from_stack`) serves an all-defaults kwargs method
# on a BARE call with the flag still set, so a bare `super` (which rebuilds
# a trailing kwargs Hash and peels it only when `!trailing_hash_positional`)
# forwarded that Hash as a POSITIONAL arg -> "wrong number of arguments
# (given 1, expected 0)". The trigger is specifically a BARE 0-/positional-
# arg call (sets the flag) to a defaulted-kwargs method (bound via the fast
# path, flag not consumed). The loops below use exactly that shape.

# --- kwargs-only, all defaulted, called BARE (S<B "own default wins") ---
class KwBase
  def calc(a: 1, b: 2)
    [a, b]
  end
end
class KwSub < KwBase
  def calc(a: 9, b: 8)
    super            # bare super forwards a:, b: as KEYWORDS
  end
end

def hot_kw(n)
  obj = KwSub.new
  last = nil
  i = 0
  while i < n
    last = obj.calc  # BARE call: sets trailing_hash_positional, defaults bind
    i += 1
  end
  last
end

p hot_kw(300)      # [9, 8]
p hot_kw(20_000)   # [9, 8]  (calc now tier-2-compiled)

# --- positional + defaulted keyword, called with only the positional ---
class PBase
  def init(x, y: 5, z: 6)
    [x, y, z]
  end
end
class PSub < PBase
  def init(x, y: 5, z: 6)
    super          # bare super forwards x positional + y:,z: keywords
  end
end

def hot_pos_kw(n)
  obj = PSub.new
  last = nil
  i = 0
  while i < n
    last = obj.init(i)  # BARE positional call, keywords defaulted
    i += 1
  end
  last
end

p hot_pos_kw(300)      # [299, 5, 6]
p hot_pos_kw(20_000)   # [19999, 5, 6]
