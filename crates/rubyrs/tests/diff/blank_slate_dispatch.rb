# minitest Mock's blank-slate dispatch family:
# 1. `alias __x x` of a builtin snapshots the BUILTIN — a later
#    user override of `x` must NOT be re-entered by the forwarder
#    (Mock: respond_to? calls __respond_to?; without the snapshot
#    semantics this recursed to SystemStackError).
# 2. Module#instance_methods lists the universal Object methods
#    (kind_of?/==/respond_to?/...) so a blank slate can enumerate
#    and undef them; undef_method accepts each.
# 3. Undef'd universals dispatch to method_missing.
# 4. redefine-after-undef wins over the stale tombstone.
# 5. A user `public_send` override is honored (Mock overrides it).

class Forwarded
  alias __respond_to? respond_to?
  def respond_to?(sym, include_private = false)
    return true if sym == :magic
    __respond_to?(sym, include_private)
  end
end
f = Forwarded.new
p f.respond_to?(:magic)
p f.respond_to?(:to_s)
p f.respond_to?(:no_such)

class Probe; end
ms = Probe.instance_methods
p ms.include?(:kind_of?)
p ms.include?(:==)
p ms.include?(:respond_to?)
p ms.include?(:object_id)
p Probe.instance_methods(false).include?(:kind_of?)

module ProbeM; end
p ProbeM.instance_methods.include?(:kind_of?)

class Slate
  keep = %i[== inspect to_s respond_to? object_id public_send send]
  instance_methods.each do |m|
    undef_method m unless keep.include?(m) || m =~ /^__/
  end
  def method_missing(sym, *args)
    [:mm, sym, args]
  end
  def respond_to?(sym, include_private = false)
    return true if sym == :special
    __respond_to_p_orig(sym) rescue false
  end
  def public_send(*args)
    [:ps, args]
  end
end
s = Slate.new
p s.kind_of?(String)
p s.nil?
p s.frozen?
p s.public_send
p s.respond_to?(:special)

# redefine-after-undef: the new definition wins over the tombstone.
class Redef
  undef_method :kind_of?
  def kind_of?(k)
    [:redefined, k]
  end
end
p Redef.new.kind_of?(Integer)
