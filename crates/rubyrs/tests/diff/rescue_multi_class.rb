# Multi-class rescue: `rescue A, B => e` should match either
# class. Prior to this commit rubyrs honoured only the first
# listed class and silently dropped the rest (documented
# divergence in SUBSET.md, P1-10).

class AErr < StandardError; end
class BErr < StandardError; end
class CErr < StandardError; end

def try(klass)
  raise klass, "msg-#{klass.name}"
rescue AErr, BErr => e
  "AB:#{e.class.name}:#{e.message}"
rescue CErr => e
  "C:#{e.class.name}:#{e.message}"
end

puts try(AErr)
puts try(BErr)
puts try(CErr)

# Order within a clause: CRuby matches the first listed class.
# `rescue A, B => e` for a raised A should bind A (not B) —
# verified indirectly via e.class.

# Clause-level priority: A multi-class clause earlier in the
# source still wins over a later catch-all.
def order_test
  raise BErr, "b"
rescue AErr, BErr => e
  "first:#{e.class.name}"
rescue => e
  "catchall:#{e.class.name}"
end
puts order_test

# Mixed: a raised CErr falls through the (A,B) clause and lands
# on the catch-all bare rescue (StandardError covers CErr).
def fallthrough
  raise CErr, "c"
rescue AErr, BErr => e
  "ab:#{e.class.name}"
rescue => e
  "bare:#{e.class.name}"
end
puts fallthrough
