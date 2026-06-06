# Regex literal flags `/pattern/imx` — the second half of
# regex-flag support: the flags now APPLY to matching and
# `#options`/`#to_s`/`#inspect` reflect them. Pre-fix `/foo/i`
# compiled flagless (`"FOO" =~ /foo/i` was nil).
#
# THE TRAP: Ruby `/m` is dot-matches-newline (engine `(?s)`), NOT
# multi-line `^`/`$`. Discovery: P3 Sinatra spike.

# i — case-insensitive matching.
puts "i_match=#{("FOO" =~ /foo/i).inspect}"
puts "i_nomatch=#{("FOO" =~ /foo/).inspect}"

# m — dot matches newline (the dotall trap; must NOT be (?m)).
puts "m_dotall=#{("a\nb" =~ /a.b/m).inspect}"
puts "no_m=#{("a\nb" =~ /a.b/).inspect}"

# x — extended (whitespace in the pattern ignored).
puts "x_match=#{("ab" =~ / a b /x).inspect}"

# Combined flags.
puts "im_match=#{("A\nB" =~ /a.b/im).inspect}"

# #options returns the bitmask.
puts "opt_none=#{/foo/.options}"
puts "opt_i=#{/foo/i.options}"
puts "opt_m=#{/foo/m.options}"
puts "opt_x=#{/foo/x.options}"
puts "opt_im=#{/foo/im.options}"
puts "opt_imx=#{/foo/imx.options}"

# #to_s and #inspect render the flag set (CRuby m,i,x ordering).
puts "to_s_none=#{/x/.to_s}"
puts "to_s_i=#{/x/i.to_s}"
puts "to_s_im=#{/x/im.to_s}"
puts "to_s_imx=#{/x/imx.to_s}"
puts "insp_none=#{/x/.inspect}"
puts "insp_i=#{/x/i.inspect}"
puts "insp_im=#{/x/im.inspect}"

# #source stays BARE (no (?is) prefix leak).
puts "source=#{/hel.lo/i.source}"

# CACHE-COLLISION pair: same source, different flags must NOT
# collide (the regex_cache key folds in flags).
puts "cache_plain=#{("foo" =~ /foo/).inspect}"
puts "cache_i=#{("FOO" =~ /foo/i).inspect}"
puts "cache_plain_again=#{("FOO" =~ /foo/).inspect}"

# Regexp.new(str, options) — Integer bitmask, truthy=>IGNORECASE,
# nil=>0.
puts "new_i=#{(Regexp.new("foo", Regexp::IGNORECASE) =~ "FOO").inspect}"
puts "new_i_opts=#{Regexp.new("foo", Regexp::IGNORECASE).options}"
puts "new_truthy=#{(Regexp.new("foo", true) =~ "FOO").inspect}"
puts "new_nil=#{Regexp.new("foo", nil).options}"

# Interpolated regex carries flags too.
word = "foo"
puts "interp_i=#{("FOO" =~ /#{word}/i).inspect}"
