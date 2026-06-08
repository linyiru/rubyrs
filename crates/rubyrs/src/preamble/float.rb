# Float class constants. CRuby exposes these on the built-in Float class;
# rubyrs's Float values are immediates with no backing constant table, so
# reopening `class Float` and assigning the constants is the portable way
# to make `Float::INFINITY` / `Float::NAN` resolve. INFINITY / NAN are
# derived arithmetically (1.0/0.0, 0.0/0.0) so they carry the exact IEEE
# bit pattern; the rest are the standard IEEE-754 double values.
class Float
  INFINITY = 1.0 / 0.0
  NAN = 0.0 / 0.0
  MAX = 1.7976931348623157e+308
  MIN = 2.2250738585072014e-308
  EPSILON = 2.220446049250313e-16
  DIG = 15
  MANT_DIG = 53
end
