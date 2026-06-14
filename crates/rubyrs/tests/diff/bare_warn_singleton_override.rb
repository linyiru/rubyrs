# A bareword `warn(...)` (implicit self) inside an instance method must
# dispatch to self's OWN `warn` override — a per-instance singleton
# method OR an instance method on the class — before falling to
# Kernel#warn. CRuby runs normal method lookup first. rack's request
# spec captures deprecation warnings by `obj.define_singleton_method(
# :warn)` while `Request#values_at` calls bare `warn(msg, uplevel: 1)`.

class Widget
  def emit; warn("deprecated thing", uplevel: 1); end
end

# (1) per-instance singleton override
w = Widget.new
captured = []
w.define_singleton_method(:warn) { |*args| captured << args }
w.emit
p captured                       # [["deprecated thing", {uplevel: 1}]]

# (2) instance-method override on the class
class Talker
  attr_reader :log
  def initialize; @log = []; end
  def warn(*args); @log << args; end
  def speak; warn("hi", category: :deprecated); end
end
t = Talker.new
t.speak
p t.log                          # [["hi", {category: :deprecated}]]

# (3) no override → Kernel#warn (goes to STDERR; STDOUT stays clean)
class Plain
  def go; warn("to stderr"); end
end
Plain.new.go
puts "after"                     # only this on STDOUT
