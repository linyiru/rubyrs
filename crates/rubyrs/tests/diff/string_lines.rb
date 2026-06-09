# String#lines / #each_line — split keeping the separator ("\n"
# default); trailing sep yields no empty tail; empty string → [].
p "a\nb\nc".lines
p "a\nb\nc\n".lines
p "".lines
p "no newline".lines
p "\n\n".lines
p "a-b-c".lines("-")
p "a-b-c-".lines("-")
p "one\ntwo\nthree".lines.map(&:chomp)
p "x\ny\nz".each_line.to_a
p "x\ny\nz".each_line.map(&:chomp)
r = []; "a\nb\nc".each_line { |l| r << l }; p r
ret = "p\nq".each_line { |l| }; p ret      # returns self
