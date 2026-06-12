# String-form class_eval runs in the RECEIVER'S class context:
# self = cls, def installs onto cls, last expression is the return
# value, nothing leaks to toplevel. minitest's infect_an_assertion
# defines every must_*/wont_* expectation this way.
class Target; end
Target.class_eval("def hello; :from_ce; end")
p Target.new.hello
p (defined?(hello) ? "leaked" : "clean")
p Target.class_eval("self") == Target
p Target.class_eval("1 + 2")
# def inside lands per-receiver, not last-write-wins-globally
class A1; end
class B1; end
A1.class_eval("def who; :a; end")
B1.class_eval("def who; :b; end")
p [A1.new.who, B1.new.who]
# Regexp equality + matcher-table include? (register_spec_type)
p(/ab/ == /ab/)
p(/ab/ == /ab/i)
p [[//, String]].include?([//, String])
# anonymous-class instance inspect nests the class display
c = Class.new
puts c.new.inspect.gsub(/0x[0-9a-f]+/, "0xN")
