# Rest-predicate body-shape fast path (the rubocop-ast `Node#type?`
# family): a pure-rest method whose body is `rest.include?(<bare
# zero-arg call>)` (optionally with the frozen-const-Hash group
# fallback) is served frame-free by the VM. This battery pins the
# semantics to CRuby across: warm-up, both body variants, group
# hits/misses, splat forwarding, non-Symbol args (deopt), scan
# short-circuit order (user `==` side effects), method redefinition
# AFTER warm-up, subclass/singleton overrides of both the predicate
# and the bare getter, live (unfrozen) group-hash mutation, const
# reassignment, defaulted hashes, non-Symbol ivars/group values, and
# rest-arg mutation visibility on the general path.

class Node
  GROUP_FOR_TYPE = {
    send: :call, csend: :call,
    true: :boolean, false: :boolean,
    int: :numeric, float: :numeric,
  }.freeze

  attr_reader :type

  def initialize(t)
    @type = t
  end

  def type?(*types)
    return true if types.include?(type)

    group_type = GROUP_FOR_TYPE[type]
    !group_type.nil? && types.include?(group_type)
  end

  def simple?(*types)
    types.include?(type)
  end
end

n = Node.new(:send)
f = Node.new(:false)

# warm the fast path
100.times { n.type?(:send); n.type?(:def); n.simple?(:send) }

puts "-- basic"
p n.type?(:send)             # direct hit
p n.type?(:csend, :send)     # hit on second
p n.type?(:def)              # miss + group miss
p n.type?(:call)             # group hit
p f.type?(:boolean)          # group hit via :false
p n.type?                    # no args
p n.simple?(:send)
p n.simple?(:x)

puts "-- splat forward"
pair = [:csend, :send]
p n.type?(*pair)
p n.type?(*[])
fifteen = %i[a b c d e f g h i j k l m n send]
p n.type?(*fifteen)

puts "-- non-Symbol args (deopt must stay exact)"
p n.type?("send")            # String never == Symbol
p n.type?(:x, "send", :send) # hit AFTER a string
p n.simple?(1, :send)

# NOTE: an Object arg whose user `==` should be consulted by
# `Array#include?` is a PRE-EXISTING rubyrs gap (the builtin include?
# doesn't dispatch user `==` for Object-vs-Symbol) — verified
# identical on the pre-fast-path baseline binary, so it is NOT pinned
# here. The fast path deopts on any non-Symbol arg, so it can never
# make that case worse.

puts "-- scan short-circuit order (side-effect probe)"
class Probe
  @@calls = 0
  def self.calls = @@calls
  def ==(_o)
    @@calls += 1
    false
  end
end
pr = Probe.new
p n.type?(:send, pr)         # hit BEFORE the probe: == must NOT run
p Probe.calls

puts "-- non-Symbol ivar"
p Node.new("send").type?(:send)
p Node.new(nil).type?(:send)
p Node.new(42).simple?(:send)

puts "-- unfrozen group hash: live mutation is visible"
class Live
  G = { a: :grp } # NOT frozen
  attr_reader :t
  def initialize(t)
    @t = t
  end

  def is3?(*x)
    return true if x.include?(t)

    g = G[t]
    !g.nil? && x.include?(g)
  end
end
lv = Live.new(:a)
100.times { lv.is3?(:zzz) }
p lv.is3?(:grp)
Live::G[:a] = :other
p lv.is3?(:grp)
p lv.is3?(:other)
Live::G[:a] = nil
p lv.is3?(:other)

puts "-- const reassignment invalidates"
Live.send(:remove_const, :G)
Live.const_set(:G, { a: :fresh })
p lv.is3?(:other)
p lv.is3?(:fresh)

puts "-- defaulted group hash (default proc consulted on miss)"
class Defaulted
  H = Hash.new { |_h, _k| :defaulted }
  H[:known] = :grp
  attr_reader :t
  def initialize(t)
    @t = t
  end

  def is4?(*x)
    return true if x.include?(t)

    g = H[t]
    !g.nil? && x.include?(g)
  end
end
dd = Defaulted.new(:unknown)
100.times { dd.is4?(:zzz) }
p dd.is4?(:defaulted)
p Defaulted.new(:known).is4?(:grp)

puts "-- non-Symbol group value"
class IntGroup
  H = { a: 42 }.freeze
  attr_reader :t
  def initialize(t)
    @t = t
  end

  def is5?(*x)
    return true if x.include?(t)

    g = H[t]
    !g.nil? && x.include?(g)
  end
end
ig = IntGroup.new(:a)
100.times { ig.is5?(:zzz) }
p ig.is5?(:b)
p ig.is5?(42)

puts "-- subclass overrides the getter (computed, not attr_reader)"
class SubNode < Node
  def type = :false
end
s = SubNode.new(:send) # ivar says :send; method says :false
100.times { s.type?(:x) }
p s.type?(:boolean)
p s.type?(:send)
p s.simple?(:false)

puts "-- singleton overrides the getter"
sg = Node.new(:send)
def sg.type = :true
p sg.type?(:boolean)
p sg.type?(:send)

puts "-- singleton overrides the predicate itself"
sq = Node.new(:send)
def sq.type?(*t)
  [:custom, t]
end
p sq.type?(:send)
p Node.new(:send).type?(:send) # other instances unaffected

puts "-- redefine the predicate AFTER warm (must invalidate)"
class Node
  def type?(*types)
    "overridden-#{types.inspect}"
  end
end
p n.type?(:send)

puts "-- redefine the getter AFTER warm (must invalidate)"
class Live
  def t = :other
end
p lv.is3?(:fresh) # ivar :a would say :fresh; computed getter says :other
p lv.is3?(:other)

puts "-- Array#include? override wins inside the body"
class Live2
  G = { a: :grp }.freeze
  attr_reader :t
  def initialize(t)
    @t = t
  end

  def is6?(*x)
    return true if x.include?(t)

    g = G[t]
    !g.nil? && x.include?(g)
  end
end
l2 = Live2.new(:a)
100.times { l2.is6?(:zzz) }
p l2.is6?(:nope)
class Array
  alias_method :__rp_orig_include?, :include?
  def include?(_x) = true
end
p l2.is6?(:nope)
class Array
  def include?(x) = __rp_orig_include?(x)
end
p l2.is6?(:nope)
p l2.is6?(:grp)

puts "-- Hash#[] override wins inside the body"
class Hash
  alias_method :__rp_orig_get, :[]
  def [](_k) = :grp
end
p l2.is6?(:forced_group_hit) # G[t] forced to :grp... still not in args
p l2.is6?(:grp)
class Hash
  def [](k) = __rp_orig_get(k)
end
p l2.is6?(:nope)

puts "-- rest-arg mutation visibility (general path, splat identity)"
def take_rest(*r)
  r << :extra
  r.length
end

def give_back(*r) = r

a = [1, 2]
p take_rest(*a)
p a
p give_back(*a).equal?(a)
b = give_back(*a)
b << 3
p a
