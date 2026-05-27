# `String#dump` — round-trippable literal representation.
#
# CRuby produces a double-quoted form that `eval` would parse
# back into the original string. The escape table:
#   \a \b \t \n \v \f \r \e  — the lettered short forms
#   \" \\                    — quote / backslash
#   \xNN                     — other bytes 0x00..=0x1F and 0x7F
#   \uHHHH / \u{HHHHH}       — non-ASCII codepoints
#   \#                       — only before `{` / `@` / `$`
#   verbatim                 — printable ASCII 0x20..=0x7E
#                              (except `"` `\` `#`-before-trigger)
#
# Motivating use: MRI's lib/erb/compiler.rb:312 (add_put_cmd)
# emits `"#{@put_cmd} #{content.dump}.freeze"` to splice
# template content into the compiled source. Without dump,
# every ERB compile call crashes inside compile_stag.

# --- ASCII printable passes through ---
puts "hello".dump                               # "hello"
puts "".dump                                    # ""
puts "ABCxyz 123 !?".dump                       # "ABCxyz 123 !?"

# --- Lettered short controls ---
puts "\a\b\t\n\v\f\r\e".dump                    # "\a\b\t\n\v\f\r\e"

# --- Quote and backslash ---
puts "with \"quote\"".dump                      # "with \"quote\""
puts "backslash\\".dump                         # "backslash\\"

# --- Other control bytes as \xNN ---
puts "null\0".dump                              # "null\x00"
puts "\x01\x7F".dump                            # "\x01\x7F"
puts "\x1F".dump                                # "\x1F"

# --- # escape: only before {/@/$ ---
puts '#{evil}'.dump                             # "\#{evil}"
puts '#@var'.dump                               # "\#@var"
puts '#$var'.dump                               # "\#$var"
puts 'plain # hash'.dump                        # "plain # hash"
puts 'trailing #'.dump                          # "trailing #"
puts 'mid #x not'.dump                          # "mid #x not"

# --- Non-ASCII BMP codepoints ---
puts "日本語".dump                              # "日本語"
puts "café".dump                                # "café"

# --- Above BMP — curly form ---
puts "smile\u{1F600}".dump                      # "smile\u{1F600}"

# --- ERB-shape probe ---
# Build the exact splice the ERB compiler uses: wrap content
# in `dump` so it round-trips through eval.
content = "Hello, <%= name %>!\nSecond line\n"
spliced = "_erbout << #{content.dump}.freeze"
puts spliced
# _erbout << "Hello, <%= name %>!\nSecond line\n".freeze

# --- Invalid UTF-8 bytes survive as \xNN ---
# Pack raw bytes (some valid as standalone leading bytes, some
# pure invalid). dump must round-trip the exact byte sequence,
# NOT replace with U+FFFD — that's the whole point: eval'ing the
# result reconstructs the original String#bytes.
arr = [0xFF, 0x80, 0x41, 0x42].pack("c*")
puts arr.dump                                   # "\xFF\x80AB"

# --- respond_to? consistency ---
puts "x".respond_to?(:dump)                     # true
