# define_method bodies are closures with per-CALL params/body-locals
# over a SHARED outer binding. Pins the fix for the whole-cell-share
# model, where the body frame WAS the captured cell: params and
# body-introduced locals leaked across calls (a later call saw the
# previous call's optional arg and skipped the default — the NFA
# campaign's `dm(1,2)` then `dm(1)` → stale `2`), while outer-local
# writes only worked by accident of the share.
#
# Documented remaining divergence (NOT covered here): named keyword
# params on a define_method-installed body (`define_method(:m) { |k: 1| }`)
# aren't bound by the closure-method binder — a separate binder gap,
# unrelated to the capture representation.

puts "== E: define_method captures =="

class EK
  x = 0
  define_method(:bump) { x += 1; x }
end
e = EK.new
puts "E1 class-body local shared across calls+instances: #{[e.bump, e.bump, EK.new.bump].inspect}"

class EK2
  x = 0
  1.times { define_method(:bump2) { x += 1; x } }
end
puts "E2 define_method created at block depth1: #{[EK2.new.bump2, EK2.new.bump2].inspect}"

class EK3
  define_method(:dm) { |a, b = (a + 10)| [a, b] }
end
e3 = EK3.new
puts "E3 optional default re-evaluates: #{[e3.dm(1, 2), e3.dm(1), e3.dm(5)].inspect}"

class EK4
  define_method(:dm4) { |v| t ||= v; t }
end
e4 = EK4.new
puts "E4 body-local fresh per call: #{[e4.dm4(1), e4.dm4(2)].inspect}"

class EK5
  define_method(:dm5) { |a, b = (a + 10), *r| [a, b, r] }
end
e5 = EK5.new
puts "E5 optional+rest: #{[e5.dm5(1, 2, 3), e5.dm5(7)].inspect}"

puts "== M: define_method writes reach the defining scope =="

class MK; end
def m1
  x = 0
  1.times do
    MK.send(:define_method, :bump) { x += 1 }
  end
  MK.new.bump
  MK.new.bump
  x
end
puts "M1 dm at block depth1 writes def-local: #{m1}"

class MK2; end
def m2
  # Same, via define_singleton_method on an object.
  x = 0
  o = MK2.new
  1.times { o.define_singleton_method(:poke) { x += 5 } }
  o.poke
  o.poke
  x
end
puts "M2 define_singleton_method at depth1: #{m2}"

# 2-arg form: define_method(:name, proc) keeps the proc's binding.
class MK3; end
def m3
  x = 0
  body = nil
  1.times { body = proc { x += 2 } }
  MK3.send(:define_method, :two_arg, body)
  MK3.new.two_arg
  MK3.new.two_arg
  x
end
puts "M3 two-arg define_method keeps binding: #{m3}"
