# Block keyword parameters — `|k1:, k2:|` required, `|k: default|`
# optional (any default expression), on proc/lambda/yield, plus the
# kwargs-vs-positional-Hash recovery heuristic and CRuby's error
# wording/ordering (missing before unknown; plural forms).
# minitest mock's expect-with-block kw tests are the motivating
# consumer (test_minitest_mock.rb:384-431).

# Required keywords bind by name, any order.
p proc { |k1:, k2:| [k1, k2] }.call(k2: 2, k1: 1)

# Mixed positional + keyword.
p proc { |a, kw:| [a, kw] }.call(7, kw: 9)

# Optional keyword: default used / overridden. Default expressions
# can reference earlier params (evaluated in block scope).
p proc { |a, k: 5| [a, k] }.call(1)
p proc { |a, k: 5| [a, k] }.call(1, k: 9)
p proc { |a, k: (a + 1)| [a, k] }.call(10)

# Lambda form, required + optional together.
p ->(k1:, k2: "d") { [k1, k2] }.call(k1: :x)

# Missing required: singular and plural messages.
begin
  proc { |k1:, k2:| }.call
rescue ArgumentError => e
  puts e.message
end

# Missing + unknown together: CRuby reports MISSING first.
begin
  proc { |k1:, k2:| }.call(k1: 1, x: 2, y: 3)
rescue ArgumentError => e
  puts e.message
end

# All present + extra: unknown keyword.
begin
  proc { |k1:, k2:| }.call(k1: 1, k2: 2, x: 3)
rescue ArgumentError => e
  puts e.message
end

# A trailing Hash whose keys DON'T name any declared keyword stays
# positional — iteration drivers yielding Hash elements must not
# have their element stolen as kwargs.
p [{ a: 1 }].map { |h, k: 2| [h, k] }

# Named keyword + **rest: rest collects only the leftover pairs.
p proc { |k1:, **rest| [k1, rest] }.call(k1: 1, b: 2, c: 3)

# yield with keywords reaches the block's kw slots.
def yfoo
  yield(k1: 10, k2: 20)
end
p(yfoo { |k1:, k2:| k1 + k2 })

# Optional kw slot re-binds every invocation (no cross-call leak).
m = proc { |k: 3| k * 2 }
p [1, 2].map { |x| m.call(k: x) }
p m.call
