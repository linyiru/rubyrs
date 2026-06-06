# `foo(*x)` call-splat coerces x to an Array per CRuby's
# splat contract (Array unchanged, nil -> [], scalar -> [scalar])
# — the same `Array(x)` coercion the array-literal splat `[*x]`
# already used. Pre-fix `foo(*5)` reached Op::ApplyCall with a
# bare Integer and raised
#   TypeError: no implicit conversion of Integer into Array
#
# Discovery: P3 Sinatra spike discovery-map — mustermann/Rack
# code splats non-Array values into calls (e.g. `routes = [*x]`
# then `m(*routes)` shapes).

def pos(*a); a end

# Single-splat forms.
puts "scalar=#{pos(*5).inspect}"
puts "nil=#{pos(*nil).inspect}"
puts "array=#{pos(*[1, 2, 3]).inspect}"
puts "string=#{pos(*"foo").inspect}"
puts "hash=#{pos(*{a: 1}).inspect}"
# NB: `*(1..3)` -> [1,2,3] is NOT asserted here — `Kernel#Array`
# (which this reuses) doesn't expand Ranges yet, a separate
# pre-existing gap also visible in `[*(1..3)]`.

# Mixed splat (leading/trailing positionals around the splat).
puts "mix_scalar=#{pos(1, *5, 9).inspect}"
puts "mix_array=#{pos(1, *[2, 3], 4).inspect}"
puts "mix_nil=#{pos(0, *nil, 7).inspect}"

# Splat into a method with fixed params still binds correctly.
def two(a, b); [a, b] end
puts "fixed=#{two(*[10, 20]).inspect}"

# Splat of an already-Array is unchanged (no double-wrap).
arr = [1, 2]
puts "no_double=#{pos(*arr).inspect}"

# Splat result feeds a real method (not just collection).
def add(x, y); x + y end
puts "add=#{add(*[3, 4])}"
