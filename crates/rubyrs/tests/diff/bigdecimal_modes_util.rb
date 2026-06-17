# BigDecimal rounding-mode constants + mode-aware #round, the finite-
# state predicates / #sign, and bigdecimal/util's #to_d conversions.
# Surfaced by money (ROUND_HALF_EVEN at load, value.round(0, mode),
# #finite?, #to_d). Values are compared via to_i/to_f to stay clear of
# the BigDecimal display divergence (rubyrs prints "1.0", CRuby "0.1e1").
require "bigdecimal"
require "bigdecimal/util"

%i[ROUND_UP ROUND_DOWN ROUND_HALF_UP ROUND_HALF_DOWN ROUND_HALF_EVEN ROUND_CEILING ROUND_FLOOR].each do |c|
  print "#{c}=#{BigDecimal.const_get(c)} "
end
puts

b = BigDecimal("3.5")
p [b.finite?, b.infinite?, b.nan?, b.zero?, b.positive?, b.negative?, b.sign]
p [BigDecimal("0").zero?, BigDecimal("-2").negative?, BigDecimal("-2").sign, BigDecimal("0").sign]
p BigDecimal("5").nonzero?.to_i
p BigDecimal("0").nonzero?

def rnd(s, mode); BigDecimal(s).round(0, mode).to_i; end
HE = BigDecimal::ROUND_HALF_EVEN; HU = BigDecimal::ROUND_HALF_UP; HD = BigDecimal::ROUND_HALF_DOWN
UP = BigDecimal::ROUND_UP; DN = BigDecimal::ROUND_DOWN; CE = BigDecimal::ROUND_CEILING; FL = BigDecimal::ROUND_FLOOR
p [rnd("2.5", HE), rnd("3.5", HE), rnd("-2.5", HE), rnd("-3.5", HE)]
p [rnd("2.5", HU), rnd("-2.5", HU), rnd("2.5", HD), rnd("-2.5", HD)]
p [rnd("2.1", UP), rnd("-2.1", UP), rnd("2.9", DN), rnd("-2.9", DN)]
p [rnd("2.1", CE), rnd("-2.1", CE), rnd("2.9", FL), rnd("-2.9", FL)]
# default mode (no second arg) = ROUND_HALF_UP
p [BigDecimal("2.5").round.to_i, BigDecimal("-2.5").round.to_i]

p [42.to_d.to_i, "1.5".to_d.to_f, 100.to_d.to_i, nil.to_d.to_i]
p BigDecimal("3.14").to_d.to_f
p 3.5.to_d.to_f
