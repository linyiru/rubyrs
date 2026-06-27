# `super` from a class method defined in an EXTENDED module reaches Object's
# instance method — a class object is an instance of Class (< Module < Object),
# so its class-method super-chain continues into that metaclass-ancestry tail.
# ActiveRecord's DynamicMatchers#respond_to_missing? (extended onto a model
# class) calls `super` exactly this way.
module DM
  def respond_to_missing?(name, include_private = false)
    name == :special ? true : super
  end
end
class D
  extend DM
end
p D.respond_to?(:special)            # DM override → true
p D.respond_to?(:definitely_not)     # super → Object#respond_to_missing? → false
p D.send(:respond_to_missing?, :x)   # direct super → false
