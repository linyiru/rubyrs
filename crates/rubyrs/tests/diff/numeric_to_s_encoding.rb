# CRuby tags numeric to_s/inspect output US-ASCII (ASCII by
# construction), not UTF-8.
p 37.to_s.encoding.name
p 0.to_s.encoding.name
p (-42).to_s.encoding.name
p 255.to_s(16).encoding.name
p 0.to_s(2).encoding.name
p 37.inspect.encoding.name
p 1.5.to_s.encoding.name
p 100.0.to_s.encoding.name
p (1.0/0).to_s.encoding.name
p (0.0/0).to_s.encoding.name
p 1.5.inspect.encoding.name
# content + interop unaffected
p 42.to_s == "42"
p ("n=" + 7.to_s)
p (7.to_s + "é")          # US-ASCII + UTF-8 concat → UTF-8 content
p (7.to_s + "é").encoding.name
p [1, 2, 3].join("-")
p "%d" % 5
p format("%05.2f", 3.1)
