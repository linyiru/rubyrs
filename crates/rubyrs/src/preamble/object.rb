# Object — CRuby's universal ancestor. Real CRuby has every
# value inheriting from Object → BasicObject. We don't model the
# full chain (primitives have no parent class in our model; user
# classes default to no superclass). But user code reaches for
# `Object` as a sentinel: `Object.new` for an anonymous receiver
# (tilt's default render scope), `class Foo < Object` for
# explicit-root inheritance, `is_a?(Object)` for the universal
# predicate. An empty stub class makes those bare references
# resolve without redirecting every primitive's class chain
# through it.
#
# Loaded by `Runtime::load_preamble` before the remaining built-in
# class stubs and mixins so any subsequent `class X < Object`
# resolves immediately.

class Object
end
