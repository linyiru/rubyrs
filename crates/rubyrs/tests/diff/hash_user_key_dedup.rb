# Duplicate-user-key dedup battery: a key whose class overrides
# `hash`/`eql?` must UPDATE the existing eql?-equal entry (original key
# object kept, position kept, value replaced — CRuby rb_hash_aset) at
# EVERY inserting entry point, not append a second pair. Found by the
# small-hash adversarial verifier (2026-07): `[]=`/`store` were correct
# but the merge family, Hash[], to_h, invert, transform_keys, group_by,
# tally, Marshal load and the lookup-shaped entry points (values_at /
# fetch_values / slice / except / dig / fetch-with-block / ==) all
# compared user keys by identity. Three-way vs CRuby, byte-exact.

class K
  attr_reader :v, :tag
  def initialize(v, tag = nil) = (@v = v; @tag = tag)
  def hash = @v.hash
  def eql?(o) = o.is_a?(K) && o.v == @v
  def ==(o) = eql?(o)
  def inspect = "K(#{@v})"
  def to_s = inspect
end

# -- 1. []= / store across the inline(<=3) / spilled(4+) / indexed(16+)
#       representations, with insert-order variation ------------------
a = K.new(1)
h = {}
h[a] = :first
h[K.new(1)] = :second
puts "aset inline:   size=#{h.size} #{h.inspect} kept=#{h.keys[0].equal?(a)}"

h = { x: 1 }
h.store(K.new(2), :one)
h.store(:y, 2)
h.store(K.new(2), :two)
puts "store inline:  size=#{h.size} #{h.inspect}"

h = {}
6.times { |i| h[i] = i }        # spill past the inline cap first
h[K.new(3)] = :one
h[K.new(3)] = :two
puts "spilled:       size=#{h.size} val=#{h[K.new(3)].inspect}"

h = {}
20.times { |i| h[i] = i }       # identity index built (>=16 entries)
h[K.new(4)] = :one
h[K.new(4)] = :two
puts "indexed:       size=#{h.size} val=#{h[K.new(4)].inspect}"

# user key first, plain keys after, dup-insert crossing the spill boundary
h = {}
h[K.new(5)] = 0
h[:p1] = 1
h[:p2] = 2
h[K.new(5)] = 3
h[:p3] = 4
h[K.new(5)] = 5
puts "boundary:      size=#{h.size} #{h.inspect}"

# -- 2. position + original-key rules -------------------------------
k1 = K.new(6)
h = { :a => 1, k1 => :one, :b => 2 }
h[K.new(6)] = :two
puts "position:      #{h.keys.inspect} kept=#{h.keys[1].equal?(k1)} val=#{h[k1].inspect}"

# delete then reinsert: the reinserted key appends at the END
h = { K.new(7) => 1, :mid => 2 }
h.delete(K.new(7))
h[K.new(7)] = 3
puts "del-reinsert:  #{h.inspect}"

# -- 3. merge family --------------------------------------------------
h = { K.new(8) => :a }
h.merge!({ K.new(8) => :b })
puts "merge!:        size=#{h.size} #{h.inspect}"

old = K.new(9, :old)
h = { old => 1 }
h.update({ K.new(9, :new) => 2 }) { |k, o, n| "#{k.tag}:#{o}:#{n}" }
puts "update-blk:    size=#{h.size} val=#{h[old].inspect} keytag=#{h.keys[0].tag}"

h = { K.new(10) => :a }.merge({ K.new(10) => :b })
puts "merge:         size=#{h.size} #{h.inspect}"

h = { K.new(11) => 1 }.merge({ K.new(11) => 2 }) { |k, o, n| o + n }
puts "merge-blk:     size=#{h.size} #{h.inspect}"

# multi-arg merge, later args win over earlier ones
h = { K.new(12) => :a }.merge({ K.new(12) => :b }, { K.new(12) => :c })
puts "merge-multi:   size=#{h.size} #{h.inspect}"

