# `self.private` / `self.public` / `self.protected` /
# `self.module_function` with an explicit `self.` receiver behave
# exactly like the receiver-less forms, bare and with args, in a class
# body, a `class << self` body, and a module_eval string. The bare
# `self.private` raised NoMethodError. ActiveSupport 8.1's
# `delegate ..., private: true` module_evals "self.private;def ...".

class A
  self.private
  def a = 1
  self.public
  def b = 2
  def c = 3
  def d = 4
  self.private :c
  self.private(def e = 5)
  self.protected :d
  self.public def f = 6
  self.protected
  def g = 7
  self.public
  self.attr_accessor :h
  self.alias_method :i, :b
  self.private_class_method def self.j = 10
end
p A.private_instance_methods(false).sort
p A.protected_instance_methods(false).sort
p A.public_instance_methods(false).sort
p A.respond_to?(:j)

# The ActiveSupport::Delegation.generate shape.
class Owner
  def target = "t"
end
Owner.module_eval("self.private;def up(...) = target.upcase(...);def rev = target.reverse", __FILE__, __LINE__)
p Owner.private_method_defined?(:up), Owner.private_method_defined?(:rev)
p Owner.new.send(:up)
Owner.class_eval("self.public;def pub = 1")
p Owner.public_method_defined?(:pub)

class C
  class << self
    self.private
    def s1 = 1
    self.public
    def s2 = 2
  end
end
p C.singleton_class.private_instance_methods(false), C.s2
p((C.s1 rescue $!.class))

module M
  self.module_function
  def mf = 1
end
p M.mf, M.private_instance_methods(false)
module N
  def n1 = 1
  self.module_function :n1
end
p N.n1

class B
  def m = self.priv
  private def priv = :ok
end
p B.new.m

# An explicit `self.private` still reaches a user override.
class U
  def self.private(*names)
    (@log ||= []) << names
    names.empty? ? nil : super
  end
  def self.log = @log
  def x = 1
  self.private :x
  self.private
end
p U.log, U.private_method_defined?(:x)

# `module_function` is Module-only: CRuby's Class undefines it, so every
# class-side form raises (NoMethodError < NameError), including inside
# `class << self` of a module, whose self is the singleton Class.
def mf_raises? = (yield; false) rescue $!.is_a?(NameError)
p mf_raises? { class MfC1; module_function; end }
p mf_raises? { class MfC2; def a; end; module_function :a; end }
p mf_raises? { class MfC3; self.module_function; end }
p mf_raises? { class MfC4; class << self; self.module_function; end; end }
p mf_raises? { module MfM1; class << self; module_function; end; end }
p mf_raises? { module MfM2; class << self; self.module_function; def f = 1; end; end }
p mf_raises? { module MfM3; class << self; def g = 1; module_function :g; end; end }
p mf_raises? { o = Object.new; class << o; module_function; end }
module MfM4; self.module_function; def h = 4; end
p MfM4.h
