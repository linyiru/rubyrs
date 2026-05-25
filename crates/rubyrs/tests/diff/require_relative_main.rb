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
