# Octal escapes inside a character class (`[^}\2]`) — CRuby/Onigmo
# read `\2` there as the octal char U+0002, not a backreference. The
# Rust regex engines need it rewritten to `\x{..}`. Discovery: P3
# Jekyll spike — kramdown's IAL parser builds such a class at load.
r = /([^}\2]+)/
p "ab}c".scan(r)
p "}".match?(r)
p "\x02".match?(r)
p "abc".match?(r)

# multi-digit octal in a class
r2 = /[\101\102]/        # \101=A \102=B
p "AxB".scan(r2)
p "C".match?(r2)

# normal classes / escapes unaffected
p "a1b2c3".scan(/[0-9]/)
p "[x]".match?(/\[\w\]/)
p "a]b".scan(/[^\]]/)      # escaped ] inside class
p "x-y".scan(/[\w-]+/)
p "a^b".scan(/[\^a]/)      # caret not at class start
p "".match?(/[^}\2]/)
