# Small CRuby method gaps: NilClass#to_a/#to_h, String#getbyte.
p nil.to_a
p nil.to_h
p nil.to_a.equal?(nil.to_a)   # fresh each call → false
p nil.respond_to?(:to_a)
p nil.respond_to?(:to_h)
p "hello".getbyte(0)
p "hello".getbyte(1)
p "hello".getbyte(-1)
p "hello".getbyte(-5)
p "hello".getbyte(99)
p "hello".getbyte(-99)
p "café".getbyte(3)           # byte index, not char
p "".getbyte(0)
p "x".respond_to?(:getbyte)
