# `super(*a, &b)` with no superclass method falls through to
# method_missing (CRuby). Sinatra's Delegator proxies a delegated
# method to a mixin's method_missing via `super if respond_to?`.
module Deleg
  def self.delegate(*ms)
    ms.each do |mn|
      define_method(mn) { |*a, &b| return super(*a, &b) if respond_to?(mn); "target:#{mn}" }
      private mn
    end
  end
  delegate :options
end
mixin = Module.new do
  def respond_to?(m, *); m.to_sym == :options or super; end
  def method_missing(m, *a, &b); return super unless m.to_sym == :options; {some: :option}; end
end
obj = Object.new
obj.extend(Deleg)
val = nil
obj.instance_eval { extend mixin; val = options }
p val

# super from method_missing itself must NOT recurse — it raises
# NoMethodError for the original missing method.
class MM
  def method_missing(m, *a, &b); return "ok:#{m}" if m == :good; super; end
end
p MM.new.good
begin; MM.new.bad; rescue NoMethodError; p :raised; end
