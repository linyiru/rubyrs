# `Array#member?` and `Range#member?` are CRuby aliases for
# `include?` (both classes define their own `include?` for
# performance; `member?` is the Enumerable-style alias that
# CRuby surfaces on the same dispatch). Tilt's lexer hits this
# via `is_erb_stag?` on an Array of trim markers — without it,
# the multiline trim path (`<%- -%>`) raises NoMethodError.

# --- Array#member? ---
puts [1, 2, 3].member?(2)                    # true
puts [1, 2, 3].member?(99)                   # false
puts ["a", "b"].member?("a")                 # true
puts [].member?(:anything)                   # false
puts [1, "1", :one].member?("1")             # true (== semantics, not eql?)

# --- respond_to? whitelist ---
puts [1, 2].respond_to?(:member?)            # true
puts (1..5).respond_to?(:member?)            # true

# --- Range#member? (Int bounds) ---
puts (1..5).member?(3)                       # true
puts (1..5).member?(5)                       # true (inclusive)
puts (1...5).member?(5)                      # false (exclusive)
puts (1..5).member?(10)                      # false

# --- Range#member? (String bounds) ---
puts ("a".."e").member?("c")                 # true
puts ("a".."e").member?("z")                 # false
