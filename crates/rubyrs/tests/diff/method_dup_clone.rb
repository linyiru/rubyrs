# Method#dup / Method#clone / UnboundMethod#dup / #clone —
# re-wrap the captured (recv, name, snapshot) tuple into a
# distinct heap object. CRuby parity:
#   - `equal?` false (distinct ObjId)
#   - `==` and `eql?` true (same recv + same Method snapshot)
#   - `hash` equal (the captured-Rc-ptr the equality chain keys on
#     is preserved by the re-wrap)

class C
  def foo; "C.foo"; end
  def bar; end
end

c = C.new
m = c.method(:foo)

# (1) BoundMethod#dup — distinct object, same identity-by-value.
md = m.dup
puts md.equal?(m)        # false
puts md == m             # true
puts md.eql?(m)          # true
puts md.hash == m.hash   # true

# (2) #clone has the same shape as #dup for Method (no
# singleton/frozen subtleties on Method itself).
mc = m.clone
puts mc.equal?(m)        # false
puts mc == m             # true
puts mc.hash == m.hash   # true

# (3) Duped Method invokes the same definition on the captured
# receiver.
puts md.call             # C.foo
puts md.receiver.equal?(c)  # true
puts md.name             # foo
puts md.owner.name       # C

# (4) UnboundMethod#dup / #clone — same parity.
u = C.instance_method(:foo)
ud = u.dup
puts ud.equal?(u)        # false
puts ud == u             # true
puts ud.eql?(u)          # true
puts ud.hash == u.hash   # true
puts ud.name             # foo
puts ud.owner.name       # C
puts ud.bind(c).call     # C.foo

# (5) Round-trip: dup → unbind / bind ↔ dup. The duped
# BoundMethod's unbind equals the unbind of the original.
puts m.unbind == md.unbind   # true

# (6) Wrong arity — both Method#dup and #clone are 0-arg.
# CRuby raises ArgumentError; rubyrs's universal arm at
# dispatch.rs raises the same message.
begin
  c.method(:foo).dup(1)
rescue ArgumentError => e
  puts e.message
end
begin
  C.instance_method(:foo).clone(:freeze)
rescue ArgumentError => e
  puts e.message
end

# (7) respond_to? must agree.
puts c.method(:foo).respond_to?(:dup)
puts c.method(:foo).respond_to?(:clone)
puts C.instance_method(:foo).respond_to?(:dup)
puts C.instance_method(:foo).respond_to?(:clone)
