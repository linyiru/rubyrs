# Integer#div — floored integer division (mini_mime's binary search does
# `(to - from).div(2)`). Distinct from `/` only in that it accepts a Float
# divisor and still returns an Integer.
p 7.div(2)         # 3
p 8.div(2)         # 4
p 7.div(3)         # 2
p(-7.div(2))       # -4 (floors toward -inf)
p 7.div(-2)        # -4
p(-7.div(-2))      # 3
p 10.div(2.0)      # 5 (Float divisor -> Integer)
p 7.div(2.0)       # 3
p 0.div(5)         # 0
p 7.respond_to?(:div)  # true
begin; 1.div(0); rescue ZeroDivisionError => e; puts "ZeroDivisionError"; end
