# %a / %A — C99 hexadecimal floating point.
[1.0, 0.5, 255.5, 0.0, -1.5, -0.0, 2.0, 1024.0, 16.0, 0.0625, 0.1, 0.3].each do |x|
  print "%a " % x
end
puts
puts "%A" % 1.0
puts "%a" % (1.0 / 0)
puts "%a" % (-1.0 / 0)
puts "%a" % (0.0 / 0)
puts "%A" % (0.0 / 0)
puts "%.3a" % 1.0
puts "%.2a" % 255.5
puts "%.5a" % 0.1
puts "%a" % 5e-324
puts "%+a" % 1.0
puts "% a" % 1.0
puts "%-15a|" % 1.0
puts format("%a", 3.14159)
