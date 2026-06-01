# M27 A4: required positional params AFTER `*rest` get peeled from
# the END of the argument list, NOT bound at the front. Pre-fix
# `def mid(a, *b, c); mid(1,2,3,4,5)` bound a=1, b=[2,3,4,5], c=nil
# instead of CRuby's a=1, b=[2,3,4], c=5. The bug masked every
# Sinatra/Rack idiom that uses `(*pre, last)` splits and breaks any
# routing layer that destructures via post-rest required params.
# CRuby is the oracle.

# Simple
def mid(a, *b, c)
  [a, b, c]
end
puts mid(1, 2, 3, 4, 5).inspect       # [1, [2,3,4], 5]
puts mid(1, 2).inspect                # [1, [], 2]  (rest is empty)
puts mid(:x, :y, :z).inspect          # [:x, [:y], :z]

# Two post-required params
def m2(a, *b, c, d)
  [a, b, c, d]
end
puts m2(1, 2, 3, 4, 5, 6).inspect     # [1, [2,3,4], 5, 6]
puts m2(1, 5, 6).inspect              # [1, [], 5, 6]

# Optional + post (def f(a, b=10, *c, d))
def opt_post(a, b=10, *c, d)
  [a, b, c, d]
end
puts opt_post(1, 99, 2, 3, 4, 5).inspect  # [1, 99, [2,3,4], 5]
puts opt_post(1, 99, 5).inspect           # [1, 99, [], 5]
puts opt_post(1, 5).inspect               # [1, 10, [], 5]  (b defaults)
