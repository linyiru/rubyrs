# CRuby-parity coverage for shapes the toplevel fixed-arity fast path
# (PR #155) routes through `try_invoke_fixed_method_from_stack`.
#
# Scope: diff_cruby compares stdout, so this fixture pins observable
# behavior, not the dispatch decision. A regression that disables the
# fast path entirely (and falls back to the slow path correctly) would
# still pass — confirming the fast path was actually taken is the job
# of the perf benchmark, not this fixture. What it DOES catch: any
# behavioral divergence (wrong arg binding, dropped block, mis-cached
# method) the new code path could introduce.

# --- hot-loop fixed-arity call (the motivating workload) ---
def add(a, b)
  a + b
end

total = 0
i = 0
while i < 50
  total = add(total, i)
  i += 1
end
puts total

# --- recursion through the fast path ---
def fact(n)
  if n < 2
    1
  else
    n * fact(n - 1)
  end
end
puts fact(0)
puts fact(1)
puts fact(8)

# --- argc=0 + argc>=2 also reach the fast path; argc=1 is covered by
# the recursive call above. Two more shapes here is a sanity that the
# stack-drain branches (zero-pop, drain) don't misbehave in arg
# binding, even though diff_cruby can't tell them apart from a slow
# path that produces the same output.
def zero
  42
end
puts zero

def three(a, b, c)
  a + b + c
end
puts three(1, 2, 3)

# --- redef invalidates the cached toplevel binding ---
def reborn
  1
end
puts reborn      # warm: populates the cache slot
def reborn
  2
end
puts reborn      # must observe v2, not the cached v1

# --- argc mismatch falls through to slow path and raises ---
def one_only(a)
  a
end
puts one_only(99)            # warm the cache with a successful call
begin
  one_only(1, 2)             # too many args — slow path must raise
  puts "should-not-reach"
rescue ArgumentError
  puts "argerr-too-many"
end
begin
  one_only                   # too few — slow path must raise
  puts "should-not-reach"
rescue ArgumentError
  puts "argerr-too-few"
end
puts one_only(100)           # cache must still work after the rescues

# --- defaulted positional params: fixed_arity must be None ---
def with_default(a, b = 10)
  a + b
end
puts with_default(1)
puts with_default(1, 2)

# --- block_given? inside a fixed-arity-eligible method ---
# No-block call goes through do_call → fast path (block_arg = None).
# Block call goes through do_call_block → slow path (block_arg set).
# Both forms must return the correct answer.
def asks
  block_given?
end
puts asks
puts asks { }

# --- IncLocal on a non-Int local (slow path of the in-place mutation) ---
x = 1.5
i = 0
while i < 5
  x += 1
  i += 1
end
puts x
