# Minimal `Struct` builtin — `Struct.new(:a, :b, ...)` returns a
# new Class whose instances have positional initializer
# `MyStruct.new(1, 2)` and reader/writer accessors for each
# named slot. Pre-shim, mustermann's
# `mustermann/ast/transformer.rb:80`
#   Operator = Struct.new(:separator, :allow_reserved, :prefix,
#                         :parametric)
# raised `NameError: uninitialized constant Struct`.
#
# Surface covered (the union actually called by mustermann +
# sinatra + their dependency chain):
#   * Struct.new(*attr_names) → fresh Class
#   * The returned class's `.new(*values)` initialises
#     positionally
#   * Per-slot reader methods (`obj.attr`) and writer methods
#     (`obj.attr=val`) on the instance
#   * `.members` returning the attribute name list (both
#     class- and instance-method shape)
#   * `.to_a` returning the values in declaration order
#   * `==` comparing same-class structs by their .to_a
#
# Documented divergences (intentionally not implemented —
# none surface in the spike chain):
#   * `keyword_init: true` shape
#   * Block form `Struct.new(:a) { def helper; end }`
#   * `Struct.new("Name", ...)` named-class form (CRuby
#     registers under top-level `Struct::Name`)
#   * `[]` / `[]=` index access by attr name
#   * `each` / `each_pair` / Enumerable mixin
#   * STRESS_GC: `Struct.new`-created classes have a
#     pre-existing root hole in rubyrs's define_method-with-
#     class-ivars-closure path; under STRESS_GC the captured
#     class's ivars table can be swept mid-dispatch. Normal
#     mode is unaffected (the Sinatra spike load this Struct
#     shim unblocks doesn't trigger STRESS_GC sweep windows).

class Struct
  def self.new(*attrs)
    cls = Class.new
    # Store attrs on the class itself as a class-level ivar
    # so the captured-in-block reference survives GC. Pure
    # block-captures of the `attrs` Array were getting swept
    # under STRESS_GC (rubyrs's block-locals capture path
    # doesn't yet GC-root captured heap values — separate
    # gap surfaced by this fixture). Routing through
    # `self.class.instance_variable_get(:@__struct_attrs)`
    # keeps the Array rooted via the Class ivars table.
    cls.instance_variable_set(:@__struct_attrs, attrs)
    cls.define_singleton_method(:members) do
      # Explicit `self.` — bare `instance_variable_get`
      # doesn't reach the universal Object arm under method
      # dispatch in rubyrs (the same gap workaround
      # Forwardable / Delegate shims use). Self here is the
      # Struct subclass (a Value::Class).
      self.instance_variable_get(:@__struct_attrs)
    end
    cls.define_method(:members) do
      self.class.instance_variable_get(:@__struct_attrs)
    end
    cls.define_method(:initialize) do |*args|
      members.each_with_index do |attr, i|
        instance_variable_set("@#{attr}".to_sym, args[i])
      end
    end
    attrs.each do |attr|
      ivar = "@#{attr}".to_sym
      writer = "#{attr}=".to_sym
      cls.define_method(attr) { instance_variable_get(ivar) }
      cls.define_method(writer) { |v| instance_variable_set(ivar, v) }
    end
    cls.define_method(:to_a) do
      members.map { |a| instance_variable_get("@#{a}".to_sym) }
    end
    cls.define_method(:==) do |other|
      # CRuby's `Struct#==` requires EXACT class match (`==`),
      # not `is_a?` — otherwise `parent_struct == child_struct`
      # would be asymmetric (parent.is_a?(parent) succeeds but
      # child.is_a?(child) only matches its own class). Mirror
      # the exact-class semantics so `==` is reflexive AND
      # symmetric across Struct subclass inheritance.
      other.class == self.class && self.to_a == other.to_a
    end
    cls
  end
end
