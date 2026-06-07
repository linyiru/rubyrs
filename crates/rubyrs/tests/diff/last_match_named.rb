# Regexp.last_match resolves the same forms as MatchData#[]: integer
# (incl. negative), and named (Symbol / String). Regression: only
# non-negative integers were handled; Symbol/String and negative
# indices silently returned nil.
"2024-06" =~ /(?<y>\d+)-(?<mo>\d+)/
p Regexp.last_match(:y)
p Regexp.last_match("mo")
p Regexp.last_match(0)
p Regexp.last_match(1)
p Regexp.last_match(-1)
p Regexp.last_match(-2)
p Regexp.last_match(-3)
p Regexp.last_match(5)
begin
  Regexp.last_match(:nope)
rescue IndexError => e
  puts "IndexError: #{e.message}"
end
