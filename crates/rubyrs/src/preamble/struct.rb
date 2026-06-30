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
  # Base-class `[]` / `[]=` so a subclass-factory's own override
  # (`class Options < Struct; def [](k); …; super; end; end`) can reach
  # the native index accessor via `super` — CRuby keeps these on Struct
  # itself, ABOVE any user override. The per-instance `members` is
  # resolved dynamically, so the same body serves every member layout.
  # (Plain `Struct.new` classes don't inherit Struct — they get an
  # equivalent copy generated onto the class in `self.new` below.)
  def [](key)
    if key.is_a?(Integer)
      to_a[key]
    else
      instance_variable_get("@#{key}".to_sym)
    end
  end
  def []=(key, val)
    name = key.is_a?(Integer) ? members[key] : key
    instance_variable_set("@#{name}".to_sym, val)
  end

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
    # Define the member-assigning initialize on an INCLUDED MODULE, not
    # on `cls` directly: a block-form `Struct.new(:a) { def initialize;
    # ...; super; end }` defines the user initialize on `cls`
    # (class_eval'd below), and bare/explicit `super` must reach this
    # generated one. With it on `cls` they'd collide (same class) and
    # super would skip to Object, leaving every member nil. CRuby places
    # the member assigner on the `Struct` superclass for the same reason.
    struct_methods_mod = Module.new
    cls.include(struct_methods_mod)
    struct_methods_mod.define_method(:initialize) do |*args|
      kw = nil
      k = self.class
      while k && (kw = k.instance_variable_get(:@__struct_kw)).nil?
        k = k.superclass
      end
      mem = members
      if kw
        # `keyword_init: true` — `S.new(a: 1, b: 2)` passes the kwargs
        # as a trailing Hash (rubyrs routes them positionally to a
        # `*args` callee). Read each member out of it; absent → nil.
        h = args.first.is_a?(Hash) ? args.first : {}
        mem.each { |attr| instance_variable_set("@#{attr}".to_sym, h[attr]) }
      elsif args.size == 1 && args.first.is_a?(Hash) && !args.first.empty? &&
            args.first.keys.all? { |k| mem.include?(k) }
        # Ruby 3.2+: a DEFAULT (non-keyword_init) Struct ALSO accepts
        # keyword init — `S.new(a: 1, b: 2)`. The kwargs arrive as a
        # trailing Hash whose keys are all members; an explicit hash
        # value for member 0 (`S.new({x: 1})` where x isn't a member)
        # stays positional. Same detection as `Data` below. Surfaced by
        # bridgetown's front-matter `Result.new(content:, front_matter:,
        # line_count:)` (was binding the whole kwargs hash to member 0).
        h = args.first
        mem.each { |attr| instance_variable_set("@#{attr}".to_sym, h[attr]) }
      else
        mem.each_with_index do |attr, i|
          instance_variable_set("@#{attr}".to_sym, args[i])
        end
      end
    end
    # For a subclass factory (`Options.new(:x)` with `class Options <
    # Struct`), the factory base may define its OWN versions of the
    # generic struct methods — faraday's `Options#[]` memoizes via a
    # custom `[]`. Generating those methods onto `cls` (which sits BELOW
    # the base in the ancestry) would SHADOW the user override, so the
    # base's method never runs. Collect the methods the base and its
    # ancestors-above-but-below-Struct define, and skip regenerating any
    # of them. (A plain `Struct.new(:x)` cls is `Class.new` rooted at
    # Object, so nothing is collected and every generic method is
    # generated as before.)
    user_struct_methods = []
    if is_subclass_factory
      ancestors.each do |anc|
        break if anc == Struct
        user_struct_methods.concat(anc.instance_methods(false))
      end
    end
    attrs.each do |attr|
      ivar = "@#{attr}".to_sym
      writer = "#{attr}=".to_sym
      cls.define_method(attr) { instance_variable_get(ivar) } unless user_struct_methods.include?(attr)
      cls.define_method(writer) { |v| instance_variable_set(ivar, v) } unless user_struct_methods.include?(writer)
    end
    cls.define_method(:to_a) do
      members.map { |a| instance_variable_get("@#{a}".to_sym) }
    end unless user_struct_methods.include?(:to_a)
    cls.define_method(:to_h) do
      h = {}
      members.each { |a| h[a] = instance_variable_get("@#{a}".to_sym) }
      h
    end unless user_struct_methods.include?(:to_h)
    cls.define_method(:each) do |&blk|
      to_a.each(&blk)
    end unless user_struct_methods.include?(:each)
    cls.define_method(:[]) do |key|
      # `s[:attr]` / `s["attr"]` / `s[index]`.
      if key.is_a?(Integer)
        to_a[key]
      else
        instance_variable_get("@#{key}".to_sym)
      end
    end unless user_struct_methods.include?(:[])
    cls.define_method(:[]=) do |key, val|
      name = key.is_a?(Integer) ? members[key] : key
      instance_variable_set("@#{name}".to_sym, val)
    end unless user_struct_methods.include?(:[]=)
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
    end unless user_struct_methods.include?(:values_at)
    cls.define_method(:dig) do |key, *rest|
      v = self[key]
      rest.empty? || v.nil? ? v : v.dig(*rest)
    end unless user_struct_methods.include?(:dig)
    cls.define_method(:==) do |other|
      # CRuby's `Struct#==` requires EXACT class match (`==`),
      # not `is_a?` — otherwise `parent_struct == child_struct`
      # would be asymmetric (parent.is_a?(parent) succeeds but
      # child.is_a?(child) only matches its own class). Mirror
      # the exact-class semantics so `==` is reflexive AND
      # symmetric across Struct subclass inheritance.
      other.class == self.class && self.to_a == other.to_a
    end unless user_struct_methods.include?(:==)
    cls.define_method(:inspect) do
      pairs = members.map { |a| "#{a}=#{instance_variable_get("@#{a}".to_sym).inspect}" }
      nm = self.class.name
      nm ? "#<struct #{nm} #{pairs.join(', ')}>" : "#<struct #{pairs.join(', ')}>"
    end unless user_struct_methods.include?(:inspect)
    cls.define_method(:to_s) { inspect } unless user_struct_methods.include?(:to_s)
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
    # Subclass `Data` (CRuby: `Data.define(...).superclass == Data`,
    # so instances are `is_a?(Data)` and `< Data` holds).
    cls = Class.new(self)
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
