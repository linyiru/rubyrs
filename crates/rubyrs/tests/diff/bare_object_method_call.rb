# Bare-form `instance_variable_get` / `_set` / `_defined?` /
# `instance_variables` dispatch through the universal Object
# arms from inside a method body whose self is a
# `Value::Object`. Pre-fix the bare-call path (`Op::CallNoRecv`)
# kept `recv = None`, so the explicit-recv arms (gated on
# `Some(Value::Object(...))`) never fired, and the dispatcher
# fell through to NoMethodError. Three shims (Forwardable,
# Delegate, Struct) used to work around this by writing
# `self.instance_variable_get(...)` explicitly; closing this
# gap lets them drop the workaround.

# 1. Bare ivar read inside an instance method works.
class C1
  def initialize(v); @v = v; end
  def read_bare
    instance_variable_get(:@v)
  end
end
puts "read=#{C1.new(99).read_bare}"

# 2. Bare ivar write.
class C2
  def set_via_bare(v)
    instance_variable_set(:@x, v)
  end
end
c2 = C2.new
c2.set_via_bare(42)
puts "write=#{c2.instance_variable_get(:@x)}"

# 3. Bare ivar-defined check.
class C3
  def initialize; @set = true; end
  def has_set?; instance_variable_defined?(:@set); end
  def has_unset?; instance_variable_defined?(:@unset); end
end
c3 = C3.new
puts "def_set=#{c3.has_set?}"
puts "def_unset=#{c3.has_unset?}"

# 4. Bare `instance_variables` listing.
class C4
  def initialize; @a = 1; @b = 2; end
  def ivars; instance_variables; end
end
puts "ivars=#{C4.new.ivars.sort.inspect}"

# 5. Inside a block — same routing because `self` is still
# the receiver of the enclosing method.
class C5
  def initialize; @x = "block-self"; end
  def via_block
    [1].map { instance_variable_get(:@x) }
  end
end
puts "block=#{C5.new.via_block.inspect}"
