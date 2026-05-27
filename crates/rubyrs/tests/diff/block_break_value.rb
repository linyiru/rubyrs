# `break val` from inside a method-attached block returns `val`
# as the method's result — CRuby applies this rule uniformly,
# including to methods that "don't iterate" (tap / then /
# yield_self) and single-shot block forms (fetch). Pre-migration
# of #151 phase 2 final, several drivers in vm/iter.rs were
# silently discarding the break value:
#
#   Object#tap / #then / #yield_self  → returned receiver / block-result
#                                       even when `break val` fired
#   String#each_byte                  → returned receiver
#   String#scan (all three branches)  → returned `nil` due to a
#                                       double-pop bug in the per-iter loop
#
# Migration to step_block forced explicit BlockStep::Break
# handling at each driver, which makes the break-value fall
# through to the method's return as CRuby does.

puts 1.tap        { break :tap_x }                  # :tap_x
puts 1.then       { break :then_x }                 # :then_x
puts 1.yield_self { break :ys_x }                   # :ys_x
puts "abc".each_byte { break :eb_x }                # :eb_x

# String#scan — three sub-branches in vm/iter.rs (regex with
# capture groups, regex without, String pattern). All three
# had the double-pop bug; this fixture exercises all three.
puts "abcabc".scan(/(a)/) { break :scan_cap_x }     # :scan_cap_x
puts "abcabc".scan(/a/)   { break :scan_nocap_x }   # :scan_nocap_x
puts "abcabc".scan("a")   { break :scan_str_x }     # :scan_str_x

# Hash#fetch block form — single-shot block. The break value
# was actually already returned correctly here pre-migration
# (the pre-pop sequence happened to capture the right value),
# but lock it explicitly so future refactors can't drift.
puts({}.fetch(:k) { break :fetch_x })               # :fetch_x

# Verify break still terminates the iteration early — without
# the early-out, the block would run multiple times on `scan`
# / `each_byte` and we'd see counter incremented past 1.
def break_terminates
  count = 0
  "aaa".each_byte { count += 1; break }
  count
end
puts break_terminates                                # 1
