# `mutex_m` — the `Mutex_m` mixin that adds `lock`/`unlock`/`synchronize`/…
# to any object or class, backed by a `Mutex`. A Ruby default gem that
# Ruby 3.4 dropped from the always-loaded stdlib, so `require "mutex_m"`
# must resolve to this. ActiveSupport 7.0's `Notifications::Fanout` is the
# discovery consumer (`require "mutex_m"`). Single-threaded-model: the
# backing `Mutex` is rubyrs's own (synchronize yields the block; lock /
# unlock / locked? track a flag) — see stdlib_vendor/monitor.rb for the
# same model.
module Mutex_m
  def self.define_aliases(cls)
    cls.send :alias_method, :locked?, :mu_locked?
    cls.send :alias_method, :lock, :mu_lock
    cls.send :alias_method, :unlock, :mu_unlock
    cls.send :alias_method, :try_lock, :mu_try_lock
    cls.send :alias_method, :synchronize, :mu_synchronize
  end

  def self.append_features(cls)
    super
    define_aliases(cls) unless cls.instance_of?(Module)
  end

  def self.extend_object(obj)
    super
    obj.mu_extended
  end

  def mu_extended
    unless (defined? @mu_locked) && @mu_locked
      Mutex_m.define_aliases(singleton_class)
      @mu_locked = false
      @_mutex = Mutex.new
    end
    self
  end

  def mu_synchronize(&block)
    (@_mutex ||= Mutex.new).synchronize(&block)
  end

  def mu_locked?
    (@_mutex ||= Mutex.new).locked?
  end

  def mu_try_lock
    (@_mutex ||= Mutex.new).try_lock
  end

  def mu_lock
    (@_mutex ||= Mutex.new).lock
  end

  def mu_unlock
    (@_mutex ||= Mutex.new).unlock
  end

  private

  def initialize(*args)
    @_mutex = Mutex.new
    super
  end
end
