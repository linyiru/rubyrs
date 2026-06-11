# CRuby String-passthrough contract for to_s-shaped output paths
# (probed vs ruby 3.4):
#   - interpolation of a part that is ALREADY a String never calls
#     to_s (rb_obj_as_string returns T_STRING as-is) — a user
#     String#to_s override must NOT leak into "#{str}"
#   - puts / print write String args directly (rb_io_puts
#     T_STRING short-circuit) — same: no user to_s consulted
#   - p still dispatches inspect, so a user String#inspect IS
#     honored
#   - non-String interpolation parts DO dispatch to_s (a plain
#     Object's user to_s is honored, and the surrounding literal
#     parts must survive the dispatch)

class Foo
  def to_s
    "FOO"
  end
end
f = Foo.new
puts "pre: #{f} post"
p "a#{f}b#{f}c"

class String
  def to_s
    "STR"
  end
end
s = "abc"
puts "interp: #{s}"          # no to_s on String parts
p "#{s}"
puts s                       # direct write
print s, "\n"                # direct write
puts ["x", "y"]              # array elements: still direct per-line

class String
  def inspect
    "INSP"
  end
end
p "z"                        # p dispatches inspect -> override wins
