# `require_relative "lib"` — loads ./require_relative_lib.rb,
# returns true on first load, false on repeats.

# First load returns true. Captured into a local to verify.
result = require_relative "require_relative_lib"
puts result

# Top-level method from the loaded file is now callable.
puts greet("Mochi")

# Top-level constant from the loaded file is reachable.
puts SHARED

# A class defined in the loaded file works the same as one
# defined inline.
g = Greeter.new("hi")
puts g.call("there")

# Repeat call returns false (already loaded; body not re-run).
result2 = require_relative "require_relative_lib"
puts result2

# Explicit `.rb` extension resolves to the same canonical path
# as the bare name above, so it dedupes via loaded_features and
# returns false (no re-execution).
result3 = require_relative "require_relative_lib.rb"
puts result3

# Outer-rescue-catches-from-required-file. The required helper
# raises mid-execution; the begin/rescue here catches it. The
# fixture verifies (a) the rescue actually fires, (b) the
# bound exception is the right object (not garbage from a
# corrupted operand stack), and (c) loaded_features was rolled
# back so a retry would attempt to load again.
begin
  require_relative "require_relative_raise"
rescue RuntimeError => e
  puts "caught: #{e.message}"
end
# Retry should attempt to load again (not silently no-op).
begin
  require_relative "require_relative_raise"
rescue RuntimeError => e
  puts "caught again: #{e.message}"
end
