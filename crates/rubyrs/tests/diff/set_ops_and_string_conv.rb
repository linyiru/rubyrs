# Set methods + Kernel#String gaps that broke RuboCop's Layout/Style cops
# under parser_prism (all general CRuby-parity fixes, not cop-specific):
#  - Set#=== (alias of include?) — NodePattern symbol-union selectors
#  - Enumerable#to_set(klass=Set, *args, &block) — block maps elements
#  - Set#compare_by_identity / compare_by_identity? / dup / freeze
#  - Kernel#String(obj) dispatches a real to_s (MatchData, user overrides)
require "set"

# Set#=== is membership (NOT identity)
s = Set.new([:public, :private, :protected])
puts(s === :private)              # true
puts(s === :nope)                # false
puts([1, 2, 3].to_set === 2)     # true

# to_set with a block maps each element (rubocop-ast: children.to_set(&:x))
puts [1, 2, 3].to_set(&:to_s).to_a.sort.inspect        # ["1", "2", "3"]
puts({ a: 1, b: 2 }.to_set { |k, v| k }.to_a.sort.inspect)  # [:a, :b]
puts((1..3).to_set(&:succ).to_a.sort.inspect)          # [2, 3, 4]
puts [1, 1, 2, 3, 3].to_set.to_a.sort.inspect          # [1, 2, 3]

# compare_by_identity returns self + sets the flag (the method must exist
# and round-trip — rubocop-ast calls it; deeper identity-keying is a
# separate Hash concern not asserted here)
cbi = Set.new.compare_by_identity
puts cbi.is_a?(Set)              # true
puts cbi.compare_by_identity?    # true
puts Set.new.compare_by_identity?  # false

# dup is independent
orig = Set.new([1, 2])
d = orig.dup
d << 3
puts orig.to_a.sort.inspect      # [1, 2]
puts d.to_a.sort.inspect         # [1, 2, 3]

# freeze
fz = Set.new([1]).freeze
puts fz.frozen?                  # true

# Kernel#String dispatches to_s (user override + MatchData)
class Custom
  def to_s = "custom-to-s"
end
puts String(Custom.new)          # custom-to-s
puts String(42)                  # 42
puts String("already")           # already
md = "a b".match(/\G\s/, 1)
puts String(md).inspect          # " "
puts String(:sym)                # sym
