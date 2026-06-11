# Task-6 trio: metaclass expression, alias-of-builtin, undef_method
# (+ the deferred Thread and Queue surfaces that are CRuby-identical
# in fork-join shape).
class K
  def self.cattr(name)
    (class << self; self; end).attr_accessor name
  end
  cattr :foo
end
K.foo = 42
p K.foo
class L
  MC = (class << self; self; end)
end
p L::MC.inspect

class AL
  alias __rt? respond_to?
end
p AL.new.__rt?(:to_s)
p AL.new.__rt?(:zzz_nope)

class Proxy
  def real; "real"; end
  undef_method :inspect
  undef :==
  def method_missing(n, *args)
    "proxied-#{n}-#{args.length}"
  end
end
px = Proxy.new
p px.real
p px.inspect
p(px == 3)
class Base2; def greet; "hi"; end; end
class Sub2 < Base2
  undef_method :greet
  def method_missing(n, *a); "mm-#{n}"; end
end
p Sub2.new.greet
p Base2.new.greet
begin
  class Bogus; undef_method :nonexistent_zzz; end
rescue NameError
  puts "NameError: ok"
end

# Deferred Thread: fork-join worker-pool drain (the minitest
# executor shape) — results identical to CRuby's concurrent run.
q = Thread::Queue.new
results = []
workers = 2.times.map do
  Thread.new(q) do |queue|
    while job = queue.pop
      results << job * 10
    end
  end
end
[1, 2, 3].each { |j| q << j }
2.times { q << nil }
workers.each(&:join)
p results.sort
t = Thread.new { 7 * 6 }
p t.value
p t.alive?
