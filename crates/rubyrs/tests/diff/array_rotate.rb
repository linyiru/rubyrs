# Array#rotate / #rotate(n) / #rotate! — left-rotate by n (negative =
# right, default 1), wrapping modulo length; empty/1-elem unchanged.
p [1,2,3,4].rotate
p [1,2,3,4].rotate(2)
p [1,2,3,4].rotate(-1)
p [1,2,3,4].rotate(0)
p [1,2,3,4].rotate(5)         # wraps
p [1,2,3,4].rotate(-6)        # wraps (right)
p [].rotate
p [].rotate(3)
p [1].rotate(99)
p ["a","b","c"].rotate(1)
a = [1,2,3]; r = a.rotate!; p r; p a   # mutates, returns self
b = [1,2,3,4]; b.rotate!(2); p b
c = []; c.rotate!; p c
p [1,2,3].respond_to?(:rotate)
p [1,2,3].respond_to?(:rotate!)
