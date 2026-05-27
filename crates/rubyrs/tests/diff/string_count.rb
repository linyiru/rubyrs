# `String#count(*selectors)` — count chars matching every
# selector. CRuby tr-style mini-syntax:
#
#   "abc".count("a")       → 1
#   "abc".count("a-c")     → 3   (range expansion)
#   "abc".count("^a")      → 2   (negation when ^ is first)
#   "abc".count("a", "b")  → 0   (multi-arg: intersection)
#
# Motivating use: MRI lib/erb/compiler.rb:312 — counts
# newlines in template content to keep line offsets accurate
# in the compiled output. `content.count("\n")` is the
# minimum shape, but the full spec is small enough to land
# in one go.

# --- Single literal char ---
puts "hello".count("l")                         # 2
puts "hello world".count("o")                   # 2
puts "Hello".count("aeiouAEIOU")                # 2 (multi-char set)
puts "abc\ndef\nghi".count("\n")                # 2

# --- Range expansion ---
puts "abcdef".count("a-c")                      # 3
puts "abcdef".count("d-z")                      # 3
puts "Hello123".count("0-9")                    # 3

# --- Negation (leading ^) ---
puts "abcdef".count("^abc")                     # 3
puts "Hello, World!".count("^a-zA-Z")           # 3 (comma, space, !)

# --- Empty selector matches nothing ---
puts "abc".count("")                            # 0
puts "".count("abc")                            # 0

# --- Multi-arg intersection ---
puts "abc".count("a", "b")                      # 0 (no char matches both)
puts "abcabc".count("a", "abc")                 # 2 (a matches both selectors)
puts "abcdef".count("a-c", "b-d")               # 2 (b and c)

# --- Multibyte ---
puts "日本語".count("日")                        # 1
puts "café".count("é")                          # 1

# --- ERB-shape probe ---
# Mirror lib/erb/compiler.rb:312's newline-counting idiom.
content = "Hello, <%= name %>!\nSecond line\nThird\n"
puts content.count("\n")                        # 3

# --- respond_to? consistency ---
puts "x".respond_to?(:count)                    # true
