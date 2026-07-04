# sprintf integer directives (%d/%i/%x/%X/%o/%b/%B) with STRING
# arguments — CRuby coerces via strict `Kernel#Integer(str, 0)`
# semantics: prefixes honored, underscores between digits, garbage
# raises `invalid value for Integer()`, past-i64 values render the
# EXACT integer. The historical rubyrs coercion was
# `parse::<i64>().unwrap_or(0)` — garbage AND overflow both
# silently rendered "0".

def try
  puts yield
rescue ArgumentError, TypeError, FloatDomainError => e
  puts "#{e.class}: #{e.message}"
end

# --- %d small ---
try { sprintf("%d", "42") }
try { sprintf("%d", "-42") }
try { sprintf("%d", " 42 ") }
try { sprintf("%d", "+42") }
try { sprintf("%d", "1_0") }
try { sprintf("%05d", "42") }
try { sprintf("%+d", "42") }

# --- %d prefix-driven (base 0) ---
try { sprintf("%d", "0x10") }
try { sprintf("%d", "-0x10") }
try { sprintf("%d", "0b101") }
try { sprintf("%d", "0o17") }
try { sprintf("%d", "0d19") }
try { sprintf("%d", "010") }

# --- %d exact bignum from strings ---
try { sprintf("%d", "18446744073709551616") }
try { sprintf("%d", "-18446744073709551617") }
try { sprintf("%d", "123456789012345678901234567890") }
try { sprintf("%d", "9223372036854775808") }
try { sprintf("%d", "-9223372036854775808") }
try { sprintf("%+d", "18446744073709551616") }
try { sprintf("%30d", "18446744073709551616") }

# --- %d errors (strict!) ---
try { sprintf("%d", "abc") }
try { sprintf("%d", "42abc") }
try { sprintf("%d", "") }
try { sprintf("%d", "4 2") }
try { sprintf("%d", "1__0") }
try { sprintf("%d", "08") }
try { sprintf("%d", nil) }

# --- radix directives with strings ---
try { sprintf("%x", "255") }
try { sprintf("%X", "255") }
try { sprintf("%o", "8") }
try { sprintf("%b", "10") }
try { sprintf("%x", "0x10") }
try { sprintf("%x", "18446744073709551616") }
try { sprintf("%o", "18446744073709551616") }
try { sprintf("%b", "18446744073709551616") }
try { sprintf("%#x", "255") }
try { sprintf("%x", "abc") }   # "abc" is NOT hex here: %x coerces base-0
try { sprintf("%x", "gg") }

# --- %i alias, format alias, String#% route ---
try { sprintf("%i", "18446744073709551616") }
try { format("%d", "18446744073709551616") }
try { "%d" % "18446744073709551616" }

# --- non-Str shapes unchanged (pinned) ---
try { sprintf("%d", 42) }
try { sprintf("%d", 2**64) }
try { sprintf("%x", 2**64) }
try { sprintf("%d", 3.9) }
try { sprintf("%d", Float::NAN) }
