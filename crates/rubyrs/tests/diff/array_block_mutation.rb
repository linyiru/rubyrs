# (delete_if / reject! / keep_if / select! / filter!).
out = []; [10, 20, 30].each_index { |i| out << i }; p out
c = [1, 2, 3, 4, 5]; p c.delete_if(&:even?); p c
d = [1, 2, 3]; p d.reject! { |x| x > 5 }; p d   # nothing removed → nil
e = [1, 2, 3, 4]; p e.select!(&:odd?); p e
f = [1, 2, 3]; p f.keep_if { |x| x > 1 }; p f
