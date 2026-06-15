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
  def self.new(*attrs, &block)
    # `keyword_init: true/false` arrives as a trailing options Hash in
    # the splat (Struct.new has no kwparams). Peel it off the attr list.
    opts = attrs.last.is_a?(Hash) ? attrs.pop : {}
    kw_init = opts[:keyword_init]
    # When `self` is a Struct SUBCLASS used as a factory
    # (`class Options < Struct; Options.new(:uri) do … end; end` —
    # faraday's Options), the generated class must INHERIT `self` so it
    # picks up the subclass's own class methods (`Options.options_method`)
    # and instance methods. CRuby:
    # `Options.new(:uri).ancestors.include?(Options)` is true. Plain
    # `Struct.new(:a)` keeps the flat parent-Object shape unchanged.
    is_subclass_factory = !equal?(Struct)
    cls = is_subclass_factory ? Class.new(self) : Class.new
    # Store attrs on the class itself as a class-level ivar
    # so the captured-in-block reference survives GC. Pure
    # block-captures of the `attrs` Array were getting swept
    # under STRESS_GC (rubyrs's block-locals capture path
    # doesn't yet GC-root captured heap values — separate
    # gap surfaced by this fixture). Routing through
    # `self.class.instance_variable_get(:@__struct_attrs)`
    # keeps the Array rooted via the Class ivars table.
    cls.instance_variable_set(:@__struct_attrs, attrs)
    cls.instance_variable_set(:@__struct_kw, kw_init)
    # Both readers WALK the superclass chain: subclasses of the
    # generated class (rack's `class BufferPart < MimePart` where
    # MimePart = Struct.new(...)) don't inherit class-level ivars,
    # so the tables live on whichever ancestor Struct.new built.
    # (Explicit `self.` — bare `instance_variable_get` doesn't
    # reach the universal Object arm under method dispatch in
    # rubyrs; same workaround as the Forwardable shim.)
    cls.define_singleton_method(:members) do
      k = self
      a = nil
      while k && (a = k.instance_variable_get(:@__struct_attrs)).nil?
        k = k.superclass
      end
      a
    end
    cls.define_method(:members) do
      k = self.class
      a = nil
      while k && (a = k.instance_variable_get(:@__struct_attrs)).nil?
        k = k.superclass
      end
      a
    end
    cls.define_method(:initialize) do |*args|
      kw = nil
      k = self.class
      while k && (kw = k.instance_variable_get(:@__struct_kw)).nil?
        k = k.superclass
      end
      if kw
        # `keyword_init: true` — `S.new(a: 1, b: 2)` passes the kwargs
        # as a trailing Hash (rubyrs routes them positionally to a
        # `*args` callee). Read each member out of it; absent → nil.
        h = args.first.is_a?(Hash) ? args.first : {}
        members.each { |attr| instance_variable_set("@#{attr}".to_sym, h[attr]) }
      else
        members.each_with_index do |attr, i|
          instance_variable_set("@#{attr}".to_sym, args[i])
        end
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
    cls.define_method(:to_h) do
      h = {}
      members.each { |a| h[a] = instance_variable_get("@#{a}".to_sym) }
      h
    end
    cls.define_method(:each) do |&blk|
      to_a.each(&blk)
    end
    cls.define_method(:[]) do |key|
      # `s[:attr]` / `s["attr"]` / `s[index]`.
      if key.is_a?(Integer)
        to_a[key]
      else
        instance_variable_get("@#{key}".to_sym)
      end
    end
    cls.define_method(:[]=) do |key, val|
      name = key.is_a?(Integer) ? members[key] : key
      instance_variable_set("@#{name}".to_sym, val)
    end
    cls.define_method(:values_at) do |*idxs|
      # Int indices (negative from end) and Ranges, like Array#values_at.
      vals = to_a
      out = []
      idxs.each do |ix|
        if ix.is_a?(Range)
          sub = vals[ix]
          out.concat(sub) if sub
        else
          out << vals[ix]
        end
      end
      out
    end
    cls.define_method(:dig) do |key, *rest|
      v = self[key]
      rest.empty? || v.nil? ? v : v.dig(*rest)
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
    cls.define_method(:inspect) do
      pairs = members.map { |a| "#{a}=#{instance_variable_get("@#{a}".to_sym).inspect}" }
      nm = self.class.name
      nm ? "#<struct #{nm} #{pairs.join(', ')}>" : "#<struct #{pairs.join(', ')}>"
    end
    cls.define_method(:to_s) { inspect }
    # Double-new: with `cls < Options < Struct`, an un-shadowed
    # `cls.new(values)` would re-resolve `new` up the chain to Struct's
    # FACTORY `self.new` and try to build YET ANOTHER subclass (treating
    # the values as member names). Shadow it with an instance builder so
    # `cls.new(*values)` allocates + initializes — the same effect the
    # flat parent-Object generated class gets for free from Object#new.
    if is_subclass_factory
      cls.define_singleton_method(:new) do |*a, &b|
        obj = allocate
        obj.send(:initialize, *a, &b)
        obj
      end
    end
    # Block form: `Struct.new(:a) { def helper; …; end }` — evaluate the
    # block in the new class so it can define methods / constants.
    cls.class_eval(&block) if block
    cls
  end
end

# Ruby 3.2 `Data` — immutable value objects. `Data.define(:x, :y)`
# returns a class whose instances take positional (`D.new(1, 2)`) OR
# keyword (`D.new(x: 1, y: 2)`) args, expose readers only (no writers —
# immutable), and support `with` (copy-with-changes), `to_h`, `==`,
# `members`, pattern-matching (`deconstruct` / `deconstruct_keys`), and
# `inspect` (`#<data D x=1, y=2>`). Lives here alongside Struct since the
# two share the value-object-factory shape. (Like Struct, `p data` prints
# the native `#<...>` until Kernel#p routes to a user `inspect`.)
class Data
  def self.define(*members, &block)
    cls = Class.new
    cls.instance_variable_set(:@__data_members, members)
    cls.define_singleton_method(:members) do
      self.instance_variable_get(:@__data_members)
    end
    cls.define_method(:members) do
      self.class.instance_variable_get(:@__data_members)
    end
    cls.define_method(:initialize) do |*args|
      m = members
      # Keyword init: a single Hash arg whose keys are all members
      # (rubyrs routes kwargs to a `*args` callee as a trailing Hash).
      # Otherwise positional. The all-members-keys test distinguishes
      # `D1.new(v: 5)` (keyword) from `D1.new({a: 1})` (positional value)
      # for a 1-member Data.
      if args.size == 1 && args.first.is_a?(Hash) && !args.first.empty? &&
         args.first.keys.all? { |k| m.include?(k) }
        h = args.first
        m.each { |a| instance_variable_set("@#{a}".to_sym, h[a]) }
      else
        m.each_with_index { |a, i| instance_variable_set("@#{a}".to_sym, args[i]) }
      end
    end
    members.each do |a|
      ivar = "@#{a}".to_sym
      cls.define_method(a) { instance_variable_get(ivar) } # reader only
    end
    cls.define_method(:to_h) do
      h = {}
      members.each { |a| h[a] = instance_variable_get("@#{a}".to_sym) }
      h
    end
    cls.define_method(:with) do |*args|
      changes = args.first.is_a?(Hash) ? args.first : {}
      # Build positionally (current value, or the change) — avoids the
      # keyword/positional ambiguity for 1-member Data.
      vals = members.map do |a|
        changes.key?(a) ? changes[a] : instance_variable_get("@#{a}".to_sym)
      end
      self.class.new(*vals)
    end
    cls.define_method(:deconstruct) do
      members.map { |a| instance_variable_get("@#{a}".to_sym) }
    end
    cls.define_method(:deconstruct_keys) do |keys|
      to_h
    end
    cls.define_method(:==) do |other|
      other.class == self.class && other.to_h == self.to_h
    end
    cls.define_method(:inspect) do
      pairs = members.map { |a| "#{a}=#{instance_variable_get("@#{a}".to_sym).inspect}" }
      nm = self.class.name
      nm ? "#<data #{nm} #{pairs.join(', ')}>" : "#<data #{pairs.join(', ')}>"
    end
    cls.define_method(:to_s) { inspect }
    cls.class_eval(&block) if block
    cls
  end
end
