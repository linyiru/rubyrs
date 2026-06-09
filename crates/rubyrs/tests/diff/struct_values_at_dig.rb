# Struct#values_at (Int / negative / Range indices) and #dig.
S = Struct.new(:a, :b, :c)
s = S.new(1, 2, 3)
p s.values_at(0, 2)
p s.values_at(0..1)
p s.values_at(-1)
p s.values_at(2, 0, 1)
p s.values_at
p s.dig(:a)
D = Struct.new(:h)
p D.new({ k: 5 }).dig(:h, :k)
p D.new({ k: { x: 7 } }).dig(:h, :k, :x)
p D.new(nil).dig(:h, :k)
p s.dig(:b)
