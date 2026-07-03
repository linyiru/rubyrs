# Char-indexed String ops + match-at-pos on LARGE strings — pins the
# char->byte index / UTF-8-validity caches (StrCell) and, critically,
# their invalidation on mutation (borrow_mut clears them; a stale
# table would slice at the wrong byte offsets after `<<` / `[]=` /
# `sub!`). Sizes are above the 256-byte cache threshold.

# --- large non-ASCII string: slicing via the char->byte table ---
seg = "abcéfg日本x"                   # 9 chars, multibyte in the middle
big = seg * 40                        # 360 chars, > threshold
puts big.length
puts big[3]
puts big[3, 4]
puts big[7]
puts big[-2]
puts big[9, 3]
puts big[3..7]
puts big[-5..]
puts big[..4]
puts big[357, 10]
puts big[360, 1].inspect              # at end -> ""
puts big[361, 1].inspect              # past end -> nil
puts big[-361, 2].inspect             # before start -> nil

# --- mutation invalidates the cached table ---
big << "ZΩ"
puts big.length
puts big[360, 2]                      # the appended pair
puts big[-1]
big[0] = "Q"                          # in-place char replace
puts big[0, 3]
big.sub!(/日本/, "**")
puts big[3, 6]
puts big.length

# --- match / match? at a char position (large ASCII) ---
line = "        expect(call(a, b)).to eq(res) # note\n"
src = line * 60                        # ~2.7KB ASCII
pos = line.length                      # start of line 2 (a space)
m = src.match(/\G\s+/, pos)
puts m[0].length                       # run of 8 leading spaces
puts m.pre_match.length
puts m.post_match.length
puts m.string.length
puts src.match(/\G\S/, pos).inspect    # \G miss at a space -> nil
puts src.match?(/\G\s/, pos)
puts src.match?(/\G\S/, pos)
puts src.match(/expect/, pos).begin(0) # forward search from pos
puts src.match?(/zzz/, pos)
puts src.match(/e/, -6)[0]             # negative pos
puts src.match(/e/, src.length).inspect  # pos == len -> nil (no match)
puts src.match(/e/, src.length + 1).inspect # out of range -> nil
puts src.match?(/e/, src.length + 1)

# --- match at pos on a large NON-ASCII string (char->byte via table) ---
nsrc = ("é" * 300) + "target here"
puts nsrc.match(/target/, 300)[0]
puts nsrc.match(/\Gtarget/, 300)[0]
puts nsrc.match(/\Gtarget/, 299).inspect
puts nsrc.match?(/\Gt/, 300)
puts nsrc =~ /target/                  # byte-independent char semantics via $~? (=~ returns char index in CRuby)

# --- mutation then re-match (validity cache invalidation) ---
nsrc << "!"
puts nsrc.match(/here!/, 300)[0]
puts nsrc[300, 6]

# --- $~ side effects after match-at-pos ---
src.match(/(exp)(ect)/, pos)
puts $~[1]
puts $~[2]
puts $1
puts $~.begin(0) > 0
