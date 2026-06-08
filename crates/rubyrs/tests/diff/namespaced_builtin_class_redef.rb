# Regression guard: a namespaced redefinition of a built-in class name
# must NOT clobber the global built-in's identity / rescue behaviour.
#
# Liquid does exactly this (lib/liquid/errors.rb):
#   module Liquid
#     ArgumentError    = Class.new(Error)
#     StandardError    = Class.new(Error)
#     ZeroDivisionError = Class.new(Error)
#   end
# The anon classes are `Liquid::ArgumentError` etc. — they must shadow
# ONLY inside Liquid, never replace the core `::ArgumentError` that
# `rescue StandardError` relies on. A bug where naming the anon class
# overwrote the core class in the global registry broke the real Jekyll
# build (every `rescue StandardError` stopped matching core errors).

module Ns
  Error = Class.new(StandardError)
  ArgumentError = Class.new(Error)
  StandardError = Class.new(Error)
  ZeroDivisionError = Class.new(Error)
end

# Core built-ins are untouched: a core ArgumentError is still a
# StandardError and is caught by a bare `rescue StandardError`.
def kaboom(o, k: 1); o; end
result =
  begin
    raise ArgumentError, "core arg error"
  rescue StandardError => e
    "rescued #{e.class}"
  end
p result                                  # "rescued ArgumentError"

# Core hierarchy intact.
p ArgumentError.ancestors.include?(StandardError)   # true
p ZeroDivisionError.ancestors.include?(StandardError) # true

# The namespaced versions are distinct classes in their own hierarchy.
p Ns::ArgumentError.ancestors.include?(Ns::Error)   # true
p Ns::ArgumentError.equal?(ArgumentError)           # false
p Ns::StandardError.equal?(StandardError)           # false

# Raising/rescuing the namespaced one works on its own terms.
r2 =
  begin
    raise Ns::ArgumentError, "ns"
  rescue Ns::Error => e
    "ns rescued #{e.message}"
  end
p r2                                      # "ns rescued ns"
