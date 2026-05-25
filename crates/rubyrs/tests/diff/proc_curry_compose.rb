# Proc#curry / Proc#>> / Proc#<< — composition and partial
# application on lambdas/procs. `>>` / `<<` already accept a
# Block on the right (L4a); this fixture exercises Block on the
# LEFT too.

add  = ->(a, b) { a + b }
mul  = ->(a, b) { a * b }
succ = ->(x) { x + 1 }
dbl  = ->(x) { x * 2 }

# curry on lambda.
c_add = add.curry
puts c_add.(3).(4)             # 7
puts c_add.(10, 20)            # 30
puts c_add.class.name          # Proc

# Three-arg lambda curry.
sum3 = ->(a, b, c) { a + b + c }.curry
puts sum3.(1).(2).(3)          # 6
puts sum3.(1, 2).(3)           # 6
puts sum3.(1)[2][3]            # 6 (bracket form)

# >> on lambda recv (already supported in L4a — included for
# coverage of the symmetry).
puts (succ >> dbl).(5)         # dbl(succ(5)) = 12
puts (dbl  >> succ).(5)        # succ(dbl(5)) = 11
puts (succ << dbl).(5)         # succ(dbl(5)) = 11

# Mixing lambda and Method.
class Squared
  def call(x); x * x; end
end
m = Squared.new.method(:call)
puts (succ >> m).(4)           # squared(succ(4)) = 25
puts (m << succ).(4)           # squared(succ(4)) = 25
puts (m.curry).(6)             # squared(6) = 36

# Explicit arity hint on Proc#curry — restricts to that count.
c_var = ->(*args) { args.inject(0) { |s, x| s + x } }.curry(3)
puts c_var.(1).(2).(3)         # 6
