# Block-binder fast arms (ADR 0037 block-frame residue): the two
# single-arg shapes invoke_block1 serves without the general binder —
# (a) rest-only `|*a|` and (b) single-Array auto-splat into `|a, b, ...|`.
# Every case must bind byte-identically to the general path / CRuby.

# (a) rest-only |*a| — no auto-splat: a lone Array stays intact.
r = []
[1, 2, 3].each { |*a| r << a }
p r
[[1, 2], [3]].each { |*a| p a }
[{ x: 1 }].each { |*a| p a }
[nil].each { |*a| p a }

# rest-only via yield
def y1
  yield 42
  yield [4, 2]
end
y1 { |*a| p a }

# rest-only proc / lambda .call
pr = proc { |*a| p a }
pr.call(7)
pr.call([8, 9])
l = ->(*a) { p a }
l.call(10)
l.call([11, 12])
[5].each(&->(*a) { p a })

# rest-only with a body-local (fresh-per-invocation semantics)
[1, 2].each { |*a| t = (t || 0) + a[0]; p t }

# capture write from a rest block
sum = 0
[1, 2, 3].each { |*a| sum += a[0] }
p sum

# (b) auto-splat: single Array into multi-param PROC block
[[1, 2], [3, 4]].each { |a, b| p [b, a] }
[[1, 2, 3, 4]].each { |a, b| p [a, b] }      # extras dropped
[[1]].each { |a, b| p [a, b] }               # short: Nil-fill
["s"].each { |a, b| p [a, b] }               # non-Array: slot0 + nil
[[1, [2, 3]], [4, [5, 6]]].each { |a, (b, c)| p [a, b, c] }  # nested destructure
[[1, 2, 3]].each { |a, b, c| p [a, b, c] }   # 3 params

# auto-splat via yield / proc.call
def y2
  yield [21, 22]
end
y2 { |a, b| p [a, b] }
p2 = proc { |a, b| p [a, b] }
p2.call([31, 32])
p2.call(33)

# lambda multi-param stays strict (no splat; wrong arity raises)
l2 = ->(a, b) { [a, b] }
begin
  l2.call([1, 2])
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end
p l2.call(1, 2)

# capture write from an auto-splat block
acc = 0
[[1, 2], [3, 4]].each { |a, b| acc += a * b }
p acc

# auto-splat block that CREATES an inner closure (copy path)
procs = [[1, 2], [3, 4]].map { |a, b| -> { a + b } }
p procs.map(&:call)

# re-entrant recursion THROUGH a rest block (copy path + reentrancy scan)
def walk(n, &b)
  b.call(n)
  walk(n - 1, &b) if n > 1
end
seen = []
walk(3) { |*a| seen << a }
p seen
