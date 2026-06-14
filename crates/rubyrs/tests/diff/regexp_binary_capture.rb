# Regex captures over an ASCII-8BIT (BINARY) subject must preserve the
# raw bytes and stay tagged ASCII-8BIT — NOT round-trip through a lossy
# UTF-8 view that turns an invalid byte into a 3-byte U+FFFD. Covers the
# $~ surface: $1..$9, $&, $+, and $~[n] (MatchData). rack's multipart
# parser reads this for the content-disposition head (StringScanner uses
# $~ under the hood), which may carry an invalid filename byte.

s = "name=\"inv\xC3.txt\"\r\n".b      # ASCII-8BIT, byte 0xC3 is invalid UTF-8

s =~ /name="(.*?)"/
p $1.bytes                            # [105,110,118,195,46,116,120,116]
p $1.encoding.to_s                    # "ASCII-8BIT"
p $&.bytes                            # whole match, byte-faithful
p $~[1].bytes                         # MatchData positional, byte-faithful
p $~[1].encoding.to_s                 # "ASCII-8BIT"

# multiple captures + $+ (last participating group)
b = "\xFFkey\xFF=val".b
b =~ /(\xFF)(\w+)(\xFF)=(\w+)/n
p [$1.bytes, $2, $4]                  # [[255], "key", "val"]
p $+                                  # "val" (last group)

# alternation: only the matched arm's group is non-nil; bytes preserved
"\xC3x".b =~ /(\xC3)|(y)/n
p [$1.bytes, $2]                      # [[195], nil]

# a plain UTF-8 subject is unaffected (normal path)
"hello world" =~ /(\w+) (\w+)/
p [$1, $2, $1.encoding.to_s]          # ["hello", "world", "UTF-8"]
