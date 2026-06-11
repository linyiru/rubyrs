# The Class/Module-receiver dispatch fast path
# (try_invoke_class_singleton_cached) + the nil?/empty? primitive fast
# arms. Pins the soundness gates: gen invalidation on reopen,
# private class methods, polymorphic call sites (instance vs class
# receiver through the SAME site), singleton inheritance, extend,
# define_singleton_method closures, denied intrinsic names.

module PathLike
  def self.join(a, b)
    "#{a}/#{b}"
  end
end

class Animal
  def self.kingdom
    "Animalia"
  end
  def speak
    "..."
  end
end
class Dog < Animal
  def speak
    "woof"
  end
end

# 1. Plain module-function + class-method dispatch, hot loop (the
#    fast path caches; result must stay correct across 1000 calls).
acc = 0
1000.times { acc += PathLike.join("a", "b").size }
puts acc
puts Animal.kingdom
puts Dog.kingdom # singleton INHERITANCE via the metaclass chain

# 2. Polymorphic call site: same textual site sees an instance recv
#    (caches the instance method under the class pointer) AND later
#    a class recv via the dynamic `subject` — the singleton entry is
#    tagged distinctly, so neither serves the other.
class Probe
  def report
    "instance-report"
  end
  def self.report
    "class-report"
  end
end
subjects = [Probe.new, Probe, Probe.new, Probe]
subjects.each { |s| puts s.report }

# 3. Reopen AFTER the call site is hot: method_gen invalidation must
#    drop the cached singleton.
class Counter
  def self.tick
    1
  end
end
total = 0
1000.times { total += Counter.tick }
class Counter
  def self.tick
    100
  end
end
total += Counter.tick
puts total

# 4. Private class method falls through to the canonical path (same
#    NoMethodError shape as before).
class Sealed
  def self.secret
    "shh"
  end
  private_class_method :secret
end
begin
  Sealed.secret
rescue NoMethodError => e
  puts e.class
end

# 5. define_singleton_method (closure) keeps captured state.
class Gauge; end
level = 41
Gauge.define_singleton_method(:level) { level + 1 }
puts Gauge.level

# 6. extend-provided class methods (module instance methods on the
#    singleton chain).
module Mixable
  def mixed
    "mixed-in"
  end
end
class Host; end
Host.extend(Mixable)
puts Host.mixed

# 7. Denied intrinsics still behave (name/to_s/ancestors/new/include?).
puts Animal.name
puts Dog.ancestors.first(2).inspect
puts Dog.new.speak
puts Dog.superclass
puts Dog.method_defined?(:speak)

# 8. nil? / empty? fast arms — plain values...
s = "abc"
puts s.nil?
puts "".empty?
puts s.empty?
puts 5.nil?
puts nil.nil?
puts :sym.nil?
# ...and after a String reopen of empty? the user method must win
# (fast_prim_str_safe revalidates off method_gen).
class String
  def empty?
    "overridden-#{length.zero?}"
  end
end
puts "".empty?
puts "x".empty?
# nil? reopen on a primitive class flips the reopen mask.
class Integer
  def nil?
    "int-nil-override"
  end
end
puts 7.nil?
puts "still-a-string".nil?
