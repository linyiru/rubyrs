# Regression: locals declared AFTER a lambda must not share
# slots with the lambda's parameter slots. Pre-fix history:
# `compile_block` snapshotted `parent.n_locals` at lambda-
# creation time but never propagated the inner builder's
# growth back. The outer scope then re-used those slot
# indices for subsequent local writes — and the next lambda
# invocation overwrote those outer locals with its params.
#
# Verified externally via L6 (Proc#curry), but the fixtures
# there exercise the symptom only indirectly. This fixture
# pins the direct shape: define-lambda, declare-local-after,
# invoke-lambda, read-local.

# Canonical case: 2-arg lambda, then a local that previously
# would have aliased the lambda's `b` slot.
f = ->(a, b) { a + b }
x = 99
puts f.(1, 2)          # 3
puts x                 # 99 — must NOT be clobbered

# Multiple post-lambda locals.
g = ->(p, q, r) { p * q + r }
m = 10
n = 20
o = 30
puts g.(2, 3, 4)       # 10
puts m                 # 10
puts n                 # 20
puts o                 # 30

# Repeated invocation doesn't mutate outer locals on each call.
h = ->(x, y) { x - y }
saved = "alive"
puts h.(10, 3)
puts h.(20, 5)
puts h.(100, 50)
puts saved             # "alive"

# Lambda body that captures an outer local — the closure
# semantics still work; outer mutations are visible inside.
counter = 0
bump = -> { counter += 1 }
later_local = "still here"
bump.call
bump.call
bump.call
puts counter           # 3
puts later_local       # "still here"

# Nested case: lambda inside a method definition, with outer
# scope locals declared after.
def make_adder
  fn = ->(a, b) { a + b }
  result_holder = nil
  result_holder = fn.(7, 3)
  [fn, result_holder]
end

f, r = make_adder
puts r                 # 10
puts f.(100, 200)      # 300

# Two lambdas sharing a scope — second one's params must not
# alias the first one's params, AND outer locals after both
# must remain untouched.
a1 = ->(x, y) { x + y }
a2 = ->(p, q, r) { p * q * r }
trailing_local = "untouched"
puts a1.(5, 6)         # 11
puts a2.(2, 3, 4)      # 24
puts trailing_local    # "untouched"
