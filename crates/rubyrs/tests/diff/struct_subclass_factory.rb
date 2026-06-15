# A Struct SUBCLASS used as a factory (`class Options < Struct;
# Options.new(:uri) do … end; end` — faraday's Options): the generated
# class inherits the subclass (its class + instance methods), and
# `cls.new(values)` builds an INSTANCE (not another subclass — the
# "double new" semantics).
class Options < Struct
  def self.tag; "opt-tag"; end
  def shared; "shared-im"; end
end

Sub = Options.new(:uri, :port) do
  def describe; "#{uri}:#{port}"; end
end

# Inherits Options (ancestors + class method + instance method).
p Sub.ancestors.include?(Options)
p Sub.superclass.equal?(Options)
p Sub.tag
p Sub.respond_to?(:tag)

# Instance building works (double-new), members + Struct surface intact.
i = Sub.new("host", 80)
p i.uri
p i.port
p i.describe                 # block-defined method
p i.shared                   # inherited from Options
p i.to_a
p i.to_h
p Sub.members
p (Sub.new("a", 1) == Sub.new("a", 1))
p (Sub.new("a", 1) == Sub.new("b", 1))

# Plain Struct.new is unchanged (parent is not the subclass).
P = Struct.new(:x, :y)
p P.new(1, 2).to_a
p P.new(1, 2) == P.new(1, 2)

# A subclass of the generated struct still builds instances.
class GrandChild < Sub
  def total; port; end
end
gc = GrandChild.new("h", 9)
p gc.total
p gc.uri
