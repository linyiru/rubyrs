# Ruby 3 keyword-argument separation: an EXPLICIT-brace hash `{...}` is
# ALWAYS a positional argument, never keywords — even when the callee has
# a keyword parameter and even when the hash has symbol keys. Only the
# bare `k: v` / `**h` syntax produces keywords.
#
# rubyrs used to peel ANY trailing Hash into kwargs whenever the method
# declared a kwparam, so `merge_data!({ "categories" => … })` (Liquid /
# Jekyll) raised `wrong number of arguments (given 0, expected 1)`. The
# fix gates the peel on the call actually using keyword syntax
# (`Op::CallKw`).

def f(o, k: 1)
  "o=#{o.inspect} k=#{k.inspect}"
end

# explicit braces -> positional, kwparam defaults
p f({"x" => 1})              # o={"x"=>1} k=1
p f({a: 1})                  # o={:a=>1} k=1   (braces win even w/ sym keys)
p f({"x" => 1}, k: 9)        # o={"x"=>1} k=9  (brace positional + real kwarg)
p f([1, 2])                  # o=[1, 2] k=1    (non-hash positional, sanity)

# bare keyword syntax -> keywords
p f(1, k: 2)                 # o=1 k=2

# Required keyword still enforced; a positional brace hash does NOT satisfy it.
def g(a, k:)
  "a=#{a.inspect} k=#{k.inspect}"
end
p g(1, k: 2)                          # a=1 k=2
# brace hash is positional `a`; required kw `k` is still missing -> raises
p(begin; g({k: 9}); rescue ArgumentError; "missing-kw"; end)

# **splat and kwrest.
def h(a, **opts)
  "a=#{a.inspect} opts=#{opts.inspect}"
end
p h(1, x: 2, y: 3)           # a=1 opts={:x=>2, :y=>3}
p h({"pos" => 1})            # a={"pos"=>1} opts={}  (brace positional, kwrest empty)
p h(1, **{z: 9})             # a=1 opts={:z=>9}

# Block + keyword combination must keep working (compiler emits CallBlock).
def b(a, k: 10)
  "a=#{a} k=#{k} y=#{block_given? ? yield : :noblk}"
end
p b(1, k: 2) { 99 }          # a=1 k=2 y=99
p b(1) { 7 }                 # a=1 k=10 y=7
# NOTE: `b({"h" => 1}) { 8 }` (block + explicit-brace POSITIONAL hash) is a
# separate PRE-EXISTING divergence — the compiler emits `Op::CallBlock` for
# both block+kwargs and block+positional-hash, so they're indistinguishable
# at runtime without a `CallKwBlock` op. Out of scope for this fix (which
# targets the plain-call path); not asserted here.
