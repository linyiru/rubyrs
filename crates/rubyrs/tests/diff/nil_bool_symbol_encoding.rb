# nil/true/false to_s/inspect are US-ASCII; Symbol#to_s/#name are
# US-ASCII iff the name is ASCII; Symbol#inspect is US-ASCII for the
# bareword `:name` form, UTF-8 for the quoted `:"..."` form (even when
# the content is ASCII — CRuby routes it through String#inspect).
p nil.to_s.encoding.name
p nil.inspect.encoding.name
p true.to_s.encoding.name
p false.to_s.encoding.name
p true.inspect.encoding.name
p false.inspect.encoding.name
p :sym.to_s.encoding.name
p :sym.name.encoding.name
p :sym.inspect.encoding.name
p :+.inspect.encoding.name
p :foo?.inspect.encoding.name
p :foo=.inspect.encoding.name
p :"a b".to_s.encoding.name        # ascii content → US-ASCII
p :"a b".inspect.encoding.name     # quoted → UTF-8
p :"1abc".inspect.encoding.name    # quoted → UTF-8
p :"é".to_s.encoding.name          # non-ascii → UTF-8
p :"é".inspect.encoding.name       # non-ascii → UTF-8
# content + interop unchanged
p nil.to_s == ""
p true.to_s == "true"
p :sym.to_s == "sym"
p :sym.inspect == ":sym"
p ("v=" + true.to_s)
p [nil, true, :s].inspect
