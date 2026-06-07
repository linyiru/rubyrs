# Exercises the Vm-level FxHash maps: top-level constants, global vars,
# top-level method dispatch, and class lookup — content correct across
# the hasher change.
TOP_A = 100
TOP_B = 200
$g = 0

def top_add(x)
  x + TOP_A
end

class Box
  VAL = 7
  def get
    VAL + TOP_B
  end
end

sum = 0
500.times do |i|
  $g += 1
  sum += top_add(i)        # top-level method + constant
  sum += Box.new.get       # class lookup + constant
end
p [sum, $g, TOP_A, TOP_B, Box::VAL]
p [defined?(TOP_A), defined?(top_add), defined?(Box)]
$g = 42
p $g
