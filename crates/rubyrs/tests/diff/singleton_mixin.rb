# `include Singleton` gives a class exactly one instance via `.instance`
# (was a no-op stub → NoMethodError). rake/early_time.rb relies on it.
require "singleton"
class Only
  include Singleton
  attr_accessor :n
  def label; "only:#{n}"; end
end
a = Only.instance
b = Only.instance
p a.equal?(b)          # true — memoized single instance
a.n = 7
p b.n                  # 7 — same object
p a.label              # "only:7"
p a.is_a?(Only)        # true
p (a.dup rescue $!.class)    # TypeError — a singleton can't be copied
# Comparable + Singleton together (EarlyTime's shape)
class Earliest
  include Comparable
  include Singleton
  def <=>(_o); -1; end
end
p (Earliest.instance < 999)   # true
