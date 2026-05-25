# String interpolation microbench. Stresses the
# InterpolatedString path: per iteration creates a new String
# from a mix of an Integer, a String, and a method call. Hits
# `to_s` dispatch on Int + String concat + the Op::InterpStr
# build.

def label(i)
  "row-#{i}"
end

i = 0
acc = ""
total = 0
while i < 200_000
  msg = "[#{i}] #{label(i)}: #{i * 3} done=#{i.odd?}"
  total = total + msg.length
  i = i + 1
end
puts total
