# Tier-2 wave-3 write-back battery (ADR 0037 wave 3).
#
# The wave-3 inline lowering keeps operand-stack values and Locals::Stack
# slots in native registers BETWEEN observation boundaries; local writes are
# WRITE-THROUGH (the canonical slot is updated at the store op itself). This
# battery pins the observability contract: every point where foreign code
# can see the frame (binding snapshots, raises + rescue, backtraces, callee
# re-entry, GC) must observe exactly the values the interpreter would.
#
# Binding note (proven, not assumed): rubyrs `Kernel#binding` SNAPSHOTS the
# frame's named locals at the call boundary (`Vm::binding_locals`); nothing
# ever writes from a Binding back into a frame (`extract_binding_ctx` only
# seeds a fresh eval frame; `Binding#local_variable_set` does not exist).
# So "set_local via binding INTO a compiled frame" CANNOT happen — the
# reload matrix needs no binding edge — and this fixture only exercises
# binding READS taken at boundaries, where rubyrs and CRuby agree.

# 1. Kernel#binding taken INSIDE a compiled body must see written-through
#    locals (the callee re-enters Ruby and reads the caller's locals).
def reader(b)
  eval("a + z", b)
end

def compiled_caller
  a = 5
  z = 7
  a += 30
  reader(binding)
end
puts compiled_caller

# 2. Raise mid-body: locals written by inline ops before the raising call
#    must be observable via a binding hostage + the backtrace must be the
#    interpreter's, byte for byte.
def risky(n)
  acc = n * 2
  acc += 4
  snap = binding
  raise ArgumentError, "acc=#{eval('acc', snap)}" if n > 3
  acc
end

begin
  risky(9)
rescue ArgumentError => e
  puts e.message
  # file:line only — CRuby 3.4 prints "in 'Object#risky'" where rubyrs
  # prints "in 'risky'" (same normalization as tier2_call_family).
  puts e.backtrace.first[%r{[^/]+:\d+}]
end
puts risky(2)

# 3. Ensure in the CALLER reading its own locals after a compiled callee
#    raised (the compiled frame unwound; the caller's state is canonical).
def with_ensure
  marker = :before
  risky(50)
  marker = :after
rescue ArgumentError
  puts "rescued with marker=#{marker}"
ensure
  puts "ensure sees marker=#{marker}"
end
with_ensure

# 4. Proc#binding snapshot taken inside a compiled body.
def proc_binding_probe
  secret = 99
  pr = proc { secret }
  b = pr.binding
  eval("secret", b)
end
puts proc_binding_probe

# 5. Deep recursion (past the native-nesting cap: deep frames interpret,
#    shallow frames run native; every hand-off must be seamless).
def deep(n, acc)
  return acc if n == 0
  x = acc + 1
  deep(n - 1, x)
end
puts deep(3000, 0)

# 6. Method redefinition mid-loop: the IC-fast call path must re-resolve.
def flip
  1
end
r = []
6.times do |i|
  r << flip
  if i == 2
    def flip
      2
    end
  end
end
puts r.inspect

# 7. Int overflow inside a compiled body: the inline add's overflow guard
#    bails to the interpreter's BigInt promotion, exactly once, no replay.
def promote(x)
  y = x + x
  y + 1
end
puts promote(4_611_686_018_427_387_904)

# 8. Non-trivial locals (Str) flow through the slow edges: reads resume to
#    the interpreter, stores drop the old value properly.
def strings(s)
  t = s + "d"
  t = t + "e"
  u = t
  t = 1          # rebind over a Str (old-value drop guard slow path)
  "#{u}/#{t}"
end
puts strings("abc")

# 9. nil? fusion parity — fast-primitive receivers and an Object receiver.
def nilq_shapes(v)
  a = v.nil?
  b = nil.nil?
  c = 5.nil?
  d = Object.new.nil?
  [a, b, c, d]
end
puts nilq_shapes(42).inspect
puts nilq_shapes(nil).inspect

# 10. Ivar round-trips: trivial (Int/Sym) and non-trivial (Str) values, on
#     an Object self, inside compiled bodies.
class IvarBox
  def initialize
    @count = 1
    @tag = :fresh
    @name = "box"
  end

  def poke
    @count += 2
    @tag = :poked
    @name = @name + "!"
    "#{@count}/#{@tag}/#{@name}"
  end
end
b = IvarBox.new
puts b.poke
puts b.poke

# 11. Truthiness shapes through the fused compare-and-branch.
def truthy_walk(v)
  if v then :t else :f end
end
puts [truthy_walk(0), truthy_walk(nil), truthy_walk(false), truthy_walk(true),
      truthy_walk(:sym), truthy_walk("s"), truthy_walk(3.5)].inspect

# 12. case/when literal dispatch (CaseEqLit fast + decline-on-arg shapes).
def classify(v)
  case v
  when :send then "sym-send"
  when 5 then "int-5"
  when nil then "nil"
  when true then "true"
  when 2.5 then "float"
  when "lit" then "str"
  else "other"
  end
end
puts [classify(:send), classify(5), classify(nil), classify(true),
      classify(2.5), classify("lit"), classify(:other), classify([1])].inspect

# 13. Sym/Int equality fast paths + mixed-tag comparisons through do_call.
def eq_shapes(a, b)
  [a == b, a != b]
end
puts (eq_shapes(:x, :x) + eq_shapes(:x, :y) + eq_shapes(1, 1) +
      eq_shapes(1, 2) + eq_shapes(1, 1.0) + eq_shapes(:x, 1) +
      eq_shapes(nil, nil) + eq_shapes(false, false)).inspect
