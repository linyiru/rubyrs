# Onigmo's POSIX bracket classes are UNICODE-aware on UTF-8 strings
# — the mirror image of the \s \d \w situation (there Ruby is the
# ASCII side): CRuby's [[:alpha:]] matches é/日/Ⅷ while the Rust
# engines' [[:alpha:]] is ASCII-only. rubyrs translates each name to
# the equivalent Unicode property set at pattern-prepare time
# (regex_engine.rs, POSIX arm of rewrite_ascii_shorthand_classes).
#
# Probe chars chosen to pin the boundary decisions:
#   Ⅷ  (Nl, has Uppercase)   → alpha+upper, not just letters
#   ʰ  (Lm, has Lowercase)   → lower includes modifier letters
#   ́   (Mn combining)        → word yes (M), alpha NO
#   ｆ  (fullwidth f)         → lower yes, xdigit NO (xdigit is ASCII)
#   ©  (So)                  → graph yes, punct NO (punct = P+Sm/Sc/Sk)
#   ­   (Cf soft hyphen)      → graph yes (Onigmo keeps Cf)
#   NBSP (Zs)                → space/blank/print yes
#   ٣  (arabic-indic Nd)     → digit yes

CHARS = ["a", "é", "日", "٣", "5", "_", " ", " ", "Ⅷ", "ʰ", "́", "！",
         "Ａ", "ｆ", "+", "$", "^", "©", "­", "\t"]
CLASSES = %w[alpha alnum digit xdigit upper lower space blank word
             punct graph print cntrl ascii]

CLASSES.each do |cl|
  pos = Regexp.new("\\A[[:#{cl}:]]\\z")
  neg = Regexp.new("\\A[[:^#{cl}:]]\\z")
  p_hits = CHARS.each_index.select { |i| CHARS[i] =~ pos }
  n_hits = CHARS.each_index.select { |i| CHARS[i] =~ neg }
  puts "#{cl}: pos=#{p_hits.join(",")} neg=#{n_hits.join(",")}"
end

puts "== composition =="
puts "mixed [[:digit:]z]:    #{!!("z" =~ /\A[[:digit:]z]\z/)} #{!!("٣" =~ /\A[[:digit:]z]\z/)} #{!!("a" =~ /\A[[:digit:]z]\z/)}"
puts "outer-neg [^[:digit:]]: #{!!("a" =~ /\A[^[:digit:]]\z/)} #{!!("٣" =~ /\A[^[:digit:]]\z/)}"
puts "two posix [[:alpha:][:digit:]]: #{!!("é" =~ /\A[[:alpha:][:digit:]]\z/)} #{!!("٣" =~ /\A[[:alpha:][:digit:]]\z/)} #{!!("+" =~ /\A[[:alpha:][:digit:]]\z/)}"
puts "scan: #{"a1é٣ x".scan(/[[:alnum:]]/).inspect}"
puts "split: #{"aé bʰ".split(/[[:space:]]/).inspect}"
