# String#gsub!/sub! with a Hash replacement table (uri's
# _encode_uri_component → rack set_cookie_header). Bang form returns
# nil when no match, self (mutated) when a substitution was made.
s = "a b/c".dup
table = { " " => "+", "/" => "%2F" }
r = s.gsub!(/[ \/]/, table)
p s
p r.equal?(s)
# no match -> nil, string unchanged
t = "xyz".dup
p t.gsub!(/[ \/]/, table)
p t
# string-pattern Hash form
u = "one two one".dup
p u.gsub!("one", { "one" => "1" })
p u
# sub! (first only)
v = "aa".dup
p v.sub!(/a/, { "a" => "Z" })
p v
# non-bang still returns a new string
w = "p q"
p w.gsub(/ /, { " " => "_" })
p w
