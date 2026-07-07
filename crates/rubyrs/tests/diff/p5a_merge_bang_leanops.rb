# Campaign P5a, part 2: Hash#merge!/#update through the msx walk
# bucket (the canonical arm served AT the arm), and the tier-2 lean
# StoreIvar / Super serves (exercised by hot loops so compiled
# bodies run the lean helpers; the diff harness re-runs this file
# under tier-2/jit-native/STRESS_GC).

# --- 1. merge!: plain fast shape, overwrite-in-place key order, self identity
h = { a: 1, b: 2 }
r = nil
30.times { |i| r = h.merge!({ b: i, c: i }) }
p h, r.equal?(h)

# --- 2. update alias + zero args (no-op) + multi-arg left-to-right
u = { x: 1 }
30.times { u.update }
p u.update, u.update({ y: 2 }, { y: 3, z: 4 })

# --- 3. to_hash coercion + TypeError shape
class Hashish
  def to_hash = { co: :erced }
end
c = { base: 1 }
20.times { c.merge!(Hashish.new) }
p c[:co], c.size
begin
  c.merge!(:not_a_hash)
rescue TypeError => e
  p e.message
end

# --- 4. frozen receiver raises through the same central guard
fz = { f: 1 }.freeze
begin
  fz.merge!({ g: 2 })
rescue FrozenError => e
  p e.class
end

# --- 5. user hash/eql? keys take the insert-in-place route
class Key
  attr_reader :k
  def initialize(k) = @k = k
  def hash = k.hash
  def eql?(o) = o.is_a?(Key) && o.k == k
end
uk = { Key.new(1) => :old }
10.times { |i| uk.merge!({ Key.new(1) => i }) }
p uk.size, uk.values

# --- 6. Hash-subclass receiver: override precedence via the cascade
class IndH < Hash
  def merge!(*others)
    [:sub_override, super.size]
  end
end
ih = IndH.new
ih[:a] = 1
p ih.merge!({ b: 2 })

# --- 7. reopen-off: an alias-only override flips the WHOLE bucket off
class SpyHash < Hash; end
20.times { ({ s: 1 }).merge!({ t: 2 }) }   # warm the bucket
class Hash
  def update(*others)
    [:reopened_update, others.size]
  end
end
p({ q: 1 }.update({ w: 2 }))
p({ q: 1 }.merge!({ w: 2 }))               # merge! itself: still canonical
# (no remove_method restore: reopening REPLACED Hash#update, so CRuby
# would raise NoMethodError afterwards — the override stays for the
# rest of the file and `update` isn't used again.)

# --- 8. block-form merge! (conflict resolver) stays on the block path
b = { k: 1 }
p(b.merge!({ k: 10, l: 2 }) { |key, old, new| old + new })

# --- 9. tier-2 lean StoreIvar: call-fed stores in a hot compiled body
class Acc
  def initialize
    @total = 0
    @label = ""
  end
  def feed(n)
    @total = compute(n)        # store fed by a CALL result (real-stack value)
    @label = "t#{@total}"      # store fed by interpolation (InterpToS)
    self
  end
  def compute(n) = @total + n
  def snap = [@total, @label]
end
a = Acc.new
2000.times { |i| a.feed(i % 7) }
p a.snap

# --- 10. StoreIvar on a frozen receiver traps with the canonical error
class Frosty
  def poke
    @x = 1
  end
end
fr = Frosty.new
fr.freeze
begin
  fr.poke
rescue FrozenError => e
  p e.class
end

# --- 11. StoreIvar exotic receivers (Class-level, Hash/Str subclass ivars)
class Klv
  def self.stamp
    @cls_ivar = (@cls_ivar || 0) + 1
  end
end
1500.times { Klv.stamp }
p Klv.instance_variable_get(:@cls_ivar)

class Hs < Hash
  def mark
    @tag = (@tag || 0) + 1
  end
  def tag = @tag
end
hs = Hs.new
1500.times { hs.mark }
p hs.tag

class Ss < String
  def mark
    @tag = (@tag || 0) + 1
  end
  def tag = @tag
end
ss = Ss.new("s")
1500.times { ss.mark }
p ss.tag

# --- 12. lean Super: hot compiled site, plain + args + redefinition
class BaseIdx
  def [](k) = "B:#{k}"
  def sum(a, b) = a + b
end
class SubIdx < BaseIdx
  def [](k) = "S(#{super})"
  def sum(a, b) = super(a, b) + 1
end
si = SubIdx.new
out = nil
2000.times { |i| out = si[i % 5]; si.sum(i, 1) }
p out, si.sum(2, 3)
class BaseIdx
  def [](k) = "B2:#{k}"
end
p si[9]

# --- 13. super raising NoSuperclass keeps its shape from a hot site
class Orphan
  def solo = super
end
o = Orphan.new
begin
  o.solo
rescue NoMethodError => e
  p e.class
end
