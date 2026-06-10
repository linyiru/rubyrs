# Array subclasses (`class StringRegister < Array` — rouge's python
# lexer) allocate a REAL tagged Array (heap ArrayObj.class_tag), the
# Array twin of the Hash-subclass support: every Array primitive
# dispatches on instances, user overrides win over primitives,
# ivars/dup/class/is_a? all see the subclass. CRuby semantics pinned
# here: derived results (map/+/select) are plain Array (Ruby 3.x),
# == compares content across the subclass boundary, Subclass[...]
# and Subclass.new(n, fill) construct tagged instances.
class SR < Array
  def shove(x); push(x); self; end
  def first_or(d); empty? ? d : first; end
end
a = SR.new
p a.class
p a.class == SR
p a.is_a?(Array)
p a.is_a?(SR)
a.shove(1).shove(2)
p a
p a.first_or(:none)
p a.size
p a + [3]
p (a + [3]).class
p a.map { |x| x * 10 }
p a.map { |x| x * 10 }.class
b = a.dup
p b.class
p a == [1, 2]
p [1, 2] == a
# ivars
class Tot < Array
  def initialize; super; @total = 0; end
  def add(n); push(n); @total += n; self; end
  attr_reader :total
end
t = Tot.new.add(3).add(4)
p t.total
p t
# Array.new arity through subclass
s2 = SR.new(3, :x)
p s2
p s2.class
# class-method []
s3 = SR[7, 8]
p s3
p s3.class
# override wins
class Cap < Array
  def push(x); super(x > 9 ? 9 : x); end
end
c = Cap.new
c.push(5); c.push(99)
p c
# inspect / to_s
p Tot.new.add(1).inspect
puts t.to_s
