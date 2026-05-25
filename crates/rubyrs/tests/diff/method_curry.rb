# Method#curry — partial application. Each `.call` gathers args
# until the underlying method's arity is hit, then invokes it
# with the full arg list. `cp.class.name == "Proc"`.

class Math3
  def add3(a, b, c); a + b + c; end
  def mul(a, b); a * b; end
  def quad(a, b, c, d); [a, b, c, d]; end
end

m = Math3.new

# One arg at a time.
cu = m.method(:add3).curry
puts cu.(1).(2).(3)               # 6
puts cu.(10).(20).(30)            # 60

# Bracket-form .[] also routes through `call` semantics.
puts cu[1][2][3]                  # 6

# Mixed: gather some, finish in one call.
puts cu.(1, 2).(3)                # 6
puts cu.(1).(2, 3)                # 6
puts cu.(1, 2, 3)                 # 6 — full args in one shot

# Two-arg method.
cu2 = m.method(:mul).curry
puts cu2.(7).(6)                  # 42
puts cu2.(7, 6)                   # 42

# Four-arg.
cu4 = m.method(:quad).curry
puts cu4.(1).(2).(3).(4).inspect  # [1, 2, 3, 4]

# Type / hierarchy reporting.
puts cu.class.name                # Proc
puts cu.is_a?(Proc)               # true

# Explicit arity hint — restricts to that count.
cu_hint = m.method(:add3).curry(3)
puts cu_hint.(1).(2).(3)          # 6

# Stored partial applications stay independent.
plus10 = cu.(10)
puts plus10.(1).(2)               # 13
puts plus10.(5).(5)               # 20 — original `plus10` not mutated
