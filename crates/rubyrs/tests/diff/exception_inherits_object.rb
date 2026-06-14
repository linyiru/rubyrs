# Exception must descend from Object (CRuby: Exception < Object, and
# the chain continues Object -> Kernel -> BasicObject). rubyrs builds
# the exception hierarchy in the preamble BEFORE Object exists, so
# `class Exception` was created as a root; it is re-parented onto Object
# once Object is defined. Exception INSTANCES therefore resolve
# Ruby-level Object/Kernel methods — not just the VM-special-cased
# natives — including methods mixed into Object afterwards (which is how
# minitest installs its `must_*` expectations).

p Exception.superclass                 # Object
p Exception.ancestors                  # [Exception, Object, Kernel, BasicObject]
p StandardError.ancestors              # [StandardError, Exception, Object, Kernel, BasicObject]
p RuntimeError.new("x").is_a?(Object)  # true

# a method added to Object after the fact resolves on an exception
class Object; def __probe_marker; :seen; end; end
p StandardError.new.__probe_marker     # :seen
p (begin; raise "boom"; rescue => e; e.__probe_marker; end)   # :seen

# a module mixed into Object resolves on an exception instance
module ProbeMixin; def __mixin_marker; :mixed; end; end
class Object; include ProbeMixin; end
p RuntimeError.new.__mixin_marker      # :mixed

# Exception-specific methods still take precedence over Object's
p RuntimeError.new("hi").message       # "hi"
