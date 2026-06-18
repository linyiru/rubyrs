# Module#included_modules — modules in the ancestor chain (classes
# excluded). Sinatra base_test asserts included_modules.include?(M).
module Greet; def hi; "hi"; end; end
module Bye; def bye; "bye"; end; end
class Base; include Greet; end
class Sub < Base; include Bye; end
p Sub.included_modules.include?(Greet)
p Sub.included_modules.include?(Bye)
p Base.included_modules.include?(Bye)
p Sub.included_modules.all? { |m| m.instance_of?(Module) }
p Sub.respond_to?(:included_modules)
