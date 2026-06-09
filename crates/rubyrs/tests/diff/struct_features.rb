# Struct: keyword_init, the block form (methods defined on the struct
# class), to_h / [] / []= / each, and inspect. NB: `p struct` / `puts
# struct` don't yet route to the user inspect/to_s (a separate broad
# gap — Kernel#p/#puts use native conversion), so this calls `.inspect`
# directly, which dispatches correctly.

# positional (baseline)
P = Struct.new(:x, :y)
pt = P.new(3, 4)
p pt.to_a
p pt.x
p pt.inspect

# keyword_init: true
K = Struct.new(:a, :b, keyword_init: true)
p K.new(a: 1, b: 2).to_a
p K.new(a: 1).b            # missing kw → nil
p K.new(a: 5, b: 6).inspect

# block form — methods on the struct class
Pt = Struct.new(:x, :y) do
  def dist2; x * x + y * y; end
end
p Pt.new(3, 4).dist2

# to_h
p P.new(1, 2).to_h
p K.new(a: 9, b: 8).to_h

# [] / []= by index, symbol
s = P.new(10, 20)
p s[0]
p s[:y]
s[:x] = 99
p s.x
s[1] = 88
p s.y

# each
acc = []
P.new(1, 2).each { |v| acc << v * 10 }
p acc

# == (exact class + values)
p P.new(1, 2) == P.new(1, 2)
p P.new(1, 2) == P.new(1, 3)

# members (class + instance)
p P.members
p P.new(1, 2).members
