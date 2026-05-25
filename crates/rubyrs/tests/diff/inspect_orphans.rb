# Inspect on the previously-uncovered built-in types: Float,
# Symbol, Bool, String. (Nil#inspect was already in place; this
# fills the gaps that broke chains like `arr.map(&:inspect)`.)

# Float.
p 1.5.inspect
p 0.0.inspect
p (-3.14).inspect
p (1.0 / 0.0).inspect       # Infinity
p (-1.0 / 0.0).inspect      # -Infinity
p (0.0 / 0.0).inspect       # NaN

# Symbol — `:name` form.
p :foo.inspect
p :bar.inspect
p :hello_world.inspect

# Bool — same as to_s.
p true.inspect
p false.inspect

# Nil — was already supported, here for completeness.
p nil.inspect

# String — wraps in double quotes.
p "hello".inspect
p "".inspect
p "with spaces".inspect

# Int — already had inspect (alias of to_s), here for parity.
p 42.inspect
p (-7).inspect
p 0.inspect

# Inspect inside containers — Array and Hash use child .inspect.
p [1, :foo, "bar", 3.14, true, nil].inspect
p({a: 1, b: "two", c: :three}.inspect)

# respond_to? now true for inspect on every built-in.
puts 1.respond_to?(:inspect)
puts 1.5.respond_to?(:inspect)
puts "x".respond_to?(:inspect)
puts :s.respond_to?(:inspect)
puts true.respond_to?(:inspect)
puts nil.respond_to?(:inspect)

# Use through map { |x| x.inspect } — explicit block form
# (we don't have symbol-to-proc &:method yet).
puts [1, 2.5, "x", :sym, true].map { |x| x.inspect }.inspect

# Inside a class.
class Wrapper
  attr_reader :v
  def initialize(v)
    @v = v
  end
  def to_s
    "<W:#{@v.inspect}>"
  end
end

p Wrapper.new(42).to_s
p Wrapper.new("hi").to_s
p Wrapper.new(:tag).to_s
p Wrapper.new(nil).to_s
p Wrapper.new(true).to_s
p Wrapper.new(3.14).to_s
