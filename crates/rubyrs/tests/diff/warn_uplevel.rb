# Kernel#warn with the uplevel:/category: keywords. Route $stderr to a
# tee that writes to $stdout, so the harness (which compares stdout)
# sees the warnings. The uplevel: prefix is "<path>:<line>: warning:
# <msg>" from the frame `uplevel` levels up; category: :deprecated is
# suppressed by default (Warning[:deprecated] is false without
# -W:deprecated).
class Tee
  def write(*a)
    a.each { |s| $stdout.write(s) }
    nil
  end
end
$stderr = Tee.new

def inner
  warn("u0", uplevel: 0)
  warn("u1", uplevel: 1)
  warn("u2", uplevel: 2)
end
def outer
  inner
end
outer

warn("plain")                        # no prefix, no "warning: "
warn("a", "b")                       # two messages, both plain
warn("top0", uplevel: 0)             # prefixed at this line
warn("dep", category: :deprecated)   # suppressed → nothing
warn("multi1", "multi2", uplevel: 1) # uplevel beyond <main> → "warning: " on first only
warn                                 # no args → nothing
warn("trailing\n")                   # already ends with newline → not doubled
p :done
