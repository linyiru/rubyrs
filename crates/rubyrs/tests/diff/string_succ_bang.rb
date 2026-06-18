# String#succ! / next! — in-place successor, returns self (Tilt
# generates unique compiled-method names by succ!-ing a counter).
s = "az".dup
r = s.succ!
p s
p r.equal?(s)
t = "Zz".dup; t.next!; p t
u = "a9".dup; u.succ!; p u
v = "199".dup; v.succ!; p v
w = "".dup; p w.succ!
