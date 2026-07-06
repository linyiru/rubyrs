# S8 argument-shape crumbs — every shape probed on CRuby 3.4.8 and
# byte-diffed here: String#byteslice coercion/arity, String#[] and
# String#[]= with a Regexp index (negative/named/nil/Float backrefs,
# subpat splicing), Regexp#match(str, pos) + block/arity/match?
# shapes, Kernel#exit status coercion, Encoding.find's to_str-only
# contract, and the anchor-vs-pos semantics (`^`/`\A`/`\b` anchor
# against the FULL subject, not the search position).

def shape
  r = yield
  "OK #{r.inspect}"
rescue Exception => e
  "#{e.class}: #{e.message}"
end

puts "== String#byteslice =="
puts(shape { "str".byteslice(1) })
puts(shape { "str".byteslice(nil) })
puts(shape { "str".byteslice(nil, 1) })
puts(shape { "str".byteslice(1, nil) })
puts(shape { "str".byteslice(99, nil) })
puts(shape { "str".byteslice("a") })
puts(shape { "str".byteslice(:a) })
puts(shape { "str".byteslice(true) })
puts(shape { "str".byteslice(1.5) })
puts(shape { "strstr".byteslice(1.9, 2) })
puts(shape { "strstr".byteslice(1, 1.5) })
puts(shape { "str".byteslice(Float::NAN) })
puts(shape { "str".byteslice(Float::INFINITY) })
puts(shape { "str".byteslice(1..2) })
puts(shape { "strstr".byteslice(1..2, 1) })
puts(shape { "str".byteslice })
puts(shape { "str".send(:byteslice, 1, 2, 3) })

puts "== String#[] regexp index =="
puts(shape { "hello"[/e(l+)/, 1] })
puts(shape { "hello"[/e(l+)/, 0] })
puts(shape { "hello"[/e(l+)/, 2] })
puts(shape { "hello"[/e(l+)/, -1] })
puts(shape { "hello"[/e(l+)/, -2] })
puts(shape { "hello"[/e(l+)(o)/, -1] })
puts(shape { "hello"[/e(l+)(o)/, -2] })
puts(shape { "hello"[/e(l+)(o)/, -3] })
puts(shape { "hexo"[/x(y)?(o)/, -2] })
puts(shape { "hello"[/e(l+)/, nil] })
puts(shape { "hello"[/e(l+)/, true] })
puts(shape { "hello"[/e(l+)/, 1.7] })
puts(shape { "hello"[/e(l+)/, 2**64] })
puts(shape { "hello"[/e(?<name>l+)/, "name"] })
puts(shape { "hello"[/e(?<name>l+)/, :name] })
puts(shape { "hello"[/e(?<name>l+)/, "nope"] })
# unknown group name on a NON-matching pattern: the match runs first,
# so this is nil, not IndexError
puts(shape { "hello"[/zz(?<g>x)/, "nope"] })
puts(shape { "hello"[/zz(?<g>x)/, "g"] })

puts "== String#[]= regexp forms =="
puts(shape { s = +"hello"; s[/el+/] = "X"; s })
puts(shape { s = +"hello"; s[/el+/, 0] = "X"; s })
puts(shape { s = +"hello"; s[/e(l+)/, 1] = "X"; s })
puts(shape { s = +"hello"; s[/e(l+)/, -1] = "X"; s })
puts(shape { s = +"hello"; s[/e(l+)/, 1.9] = "X"; s })
puts(shape { s = +"hello"; s[/e(?<g>l+)/, "g"] = "X"; s })
puts(shape { s = +"hello"; s[/e(?<g>l+)/, :g] = "X"; s })
puts(shape { s = +"hello"; s[/zz/] = "X"; s })
puts(shape { s = +"hello"; s[/zz/, nil] = "X"; s })
puts(shape { s = +"hello"; s[/zz/] = 5; s })
puts(shape { s = +"hello"; s[/e(l+)/, 2] = "X"; s })
puts(shape { s = +"hello"; s[/e(l+)/, 9] = 5; s })
puts(shape { s = +"hello"; s[/e(l+)/, -3] = "X"; s })
puts(shape { s = +"hexo"; s[/x(y)?(o)/, 1] = "X"; s })
puts(shape { s = +"hello"; s[/e(?<g>l+)/, "nope"] = "X"; s })
puts(shape { s = +"hello"; s[/e(l+)/, nil] = "X"; s })
puts(shape { s = +"hello"; s[/e(l+)/, 1] = 5; s })
puts(shape { "hello".freeze[/el+/] = "X" })
# $~ reflects the pre-mutation subject after a subpat write
s = +"hello"
s[/e(l+)/, 1] = "LL"
p [s, $~[0], $1]

