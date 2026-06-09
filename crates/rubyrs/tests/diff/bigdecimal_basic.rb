# Rational-backed BigDecimal (require "bigdecimal"). Values compared via
# #to_f / #to_i / relations — the CRuby scientific #to_s ("0.314e1") is a
# documented non-goal; the consumers (liquid numeric filters) always
# convert back to Float/Integer. Oracle: this fixture only uses to_f/to_i
# so disable-gems CRuby (which has bigdecimal) matches.
require "bigdecimal"
p BigDecimal("3.14159").is_a?(BigDecimal)                 # true
p BigDecimal("3.14159").round(2).to_f                     # 3.14
p BigDecimal("3.14159").round(4).to_f                     # 3.1416
p (BigDecimal("100.0") / BigDecimal("7.0")).round(2).to_f # 14.29
p (BigDecimal("1.5") + BigDecimal("2.3")).to_f            # 3.8
p (BigDecimal("10") * BigDecimal("0.1")).to_f             # 1.0
p (BigDecimal("5.5") - BigDecimal("2.2")).to_f            # 3.3
p BigDecimal("3.7").round.to_i                            # 4
p BigDecimal("3.2").ceil.to_i                             # 4
p BigDecimal("3.8").floor.to_i                            # 3
p BigDecimal("-2.5").round.to_i                           # -3
p BigDecimal("2.5").round.to_i                            # 3
p (BigDecimal("7.5") % BigDecimal("2")).to_f             # 1.5
p BigDecimal(5).to_f                                      # 5.0
p BigDecimal("123.456").to_i                             # 123
p BigDecimal("2.5") > BigDecimal("2.4")                  # true
p (BigDecimal("1.1") + BigDecimal("2.2")).round(1).to_f  # 3.3
p BigDecimal("1.5e3").to_f                               # 1500.0
p BigDecimal("0.001").to_f                               # 0.001
