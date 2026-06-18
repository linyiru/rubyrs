# Subclassing String: instances carry String content + methods, report
# the subclass, run String#initialize, hold ivars, and user methods
# override String primitives.
class Tagged < String
  attr_reader :note
  def initialize(raw, note = nil); replace(raw); @note = note; end
  def shout; upcase + "!"; end
  def ==(o); "custom:#{o}"; end
end
t = Tagged.new("hello", "n")
p t.class.name
p t.is_a?(String)
p (Tagged < String)
p t.length
p t.upcase
p t.split("l")
p t[0, 3]
p t.note
p t.shout
p (t == "x")                 # user override wins
p t.reverse
p t + " world"
p t.instance_variables.sort

# No custom initialize: String#initialize copies the arg.
class Plain < String; end
pl = Plain.new("abc")
p [pl.class.name, pl.length, pl.upcase]
p Plain.new.length            # zero-arg → empty

# A module mixed in below String overrides too.
module Yell; def upcase; "YELLED"; end; end
class Loud < String; include Yell; end
p Loud.new("hi").upcase

# Plain string still behaves (no tag, fast path unaffected).
p "plain".upcase
p ("a" == "a")