# double-splat literal (lowers through the merge chain)
s1 = { K.new(13) => 1 }
s2 = { K.new(13) => 2 }
h = { **s1, **s2 }
puts "splat:         size=#{h.size} #{h.inspect}"
h = { K.new(13) => 0, **s2 }
puts "splat2:        size=#{h.size} #{h.inspect}"

# -- 4. constructors --------------------------------------------------
h = Hash[[[K.new(14), :a], [K.new(14), :b]]]
puts "Hash[pairs]:   size=#{h.size} #{h.inspect}"
h = Hash[K.new(14), :a, K.new(14), :b]
puts "Hash[k,v]:     size=#{h.size} #{h.inspect}"
h = Hash[:a, 1, :a, 2]
puts "Hash[plain]:   size=#{h.size} #{h.inspect}"
h = [[K.new(15), :a], [K.new(15), :b]].to_h
puts "to_h:          size=#{h.size} #{h.inspect}"
h = [1, 2].to_h { |i| [K.new(16), i] }
puts "to_h-blk:      size=#{h.size} #{h.inspect}"
h = { K.new(17) => :one, K.new(17) => :two }
puts "literal:       size=#{h.size} #{h.inspect}"

# -- 5. derived builders ----------------------------------------------
h = { :a => K.new(18), :b => K.new(18) }.invert
puts "invert:        size=#{h.size} #{h.inspect}"
h = { :a => 1, :b => 2 }.transform_keys { |k| K.new(19) }
puts "tk-blk:        size=#{h.size} #{h.inspect}"
h = { K.new(20) => 1 }.transform_keys({ K.new(20) => :mapped })
puts "tk-map:        #{h.inspect}"
h = { :a => 1, :b => 2 }
h.transform_keys! { |k| K.new(21) }
puts "tk-bang:       size=#{h.size} #{h.inspect}"
g = [1, 2, 3].group_by { |i| K.new(22) }
puts "group_by:      size=#{g.size} #{g.inspect}"
g = { :a => 1, :b => 2 }.group_by { |k, v| K.new(23) }
puts "group_by-h:    size=#{g.size} #{g.inspect}"
t = [K.new(24), K.new(24), K.new(25)].tally
puts "tally:         #{t.inspect}"

# -- 6. lookup-shaped entry points ------------------------------------
h = { K.new(26) => :x, :plain => 1 }
puts "values_at:     #{h.values_at(K.new(26), :plain, :miss).inspect}"
puts "fetch_values:  #{h.fetch_values(K.new(26)).inspect}"
begin
  h.fetch_values(K.new(99))
rescue KeyError => e
  puts "fv-miss:       #{e.message}"
end
puts "fv-blk:        #{h.fetch_values(K.new(26), K.new(99)) { |k| "d:#{k}" }.inspect}"
puts "fetch-blk:     #{h.fetch(K.new(26)) { :miss }.inspect} #{h.fetch(K.new(99)) { :miss }.inspect}"
puts "dig:           #{h.dig(K.new(26)).inspect}"
puts "slice:         #{h.slice(K.new(26), K.new(26)).inspect}"
puts "except:        #{h.except(K.new(26)).inspect}"
begin
  h.fetch(K.new(99))
rescue KeyError => e
  puts "fetch-miss:    #{e.message}"
end

# -- 7. equality -------------------------------------------------------
p({ K.new(27) => 1 } == { K.new(27) => 1 })
p({ K.new(27) => 1 } == { K.new(28) => 1 })
p({ K.new(27) => 1 } != { K.new(27) => 1 })
p({ K.new(27) => 1 }.eql?({ K.new(27) => 1 }))
p({ K.new(27) => 1 }.eql?({ K.new(27) => 1.0 }))

# -- 8. Marshal round-trip ---------------------------------------------
m = Marshal.load(Marshal.dump({ K.new(29) => :a, :x => 1 }))
puts "marshal:       size=#{m.size} hit=#{m[K.new(29)].inspect}"

