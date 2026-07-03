# Tier-2 fallback-census absorption battery (ADR 0037, 2026-07).
#
# Three shipped pieces:
#   A. `Vm::try_walk_fast_buckets` — the mid-cascade fast-bucket zone
#      extracted from `do_call` (`===`, universal/collection buckets,
#      send-family #1-#3) is now ALSO probed by the tier-2 `t2_call`
#      family, so in-body calls those buckets serve stop paying the
#      full `do_call` preamble. The per-KIND singleton gate mirrors the
#      str/heap/hash singleton arms that run before the zone.
#   B. `T2_CALL_MAX_ARGC` — the framed tier routes argc ≤ 8 (was ≤ 2)
#      `Call`/`CallNoRecv` ops through the IC-fast helpers.
#   C. New census-ranked buckets: `Array#drop/freeze/dup`,
#      `Hash#fetch` (1- and 2-arg, blockless), `String#dup`,
#      `Object#class`, bare `block_given?` — each mirrors its
#      canonical arm byte-for-byte and declines anything uncertain.
#
# Every scenario loops past the tier-2 compile threshold so the
# compiled-body path (and the zone probe) is what actually runs under
# RUBYRS_JIT_TIER2=1 / THRESHOLD=1, while plain configs pin the
# interpreter's own bucket behaviour. Redefinition + singleton
# scenarios pin the method_gen / per-kind-gate invalidation edges.

N = 60

# ---- piece C: Array#drop -------------------------------------------------
class DropWalk
  def run(arr, n)
    arr.drop(n)
  end
end

dw = DropWalk.new
base = [1, 2, 3, 4, 5]
acc = nil
N.times { acc = dw.run(base, 2) }
p acc
p dw.run(base, 0)
p dw.run(base, 9)
begin
  dw.run(base, -1)
rescue ArgumentError => e
  puts "drop-neg: #{e.message}"
end

# dropped result is a fresh plain Array (mutating it leaves the source).
d = dw.run(base, 1)
d << 99
p base
p d

# redefinition-after-warm: user Array#drop wins.
class Array
  def drop(_n)
    :user_drop
  end
end
p dw.run(base, 2)
class Array
  remove_method :drop
end
p dw.run(base, 2)

# ---- piece C: Array#freeze / dup ----------------------------------------
class FreezeWalk
  def fr(a) = a.freeze
  def du(a) = a.dup
end

fw = FreezeWalk.new
N.times { fw.du(base) }
tgt = [7, 8]
p fw.fr(tgt).equal?(tgt) # freeze returns the receiver
p tgt.frozen?
begin
  tgt << 9
rescue => e
  puts "frozen-push: #{e.class}"
end
cp = fw.du(tgt)
p cp.frozen?  # dup resets frozen
cp << 10
p cp
p tgt

# tagged subclass instances decline the bucket — the canonical arm
# preserves the subclass on dup.
class MyArr < Array; end
ma = MyArr.new
ma << 1
ma << 2
md = fw.du(ma)
p md.class
p md

# ---- piece C: String#dup -------------------------------------------------
class StrWalk
  def du(s) = s.dup
end
sw = StrWalk.new
N.times { sw.du("warm") }
fs = "frozen-src".freeze
ds = sw.du(fs)
p ds.frozen?
ds << "!"
p ds
p fs

# ---- piece C: Hash#fetch -------------------------------------------------
class FetchWalk
  def one(h, k) = h.fetch(k)
  def two(h, k, d) = h.fetch(k, d)
end
fh = FetchWalk.new
h = { a: 1, "b" => 2, 3 => :three }
N.times { fh.one(h, :a) }
N.times { fh.two(h, :zz, :dflt) }
p fh.one(h, "b")
p fh.one(h, 3)
p fh.two(h, :a, :ignored)
p fh.two(h, :missing, :fallback)
begin
  fh.one(h, :nope)
rescue KeyError => e
  puts "fetch-miss: #{e.message}"
end
# defaulted hash: fetch NEVER consults the default.
dh = Hash.new(:default_val)
dh[:k] = 1
p fh.one(dh, :k)
p fh.two(dh, :absent, :arg_default)
begin
  fh.one(dh, :absent)
