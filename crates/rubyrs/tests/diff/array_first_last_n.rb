# `Array#first(n)` and `Array#last(n)` — variadic forms.
#
# The no-arg shape (`arr.first` / `arr.last`) returns the single
# end element and was already covered by array_basics.rb; this
# fixture is for the with-count variant which returns a fresh
# Array of up to N elements (capped at the receiver's length).
#
# Surfaced by the cf-worker PoC's heavy-compute smoke test,
# which used `arr.last(5)` and hit NoMethodError on the
# unimplemented variadic form.

a = [10, 20, 30, 40, 50]

# Basic first(n) / last(n) within bounds.
puts a.first(2).inspect           # [10, 20]
puts a.last(2).inspect            # [40, 50]
puts a.first(3).inspect           # [10, 20, 30]
puts a.last(3).inspect            # [30, 40, 50]

# n == 0 — empty array, not nil.
puts a.first(0).inspect           # []
puts a.last(0).inspect            # []

# n > len — capped at receiver length, returns whole array.
puts a.first(99).inspect          # [10, 20, 30, 40, 50]
puts a.last(99).inspect           # [10, 20, 30, 40, 50]
puts a.first(5).inspect           # [10, 20, 30, 40, 50]
puts a.last(5).inspect            # [10, 20, 30, 40, 50]

# n > usize::MAX on wasm32 — guards against the i64→usize
# truncation that `*n as usize` would have introduced on the
# 32-bit wasi target (2**32 would wrap to 0 and `first(2**32)`
# would silently return `[]` instead of the whole array).
# Native hosts (usize == u64) pass this trivially; the wasm
# diff matrix is where this case actually changes anything.
puts a.first(4_294_967_296).inspect  # [10, 20, 30, 40, 50]
puts a.last(4_294_967_296).inspect   # [10, 20, 30, 40, 50]

# Empty receiver — even with positive n, returns [].
puts [].first(3).inspect          # []
puts [].last(3).inspect           # []
puts [].first(0).inspect          # []
puts [].last(0).inspect           # []

# No-arg form still returns the end element (regression guard
# against breaking the existing arms while adding the new ones).
puts a.first.inspect              # 10
puts a.last.inspect               # 50
puts [].first.inspect             # nil
puts [].last.inspect              # nil

# Single-element array — both `first(n)` and `last(n)` return the
# whole one-element array when n > len, no surprise.
puts [42].first(3).inspect        # [42]
puts [42].last(3).inspect         # [42]

# Result is a NEW array; mutating it doesn't touch the source.
src = [1, 2, 3]
front = src.first(2)
front << 99
puts src.inspect                  # [1, 2, 3]
puts front.inspect                # [1, 2, 99]

# Negative n — ArgumentError "negative array size", same wording
# as CRuby.
begin
  a.first(-1)
rescue ArgumentError => e
  puts "first(-1): #{e.message}"  # first(-1): negative array size
end
begin
  a.last(-1)
rescue ArgumentError => e
  puts "last(-1): #{e.message}"   # last(-1): negative array size
end

# Block attached — CRuby silently discards. `first(n)` / `last(n)`
# don't yield to a block, so the block is dead code. Before the
# Apr 2026 iter.rs fix, the block-aware dispatch path had no
# `first` / `last` arms and the call NoMethodError'd here.
puts(a.first(2) { puts "should-never-run" }.inspect)  # [10, 20]
puts(a.last(2)  { puts "should-never-run" }.inspect)  # [40, 50]
puts(a.first    { puts "should-never-run" }.inspect)  # 10
puts(a.last     { puts "should-never-run" }.inspect)  # 50
