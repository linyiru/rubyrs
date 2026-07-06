# Dispatch-campaign P5b name-keyed probe filter parity: names OUTSIDE
# the probe_name_mask must resolve identically through the slow
# cascade, names INSIDE it must keep their fast-bucket serves, and
# user methods that COLLIDE with mask names must keep explicit-recv
# precedence over the (declining) buckets.

# --- 1. uncovered names (mask-miss): plain, NFA, kwargs-optional shapes
class Widget
  def initialize = @n = 0
  def zorple(x, opt: nil) = (@n += x; opt ? [@n, opt] : @n)
  def blivet(a, b = 10, *rest) = [a, b, rest]
  def flumox = "bare"
end
w = Widget.new
p w.zorple(1)
p w.zorple(2, opt: :z)
p w.blivet(1)
p w.blivet(1, 2, 3, 4)
p w.flumox

# --- 2. user methods COLLIDING with mask names (bucket declines,
#        explicit-recv/user table must win)
class Colliding
  def size = "user-size"
  def merge(other) = "user-merge:#{other}"
  def fetch(k) = "user-fetch:#{k}"
  def push(v) = "user-push:#{v}"
  def call = "user-call"
end
c = Colliding.new
p c.size, c.merge(1), c.fetch(:k), c.push(9), c.call

# --- 3. mask-name serves stay fast-bucket-correct on their shapes
h = { a: 1 }
a = [1, 2, 3]
p h[:a], h.key?(:a), h.merge(b: 2), h.slice(:a), h.except(:a)
p a.size, a.length, a.empty?, a.include?(2), a.push(4), a << 5
p 1.is_a?(Integer), :s === :s, "x" === "x", nil.nil?, !false
p a.frozen?, :sym.to_s, :sym.to_sym, 5.to_s, "str".empty?

# --- 4. method_missing under a mask-miss name (permanently uncovered)
class Ghost
  def method_missing(name, *args) = "mm:#{name}:#{args.inspect}"
  def respond_to_missing?(n, all = false) = true
end
g = Ghost.new
p g.wibble, g.wobble(1, 2)

# --- 5. send-family re-aim to an uncovered target name (mask-hit
#        `send` re-aims to a mask-miss target)
p w.send(:flumox), w.public_send(:zorple, 5), g.__send__(:phantom)

# --- 6. class-self bare calls with uncovered names (the walk zone's
#        shape-keyed bucket is exempt from the mask — must still serve)
module Conf
  def self.setting = "cfg"
  def self.lookup(k) = "cfg:#{k}"
  def self.probe = [setting, lookup(:x)]
end
p Conf.probe

# --- 7. define_method under a mask-miss name + a mask-hit name
class Dyn
  define_method(:quux) { |x = 1| x * 3 }
  define_method(:size) { "dyn-size" }
end
d = Dyn.new
p d.quux, d.quux(4), d.size

# --- 8. proc.call (mask-hit `call` on a Block) vs uncovered alias
pr = proc { |x| x + 100 }
p pr.call(1), pr.(2), pr[3]

# --- 9. refinement on a mask-miss name still detours correctly
class Plain2; def orig = "orig"; end
module Ref
  refine Plain2 do
    def orig = "refined"
    def zorple2 = "refined-new"
  end
end
p Plain2.new.orig
using Ref
p Plain2.new.orig, Plain2.new.zorple2
