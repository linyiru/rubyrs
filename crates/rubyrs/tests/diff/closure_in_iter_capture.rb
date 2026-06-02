# Per-invocation closure capture in iterator blocks — the load-
# bearing shape `sinatra_plugin_smoke` surfaced. The bug it
# pins: each iteration of an enclosing block used to share the
# same locals Vec via Rc, so inner closures captured DURING the
# iteration all resolved their captured block-params to the
# LAST iteration's value. Fix lives in `vm/dispatch.rs::
# invoke_block` (fresh per-invocation locals Vec) +
# `vm/step.rs::Op::StoreLocal/IncLocal/IncLocalNoPush` (outer-
# scope write propagation via `propagate_outer_write`).

# (1) The headline shape — `.each { |s| -> { s } }`. Pre-fix
# rubyrs returned [:c, :c, :c]. CRuby gives one lambda per
# iter, each closing over THAT iter's `s`.
ls = [:a, :b, :c].map { |s| -> { s } }
p ls.map(&:call)

# (2) define_method-with-block-capture — the M27 A4 batch
# claim. Generates three uniquely-bound greeter methods.
class Greeter
  [:formal, :casual, :friendly].each do |style|
    define_method("greet_#{style}") do |name|
      "[#{style}] #{name}"
    end
  end
end
g = Greeter.new
puts g.greet_formal("Alice")
puts g.greet_casual("Bob")
puts g.greet_friendly("Cara")

# (3) Loop with block-local var, multiple closures captured.
collected = []
3.times do |i|
  local = i * 10
  collected << -> { local }
end
p collected.map(&:call)   # CRuby: [0, 10, 20]

# (4) Counter aggregation across iterations — the OUTER-scope
# write-through invariant the fresh-clone fix must NOT regress.
counter = 0
[1, 2, 3].each { |x| counter += x }
puts counter             # CRuby: 6

# (5) Nested block writing to outer-method local — multi-level
# propagation. Pre-fix this returned nil because the inner
# block's StoreLocal landed in its fresh Vec without walking
# back to the method's locals.
def nested_writer
  result = nil
  [1].each do
    [1].each do
      result = :reached
    end
  end
  result
end
p nested_writer

# (6) Same-iter reassignment of captured param — inner closure
# reads the CURRENT-iter value, not the snapshot at create.
result6 = []
[1].each do |x|
  result6 << -> { x }
  x = x * 100
end
p result6[0].call         # CRuby: 100
