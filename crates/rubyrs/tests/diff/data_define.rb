# Ruby 3.2 Data.define — immutable value objects. (Calls `.inspect`
# directly; `p data` doesn't route to a user inspect yet — separate gap.)

D = Data.define(:x, :y)

# positional + keyword construction
p D.new(1, 2).to_h
p D.new(x: 3, y: 4).to_h
p [D.new(1, 2).x, D.new(1, 2).y]

# immutable: reader yes, writer no
p D.new(1, 2).respond_to?(:x)
p D.new(1, 2).respond_to?(:x=)

# == by class + values
p D.new(1, 2) == D.new(1, 2)
p D.new(1, 2) == D.new(1, 3)

# with — copy with changes, original unchanged
orig = D.new(1, 2)
nu = orig.with(y: 99)
p [nu.x, nu.y]
p [orig.x, orig.y]

# members + inspect
p D.members
p D.new(1, 2).inspect
p D.new(x: 5, y: 6).inspect

# block form
E = Data.define(:a) do
  def double; a * 2; end
end
p E.new(5).double
p E.new(a: 9).double

# pattern matching (deconstruct / deconstruct_keys)
case D.new(1, 2)
in { x:, y: } then p [:hash, x, y]
end
case D.new(3, 4)
in [a, b] then p [:arr, a, b]
end

# 1-member disambiguation
V = Data.define(:v)
p V.new(5).v
p V.new(v: 7).v
p V.new({ a: 1 }).v

# to_h round-trips through with
p D.new(1, 2).with.to_h
