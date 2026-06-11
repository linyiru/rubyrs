# Reopen-precedence early gate (prim_reopen_mask, vm/dispatch.rs):
# a user `def` directly on a primitive's class must win over the
# builtin arm — method-call syntax and send-form both honor it;
# only operator SYNTAX on numerics (`5 + 1`, and its sugar `5.+(1)`)
# stays native (compiles to Op::BinOp, never reaches dispatch —
# documented Tier-1 boundary, deliberately NOT pinned here).
class String
  def upcase; "UP-#{self}"; end
  def length; 99; end
end
s = "ab"
p s.upcase
p s.length
p s.size           # size NOT reopened -> native 2
p s.downcase       # native still serves un-reopened names

class Integer
  def to_s; "INT"; end
  def times; "no-loop"; end
  def chr; "CHR"; end
end
p 5.to_s
puts "interp: #{5}"  # interpolation dispatches to_s -> reopen wins
p 65.chr
acc = []
r = 3.times { |i| acc << i }
p r                # block-form: user times wins over the iter driver
p acc
p 5.send(:to_s)

class Symbol
  def to_s; "SYM"; end
end
p :a.to_s

class Float
  def round(*); "ROUND"; end
end
p 1.7.round

class NilClass
  def to_a; ["nil-arr"]; end
end
p nil.to_a

# send-form operator reopen reaches dispatch (operator syntax does
# not — boundary above).
class Integer
  def +(o); 42; end
end
p 5.send(:+, 1)

# NOTE deliberately not pinned: `include`-provided methods shadowing
# a builtin arm (e.g. a module `<` included into String — CRuby's
# own String#< actually lives in Comparable, so a later include DOES
# win there). rubyrs keeps builtin arms ahead of includes — a
# pre-existing model boundary this gate intentionally preserves
# (own-table reopens only).
