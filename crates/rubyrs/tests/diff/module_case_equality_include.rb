# `Module#===` ≡ `obj.is_a?(Module)` — must honour INCLUDED modules, not
# just the superclass chain. Surfaced by net/http's `if URI === uri`
# guard (URI::Generic does `include URI`).
module M; end
class C; include M; end
class D < C; end

p M === C.new        # true (direct include)
p M === D.new        # true (inherited include)
p M === Object.new   # false
p Comparable === 5   # true (Integer includes Comparable)
p Comparable === Object.new  # false

# Class receiver still works (plain inheritance).
p C === C.new        # true
p C === D.new        # true
p D === C.new        # false

# case/when on a module dispatches via ===.
def kind(x)
  case x
  when M then :is_m
  when Comparable then :cmp
  else :other
  end
end
p kind(C.new)
p kind(42)
p kind(Object.new)
