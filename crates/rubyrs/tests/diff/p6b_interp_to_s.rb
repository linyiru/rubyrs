# Campaign P6b, Item 1: the tier-2 lean `Op::InterpToS` serve — string
# interpolation `to_s` inside a compiled body. String passthrough +
# Symbol/Integer primitive fast serve inline; a user `to_s`, a
# Float/Bool/Nil primitive, or a reopened/refined Symbol|Integer
# declines to the full `do_call(:to_s)`. Every arm CRuby-exact
# (rb_obj_as_string semantics).

# Interpolation lives inside methods, called hot, so tier-2 compiles
# the bodies and the InterpToS op runs through the lean serve (not the
# generic t2_op boundary). The non-reopened fast paths are exercised
# FIRST; the global reopens / refinement come last so they can't
# perturb the earlier rows.

# --- 1. Symbol interpolation (the AM-dominant kind: attribute names).
def sym_interp(s) = "attr=#{s}!"
acc = nil
200.times { acc = sym_interp(:age) }
p acc                              # "attr=age!"
p sym_interp(:名前)                # non-ascii symbol name (UTF-8 tag)
p sym_interp(:"a b")               # symbol with a space

# --- 2. Integer interpolation (fast_prim_int_safe path).
def int_interp(n) = "n=#{n}"
200.times { int_interp(7) }
p int_interp(42), int_interp(-13), int_interp(0)

# --- 3. String passthrough — rb_obj_as_string returns the String
# as-is; user String#to_s is NEVER consulted (verified for the plain
# String even AFTER a global override lands, below).
def str_interp(s) = "[#{s}]"
200.times { str_interp("x") }
p str_interp("hello")
p str_interp("")                   # empty string passthrough

# --- 4. String SUBCLASS instance interpolates to its own content.
class MyStr < String; end
def sub_interp(s) = "sub<#{s}>"
ms = MyStr.new("payload")
50.times { sub_interp(ms) }
p sub_interp(ms)                   # "sub<payload>"

# --- 5. Float / true / false / nil — no dedicated fast arm, decline to
# do_call (built-in to_s, no frame).
def prim_interp(v) = "v=#{v}."
100.times { prim_interp(1.5); prim_interp(nil); prim_interp(true) }
p prim_interp(1.5), prim_interp(2.0), prim_interp(nil)
p prim_interp(true), prim_interp(false)

# --- 6. User object — declines to do_call, drives the pushed to_s
# frame with t2_finish.
class Widget
  def initialize(t) = (@t = t)
  def to_s = "W(#{@t})"
end
def obj_interp(w) = "got #{w} ok"
w = Widget.new(3)
100.times { obj_interp(w) }
p obj_interp(w)                    # "got W(3) ok"
p obj_interp(Widget.new(:z))

# --- 7. Multiple interpolations in one string + nesting.
def multi(a, b, c) = "#{a}/#{b}/#{c}"
100.times { multi(:k, 5, "s") }
p multi(:k, 5, "s")                # "k/5/s"
p multi(Widget.new(1), :two, 3)    # "W(1)/two/3"

# --- 8. to_s that RAISES — the error surfaces through the compiled
# body exactly as the interpreter would report it.
class Boom
  def to_s = raise "boom-from-to_s"
end
def raise_interp(x) = "pre #{x} post"
b = Boom.new
begin
  raise_interp(b)
rescue RuntimeError => e
  p e.message                      # "boom-from-to_s"
end

# --- 9. Integer#to_s REOPEN — interpolation must now use the override
# (fast_prim_int_safe flips off; declines to do_call which honors it).
class Integer
  def to_s(*) = "INT_OVR"
end
p int_interp(99)                   # "n=INT_OVR"
100.times { int_interp(1) }
p int_interp(1)                    # still "n=INT_OVR" after re-warm

# --- 10. Symbol#to_s REOPEN — same, via the prim_reopen_mask bit-3
# gate flipping off.
class Symbol
  def to_s = "SYM_OVR"
end
p sym_interp(:whatever)            # "attr=SYM_OVR!"

# --- 11. String#to_s REOPEN — interpolation STILL returns the string
# as-is (passthrough never calls to_s), even with the override present.
class String
  def to_s = "STR_OVR"
end
p str_interp("literal")            # "[literal]" — NOT "[STR_OVR]"
p "explicit: #{"literal".to_s}"    # explicit .to_s -> "STR_OVR"

# --- 12. Refinement of Symbol#to_s — the refinement is active in the
# refining scope, so interpolation there declines to do_call and picks
# up the refined method; outside, the base (reopened) method stands.
module SymRef
  refine Symbol do
    def to_s = "REFINED"
  end
end
class InRef
  using SymRef
  def r(s) = "ref:#{s}"
end
p InRef.new.r(:zzz)                # "ref:REFINED"
