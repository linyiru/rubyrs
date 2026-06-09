# Symbol#to_proc — EXPLICIT `:sym.to_proc` conversion (the literal &:sym
# block-pass is covered by symbol_to_proc.rb; this exercises the method
# call that path doesn't go through).
p :upcase.to_proc.call("hi")
p :+.to_proc.call(2, 3)
p :*.to_proc.call(3, 4)
p [1, 2, 3].map(&:to_s.to_proc)
p [1, -2, 3].map(&:abs.to_proc)
p %w[a b c].map(&:upcase.to_proc)
p [1, 2, 3, 4].reduce(&:+.to_proc)
p :upcase.to_proc.class
p :to_s.respond_to?(:to_proc)
add = :+.to_proc
p [10, 20, 30].map { |x| add.call(x, 1) }
double = :*.to_proc
p [1, 2, 3].map { |x| double.call(x, 2) }
