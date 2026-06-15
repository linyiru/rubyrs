# Module#remove_const on a PENDING autoload (registered via
# Module#autoload but never triggered) removes the autoload and
# returns nil, like CRuby — it does NOT raise NameError. zeitwerk's
# reload/unload teardown removes autoload constants this way.
# (remove_const is private, so call via send, as zeitwerk does with
# __send__.)
$LOAD_PATH.unshift("/tmp/rcfix")

# Toplevel pending autoload → removed, returns nil.
Object.autoload(:TopThing, "/tmp/rcfix/top_thing.rb")
p Object.send(:remove_const, :TopThing)

# Scoped pending autoload on a named owner → removed, returns nil.
module Outer; end
Outer.autoload(:Inner, "/tmp/rcfix/inner.rb")
p Outer.send(:remove_const, :Inner)

# Still raises NameError for a genuinely undefined constant.
begin
  Object.send(:remove_const, :NeverEverDefined)
rescue NameError
  p :name_error
end

# A real (loaded) constant removal still works + returns the value.
FOO = 42
p Object.send(:remove_const, :FOO)
