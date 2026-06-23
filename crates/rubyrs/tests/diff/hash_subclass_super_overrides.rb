# Hash subclass overrides + super to block-form primitives, mirroring
# Sinatra's IndifferentHash (super into transform_values!, select!,
# fetch { }, fetch_values { }, default(key), nested dig).
class IH < Hash
  def ck(k); k.is_a?(Symbol) ? k.to_s : k; end
  def cv(v); v.is_a?(Hash) ? (v.is_a?(IH) ? v : IH[v]) : v; end
  def self.[](*a); new.merge!(Hash[*a]); end
  def [](k); super(ck(k)); end
  def []=(k,v); super(ck(k), cv(v)); end
  def fetch(k, *a); a.map!(&method(:cv)); super(ck(k), *a); end
  def fetch_values(*ks); ks.map!(&method(:ck)); super(*ks); end
  def dig(k, *rest); super(ck(k), *rest); end
  def default(*a); a.map!(&method(:ck)); super(*a); end
  def merge!(*others)
    others.each { |h| h.each { |k, v| self[k] = v } }
    self
  end
  def transform_values!; super(&method(:cv)); end
  def select(&b); dup.tap { |h| h.select!(&b) }; end
  def reject(&b); dup.tap { |h| h.reject!(&b) }; end
end

h = IH[a: 1, b: 2]
p h["a"]                          # 1
p h.fetch(:a)                     # 1
p h.fetch(:z, 99)                 # 99
p h.fetch(:z) { 7 }               # 7
p h.fetch(:z) { |k| k }           # "z"
p h.fetch_values(:a, :b)          # [1, 2]
p h.fetch_values(:a, :z) { |k| k.upcase }  # [1, "Z"]
p h.dig(:a)                       # 1
sel = h.select { |k, v| v == 1 }
p sel.class                       # IH
p sel["a"]                        # 1
rej = h.reject { |k, v| v == 1 }
p rej.class                       # IH
p rej["b"]                        # 2
h.transform_values! { |v| v * 10 }
p h["a"]                          # 10

# default block via default(key)
d = IH.new { |hash, k| hash[k] = k.upcase }
p d.default                       # nil
p d.default(:x)                   # "X"

# nested dig through Hash + Array subclass values
n = IH.new
n[:outer] = IH[inner: [IH[deep: :found]]]
p n.dig(:outer, :inner, 0, :deep) # :found
