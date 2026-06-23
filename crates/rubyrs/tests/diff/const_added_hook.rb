# Module#const_added (CRuby 3.2+) fires when a class/module constant is
# defined on a module — via `extend` (singleton) or `Module.prepend`
# (instance). zeitwerk prepends a const_added to register a namespace's
# child autoloads the moment the namespace appears. (rubyrs fires it for
# class/module definitions; plain-constant assignment is not covered.)
module Tracker
  def const_added(name); (@seen ||= []) << name; super; end
  def seen; @seen || []; end
end

module HostA
  extend Tracker            # singleton-method form
  class Foo; end
  module Bar; end
  class Foo; end            # reopen: does NOT re-fire
end
p HostA.seen                # [:Foo, :Bar]

$log = []
module ConstAddedAll
  def const_added(name); $log << "#{self}::#{name}"; super; end
end
Module.prepend(ConstAddedAll)   # instance-method form (zeitwerk shape)
module Outer
  class Inner; end
  module Mid; end
end
p $log                      # ["Object::Outer", "Outer::Inner", "Outer::Mid"]
