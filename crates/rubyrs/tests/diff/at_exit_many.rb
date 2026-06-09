# Multiple at_exit handlers run LIFO at program exit. Regression guard
# for a GC-rooting bug: the handler Block objects are held only by ObjId
# in at_exit_handlers, so without rooting them a GC between registration
# and exit swept the bodies and reused their ObjIds — every handler then
# aliased the last-registered block (under STRESS_GC all five printed
# the fifth body). diff_cruby runs this fixture under STRESS_GC in CI.

at_exit { puts "exit-a" }
at_exit { puts "exit-b" }
at_exit { puts "exit-c" }
at_exit { puts "exit-d" }
at_exit { puts "exit-e" }

# Allocate a pile of throwaway objects so a stress GC actually runs
# between registration and exit, reclaiming anything unrooted.
1000.times { |i| [i, "s#{i}", { k: i }] }

puts "main done"
