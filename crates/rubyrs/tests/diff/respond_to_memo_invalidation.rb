# The respond_to? (class, name, method_gen) memo (vm/lookup.rs
# responds_to_object_memo) caches the method-table VERDICT per class
# pointer + name, invalidated by method_gen + a Weak<Class> ptr_eq
# guard. This fixture warms each answer (200-call loops fill the memo)
# and then mutates the class graph in every way that must flip the
# cached verdict: include/prepend after warm, undef_method,
# remove_method (own-table and override-reveals-parent), obj.extend,
# def obj.m on a pre-existing eigenclass, negative-then-define, the
# implicit class-body private/public/protected :m Cell flip (the one
# visibility path that historically did NOT bump method_gen), and a
# respond_to_missing? hook whose answer depends on mutable state (the
# hook's RESULT must never be memoized — only its existence).
# Companion to send_family_warm_override.rb; run under default,
# STRESS_GC, TIER2 and JIT.

# --- include after warm: answer flips false -> true ---
module LateInc
  def late_inc_m; :li; end
end
class IncHost; end
ih = IncHost.new
p 200.times.map { ih.respond_to?(:late_inc_m) }.uniq
class IncHost; include LateInc; end
p ih.respond_to?(:late_inc_m)

# --- prepend after warm: new name appears via the prepend chain ---
module LatePre
  def late_pre_m; :lp; end
end
class PreHost; end
ph = PreHost.new
p 200.times.map { ph.respond_to?(:late_pre_m) }.uniq
class PreHost; prepend LatePre; end
p ph.respond_to?(:late_pre_m)

# --- undef_method after warm: true -> false (tombstone shadows parent) ---
class UndefParent
  def doomed; :d; end
end
class UndefChild < UndefParent; end
uc = UndefChild.new
p 200.times.map { uc.respond_to?(:doomed) }.uniq
class UndefChild; undef_method :doomed; end
p uc.respond_to?(:doomed)
p uc.respond_to?(:doomed, true)
p UndefParent.new.respond_to?(:doomed)   # the parent is unaffected

# --- remove_method after warm: own-table removal ---
class RmOwn
  def gone; :g; end
end
ro = RmOwn.new
p 200.times.map { ro.respond_to?(:gone) }.uniq
class RmOwn; remove_method :gone; end
p ro.respond_to?(:gone)

# --- remove_method reveals the inherited definition (stays true) ---
class RevealParent
  def veiled; :parent; end
end
class RevealChild < RevealParent
  def veiled; :child; end
end
rc = RevealChild.new
p 200.times.map { rc.respond_to?(:veiled) }.uniq
class RevealChild; remove_method :veiled; end
p rc.respond_to?(:veiled)
p rc.veiled

# --- obj.extend after warm: eigenclass gains the module ---
module ExtLate
  def ext_late_m; :el; end
end
class ExtHost; end
e1 = ExtHost.new
e2 = ExtHost.new
p 200.times.map { e1.respond_to?(:ext_late_m) }.uniq
e1.extend(ExtLate)
p e1.respond_to?(:ext_late_m)
p e2.respond_to?(:ext_late_m)            # other instances unaffected

# --- def obj.m landing on an ALREADY-WARM eigenclass ---
# (extend first so the eigenclass exists and its pointer is the warm
# memo key; the later singleton def mutates that same eigenclass and
# must invalidate.)
module EigenSeed
  def seed_m; :seed; end
end
es = ExtHost.new
es.extend(EigenSeed)
p 200.times.map { es.respond_to?(:eigen_late) }.uniq
def es.eigen_late; :elate; end
p es.respond_to?(:eigen_late)

# --- negative-then-define flip (plain def after a warm false) ---
class NegDef; end
nd = NegDef.new
p 200.times.map { nd.respond_to?(:appears) }.uniq
class NegDef
  def appears; :a; end
end
p nd.respond_to?(:appears)

# --- implicit class-body visibility flips after warm ---
# `private :m` / `public :m` / `protected :m` inside a reopened class
# body flip the Method's visibility Cell in place — the one mutation
# that historically skipped the method_gen bump (inline caches re-read
# the Cell; a verdict memo cannot).
class VisFlip
  def flip_p; :fp; end
  def flip_back; :fb; end
  private def flip_up; :fu; end
  def flip_prot; :fpr; end
end
vf = VisFlip.new
p 200.times.map { vf.respond_to?(:flip_p) }.uniq        # warm: public
p 200.times.map { vf.respond_to?(:flip_up) }.uniq       # warm: private
p 200.times.map { vf.respond_to?(:flip_up, true) }.uniq
class VisFlip
  private :flip_p
  public :flip_up
  protected :flip_prot
end
p vf.respond_to?(:flip_p)                # false — went private
p vf.respond_to?(:flip_p, true)          # true under include_all
p vf.respond_to?(:flip_up)               # true — went public
p vf.respond_to?(:flip_prot)             # false — protected excluded
p vf.respond_to?(:flip_prot, true)
p vf.respond_to?(:flip_back)             # untouched name still public

# --- respond_to_missing? result is NEVER memoized ---
# The hook consults mutable state; flipping that state between calls
# (with NO method-table change anywhere) must change the answer.
class Moody
  @@mood = false
  def self.mood=(v); @@mood = v; end
  def respond_to_missing?(name, include_all = false)
    name == :maybe && @@mood
  end
end
md = Moody.new
p 200.times.map { md.respond_to?(:maybe) }.uniq
Moody.mood = true
p md.respond_to?(:maybe)
Moody.mood = false
p md.respond_to?(:maybe)

# --- deep-ancestry chain (10 modules) with mixed visibility ---
mods = (0...10).map do |i|
  Module.new do
    define_method("deep_m#{i}") { i }
  end
end
class DeepHost; end
mods.each { |m| DeepHost.include(m) }
dh = DeepHost.new
p 200.times.map { dh.respond_to?(:deep_m0) }.uniq
p 200.times.map { dh.respond_to?(:deep_m9) }.uniq
p 200.times.map { dh.respond_to?(:deep_absent) }.uniq

# --- dynamic-name churn: distinct interned names stay correct ---
# (exercises memo growth; each name is a fresh (class, name) key)
class Churn
  def churn_500; :c; end
end
ch = Churn.new
hits = 0
1000.times { |i| hits += 1 if ch.respond_to?(:"churn_#{i}") }
p hits

# --- anonymous class churn: dropped classes' ptrs must not leak
# stale verdicts into a fresh class reusing the address ---
20.times do
  k = Class.new { def anon_hit; :ah; end }
  inst = k.new
  50.times { inst.respond_to?(:anon_hit) }
end
k2 = Class.new
p k2.new.respond_to?(:anon_hit)
