# Endless `(a..)` and infinite-bounded `(a..Float::INFINITY)` ranges:
# `each` counts Ints up from the start forever until the block breaks /
# returns. This is the iteration primitive Enumerator::Lazy walks for
# infinite sources. (Eager whole-sequence ops — to_a / next / size — on an
# infinite source would run forever and are intentionally not exercised;
# lazy short-circuits via first/take instead.)
out = []
(1..).each { |x| break if x > 5; out << x }
p out                                            # [1,2,3,4,5]
out2 = []
(1..Float::INFINITY).each { |x| break if x > 3; out2 << x }
p out2                                           # [1,2,3]
out3 = []
(0...).each { |x| break if x >= 3; out3 << x }   # exclusive endless
p out3                                           # [0,1,2]
# no-block endless each → Enumerator; first(n) short-circuits via throw
p (1..).each.class                               # Enumerator
p (1..).each.first(4)                            # [1,2,3,4]
p (1..Float::INFINITY).each.first(3)             # [1,2,3]
p (5..).each.first(0)                            # []
# break value propagates
r = (1..).each { |x| break x * 10 if x == 3 }
p r                                              # 30
# respond_to? lockstep
p (1..).respond_to?(:each)                        # true