rescue KeyError => e
  puts "fetch-dflt-miss: #{e.class}"
end
# redefinition-after-warm: user Hash#fetch wins (no restore — CRuby's
# `remove_method :fetch` would delete the builtin slot too).
class Hash
  def fetch(*_a)
    :user_fetch
  end
end
p fh.one(h, :a)
p fh.two(h, :a, :d)

# ---- piece C: Object#class ----------------------------------------------
class Leaf; end
class ClsWalk
  def of(o) = o.class
end
cw = ClsWalk.new
leaf = Leaf.new
N.times { cw.of(leaf) }
p cw.of(leaf)
p cw.of(leaf).name
# singleton methods don't change #class
def leaf.extra = :e
p cw.of(leaf)
# define_method override wins after warm
class Leaf
  define_method(:class) { :fake_class }
end
p cw.of(leaf)

# ---- piece C: block_given? ----------------------------------------------
class BgWalk
  def probe
    block_given? ? :with : :without
  end

  def via_block
    [1].map { block_given? }.first
  end
end
bg = BgWalk.new
N.times { bg.probe }
p bg.probe
p(bg.probe { :x })
p bg.via_block
p(bg.via_block { :y })

# user override wins (public fixed-arity, served upstream of the bucket).
class BgWalk
  def block_given?
    :override
  end
end
p bg.probe
p(bg.probe { :x })

# ---- piece A: zone serves from compiled bodies ---------------------------
class ZoneWalk
  def caseeq(a, b) = a === b
  def isa(o, k) = o.is_a?(k)
  def rsp(o, m) = o.respond_to?(m)
  def snd(o, m, x) = o.public_send(m, x)
  def bare_send(m) = send(m)
  def arr_ops(a)
    [a.size, a.length, a.empty?, a.include?(3), a.member?(9)]
  end
  def sized? = true
end
zw = ZoneWalk.new
arr = [1, 2, 3]
N.times { zw.caseeq(:sym, :sym); zw.isa(zw, ZoneWalk); zw.arr_ops(arr) }
p zw.caseeq(:sym, :sym)
p zw.caseeq("st", "st")
p zw.caseeq(Comparable, 3)
p zw.caseeq(ZoneWalk, zw)
p zw.isa(zw, ZoneWalk)
p zw.isa(:s, Symbol)
p zw.isa(nil, NilClass)
p zw.rsp(zw, :caseeq)
p zw.rsp(zw, :not_there)
p zw.snd(arr, :include?, 2)
p zw.bare_send(:sized?)
p zw.arr_ops(arr)

# per-kind singleton gate: a singleton on ONE array must not be shadowed
# by the zone probe after the site is warm on plain arrays.
special = [9, 9]
def special.empty?
  :singleton_empty
end
class SingWalk
  def emp(a) = a.empty?
end
sg = SingWalk.new
N.times { sg.emp(arr) }
p sg.emp(arr)
p sg.emp(special)
p sg.emp([])

# ---- piece B: argc 3/4 calls from compiled bodies ------------------------
class ArgWalk
  def four(a, b, c, d)
    a + b + c + d
  end

  def three(a, b, c)
    a * 100 + b * 10 + c
  end

  private def secret3(a, b, c)
    [a, b, c]
  end

  def call_private
    secret3(1, 2, 3)
  end

  def chain(o)
    o.three(1, 2, 3) + four(1, 1, 1, 1)
  end

  def trailing(a, b, c)
    c
  end
end
aw = ArgWalk.new
N.times { aw.chain(aw) }
p aw.chain(aw)
p aw.call_private
p aw.four(1, 2, 3, 4)
p aw.trailing(1, 2, { k: 3 })
begin
  aw.three(1, 2)
rescue ArgumentError => e
  puts "arity3: #{e.message}"
end
p aw.__send__(:four, 5, 6, 7, 8)

# method_missing at argc 3 from a warmed compiled body.
class MM3
  def method_missing(name, *args)
    [name, args]
  end

  def respond_to_missing?(_n, _p = false) = true

  def go
    ghost(1, 2, 3)
  end
end
m3 = MM3.new
N.times { m3.go }
p m3.go
