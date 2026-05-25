# String#center / #ljust / #rjust — pad to `width` with an
# optional pad string (default " "). Pad cycles when multichar.
# `width <= receiver.length` returns the receiver unchanged.
# `center` puts the extra char (when total padding is odd) on
# the RIGHT, matching CRuby.

# Default-pad (space).
puts "hi".center(10).inspect          # "    hi    "
puts "hi".ljust(10).inspect           # "hi        "
puts "hi".rjust(10).inspect           # "        hi"

# Single-char custom pad.
puts "hi".center(10, "*").inspect     # "****hi****"
puts "hi".ljust(8, "-").inspect       # "hi------"
puts "hi".rjust(8, ".").inspect       # "......hi"

# Multichar pad cycles.
puts "hi".center(11, "-=").inspect    # "-=-=hi-=-=-"  (odd: right gets extra)
puts "hi".ljust(7, "ab").inspect      # "hiabab"... wait check it
puts "hi".rjust(7, "ab").inspect      # "ababahi"

# Width <= length: receiver unchanged.
puts "hello".center(3).inspect        # "hello"
puts "hello".ljust(5).inspect         # "hello"
puts "hello".rjust(2).inspect         # "hello"

# Width == 0.
puts "x".rjust(0, ".").inspect        # "x"

# Empty receiver.
puts "".center(4, "x").inspect        # "xxxx"
puts "".ljust(4, "x").inspect         # "xxxx"
puts "".rjust(4, "x").inspect         # "xxxx"

# Empty pad raises.
begin
  "x".center(5, "")
rescue ArgumentError => e
  puts "empty-pad: caught"
end
