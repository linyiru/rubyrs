# `super` from a user override reaching a native builtin:
# BasicObject#method_missing (raises NoMethodError for the ORIGINAL
# name), String#initialize from a String subclass, and
# Class#subclasses.

class MmProxy
  def method_missing(name, *args)
    return [:handled, args] if name == :known
    super
  end

  def respond_to_missing?(name, include_private = false)
    name == :known || super
  end
end

class MmForward
  def method_missing(name, ...)
    return :fwd if name == :known
    super
  end
end

x = MmProxy.new
p x.known(1)
p x.respond_to?(:known)
p x.respond_to?(:other)
[MmProxy.new, MmForward.new].each do |obj|
  begin
    obj.unknown_thing(1, 2)
    p :no_error
  rescue NoMethodError => e
    p e.class
    p e.message.include?("unknown_thing")
    p e.message.include?("method_missing")
  end
end

class Inquirer < String
  def initialize(env)
    super(env)
    @env = env
  end

  def production? = self == "production"
end

i = Inquirer.new("production")
p i
p i.production?
p i.size
p i.class
p i.upcase
p Inquirer.new("dev").production?

class SubBase
  def self.subclasses = super.sort_by(&:name)
end
class Zed < SubBase; end
class Alpha < SubBase; end
p SubBase.subclasses

# String#initialize keeps its type and arity checks under super.
class StrSub < String
  def initialize(*a)
    super
  end
end
p StrSub.new
[[123], [nil], ["a", "b"]].each do |args|
  begin
    StrSub.new(*args)
    p :no_error
  rescue TypeError, ArgumentError => e
    p [e.class, e.message]
  end
end
