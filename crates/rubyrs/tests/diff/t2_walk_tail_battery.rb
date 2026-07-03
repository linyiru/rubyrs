# Census-TAIL fast-bucket battery (ADR 0037 follow-on, 2026-07).
#
# Four buckets the fallback-census wave declined at <1.2ms each, now
# absorbed in `Vm::try_walk_fast_buckets` (and therefore served from
# both `do_call` and the tier-2 `t2_call` probe):
#   1. Array `[]=` argc-3 splice-assign (`a[i, len] = v`) — the
#      hottest remaining slow-cascade shape (~4.8K/walk).
#   2. `Object#equal?` — identity, IC-miss-gated.
#   3. `Module#method_defined?` — shares `class_method_defined` (the
#      canonical arm's helper, riding the respond_to? memo with the
#      new RESPOND_PROT_BIT) so fast/slow answers can't drift.
#   4. Bare `__method__` — the kernel arm's frame walk, IC-miss-gated
#      like `block_given?`.
#
# Every scenario loops past the tier-2 compile threshold so the
# compiled-body path (and the zone probe) is what actually runs under
# RUBYRS_JIT_TIER2=1 / THRESHOLD=1, while plain configs pin the
# interpreter's own bucket behaviour. Redefinition / visibility-flip
# scenarios pin the method_gen invalidation edges.

N = 60

# ---- Array []= argc-3 splice-assign --------------------------------------
class AsetWalk
  def set(a, i, l, v)
    a[i, l] = v
  end
end

aw = AsetWalk.new
acc = nil
N.times do
  arr = [1, 2, 3, 4, 5]
  aw.set(arr, 1, 2, [9, 8])
  acc = arr
end
p acc

# Expression value is the RHS as-is (not the receiver, not the splice).
arr = [1, 2, 3, 4, 5]
r = (arr[1, 2] = [7, 7])
p r
p arr

# Non-Array RHS wraps as a single element.
arr = [1, 2, 3, 4, 5]
arr[1, 2] = :x
p arr

# Length clamps at the current end.
arr = [1, 2, 3]
arr[1, 99] = :clamped
p arr

# Zero-length splice is an insert.
arr = [1, 2, 3]
arr[1, 0] = [:ins]
p arr

# Start past the end pads with nil.
arr = [1]
arr[3, 0] = [5]
p arr
arr = [1]
arr[4, 2] = :far
p arr

# Negative start wraps from the end.
arr = [1, 2, 3, 4]
arr[-2, 1] = :z
p arr

# Aliasing: assigning a slice of the receiver itself.
al = [1, 2, 3]
al[1, 1] = al
p al

# Raising shapes decline to the canonical arm (exact messages).
begin
  [1, 2][-5, 1] = :b
rescue IndexError => e
  puts "aset-neg-start: #{e.message}"
end
begin
  [1, 2][0, -1] = :b
rescue IndexError => e
  puts "aset-neg-len: #{e.message}"
end
begin
  [1].freeze[0, 1] = :b
rescue FrozenError => e
  puts "aset-frozen: #{e.message}"
end

# Explicit send form: the METHOD return value is the assigned value.
p [1, 2, 3].send(:[]=, 1, 1, :s)

# Range form keeps its canonical arm.
arr = [1, 2, 3, 4, 5]
arr[1..2] = [:r]
p arr

# Subclass instances (class_tag) decline to the subclass gate.
class MyArr < Array; end
ma = MyArr.new
ma.push(1, 2, 3)
N.times { |i| aw.set(ma, 1, 1, i) }
ma[0, 2] = [:sub]
p ma.to_a
p ma.class

# Redefinition-after-warm: a user Array#[]= wins (method_gen flip).
# Alias-save/restore: CRuby's remove_method after a redefine leaves
# NO []= at all (unlike #drop, which falls back to Enumerable), so
# the original is stashed and re-aliased.
class Array
  alias_method :__tail_orig_aset, :[]=
  def []=(*_args)
    :user_aset
  end
end
p [1, 2, 3].send(:[]=, 1, 1, :ignored)
class Array
  alias_method :[]=, :__tail_orig_aset
  remove_method :__tail_orig_aset
end
arr = [1, 2, 3]
arr[0, 2] = :back
p arr

# ---- Object#equal? --------------------------------------------------------
class EqWalk
  def same(a, b) = a.equal?(b)
end

class QThing; end
ew = EqWalk.new
q1 = QThing.new
q2 = QThing.new
acc = nil
N.times { acc = ew.same(q1, q1) }
p acc
p ew.same(q1, q2)
p ew.same(q1, 5)
p ew.same(q1, nil)
p ew.same(q1, "str")

# Public fixed-arity override wins (served upstream of the bucket).
class QThing
  def equal?(_o)
    :user_equal
  end
end
p ew.same(q1, q1)
class QThing
  remove_method :equal?
end
p ew.same(q1, q1)

# Non-Object receivers keep their canonical arms.
p nil.equal?(nil)
p :a.equal?(:a)
arr_id = [1]
p arr_id.equal?(arr_id)
p arr_id.equal?([1])

# ---- Module#method_defined? (dispatch shapes) -----------------------------
class MdWalk
  def probe(cls, name) = cls.method_defined?(name)
  def probe2(cls, name, inh) = cls.method_defined?(name, inh)
end

class Target
  def zap; end
end
class SubTarget < Target; end

mdw = MdWalk.new
acc = nil
N.times { acc = mdw.probe(Target, :zap) }
p acc
p mdw.probe(Target, :nope)
p mdw.probe(SubTarget, :zap)
p mdw.probe2(SubTarget, :zap, true)
p mdw.probe2(SubTarget, :zap, false)
p mdw.probe2(Target, :zap, false)

# Late definition after warm (method_gen bump invalidates the memo).
N.times { mdw.probe(Target, :late) }
p mdw.probe(Target, :late)
class Target
  def late; end
end
p mdw.probe(Target, :late)

# Visibility flips after warm (the Cell flip bumps method_gen too).
class Target
  private :late
end
p mdw.probe(Target, :late)
class Target
  public :late
end
p mdw.probe(Target, :late)
class Target
  protected :late
end
p mdw.probe(Target, :late)

# Module receivers.
module MdMod
  def mod_m; end
end
N.times { acc = mdw.probe(MdMod, :mod_m) }
p acc
p mdw.probe(MdMod, :absent)

# ---- bare __method__ ------------------------------------------------------
class WhichWalk
  def which = __method__
  def in_block = [1].map { __method__ }.first
  define_method(:dm) { __method__ }
end

ww = WhichWalk.new
acc = nil
N.times { acc = ww.which }
p acc
p ww.in_block
p ww.dm

# Aliased calls report the ORIGINAL defined name (__method__ contract).
class WhichWalk
  alias_method :which_alias, :which
end
p ww.which_alias

# Toplevel __method__ is nil (non-Object self declines to the cascade).
p __method__
