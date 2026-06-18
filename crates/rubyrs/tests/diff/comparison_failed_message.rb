# Comparable's failure message mirrors CRuby's rb_cmperr:
# "comparison of <self class> with <other> failed", where <other> is the
# VALUE for a Numeric/nil/true/false operand and the CLASS name otherwise.
def m(&b); b.call; "NO-RAISE"; rescue ArgumentError => e; e.message; end
p m { 5 < "x" }
p m { 1.5 < "x" }
p m { "a" < 5 }
p m { "a" <= 5 }
p m { "a" > 5 }
p m { "a" >= 5 }
class C; include Comparable; def <=>(o); nil; end; end
p m { C.new < C.new }
p m { C.new < 42 }
p m { C.new < nil }
p m { C.new < 3.5 }
# normal comparisons unaffected
p (1 < 2)
p ("a" < "b")
p [3, 1, 2].sort
p 5.clamp(1, 10)
