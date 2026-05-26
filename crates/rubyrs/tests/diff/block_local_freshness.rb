# Block-local variable freshness — variables first-assigned
# inside a `do ... end` block (or a `proc { }` body) get
# fresh `nil` slots on every invocation, not the previous
# invocation's value. Outer-scope variables stay shared
# (the closure-modification case).
#
# Pre-fix rubyrs leaked block-locals across invocations:
# `n ||= 0; n += 1` inside a proc counted 1, 2, 3, ... ;
# CRuby resets `n` to `nil` each call so it always sees 1.
# `y = expr if cond` inside `.each` kept `y` at the previous
# iteration's value when `cond` was false; CRuby returns
# `nil` instead. PR-driven by string_high_byte_literal
# fixture authoring where the if-modifier shape exposed it.

# --- 1. if-modifier in `.each` block: var fresh each iter ---
puts "--- 1. if-modifier ---"
[1, -2, 3, -4, 5].each do |x|
  y = 100 if x > 0
  puts y.inspect
end

# --- 2. proc with `||= 0` counter: stays at 1 every call ---
# (the canonical Ruby-newbie surprise that pre-fix rubyrs
# silently made work like a stateful counter)
puts "--- 2. proc counter ---"
counter = proc { n ||= 0; n += 1; puts n }
3.times { counter.call }

# --- 3. Outer-scope var still shared (regression guard) ---
# Pre-fix this was actually correct because the rule covers
# *body-introduced* vars only; pinning the guard so future
# refactors don't break the outer-scope path.
puts "--- 3. outer shared ---"
total = 0
[1, 2, 3].each do |n|
  total += n
end
puts total                                  # 6

# --- 4. Block param itself is per-iteration (always was) ---
puts "--- 4. param fresh ---"
[10, 20, 30].each do |x|
  puts x
end

# --- 5. Lambda has the same "fresh each call" semantics ---
puts "--- 5. lambda ---"
greet = ->(name) {
  prefix = "Hello, "                       # fresh each call
  puts "#{prefix}#{name}"
}
greet.call("A")
greet.call("B")

# --- 6. Mixed: block-local AND outer-shared in same body ---
puts "--- 6. mixed ---"
visited = []
[7, 8, 9].each do |x|
  doubled = x * 2                          # block-local
  visited << doubled                        # outer-shared
end
puts visited.inspect                        # [14, 16, 18]

# --- 7. Block-local that's nested inside a conditional ---
puts "--- 7. conditional ---"
[true, false, true].each do |flag|
  if flag
    msg = "set"
  end
  puts msg.inspect
end

# --- 8. ||= on block-local: should always assign on every
#        call because the lhs is freshly nil each time ---
puts "--- 8. ||= each call ---"
[1, 2, 3].each do |x|
  cached ||= x * 100
  puts cached
end
