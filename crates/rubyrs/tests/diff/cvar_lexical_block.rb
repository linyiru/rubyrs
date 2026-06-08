# Class variables (@@v) inside a block resolve through the LEXICAL class
# where the block was written, NOT `self` — so a block run with a
# different self (instance_eval) still sees the cvars of its defining
# scope. rouge's RegexLexer `state ... do rule @@id end` (run via
# instance_eval in load!) depends on this.
class Holder
  def run(&b); instance_eval(&b); end       # instance_eval changes self
end

class P
  @@v = "lexical"
  def self.blk; proc { @@v }; end
end
p Holder.new.run(&P.blk)                     # "lexical" (not via Holder)

# inherited cvar, block defined in a subclass body, run via instance_eval
class Q
  @@shared = "base-cvar"
end
class R < Q
  def self.blk; proc { @@shared }; end
end
p Holder.new.run(&R.blk)                     # "base-cvar"

# nested blocks share the enclosing lexical cref
class N
  @@n = 42
  def self.blk; proc { [1].map { @@n }.first }; end
end
p Holder.new.run(&N.blk)                     # 42

# normal block call (no instance_eval) — unchanged path
class S
  @@s = "normal"
  def read; [1].each { return @@s }; end
end
p S.new.read                                 # "normal"

# write-through from an instance_eval'd block updates the lexical cvar
class W
  @@w = 0
  def self.setter; proc { @@w = 99 }; end
  def self.w; @@w; end
end
Holder.new.run(&W.setter)
p W.w                                        # 99
