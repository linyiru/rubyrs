# Integer#digits and Integer#bit_length.
# digits: LSB-first digit array (default base 10); base arg
# changes radix. 0.digits == [0]. Negative raises.
# bit_length: ceil(log2(abs(n)+1)), with two's-complement
# semantics for negatives.

# digits — default base 10.
puts 12345.digits.inspect              # [5, 4, 3, 2, 1]
puts 7.digits.inspect                  # [7]
puts 0.digits.inspect                  # [0]
puts 100.digits.inspect                # [0, 0, 1]

# Custom base.
puts 255.digits(16).inspect            # [15, 15]
puts 8.digits(2).inspect               # [0, 0, 0, 1]
puts 100.digits(7).inspect             # [2, 0, 2]

# Negative-receiver and base<2 errors are out of fixture scope:
# CRuby raises `Math::DomainError` / `ArgumentError` which the
# subset narrows to a single `ArgumentError`-shaped Trap. See
# SUBSET.md.

# bit_length — positive cases.
puts 0.bit_length                      # 0
puts 1.bit_length                      # 1
puts 7.bit_length                      # 3
puts 255.bit_length                    # 8
puts 256.bit_length                    # 9

# Negative — two's complement: bit_length(-n) = bit_length(n-1).
puts (-1).bit_length                   # 0
puts (-256).bit_length                 # 8
puts (-257).bit_length                 # 9
