# Minimal `Forwardable` + `SingleForwardable` shim. Sinatra,
# Mustermann, and Rack all `extend Forwardable` and then call
# `def_delegators :receiver, *methods` from class bodies to
# build delegation surfaces. Pre-shim those required-at-load-
# time calls hit `NoMethodError: undefined method
# 'def_delegators' for Class`.
#
# Surface covered (the union actually called by mustermann +
# rack-3.1.10 + sinatra-4.2.1):
#   * Forwardable#def_delegators(accessor, *methods)
#   * Forwardable#def_delegator(accessor, method, ali = method)
#   * SingleForwardable#single_delegate(hash) — kwarg form
#     where every (method_name, accessor) pair defines a
#     delegate method ON THE SINGLETON. Mustermann's
#     `single_delegate on: :parser, suffix: :parser` shape.
#   * SingleForwardable#def_single_delegator(accessor, method,
#     ali = method)
#   * SingleForwardable aliases delegate / def_delegators /
#     def_delegator (matches CRuby).
#
# Idempotency guard: the kernel-side `is_stdlib_stub_name`
# preamble creates empty Forwardable / SingleForwardable
# modules at `require "forwardable"` time; this shim reopens
# those modules to install the method bodies.

module Forwardable
  # @!visibility private
  def def_delegators(accessor, *methods)
    methods.each { |m| def_delegator(accessor, m) }
  end

  # @!visibility private
  def def_delegator(accessor, method, ali = method)
    # CRuby's delegator captures `accessor` as a Symbol naming
    # either an ivar (`:@x`) or a reader method (`:foo`). The
    # generated body resolves the receiver freshly per call —
    # so subclasses that override the reader see the new
    # value, and ivar-named accessors reflect mutation.
    accessor_str = accessor.to_s
    is_ivar = accessor_str.start_with?("@")
    # A dotted accessor (`'self.class'`) is a RECEIVER EXPRESSION,
    # not a single method name — CRuby's Forwardable splices it
    # verbatim into the generated body. mustermann's
    # `instance_delegate [...] => 'self.class'` (ast/pattern.rb:23)
    # is the canonical (and only real-world) caller.
    #
    # GC discipline: classify the accessor shape OUT HERE, where
    # `accessor`/`accessor_str` are fresh locals, and capture only
    # the resulting BOOL into the define_method closure. Calling a
    # method on a captured heap String inside the block (e.g.
    # `accessor.split`) tripped `ICE: class_of on non-Object slot`
    # under STRESS_GC — the closure doesn't mark its captured
    # heap-String, so it gets swept mid-call. `__send__(accessor)`
    # / `instance_variable_get(accessor)` stay safe (they don't
    # `class_of` the symbol-ish accessor).
    is_self_class = (accessor_str == "self.class")
    define_method(ali) do |*args, &blk|
      target =
        if is_ivar
          instance_variable_get(accessor)
        elsif is_self_class
          self.class
        else
          __send__(accessor)
        end
      # Trailing kwargs would normally split via `**kw` but
      # rubyrs's block params don't yet bind `**kw` (probe:
      # the trailing hash stays in `*args`). For the
      # gem-load surface (mustermann/sinatra/rack delegate
      # to simple arity-0 / arity-1 methods like `:eos?`,
      # `:size`, `:[]`) the positional forwarding suffices.
      # Kwargs-passing delegates would need a separate fix.
      target.__send__(method, *args, &blk)
    end
  end

  # CRuby's `instance_delegate(hash)` form is the hash-shaped
  # cousin of `def_delegators`. Same iteration shape as
  # SingleForwardable#single_delegate below — for each
  # (methods, accessor) pair, define one or many delegates.
  # Not hit by mustermann/sinatra at load time but listed for
  # completeness so a future caller doesn't NoMethodError.
  def instance_delegate(hash)
    hash.each do |methods, accessor|
      if methods.respond_to?(:each) && !methods.is_a?(Symbol) && !methods.is_a?(String)
        methods.each { |m| def_delegator(accessor, m) }
      else
        def_delegator(accessor, methods)
      end
    end
  end
end

module SingleForwardable
  # @!visibility private
  def def_single_delegator(accessor, method, ali = method)
    # `accessor` is a Symbol-named singleton method (or an ivar
    # on the singleton). The defined method lives on the
    # singleton class (this is what makes it "single" delegate
    # — the receiver doing the extending is THE object that
    # receives the new method, not its class's instances).
    accessor_str = accessor.to_s
    is_ivar = accessor_str.start_with?("@")
    singleton_class.define_method(ali) do |*args, &blk|
      # Same `self.`-routing and `**kw` block-param caveat
      # as Forwardable#def_delegator above. Mustermann's
      # `single_delegate on: :parser, suffix: :parser`
      # delegates to arity-0 methods (`parser.on`,
      # `parser.suffix`), so positional-only forwarding
      # covers the gem-load surface.
      target =
        if is_ivar
          instance_variable_get(accessor)
        else
          __send__(accessor)
        end
      target.__send__(method, *args, &blk)
    end
  end

  # @!visibility private
  def def_single_delegators(accessor, *methods)
    methods.each { |m| def_single_delegator(accessor, m) }
  end

  # CRuby's hash-shaped form: each (methods, accessor) pair
  # builds delegate(s). Mustermann's
  # `single_delegate on: :parser, suffix: :parser` calls this
  # with `{on: :parser, suffix: :parser}` — for each entry,
  # the KEY names the delegate method and the VALUE names the
  # accessor (note the reversal vs the positional shape).
  def single_delegate(hash)
    hash.each do |methods, accessor|
      if methods.respond_to?(:each) && !methods.is_a?(Symbol) && !methods.is_a?(String)
        methods.each { |m| def_single_delegator(accessor, m) }
      else
        def_single_delegator(accessor, methods)
      end
    end
  end

  alias_method :delegate, :single_delegate
  alias_method :def_delegators, :def_single_delegators
  alias_method :def_delegator, :def_single_delegator
end
