# Encoding — Tier 1 preamble stub.
#
# CRuby distinguishes encodings (UTF-8, ASCII-8BIT, US-ASCII,
# UTF-16LE, ...) per-string with a tag carried alongside the
# bytes. rubyrs stores raw bytes with no per-string encoding
# tag, so every String reports as UTF-8. The Encoding stub
# exists so codebases that check `enc.dummy?`,
# `enc.ascii_compatible?`, `s.encoding == Encoding::UTF_8`,
# or pass encodings around still get the right shape from the
# call site.
#
# Motivating use: MRI's `lib/erb/compiler.rb:317` calls
# `enc = s.encoding; raise if enc.dummy?` near the top of the
# template-compile path. Without an Encoding instance behind
# `s.encoding`, the dummy? call would NoMethodError on String.

# --- String#encoding returns an Encoding instance ---
s = "hello"
puts s.encoding.inspect                         # #<Encoding:UTF-8>
puts s.encoding.class                           # Encoding
puts s.encoding.name                            # UTF-8
puts s.encoding.to_s                            # UTF-8

# --- Always-false dummy? / always-true ascii_compatible? ---
# rubyrs doesn't model dummy encodings (the ones CRuby uses
# for, e.g., UTF-16 byte-order ambiguity); every encoding we
# serve up reports false for dummy? and true for
# ascii_compatible?. Stable answers across all four
# predefined constants.
puts s.encoding.dummy?                          # false
puts s.encoding.ascii_compatible?               # true

# --- Encoding.find returns the cached instance ---
# Repeated calls with the same name return the SAME instance
# (Encoding has a class-variable cache keyed by name string).
# This is critical for the `enc == Encoding::UTF_8` idiom.
u1 = Encoding.find("UTF-8")
u2 = Encoding.find("UTF-8")
puts u1.equal?(u2)                              # true
puts u1.equal?(Encoding::UTF_8)                 # true

# --- Predefined constants ---
puts Encoding::UTF_8.name                       # UTF-8
puts Encoding::US_ASCII.name                    # US-ASCII
puts Encoding::ASCII_8BIT.name                  # ASCII-8BIT
puts Encoding::BINARY.name                      # ASCII-8BIT
# BINARY is an alias for ASCII_8BIT — same instance.
puts Encoding::BINARY.equal?(Encoding::ASCII_8BIT)  # true

# --- String#b — receiver in our subset, ASCII-8BIT copy in CRuby ---
# rubyrs doesn't tag encodings per-string, so the receiver
# satisfies the contract callers (ERB's compiler at
# lib/erb/compiler.rb:319) expect: a String whose bytes are
# the same and whose subsequent regex matches work the same.
b = "raw".b
puts b                                          # raw
puts b.length                                   # 3

# --- Comparison ---
# `s.encoding == Encoding::UTF_8` is the canonical idiom for
# the ASCII-vs-other branch. Same instance, so == is true.
puts(s.encoding == Encoding::UTF_8)             # true

# --- ERB-shape probe ---
# Mirror the lib/erb/compiler.rb:317 entry shape:
#   enc = s.encoding
#   raise ArgumentError, "..." if enc.dummy?
# Should pass through cleanly without raising.
begin
  enc = "template body".encoding
  raise ArgumentError, "should not raise" if enc.dummy?
  puts "passed the dummy? check"
rescue ArgumentError => e
  puts "unexpected: #{e.message}"
end
