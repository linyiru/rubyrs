# `Kernel#sprintf(fmt, *args)` and its alias `Kernel#format`.
# Same underlying `ruby_sprintf` engine `String#%` uses (the
# logic lives in `vm/sprintf.rs`); this commit just exposes
# the no-recv Kernel entry point so scripts can use the more
# common `sprintf "%d", n` idiom alongside `"%d" % n`.

# String interpolation.
puts sprintf("hello %s", "world")         # "hello world"
puts sprintf("%s is %s", "x", "y")        # "x is y"

# Integer formatting.
puts sprintf("%d", 42)                    # "42"
puts sprintf("%05d", 42)                  # "00042" (zero-padded)
puts sprintf("%-5d|", 42)                 # "42   |" (left-aligned)
puts sprintf("%+d", 42)                   # "+42" (forced sign)
puts sprintf("%d + %d = %d", 2, 3, 5)     # "2 + 3 = 5"

# Hex / octal / binary.
puts sprintf("%x", 255)                   # "ff"
puts sprintf("%X", 255)                   # "FF"
puts sprintf("%o", 8)                     # "10"
puts sprintf("%b", 10)                    # "1010"
puts sprintf("0x%04x", 42)                # "0x002a"

# Float formatting.
puts sprintf("%f", 3.14)                  # "3.140000"
puts sprintf("%.3f", 3.14159)             # "3.142"
puts sprintf("%.0f", 3.7)                 # "4"
# `%g` (general-float) not modelled in the Tier 1
# `ruby_sprintf` engine yet; documented gap, not exercised
# here.

# `format` is the canonical alias — same engine, same result.
puts format("formatted: %s", "ok")        # "formatted: ok"
puts format("%d + %d", 1, 2)              # "1 + 2"

# `defined?(sprintf)` — confirmed as a method.
puts defined?(sprintf).inspect            # "method"
puts defined?(format).inspect             # "method"

# Empty format — degenerate but legal.
puts sprintf("").inspect                  # "\"\""

# No-arg error: CRuby raises ArgumentError with the exact
# message "too few arguments".
begin
  sprintf
rescue ArgumentError => e
  puts "AE: #{e.message}"
end

# Non-String format → TypeError ("no implicit conversion of
# Integer into String"). Matches CRuby's class + message.
begin
  sprintf(42)
rescue TypeError => e
  puts "TE: #{e.message}"
end

# String#% still works (regression check) — same engine, just
# the receiver-method form.
puts "%s + %s" % ["a", "b"]               # "a + b"
puts "%d" % 7                             # "7"
