# Named format directives — `%<name>s` (full flags/width/precision on
# the named hash value) and the self-contained `%{name}` (value.to_s).
# Driver: rubocop-ast's `format("%<keyword>s: %<keyword>s", keyword: k)`.
p(format("%<k>s", k: "x"))
p(format("%<n>5d", n: 42))
p(format("%<n>-5d|", n: 42))
p(format("%<f>.2f", f: 3.14159))
p(format("%{k}", k: "x"))
p(format("%{k}-%{k}", k: "y"))
p(format("a %<x>d b %<y>s", x: 1, y: "z"))
p(format("%06.2<f>f", f: 3.14159))                 # name AFTER flags/width/prec
p(format("%<keyword>s: %<keyword>s", keyword: "Foo"))  # rubocop-ast's shape
p("%{name}!" % { name: "bob" })                    # String#% with hash
begin
  format("%<missing>s", k: 1)
rescue => e
  p [e.class.to_s, e.message]
end
begin
  format("%{missing}", k: 1)
rescue => e
  p [e.class.to_s, e.message]
end
