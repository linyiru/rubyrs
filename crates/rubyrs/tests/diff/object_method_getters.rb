# Object#singleton_method(:name) and #public_method(:name) —
# narrowed siblings of the existing #method(:name) getter.
# All three return a BoundMethod; the differences are which
# entries they consider and which mismatches raise NameError.
#
#   method            — full chain (own + includes + super),
#                       any visibility
#   singleton_method  — only entries installed on the
#                       eigenclass (Object) or
#                       cls.singleton_methods (Class)
#   public_method     — full chain; NameError if visibility
#                       is Private OR Protected, and also
#                       NameError if the method doesn't exist
#                       (Only Public passes — CRuby parity)

class C
  def pub; "public"; end
  private
  def priv; "private"; end
  protected
  def prot; "protected"; end
end

c = C.new
def c.sing; "singleton"; end

# (1) singleton_method — only direct-on-eigenclass entries
puts c.singleton_method(:sing).call
begin
  c.singleton_method(:pub)
rescue NameError
  puts "ne-singleton-pub"
end
begin
  c.singleton_method(:priv)
rescue NameError
  puts "ne-singleton-priv"
end
begin
  c.singleton_method(:nope)
rescue NameError
  puts "ne-singleton-nope"
end

# (2) public_method — chain, Private → NameError
puts c.public_method(:pub).call
puts c.public_method(:sing).call          # singleton is public
begin
  c.public_method(:priv)
rescue NameError
  puts "ne-public-priv"
end
# Protected also raises NameError (CRuby parity — only Public
# passes, both Private and Protected are rejected)
begin
  c.public_method(:prot)
rescue NameError
  puts "ne-public-prot"
end

# Cycle-1: public_method must also raise NameError on a
# missing method — capturing should fail at getter time, not
# defer to call time.
begin
  c.public_method(:nope)
rescue NameError
  puts "ne-public-missing"
end

# (3) Class receiver — singleton_method reads
# cls.singleton_methods (i.e. class methods)
class K
  def self.cls_m; "k.cls_m"; end
  def inst_m; "k.inst_m"; end
end
puts K.singleton_method(:cls_m).call
# Instance methods are NOT class methods → NameError
begin
  K.singleton_method(:inst_m)
rescue NameError
  puts "ne-K-inst_m"
end
# `new` is a primitive class-level method, not installed in
# cls.singleton_methods → NameError (consistent with the
# "directly-installed" definition we use here)
begin
  K.singleton_method(:new)
rescue NameError
  puts "ne-K-new"
end

# Cycle-2: public_method must NOT raise NameError for
# universal arms / built-ins that have no per-class Method
# entry but DO dispatch (e.g. `Object#to_s`, `Class#new`).
# Cycle-1's snapshot-is-None branch wrongly triggered for
# these; cycle-2 consults `responds_to` so only truly-missing
# methods raise.
puts c.public_method(:to_s).call.start_with?("#<C:")
puts c.public_method(:object_id).call.is_a?(Integer)
# Cycle-2: Class receiver — `method`/`public_method` must
# also reach `cls.singleton_methods` (the cycle-1 snapshot
# lookup used `Vm::class_of` and missed every class method).
puts K.public_method(:cls_m).call
# Class-level built-in like `:new` reaches dispatch arm,
# snapshot is None but responds_to is true → must NOT raise.
puts K.public_method(:new).is_a?(Method)

# Cycle-3: error message for Class receivers must reference
# the eigenclass-shell form (CRuby: \"#<Class:K>\"), not
# `Vm::class_of(K) == \"Class\"`. Pre-fix the message read
# `for class 'Class'`, leaking the metaclass instead of K.
begin
  K.public_method(:totally_missing_xyz)
rescue NameError => e
  puts e.message
end

# (4) Returned value is a BoundMethod that calls correctly
m1 = c.public_method(:pub)
m2 = c.singleton_method(:sing)
puts m1.class.name
puts m2.class.name
puts m1.call
puts m2.call

# (5) Bound to the receiver — calling on a different receiver
# is rejected unless we go through unbind/bind, matching
# how plain #method works.
c2 = C.new
def c2.sing; "c2-sing"; end
puts c.singleton_method(:sing).call    # original receiver
puts c2.singleton_method(:sing).call   # different receiver, own singleton

# (6) respond_to? must agree
puts Object.new.respond_to?(:singleton_method)
puts Object.new.respond_to?(:public_method)
puts 42.respond_to?(:singleton_method)
puts 42.respond_to?(:public_method)
