# Instance variables on a String value (CRuby allows ivars on String;
# rubyrs stores them in a side-table). Surfaced by serbea's
# `String#html_safe` (`dup.tap { _1.instance_variable_set(:@html_safe,
# true) }`) on the Bridgetown render path.
s = String.new("hi")
p s.frozen?
p s.instance_variable_set(:@html_safe, true)
p s.instance_variable_get(:@html_safe)
p s.instance_variable_defined?(:@html_safe)
p s.instance_variable_defined?(:@nope)
p s.instance_variable_get(:@nope)            # unset -> nil
p s.instance_variables.sort
s.instance_variable_set(:@count, 3)
p s.instance_variables.sort
p s.remove_instance_variable(:@count)
p s.instance_variables.sort

# distinct strings have distinct ivars
a = +"a"; b = +"b"
a.instance_variable_set(:@tag, 1)
p b.instance_variable_defined?(:@tag)

# frozen string rejects ivar set
begin
  "frozen".freeze.instance_variable_set(:@x, 1)
rescue FrozenError
  puts "FrozenError"
end

# the html_safe pattern
class String
  def my_safe = self.class.new(self).tap { _1.instance_variable_set(:@safe, true) }
  def my_safe? = instance_variable_get(:@safe) == true
end
puts "plain".my_safe?
puts "tagged".my_safe.my_safe?
