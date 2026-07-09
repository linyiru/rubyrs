# `Value::Block`-receiver `.call` served directly inside a hot
# (tier-2-compiled) method body — campaign P7. The warm loop below
# trips the tier-2 threshold so `invoke_proc_call_body` runs from the
# in-body serve (`t2_call_impl`), not just the interpreter fast arm;
# parity here therefore pins the tier-2 serve == interp == CRuby.
#
# The AS callback machinery this targets (`invoke_sequence.call` + the
# filter lambdas) is exactly this shape: a hot method invoking stored
# procs/lambdas with argc 0/1/2. Covers the semantics the serve must
# preserve: proc-LENIENT vs lambda-STRICT arity, proc non-local return,
# splat/kwargs/&block args, and the Symbol#to_proc / Method#to_proc /
# curried-proc receiver kinds. (A lambda `break` is intentionally NOT
# exercised — that is a pre-existing interp+tier2 divergence, out of
# P7's scope, and would be equally "wrong" on both paths anyway.)

L0 = -> { 7 }
L1 = ->(a) { a * 2 }
L2 = ->(a, b) { a + b }
PR = proc { |a, b| [a, b] }

def hot(n)
  acc = 0
  i = 0
  while i < n
    acc += L0.call            # argc 0
    acc += L1.call(i)         # argc 1
    acc += L2.call(i, 1)      # argc 2
    acc += PR.call(i).first   # proc lenient: PR.call(i) => [i, nil]
    i += 1
  end
  acc
end

# warm past the tier-2 compile threshold, then assert the hot result
puts hot(300)
puts hot(20_000)

# --- arity: lambda STRICT, proc LENIENT ---
begin; L2.call(1); rescue ArgumentError => e; puts "arity: #{e.message}"; end
puts PR.call(1).inspect            # [1, nil]
puts PR.call(1, 2, 3).inspect      # [1, 2] (drops extra)
puts PR.call([9, 8]).inspect       # autosplat -> [9, 8]

# --- proc non-local return (returns from the defining method) ---
def proc_return
  pr = proc { return 99 }
  pr.call
  :unreachable
end
puts proc_return

# --- dead proc return -> LocalJumpError ---
def make_dead; proc { return 5 }; end
begin; make_dead.call; rescue LocalJumpError => e; puts "dead: #{e.message}"; end

# --- break in a `.call`-invoked proc -> LocalJumpError ---
def brk; proc { break 1 }.call; :after; end
begin; brk; rescue LocalJumpError => e; puts "break: #{e.message}"; end

# --- splat / kwargs / &block args ---
splat = proc { |*a| a }
puts splat.call(1, 2, 3).inspect
puts splat.call(*[4, 5]).inspect
kw = ->(a:, b: 10) { [a, b] }
puts kw.call(a: 1).inspect
puts kw.call(a: 1, b: 2).inspect
takes_blk = ->(x, &b) { b.call(x) }
puts takes_blk.call(3) { |v| v + 100 }
puts takes_blk.call(4, &->(v) { v * 10 })

# --- invocation aliases ---
sq = ->(x) { x * x }
puts [sq.call(5), sq.(5), sq[5], sq.yield(5), (sq === 5)].inspect

# --- special proc kinds (all Proc-class receivers) ---
puts :upcase.to_proc.call("hi")
puts "world".method(:upcase).to_proc.call
puts(->(a, b, c) { a + b + c }.curry.call(1).call(2).call(3))
