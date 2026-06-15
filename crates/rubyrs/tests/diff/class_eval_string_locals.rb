# String-form `class_eval` / `module_eval` evaluates with the CALLER's
# local binding — bare identifiers in the string resolve to the enclosing
# method's locals, while `def` still installs onto the receiver class.
# Surfaced by faraday's Options.memoized:
#   class_eval("remove_method(key) if method_defined?(key, false)
#               def #{key}() self[:#{key}]; end")
class Builder
  def self.add(name, value)
    class_eval <<-RUBY
      def #{name}
        # `value` is the caller's method local, captured by class_eval.
        #{value.inspect}
      end
    RUBY
  end
end

class Target < Builder
  add(:greeting, "hello")
  add(:count, 7)
end

t = Target.new
p t.greeting
p t.count

# module_eval string form, same capture; bare local reference + interpolation.
module M
  def self.wire(key)
    module_eval <<-RUBY
      def #{key}_present?
        respond_to?(:#{key})
      end
      KEY_NAME = #{key.inspect}
    RUBY
  end
  wire(:token)
end
p M::KEY_NAME
