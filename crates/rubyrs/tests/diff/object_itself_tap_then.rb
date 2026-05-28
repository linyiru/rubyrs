# Object#itself + tap/then/yield_self universal arms.
#
# `tap`, `then`, and `yield_self` (alias of `then`) with a
# block were already wired through `collection_call_block` in
# vm/iter.rs. This commit adds:
#
#   1. `Object#itself` — returns self unchanged. Common with
#      Symbol#to_proc (`group_by(&:itself)`).
#   2. No-block fallback for `tap`/`then`/`yield_self` raises
#      LocalJumpError instead of NoMethodError, matching
#      CRuby's `tap` (and a documented divergence from CRuby's
#      `then`/`yield_self` which would return an Enumerator —
#      rubyrs has no Enumerator type yet).
#   3. respond_to? whitelist additions so feature detection
#      agrees with dispatch behaviour.

# itself — identity on every value type
puts 5.itself
puts "hi".itself
puts nil.itself.inspect
puts true.itself
puts :foo.itself
puts [1, 2].itself.inspect

# Symbol#to_proc idiom — `group_by(&:itself)` buckets equal
# values together. Hash#hash from PR #286 is what makes this
# work for Array elements.
puts [1, 1, 2, 3, 3, 3].group_by(&:itself).inspect

# tap returns self, runs block for side effects
arr = [3, 1, 2]
out = arr.tap { |a| a.sort! }
puts out.equal?(arr)              # true — same object
puts arr.inspect                  # mutated: [1, 2, 3]

# then / yield_self return block result
puts 5.then { |n| n * 2 }
puts "abc".yield_self { |s| s.upcase }

# Chainable Kleisli — common FP pattern
result = 1.then { |n| n + 1 }.then { |n| n * 10 }
puts result

# tap is debug-style — block result is ignored
puts 5.tap { |n| "ignored" }

# break inside tap propagates (PR #272-era guarantee mentioned
# in the iter.rs comment).
broken = 99.tap { break :short }
puts broken

# respond_to? must agree with dispatch
puts 42.respond_to?(:itself)
puts 42.respond_to?(:tap)
puts 42.respond_to?(:then)
puts 42.respond_to?(:yield_self)
puts Object.new.respond_to?(:itself)

# No-block tap raises LocalJumpError (both CRuby and rubyrs)
begin
  5.tap
rescue LocalJumpError => e
  puts "tap-noblock"
end

# Arity guard — CRuby raises ArgumentError on extra args for
# every member of this family regardless of block presence
# (cycle-1 review of PR #290). Without an explicit guard
# rubyrs would fall through to NoMethodError.
[:itself, :tap, :then, :yield_self].each do |m|
  begin
    5.send(m, 1)
  rescue ArgumentError => e
    puts "#{m}-extra-arg"
  end
end

# itself with an attached block — CRuby silently ignores the
# block and still returns the receiver. We must NOT raise
# LocalJumpError here even though there's no `yield` path —
# the cycle-1 reviewer flagged this as a divergence from
# CRuby's universal-arm semantics.
puts 7.itself { raise "block must not run" }

# Note: rubyrs's `then` / `yield_self` without a block raise
# LocalJumpError, while CRuby returns an Enumerator. This
# divergence is exercised by a rubyrs-only unit test in
# crates/rubyrs/src/vm/dispatch.rs's test module instead of
# this diff fixture so CRuby parity stays clean.
