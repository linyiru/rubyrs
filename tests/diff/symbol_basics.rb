puts :foo
puts :foo == :foo
puts :foo == :bar
puts :foo.to_s
puts :foo.to_s.length

h = {a: 1, b: 2, c: 3}
puts h.length
puts h[:a]
puts h[:b]
puts h[:c]
h[:d] = 4
puts h[:d]
puts h.length
