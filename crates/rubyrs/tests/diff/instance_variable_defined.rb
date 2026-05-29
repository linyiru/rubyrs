# Object#instance_variable_defined? — true iff the ivar has
# been set (even to nil). Closes the last gap in the
# instance_variable_* trio (get/set already existed).

class C
  def initialize
    @x = 1
    @y = nil      # explicit nil set → defined? is true
  end
end

c = C.new
puts c.instance_variable_defined?(:@x)         # true
puts c.instance_variable_defined?(:@y)         # true — set-to-nil counts
puts c.instance_variable_defined?(:@z)         # false — never set
puts c.instance_variable_defined?("@x")        # true — String accepted

# After set, defined? flips to true
c.instance_variable_set(:@late, 42)
puts c.instance_variable_defined?(:@late)      # true

# Receivers without ivar tables (immediates, Str/Array/Hash
# without ivar slots in rubyrs's heap model) → always false
puts 5.instance_variable_defined?(:@x)
puts "hi".instance_variable_defined?(:@x)
puts nil.instance_variable_defined?(:@x)
puts true.instance_variable_defined?(:@x)
puts :sym.instance_variable_defined?(:@x)
puts [].instance_variable_defined?(:@x)
puts({}.instance_variable_defined?(:@x))

# Class-level ivars work the same as Instance ivars
class K; end
K.instance_variable_set(:@foo, 1)
puts K.instance_variable_defined?(:@foo)       # true
puts K.instance_variable_defined?(:@bar)       # false

# Invalid name → NameError (same validator as get/set)
begin
  c.instance_variable_defined?(:foo)
rescue NameError
  puts "name-error-no-at"
end

# Digit-start ivar names (e.g. `@1bad`) are rejected at the
# parser level for symbol literals, so we exercise the runtime
# validator via a String arg instead.
begin
  c.instance_variable_defined?("@1bad")
rescue NameError
  puts "name-error-digit-start"
end

# Arity guard — CRuby ArgumentError for 0 or 2+ args
begin
  c.instance_variable_defined?
rescue ArgumentError
  puts "arity-0"
end

begin
  c.instance_variable_defined?(:@x, :@y)
rescue ArgumentError
  puts "arity-2"
end

# respond_to? must agree with dispatch
puts 42.respond_to?(:instance_variable_defined?)
puts Object.new.respond_to?(:instance_variable_defined?)
