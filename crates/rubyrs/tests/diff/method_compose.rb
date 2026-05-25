# Method#>> and Method#<< — function composition.
# (f >> g).(x) == g.(f.(x))   — args flow left-to-right
# (f << g).(x) == f.(g.(x))   — args flow right-to-left

class Calc
  def dbl(x); x * 2; end
  def succ(x); x + 1; end
  def to_s_pretty(x); "n=#{x}"; end
end

c = Calc.new
f = c.method(:dbl)
g = c.method(:succ)
h = c.method(:to_s_pretty)

# Basic >>
puts (f >> g).(5)               # succ(dbl(5))  = 11
puts (g >> f).(5)               # dbl(succ(5))  = 12

# Basic <<
puts (f << g).(5)               # dbl(succ(5))  = 12
puts (g << f).(5)               # succ(dbl(5))  = 11

# Triple chain: dbl >> succ >> to_s_pretty
chain = (f >> g) >> h
puts chain.(5)                  # to_s_pretty(succ(dbl(5))) = "n=11"

# Mixed Method + Proc
add5 = ->(x) { x + 5 }
puts (f >> add5).(3)            # add5(dbl(3)) = 11
puts (add5 << f).(3)            # add5(dbl(3)) = 11 — Proc << Method

# Stored and reused.
sq_succ = c.method(:dbl) >> c.method(:succ)
puts sq_succ.(10)               # 21
puts sq_succ.(20)               # 41

# Multi-arg method on the left, single-arg on the right.
class Adder
  def add(a, b); a + b; end
  def shout(s); "#{s}!"; end
end
a = Adder.new
add_then_shout = a.method(:add) >> a.method(:shout)
puts add_then_shout.(2, 3)      # "5!"
