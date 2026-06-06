# Bare-receiver method calls at `class << self` body top level now
# EXECUTE (instead of deferring a NotImplementedError). `extend M`
# makes M's methods reachable for a following bare call in the body.
# Discovery: P3 Jekyll spike — addressable's idna does `class << self;
# def x; end; extend Gem::Deprecate; deprecate :x, …; end`; the
# `deprecate` no-op must run for the require to complete.
#
# NOTE: only the in-body call's *output* is asserted — that's
# parity-safe. rubyrs runs the body with self = the enclosing class
# (CRuby uses the eigenclass), a documented Tier-1 divergence, so we
# do NOT assert where `extend` ultimately installs the methods.

module Noter
  def note(x); puts "noted: #{x}"; end
end

class Widget
  class << self
    def build; "built"; end
    extend Noter
    note "first"
    note "second"
  end
end
puts Widget.build

# bare call that takes no module (a plain Kernel-ish call): `puts`
# inside class << self runs in the surrounding context.
class Logger2
  class << self
    puts "configuring"
    def level; :info; end
  end
end
p Logger2.level