# -- 9. adversarial key shapes -----------------------------------------
# hash collides, eql? false → two entries
class ColK
  attr_reader :v
  def initialize(v) = @v = v
  def hash = 42
  def eql?(o) = o.is_a?(ColK) && o.v == @v
  def inspect = "ColK(#{@v})"
end
h = {}
h[ColK.new(1)] = :a
h[ColK.new(2)] = :b
h[ColK.new(1)] = :c
puts "collide:       size=#{h.size} #{h.inspect}"

# eql? true but hash differs (inconsistent key) → duplicates legitimately
class BadK
  @@ctr = 0
  def hash = (@h ||= (@@ctr += 1))
  def eql?(o) = o.is_a?(BadK)
  def inspect = "BadK"
end
h = {}
h[BadK.new] = :a
h[BadK.new] = :b
puts "inconsistent:  size=#{h.size}"

# compare_by_identity: distinct instances MUST duplicate; same instance updates
h = {}.compare_by_identity
ki = K.new(30)
h[K.new(30)] = :a
h[ki] = :b
h[ki] = :c
puts "cbi:           size=#{h.size} vals=#{h.values.inspect}"
h.merge!({ ki => :d })
puts "cbi-merge:     size=#{h.size} vals=#{h.values.inspect}"

# -- 10. rehash interplay ----------------------------------------------
mut = K.new(31)
h = { mut => :a, K.new(32) => :b }
mut.instance_variable_set(:@v, 32)   # mut becomes eql? to K(32)
h.rehash
puts "rehash:        size=#{h.size} #{h.inspect}"

# default-proc-assigned insert then dup-insert
h = Hash.new { |hh, k| hh[k] = :default }
h[K.new(33)]
h[K.new(33)] = :two
puts "proc-insert:   size=#{h.size} #{h.inspect}"

# -- 11. adjacent shapes surfaced by the fuzz battery -------------------
# delete-with-block honors user eql?
h = { K.new(35) => :val }
puts "del-blk:       #{h.delete(K.new(35)) { |k| "missing #{k.inspect}" }.inspect} size=#{h.size}"
puts "del-blk-miss:  #{h.delete(K.new(35)) { |k| "missing #{k.inspect}" }.inspect}"

# slice puts the ARGUMENT key in the result; assoc returns the STORED pair
p({ -0.0 => 2 }.slice(0.0))
p({ -0.0 => 2 }.assoc(0.0))

# non-empty hashes with mismatched compare_by_identity flags are never ==
p({ x: 1 }.compare_by_identity == { x: 1 })
p({} == {}.compare_by_identity)
cbik = K.new(36)
p({ cbik => 1 }.compare_by_identity == { cbik => 1 })

# replace copies the other hash's default (value or proc) + cbi flag
h = Hash.new(:D); h.replace({ x: 1 }); p h.default
h = {}; h.replace(Hash.new(:E)); p h.default
h = Hash.new(:D); h.replace(Hash.new { |hh, k| "p:#{k}" }); p h[:zz]
h = {}; h.replace({}.compare_by_identity); p h.compare_by_identity?

# values_at fires the default proc per missing key (aref semantics)
h = Hash.new { |hh, k| "p:#{k.inspect}" }
h[K.new(37)] = :hit
p h.values_at(K.new(37), K.new(38), :plain)

# Marshal round-trips compare_by_identity: flag preserved AND
# identity-duplicate (eql?-equal-but-distinct) keys survive the load
h = {}.compare_by_identity
h[K.new(39)] = 1
h[K.new(39)] = 2
m = Marshal.load(Marshal.dump(h))
p [m.size, m.compare_by_identity?, m.values.sort]

# dup/clone then dup-insert into the copy
src = { K.new(34) => :a }
d = src.dup
d[K.new(34)] = :b
c = src.clone
c[K.new(34)] = :c
puts "dup/clone:     #{d.inspect} #{c.inspect} src=#{src.inspect}"
