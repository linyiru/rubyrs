# `Class.class_eval { alias x inherited }` — CRuby ships real empty
# hook defaults; aliasing them must work, and an aliased-in override
# must fire on subclass creation (minitest's with_overridden_include
# save/restore cycle).
Class.class_eval do
  def inherited_with_hacks(_k)
    throw :inherited_hook
  end
  alias inherited_without_hacks inherited
  alias inherited inherited_with_hacks
end
caught = catch(:inherited_hook) do
  Class.new(Object)
  :not_thrown
end
p caught
Class.class_eval do
  alias inherited inherited_without_hacks
  undef_method :inherited_with_hacks
  undef_method :inherited_without_hacks
end
p Class.new(Object).is_a?(Class)
p Class.respond_to?(:inherited_with_hacks)
p Object.respond_to?(:inherited_without_hacks)
