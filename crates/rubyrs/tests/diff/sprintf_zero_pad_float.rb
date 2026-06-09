# The `0` flag zero-pads a float to width even WITH a precision
# (`%05.2f` of 3.14 → "03.14"). Previously a precision suppressed the
# zero flag for floats too (it only legitimately does so for integers).
# The sign / space prefix stays outside the zeros.

p("%05.2f" % 3.14159)     # "03.14"
p("%08.2f" % 3.14)        # "00003.14"
p("%+08.2f" % 3.14)       # "+0003.14"   (sign before zeros)
p("%05.1f" % -3.2)        # "-03.2"      (minus before zeros)
p("% 08.2f" % 3.14)       # " 0003.14"   (space before zeros)
p("%08.0f" % 42.0)        # "00000042"
p("%-8.2f|" % 3.14)       # "3.14    |"  (left-justify, no zeros)
p("%5.2f" % 3.14)         # " 3.14"      (no zero flag → spaces)

# integer conversions: a precision overrides the 0 flag (CRuby parity)
p("%05.2d" % 1)           # "   01"
p("%05d" % 42)            # "00042"      (no precision → zeros)
p("%08x" % 255)           # "000000ff"
p("%+06d" % 42)           # "+00042"

# no width / plain
p("%.2f" % 3.14159)       # "3.14"
p("%f" % 1.5)             # "1.500000"