puts "== Regexp#match(str, pos) =="
puts(shape { /l+/.match("hello", 2).to_a })
puts(shape { /l+/.match("hello", 0).to_a })
puts(shape { /l+/.match("hello", -2).to_a })
puts(shape { /l+/.match("hello", -99) })
puts(shape { /l+/.match("hello", 99) })
puts(shape { /l+/.match("hello", nil) })
puts(shape { /l+/.match("hello", "x") })
puts(shape { /l+/.match("hello", 1.9).to_a })
puts(shape { /l+/.match("hello", Float::NAN) })
puts(shape { /l+/.match(:hello, 3).to_a })
puts(shape { /l+/.match(:hello, nil) })
puts(shape { /l/.match })
puts(shape { /l/.match("hello", 1, 2) })
puts(shape { /l/.match(123) })
puts(shape { /l/.match(nil, 2) })
puts(shape { /l/.match(nil, nil) })
m = /(l+)/.match("hello", 3)
p [m[0], m[1], m.begin(0), m.pre_match, m.post_match]
p(/zz/.match("hello", 1))
p $~
# block forms: yields the MatchData on a hit; miss returns nil
# without calling the block
puts(shape { /l+/.match("hello") { |mm| mm[0] } })
puts(shape { /l+/.match("hello", 3) { |mm| mm[0] } })
puts(shape { /zz/.match("hello") { |_| :ran } })

puts "== Regexp#match? extras =="
puts(shape { /l+/.match?("hello", 2) })
puts(shape { /l+/.match?("hello", 99) })
puts(shape { /l+/.match?("hello", 1.5) })
puts(shape { /l+/.match?("hello", nil) })
puts(shape { /l+/.match?("hello", "x") })
puts(shape { /l+/.match?(:hello, 3) })
puts(shape { /l+/.match?(:hello) })
puts(shape { /l/.match?(123) })
puts(shape { /l/.match?(nil, 2) })
puts(shape { /l/.match? })
puts(shape { /l/.match?("hello", 1, 2) })

puts "== anchors vs pos (full-subject context) =="
puts(shape { /^l/.match("hello", 2) })
puts(shape { /\Al/.match("hello", 2) })
puts(shape { /^l/.match?("hello", 2) })
puts(shape { /\bl/.match("hello", 3) })
puts(shape { /\bw/.match("hello world", 6).to_a })
puts(shape { "he\nllo".match(/^l/, 2).to_a })
puts(shape { "hello".match(/^h/, 1) })
puts(shape { "hello".match(/l+/, 1.9).to_a })
puts(shape { "hello".match(/l+/, nil) })

puts "== Kernel#exit shapes =="
puts(shape { exit(nil) })
puts(shape { exit("str") })
puts(shape { exit(Float::NAN) })
puts(shape { exit(Float::INFINITY) })
puts(shape { begin; exit(2.5); rescue SystemExit => e; e.status; end })
puts(shape { begin; exit(-3.9); rescue SystemExit => e; e.status; end })
puts(shape { begin; exit(true); rescue SystemExit => e; e.status; end })
puts(shape { begin; exit(false); rescue SystemExit => e; e.status; end })
puts(shape { begin; exit(7); rescue SystemExit => e; e.status; end })
puts(shape { begin; exit; rescue SystemExit => e; e.status; end })

puts "== Encoding.find =="
puts(shape { Encoding.find(:utf8) })
puts(shape { Encoding.find(:"UTF-8") })
puts(shape { Encoding.find(nil) })
puts(shape { Encoding.find(123) })
puts(shape { Encoding.find(1.5) })
puts(shape { Encoding.find(true) })
puts(shape { Encoding.find(false) })
puts(shape { Encoding.find([]) })
puts(shape { Encoding.find(Object.new) })
puts(shape { Encoding.find(Encoding::UTF_8) })
puts(shape { Encoding.find("bogus") })
class S8ToStr
  def to_str; "UTF-8"; end
end
puts(shape { Encoding.find(S8ToStr.new) })
class S8BadToStr
  def to_str; 42; end
end
puts(shape { Encoding.find(S8BadToStr.new) })
