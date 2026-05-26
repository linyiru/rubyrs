# `String#index(needle, offset)` — Tier 1 two-arg form.
# StringIO#gets needed this for incremental newline scanning;
# pre-fix the rubyrs preamble had to slice with `s[@pos..]`
# then call the one-arg `index` on the substring, paying an
# allocation per call.

s = "hello world hello"

# Default offset (== single-arg form) — regression check.
puts s.index("hello")              # 0
puts s.index("hello", 0)           # 0
puts s.index("world", 0)           # 6

# Positive offset — first match AT OR AFTER the index.
puts s.index("hello", 1)           # 12 (skip the leading occurrence)
puts s.index("o", 5)               # 7  ("world"'s 'o')
puts s.index("o", 8)               # 16 ("hello"'s second 'o' at the tail)

# Offset that lands past the start of an occurrence but still
# inside it — `index` skips that one because the FULL needle
# must fit AT OR AFTER offset.
puts s.index("hello", 13).inspect  # nil (past the second "hello")

# Returned index is ABSOLUTE in the receiver, not relative to
# offset.
puts s.index("hello", 5) == 12     # true
puts s.index("world", 5) == 6      # true

# Out-of-range positive offset → nil.
puts s.index("hello", 100).inspect # nil
puts s.index("hello", s.length).inspect # nil (offset == len, needle non-empty)

# Negative offset — counts from the end.
puts s.index("hello", -5)          # 12 (start near "hello" tail)
puts s.index("hello", -17)         # 0  (start at index 0)
puts s.index("hello", -18).inspect # nil (negative offset past start)
puts s.index("hello", -100).inspect # nil (way past)

# Empty needle — matches at the offset itself, or 0 by default.
puts "abc".index("")               # 0
puts "abc".index("", 1)            # 1
puts "abc".index("", 3)            # 3 (offset == len allowed for empty needle)
puts "abc".index("", 4).inspect    # nil (past end even for empty)
