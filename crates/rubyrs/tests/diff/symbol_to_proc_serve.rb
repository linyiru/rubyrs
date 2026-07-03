# `&:sym` sym-proc DIRECT SERVE battery (ADR 0037 tail): 1-arg
# invocations of the symbol-to-proc desugar dispatch `arg.sym()` directly
# (no rest Array, no block frame — matching CRuby's frame-free
# vm_call_symbol). Everything observable must stay CRuby-identical:
# visibility, method_missing, raises, redefinition, multi-arg forwarding.
N = 300

class Point
  attr_reader :x
  def initialize(x) = @x = x
  def dbl = @x * 2
end
pts = (1..10).map { |i| Point.new(i) }

# 1. The hot shapes: map/select/each over &:getter and &:method.
N.times { pts.map(&:x); pts.select(&:dbl) }
p pts.map(&:x)
p pts.map(&:dbl)

# 2. Redefinition AFTER the serve is warm: the shared IC must re-resolve.
class Point
  def dbl = @x * 100
end
p pts.map(&:dbl)

# 3. Multi-arg forwarding still routes through the full body:
#    reduce(&:+) yields 2 args -> acc.+(x).
p (1..6).reduce(&:+)
p [[1, 2], [3, 4]].map(&:first) # lone Array arg stays intact (rest-only)

# 4. Visibility: &:sym is an explicit-receiver call — a private method
#    must raise NoMethodError exactly like CRuby.
class Sealed
  private def hidden = 1
end
begin
  [Sealed.new].map(&:hidden)
rescue NoMethodError => e
  puts e.message[/private method/] ? "private-raise" : "other"
end

# 5. method_missing through &:sym.
class Ghostly
  def method_missing(name, *args) = "mm:#{name}:#{args.length}"
  def respond_to_missing?(*) = true
end
N.times { [Ghostly.new].map(&:phantom) }
p [Ghostly.new].map(&:phantom)

# 6. A raise inside the target method propagates (and is rescuable).
class Boomer
  def boom = raise(ArgumentError, "from-boom")
end
begin
  [Boomer.new].each(&:boom)
rescue ArgumentError => e
  puts e.message
end

# 7. Primitive receivers (the serve's do_call handles every shape).
N.times { %w[a b].map(&:upcase); [1, 2].map(&:to_s) }
p %w[ab cd].map(&:upcase)
p [1, 2, 3].map(&:to_s)
p({ a: 1, b: 2 }.map(&:first))
p [1, nil, 2, nil].filter_map(&:itself)

# 8. &:sym on a method that itself takes a block-less enumerable walk
#    (the served call may push a real frame — driven to completion).
class Nested
  def initialize(vals) = @vals = vals
  def total = @vals.sum { |v| v + 1 }
end
ns = [Nested.new([1, 2]), Nested.new([3])]
N.times { ns.map(&:total) }
p ns.map(&:total)

# 9. to_proc-ish value shapes: proc from &:sym captured then reused.
add_one = :succ.to_proc
p [1, 2, 3].map(&add_one)

# 10. A USER-WRITTEN look-alike block keeps its full body semantics —
#     it must go through Array#[] / Array#drop like any other code.
def look_alike(arr)
  arr.map { |*a| a[0].to_s(*a.drop(1)) }
end
N.times { look_alike([10, 11]) }
p look_alike([10, 11])
