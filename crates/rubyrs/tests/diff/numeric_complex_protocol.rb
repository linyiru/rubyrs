# The shared Numeric real-number protocol (CRuby installs most of
# these on the Numeric superclass): the complex-decomposition views
# real / imaginary / conjugate / rectangular / polar / arg / abs2,
# plus magnitude / integer? / numerator / denominator / fdiv /
# finite? / infinite?. Each holds for Integer, Float, and Rational
# because their imaginary part is zero. Byte-stable against CRuby —
# the values are exact (Float arg/fdiv print identically).

vals = [5, -5, 0, 12, 1.5, -2.5, 0.0, Rational(3, 4), Rational(-7, 2)]

vals.each do |v|
  puts "--- #{v.inspect} (#{v.class}) ---"
  puts "real=#{v.real.inspect} imag=#{v.imaginary.inspect}/#{v.imag.inspect}"
  puts "conj=#{v.conjugate.inspect}/#{v.conj.inspect} real?=#{v.real?}"
  puts "rect=#{v.rectangular.inspect}/#{v.rect.inspect}"
  puts "polar=#{v.polar.inspect}"
  puts "arg=#{v.arg.inspect} angle=#{v.angle.inspect} phase=#{v.phase.inspect}"
  puts "abs2=#{v.abs2.inspect} magnitude=#{v.magnitude.inspect}"
  puts "integer?=#{v.integer?} finite?=#{v.finite?} infinite?=#{v.infinite?.inspect}"
  puts "numerator=#{v.numerator.inspect} denominator=#{v.denominator.inspect}"
  puts "fdiv(2)=#{v.fdiv(2).inspect}"
end
