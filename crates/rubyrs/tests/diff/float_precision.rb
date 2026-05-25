# Float#round(n) and Float#truncate(n) — precision-arg forms.
# Default (no-arg) returns Int.
# n > 0  → keep n digits after decimal, returns Float.
# n == 0 → returns Int (same as no-arg).
# n < 0  → zero out low-order Integer digits, returns Int.

# round
puts 3.14159.round                          # 3
puts 3.14159.round(0)                       # 3
puts 3.14159.round(2)                       # 3.14
puts 3.14159.round(4)                       # 3.1416
puts 3.5.round                              # 4
puts (-3.5).round                           # -4  (banker's tie-break: round half to even? actually CRuby ties-away)
puts 1234.5678.round(-2)                    # 1200
puts 1234.5678.round(-3)                    # 1000
puts 0.5.round(1)                           # 0.5

# truncate
puts 3.14159.truncate                       # 3
puts 3.14159.truncate(0)                    # 3
puts 3.14159.truncate(2)                    # 3.14
puts 3.14159.truncate(4)                    # 3.1415
puts (-3.14159).truncate(2)                 # -3.14
puts 1234.5678.truncate(-2)                 # 1200
puts 1234.5678.truncate(-3)                 # 1000
puts 999.999.truncate(-1)                   # 990
