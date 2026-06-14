# frozen_string_literal: true
#
# The `# frozen_string_literal: true` magic comment on line 1 freezes
# every PLAIN string literal in this file. Interpolated strings stay
# mutable (CRuby semantics). rack's spec_builder loads a frozen.ru
# rackup that asserts `'frozen'.frozen?`. (The FrozenError *message*
# carries the receiver's inspect; this fixture catches by class to
# stay independent of that formatting.)

s = "hello"
p s.frozen?                 # true
p "another".frozen?         # true
p :"sym".frozen?            # true (symbols always frozen)

# Interpolated strings are NOT frozen even under the magic comment.
x = 42
p "interp #{x}".frozen?     # false
p "#{1}".frozen?            # false

# Mutating a frozen literal raises FrozenError.
begin
  s << " world"
  puts "no raise"
rescue FrozenError
  puts "FrozenError"
end

# `dup` of a frozen literal is mutable.
d = "base".dup
p d.frozen?                 # false
d << "!"
p d                         # "base!"

# eval gets its OWN frozen_string_literal setting — it does NOT
# inherit this file's. A bare eval string is mutable...
p eval('"e"').frozen?       # false
# ...unless the eval source carries its own magic comment.
p eval("# frozen_string_literal: true\n\"f\"").frozen?  # true

# Frozen literals still compare / read normally.
p("hello" == s)             # true
p s.upcase                  # "HELLO" (returns a new string)
p s.length                  # 5
