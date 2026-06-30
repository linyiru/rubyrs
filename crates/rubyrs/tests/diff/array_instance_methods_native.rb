own = Array.instance_methods - Object.instance_methods - [:to_a]
p own.include?(:empty?)
p own.include?(:size)
p own.include?(:length)
p own.include?(:map)
p own.include?(:[])
p own.include?(:fetch)

class NativeArrayMethodsSubclass < Array
end

sub = NativeArrayMethodsSubclass.instance_methods
p sub.include?(:empty?)
p sub.include?(:size)
p sub.include?(:length)
