# `$foo` global variable read / write. Spike subset:
# user-defined globals stored in Vm.globals; unknown globals
# read as nil (CRuby lenient default); `$$` intercepted to
# return Process.pid as Integer. Both rubyrs and CRuby must
# produce identical stdout.

# Basic write + read.
$counter = 0
puts $counter
$counter = 5
puts $counter

# String global.
$greeting = "hello"
puts $greeting
puts $greeting.length

# Unknown global reads as nil.
puts $unset.inspect
puts $unset.nil?

# Globals are visible across methods (the whole point of
# globals — distinct from locals which are method-scoped).
def bump
  $counter = $counter + 1
end
bump
bump
bump
puts $counter

# Assignment-as-expression: `$g = expr` evaluates to expr.
x = ($paged = 99)
puts x
puts $paged

# $$ is the process pid — at least an Integer > 0. We can't
# stdout-compare the exact value (differs per run); just
# check the shape.
puts $$.is_a?(Integer)
puts $$ > 0

# Globals interact with normal arithmetic / comparison via the
# bare write form. `$total += 50` is a GlobalVariableOperatorWriteNode
# in Prism — that node type isn't in the spike subset yet, so we
# write the equivalent explicit form here.
$total = 100
$total = $total + 50
puts $total

# Multiple globals together.
$a = 1
$b = 2
$c = $a + $b
puts $c
