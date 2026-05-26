# String#+@ / String#-@ — the unfreeze / freeze unary idioms.
#
# CRuby 3.x:
#   +s : ALWAYS a fresh non-frozen dup (older docs implied
#        "same object when not frozen" — empirical verification
#        against CRuby 3.4 shows a new instance every time).
#   -s : returns s itself if frozen, otherwise a frozen dup
#        (CRuby additionally dedupes via a hidden frozen-string
#         table; rubyrs's subset doesn't model that, but the
#         observable contract — same value, guaranteed frozen
#         state — matches)
#
# Motivating use: MRI's `lib/erb/compiler.rb:282`:
#   @script = +''
# Builds a mutable string in `# frozen_string_literal: true`
# files where the literal `''` is frozen. Without `+@`, the
# subsequent `@script << "..."` raises FrozenError.

# --- +@ on a non-frozen string returns a fresh dup ---
# Even though the input is unfrozen, CRuby always allocates a
# new String for +@. Same content, different object identity.
s = "hello"
puts s.frozen?                                  # false
result = +s
puts result.equal?(s)                           # false — fresh object
puts result.frozen?                             # false
puts result                                     # hello

# --- +@ on a frozen string returns an unfrozen dup ---
f = "frozen".freeze
puts f.frozen?                                  # true
result = +f
puts result.equal?(f)                           # false — fresh object
puts result.frozen?                             # false
puts result                                     # frozen — same content
# The dup is mutable.
result << "!"
puts result                                     # frozen!
# Original untouched.
puts f                                          # frozen

# --- -@ on a frozen string returns receiver (identity) ---
g = "abc".freeze
puts g.frozen?                                  # true
neg = -g
puts neg.equal?(g)                              # true (same object)

# --- -@ on a non-frozen string returns a frozen dup ---
m = "mutable"
puts m.frozen?                                  # false
neg = -m
puts neg.equal?(m)                              # false — fresh object
puts neg.frozen?                                # true
puts neg                                        # mutable — same content
# Original still mutable.
puts m.frozen?                                  # false
m << "?"
puts m                                          # mutable?

# --- ERB-shape probe ---
# Mirror `@script = +''` then mutate. The literal '' in a
# frozen_string_literal context would be frozen; +'' yields a
# mutable string. We can't toggle the magic comment per test, so
# explicit-freeze the source then exercise +@.
script = +("init".freeze)
script << " then more"
puts script                                     # init then more
puts script.frozen?                             # false

# --- respond_to? consistency ---
# Both methods must appear in the dispatch whitelist so
# feature-detection agrees with the actual call path.
puts "x".respond_to?(:+@)                       # true
puts "x".respond_to?(:-@)                       # true
