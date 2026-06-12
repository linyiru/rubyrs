# undef_method must kill a method defined ON the same class (not
# just block ancestors) — minitest Object#stub's restore path does
# def obj.x; ...; obj.singleton_class.undef_method :x.
o = Object.new
def o.zap; "alive"; end
sc = o.singleton_class
sc.send :undef_method, :zap
begin
  o.zap
rescue NoMethodError
  puts "call: NoMethodError"
end
p o.respond_to?(:zap)
p sc.method_defined?(:zap)
# redefine AFTER undef wins again
def o.zap; "back"; end
p o.zap
# plain-class own-method undef
class UOwn
  def gone; 1; end
  undef_method :gone
end
begin
  UOwn.new.gone
rescue NoMethodError
  puts "own: NoMethodError"
end
# full stub-style cycle: alias / cover / undef / restore / undef
o2 = Object.new
def o2.real(_a); "real"; end
sc2 = o2.singleton_class
sc2.send :alias_method, :__save__, :real
sc2.send :define_method, :real do |*_a| "stubbed" end
p o2.real(1)
sc2.send :undef_method, :real
sc2.send :alias_method, :real, :__save__
sc2.send :undef_method, :__save__
p o2.real(1)
p o2.respond_to?(:__save__)
