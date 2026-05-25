# Method#owner / Method#receiver — reflection on a captured
# method. `owner` reports the class that DEFINED the method
# (walks ancestors for inherited methods); `receiver` returns
# the stored recv for BoundMethod. UnboundMethod#receiver
# raises NoMethodError.

class Base
  def shared; "base"; end
end

class Child < Base
  def own; "child"; end
end

c = Child.new

# Inherited method: owner is the defining class, not Child.
puts c.method(:shared).owner.name           # Base

# Own method: owner is Child.
puts c.method(:own).owner.name              # Child

# Receiver round-trips.
puts c.method(:own).receiver.class.name     # Child
puts c.method(:own).receiver.equal?(c)      # true

# UnboundMethod#owner — same as bound.
puts Child.instance_method(:shared).owner.name   # Base
puts Child.instance_method(:own).owner.name      # Child

# Through unbind round-trip.
puts c.method(:shared).unbind.owner.name    # Base

# UnboundMethod has no receiver.
begin
  Child.instance_method(:own).receiver
rescue NoMethodError => e
  puts "caught: #{e.class.name}"
end

# Primitive receiver: 7.method(:+).receiver is 7.
puts 7.method(:+).receiver                  # 7
