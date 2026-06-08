# Float class constants (CRuby exposes these on the built-in Float;
# rubyrs defines them in preamble/float.rb by reopening the class).
# Values compared via predicates/relations to avoid the separate
# large-exponent float-formatting divergence (e+308 vs e308).
p Float::INFINITY                       # Infinity
p(-Float::INFINITY)                     # -Infinity
p Float::INFINITY.infinite?             # 1
p (-Float::INFINITY).infinite?          # -1
p Float::NAN.nan?                       # true
p Float::MAX > 1.0e308                  # true
p Float::MAX.finite?                    # true
p Float::MIN > 0.0                      # true
p Float::MIN < 1.0e-307                 # true
p Float::EPSILON < 1.0e-15             # true
p Float::EPSILON > 0.0                  # true
p Float::DIG                            # 15
p Float::MANT_DIG                       # 53
p 1.0.infinite?                         # nil
p (1.0 / 0.0) == Float::INFINITY        # true
p 5 < Float::INFINITY                   # true
p (1..Float::INFINITY).class            # Range (infinite range constructs)
