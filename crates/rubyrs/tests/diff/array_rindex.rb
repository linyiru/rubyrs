# Array#rindex value + block forms (minitest's backtrace filter
# anchors on `bt.rindex { |s| s.match? RE }`).
a = [1, 2, 3, 2, 1]
p a.rindex(2)
p a.rindex(9)
p a.rindex { |x| x < 3 }
p a.rindex { |x| x > 9 }
p [].rindex(1)
p %w[x y z y].rindex("y")
