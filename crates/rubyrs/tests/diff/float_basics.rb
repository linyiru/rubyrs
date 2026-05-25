# Float literals and to_s formatting
puts 0.0
puts 1.0
puts 3.14
puts -2.5
puts 100.0

# Pure Float arithmetic
puts 1.5 + 2.5
puts 2.0 - 0.5
puts 0.5 * 4.0
puts 7.0 / 2.0
puts 7.5 % 3.0

# Mixed Int / Float coercion — result is Float
puts 5 + 0.5
puts 0.5 + 5
puts 2 * 0.5
puts 10 / 4.0
puts 10.0 / 4
puts 7 % 2.0

# Comparisons (including cross-type)
puts 1.5 < 2.0
puts 1.5 <= 1.5
puts 3.0 > 2.5
puts 5 == 5.0
puts 5.0 == 5
puts 5 != 5.0
puts 5.5 == 5

# Unary minus / abs
puts -3.14
puts (-2.5).abs
puts 0.0.abs

# Conversions
puts 3.14.to_i      # 3 (truncates toward zero)
puts (-3.7).to_i    # -3 (CRuby truncates toward zero, not floor)
puts 5.to_f
puts 5.0.to_f
puts "3.14".to_f
puts "  -2.5xyz".to_f
puts "abc".to_f     # 0.0
puts "".to_f        # 0.0

# Predicates
puts 0.0.zero?
puts 0.5.zero?
puts 1.5.positive?
puts (-1.5).negative?
puts 0.0.positive?

# Rounding / floor / ceil (Integer results)
puts 3.7.round
puts 3.4.round
puts 3.6.floor
puts 3.1.ceil
puts (-3.6).floor
puts (-3.1).ceil

# Infinity / NaN sentinels
puts (1.0 / 0.0).infinite?
puts (-1.0 / 0.0).infinite?
puts 1.0.infinite?.nil?
puts (0.0 / 0.0).nan?
puts 1.0.nan?
puts 1.5.finite?

# Class identity
puts 1.5.class
puts 1.5.class.name
puts 1.5.class == Float

# respond_to? on Float
puts 1.5.respond_to?(:floor)
puts 1.5.respond_to?(:abs)
puts 1.5.respond_to?(:upcase)
