# --- 1. AS DeprecationProxy shape: undef'd universal names on a proxy class
class Proxy
  instance_methods.each { |m| undef_method(m) unless m.to_s.start_with?("__") || m == :object_id }
  def method_missing(name, *args) = "mm:#{name}"
  def respond_to_missing?(name, all = false) = true
end
pr = Proxy.new
p pr.is_a?(Proxy)
p pr.nil?
p pr.class
p pr.equal?(pr)
p pr.respond_to?(:anything)
# --- 2. other classes stay correct
class Plain; end
o = Plain.new
p o.is_a?(Plain), o.is_a?(Comparable), o.nil?, o.class, o.equal?(o), o.equal?(Plain.new)
# --- 3. redefine-after-undef (stale tombstone must not block)
class Redef
  def ping; "orig"; end
  undef_method :ping
  def ping; "redef"; end
end
p Redef.new.ping
p Redef.new.is_a?(Redef)
# --- 4. Symbol equal?/inspect + reopen precedence
p :a.equal?(:a), :a.equal?(:b), :a.equal?("a"), :a.inspect, :"a-b".inspect, :"".inspect
class Symbol; def inspect; "SYM!"; end; end
p :a.inspect
# --- 5. Kernel#Array shapes + to_a reopen flips the wrap bucket off
p Array(nil), Array(1), Array(:s), Array("x"), Array(2.5), Array(true), Array([4, 5])
class Integer; def to_a; [:int_to_a]; end; end
p Array(7)
# --- 6. is_a? on primitives
p 1.is_a?(Integer), 1.is_a?(Comparable), "s".is_a?(String), "s".is_a?(Enumerable), true.is_a?(FalseClass), 2.5.is_a?(Numeric), nil.is_a?(NilClass)
# --- 7. respond_to? on Class + class-level respond_to_missing?
class Widget
  def self.hook; 1; end
  def self.respond_to_missing?(name, all = false); name == :virtual; end
end
p Widget.respond_to?(:hook), Widget.respond_to?(:virtual), Widget.respond_to?(:nope), Widget.respond_to?(:new), Widget.respond_to?(:name)
module Bare; end
p Bare.respond_to?(:module_function, true), Bare.respond_to?(:allocate)
# --- 8. class-self bare calls
module Helpers
  def helper_conf; "conf:#{leaf}"; end
  def leaf; 42; end
end
module Registry
  extend Helpers
  def self.fetch(a, b); a + b + leaf; end
  def self.run; helper_conf; end
end
p Registry.run, Registry.fetch(1, 2)
class Acct
  def self.pub; secret * 2; end
  def self.secret; 21; end
  private_class_method :secret
end
p Acct.pub
p (begin; Acct.secret; rescue NoMethodError; "priv"; end)
class BG
  def self.check; block_given? ? "yes" : "no"; end
end
p BG.check
# --- 9. tombstoned send
class NoSend
  undef_method :send
  def method_missing(n, *a) = "mm:#{n}"
  def respond_to_missing?(n, all = false) = true
end
p NoSend.new.send(:anything)
