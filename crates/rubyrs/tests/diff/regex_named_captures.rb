# Named captures resolve through $~ / Regexp.last_match (not just
# String#match), and Regexp#match works as the symmetric form.
"2024-05 hello" =~ /(?<year>\d+)-(?<mon>\d+)/
p $~[:year]
p $~["mon"]
p Regexp.last_match(0)
p Regexp.last_match(1)
p Regexp.last_match.pre_match
p Regexp.last_match.post_match
p $~.post_match
# Regexp#match (receiver is the regexp)
m = /(?<a>\d+)-(?<b>\w+)/.match("12-foo")
p [m["a"], m["b"], m[0]]
p(/x/.match("abc"))
p(/(\d+)/.match("a99b")[1])
