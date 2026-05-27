# `Class#instance_method(name)` accepts both Symbol and String.
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:489`
#
#   method = TOPOBJECT.instance_method(method_name)
#
# `method_name` is the synthesised compiled-template name —
# `"__tilt_#{Thread.current.object_id.abs}"` — a String, not a
# Symbol. Without the String overload, tilt's `evaluate` fails
# with NoMethodError even though the Symbol form works.
#
# Coverage:
#   - Symbol form returns UnboundMethod (pre-existing)
#   - String form returns UnboundMethod (this PR)
#   - Missing-method on user class raises NameError (both shapes)
#   - Primitive-class (e.g. `String`) accepts both Symbol and
#     String arg shapes for a real method like `:length` — locked
#     here to confirm the new String overload doesn't regress the
#     existing primitive-receiver path.
#   - Module receiver also accepts both shapes (tilt's TOPOBJECT
#     is a Module — `module CompiledTemplates`)

class Foo
  def bar; "from-bar"; end
end

# Symbol form (pre-existing) ---
puts Foo.instance_method(:bar).class         # UnboundMethod

# String form (this PR) ---
puts Foo.instance_method("bar").class        # UnboundMethod

# Both forms produce equivalent handles (bind + call) ---
m_sym = Foo.instance_method(:bar)
m_str = Foo.instance_method("bar")
puts m_sym.bind(Foo.new).call                # from-bar
puts m_str.bind(Foo.new).call                # from-bar

# Missing method on a user class raises NameError ---
begin
  Foo.instance_method("nonexistent")
rescue NameError
  puts "missing(str) → NameError"
end
begin
  Foo.instance_method(:nonexistent)
rescue NameError
  puts "missing(sym) → NameError"
end

# Primitive-class receiver (`String`) accepts both Symbol and
# String arg shapes for an existing method ---
puts String.instance_method(:length).class    # UnboundMethod
puts String.instance_method("length").class   # UnboundMethod

# Module receiver also works (tilt's TOPOBJECT is a Module) ---
module CompiledLike
  def render; "rendered"; end
end
puts CompiledLike.instance_method("render").class  # UnboundMethod
