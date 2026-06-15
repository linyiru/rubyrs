# Singleton — the stdlib mixin giving a class exactly one instance,
# reached via `.instance` (and a privatised `.new`). rubyrs previously
# materialised an EMPTY `Singleton` module on `require "singleton"`, so
# `include Singleton` added nothing and `.instance` raised NoMethodError.
# Discovery: rake/early_time.rb (`class EarlyTime; include Singleton`).
#
# Mirrors CRuby's: `included` extends the class with an `instance` class
# method memoising a single `new` in a class ivar, and privatises `new`.
module Singleton
  module SingletonClassMethods
    def instance
      @singleton__instance__ ||= new
    end
  end

  def self.included(klass)
    klass.extend(SingletonClassMethods)
    # CRuby privatises `new` so the only entry point is `.instance`.
    klass.private_class_method(:new)
  end

  # A singleton can't be copied — CRuby raises TypeError on dup/clone.
  def dup
    raise TypeError, "can't dup instance of singleton #{self.class}"
  end

  def clone(freeze: true)
    raise TypeError, "can't clone instance of singleton #{self.class}"
  end
end
