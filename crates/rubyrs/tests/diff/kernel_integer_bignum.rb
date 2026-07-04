# `Kernel#Integer` — BigInt promotion, negative bases, the
# `exception:` keyword, and CRuby's exact error-message formats
# (probed against 3.4). Complements `kernel_integer_radix.rb`
# (which pins the basic strict rules); this fixture pins what the
# str2int rework added.

def try
  puts yield.inspect
rescue ArgumentError, TypeError, FloatDomainError => e
  puts "#{e.class}: #{e.message}"
end

# --- exact BigInt promotion ---
try { Integer("18446744073709551616") }
try { Integer("-18446744073709551617") }
try { Integer("123456789012345678901234567890") }
try { Integer("ffffffffffffffffff", 16) }
try { Integer("1_000_000_000_000_000_000_000") }
try { Integer("9" * 80) }
try { Integer("18446744073709551616").class }

# --- i64 boundary discipline ---
try { Integer("9223372036854775807") }
try { Integer("9223372036854775808") }
try { Integer("-9223372036854775808") }
try { Integer("-9223372036854775809") }

# --- Integer identity holds for the Bignum span ---
try { Integer(2 ** 100) }
try { Integer(Integer("18446744073709551616")) }

# --- negative bases: prefix-driven with default |base| ---
try { Integer("10", -16) }
try { Integer("0x10", -16) }
try { Integer("0b10", -16) }   # prefix OVERRIDES the default
try { Integer("042", -10) }    # bare leading 0 → octal, even here
try { Integer("010", -1) }     # -1 ≡ auto/10
try { Integer("z", -1) }
try { Integer("10", -2) }
try { Integer("ffffffffffffffffff", -16) }
try { Integer("10", -37) }     # message shows the NEGATED radix
try { Integer("10", 37) }
try { Integer("10", 1) }
try { Integer("0", -1) }

# --- `exception:` keyword ---
try { Integer("42", exception: false) }
try { Integer("42", exception: true) }
try { Integer("abc", exception: false) }
try { Integer("18446744073709551616", exception: false) }
try { Integer("abc", 16, exception: false) }   # "abc" IS hex
try { Integer("zz", 16, exception: false) }
try { Integer("zz", 36, exception: false) }    # valid base-36 parse WITH kwarg
try { Integer(nil, exception: false) }
try { Integer(:sym, exception: false) }
try { Integer(Float::NAN, exception: false) }
try { Integer(42, exception: false) }          # Int identity WITH kwarg
try { Integer(42, 10, exception: false) }      # base-for-non-string → nil
try { Integer("10", 99, exception: false) }    # invalid radix STILL raises
try { Integer("abc", exception: nil) }         # must be literal true/false
try { Integer("42", exception: "x") }          # raises even when parseable

# --- kwargs syntax vs positional literal brace-hash ---
# The VM records how a trailing Hash was passed
# (`trailing_hash_positional`, the same signal the user-method
# keyword binder consumes), so a literal `{...}` in a positional
# slot is NOT peeled as keywords: it's a (bad) radix / value / third
# positional, exactly as CRuby treats it.
try { Integer("42", {exception: false}) }        # TypeError (Hash radix)
try { Integer("42", {exception: "x"}) }          # TypeError (Hash radix)
try { Integer({exception: false}) }              # TypeError (Hash value)
try { Integer("ff", 16, {exception: false}) }    # arity (given 3)
try { Integer("42", {"exception" => false}) }    # TypeError (Hash radix)
h = {exception: false}
try { Integer("abc", h) }                        # positional var → TypeError
try { Integer("abc", **h) }                      # kwargs splat → nil
try { Integer("42", **{}) }                      # empty splat → plain call
try { send(:Integer, "42", {exception: false}) } # positional through send
try { send(:Integer, "abc", exception: false) }  # kwargs through send

# --- unknown keywords (kwargs syntax established → CRuby messages,
# key.inspect for Symbol and non-Symbol keys alike) ---
try { Integer("42", bogus: false) }
try { Integer("42", bogus: 1, extra: 2) }
try { Integer("42", exception: false, bogus: 1) }
try { s = {"a" => 1}; Integer("42", **s) }

# --- message formatting uses String#inspect ---
try { Integer("4\n2") }
try { Integer("42\0") }
try { Integer("42\0abc") }
try { Integer("4\t2") }
try { Integer("１０") }        # fullwidth digits are invalid
try { Integer("＋42") }

# --- whitespace: ASCII yes, unicode no ---
try { Integer(" 42 ") }
try { Integer("\t\n\v\f\r42\t\n\v\f\r") }
try { Integer("42 ") }         # NBSP tail → invalid
try { Integer(" 42") }         # NBSP lead → invalid
try { Integer("  -0x10  ") }
try { Integer("  0x10  ", 16) }

# --- underscore strictness ---
try { Integer("1_0") }
try { Integer("1__0") }
try { Integer("1_") }
try { Integer("_1") }
try { Integer("0x_10") }
try { Integer("0x1_0") }
try { Integer("0x10_") }
try { Integer("0_1_0") }
try { Integer("0_") }
try { Integer("0b_10", 2) }

# --- prefixes / octal strictness ---
try { Integer("0x10") }
try { Integer("0b10") }
try { Integer("0o17") }
try { Integer("0d19") }
try { Integer("010") }
try { Integer("08") }
try { Integer("0o10", 0) }
try { Integer("0x10", 0) }
try { Integer("0x10", 10) }
try { Integer("0xb", 11) }
try { Integer("0b10", 36) }
try { Integer("00x10", 16) }
try { Integer("0x", 16) }
try { Integer("0", 16) }
try { Integer("010", 10) }
try { Integer("00") }
try { Integer("0_0") }

# --- non-String shapes (unchanged behavior, pinned) ---
try { Integer(42) }
try { Integer(3.9) }
try { Integer(-3.9) }
try { Integer(Float::NAN) }
try { Integer(Float::INFINITY) }
try { Integer(nil) }
try { Integer(42, 10) }
try { Integer(nil, 10) }

# --- Kernel.Integer module-function form keeps the same path ---
try { Kernel.Integer("18446744073709551616") }
try { Kernel.Integer("abc", exception: false) }
