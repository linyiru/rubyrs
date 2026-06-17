# A `class Base < Struct` that defines its own `[]` (and calls `super`)
# must have that override honored — and reachable via super — by the
# member-structs built from it (`Base.new(:members)`). rubyrs generated
# the native struct `[]` onto each member-struct class, BELOW the user
# override, so the override was shadowed and `super` had no target.
# Surfaced by faraday's Options#[] memoization (options.rb).

# Override honored on a member-struct built from a Struct subclass.
class B < Struct
  def [](k); "B#[] #{k}"; end
end
C = B.new(:x)
p C.new(1)[:x]                 # "B#[] x"

# Override that calls super reaches the native accessor.
class Memo < Struct
  def self.store; @s ||= {}; end
  def self.note(k, &blk); store[k] = blk; class_eval("def #{k}; self[:#{k}]; end"); end
  def [](key)
    key = key.to_sym
    if (blk = self.class.store[key])
      super || (self[key] = instance_eval(&blk))
    else
      super
    end
  end
  def self.inherited(sub); super; sub.store.update(store); end
end
VALUE = "computed"
Conn = Memo.new(:cached) do
  note(:cached) { VALUE }
end
o = Conn.new
p o.cached                     # "computed"  (memoized via [] + super)
p o[:cached]                   # "computed"

# Plain Struct.new still works (native [] generated on the class).
Pt = Struct.new(:x, :y)
pt = Pt.new(3, 4)
p pt[:x]                       # 3
p pt[1]                        # 4
pt[:x] = 9
p pt.x                         # 9

# A user [] on a subclass of a PLAIN struct is still honored.
class A < Struct.new(:z)
  def [](k); "A#[] #{k}"; end
end
p A.new(5)[:z]                 # "A#[] z"
