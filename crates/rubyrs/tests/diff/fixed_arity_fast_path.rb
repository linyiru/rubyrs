# Coverage for the toplevel fixed-arity fast path introduced in
# PR #155. Each block below targets a distinct edge of
# `try_invoke_fixed_method_from_stack` + `lookup_toplevel_method_cached`
# + the `IncLocal` in-place mutation, in shapes where CRuby is the
# oracle (so any divergence shows up as a diff_cruby failure).

# --- hot-loop fixed-arity call (the motivating workload) ---
def add(a, b)
  a + b
end

total = 0
i = 0
while i < 1000
  total = add(total, i)
  i += 1
end
puts total

# --- argc=0 fast path (separate stack-pop branch in the fast path) ---
def zero
  42
end
puts zero
puts zero
puts zero

# --- argc=1 fast path (one-element pop branch) ---
def one(x)
  x * x
end
puts one(7)
puts one(8)

# --- argc>=2 fast path (drain branch) ---
def three(a, b, c)
  a + b + c
end
puts three(1, 2, 3)
puts three(10, 20, 30)

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

# --- mutual recursion (cache populated for two distinct names) ---
def even?(n)
  if n == 0
    true
  else
    odd?(n - 1)
  end
end
def odd?(n)
  if n == 0
    false
  else
    even?(n - 1)
  end
end
puts even?(10)
puts odd?(11)
puts even?(7)

# --- redef invalidates the cached toplevel binding ---
def reborn
  1
end
puts reborn      # warm: caches v1
puts reborn      # hit:  still v1
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
rescue ArgumentError => e
  puts "argerr-too-many"
end
begin
  one_only                   # too few — slow path must raise
  puts "should-not-reach"
rescue ArgumentError => e
  puts "argerr-too-few"
end
puts one_only(100)           # cache must still work after the rescues

# --- defaulted positional params: fixed_arity must be None, slow path handles ---
def with_default(a, b = 10)
  a + b
end
puts with_default(1)
puts with_default(1, 2)
puts with_default(5)

# --- splat / kwargs / block param: also non-fixed; slow path handles ---
def splat(*xs)
  xs.length
end
puts splat
puts splat(1)
puts splat(1, 2, 3)

def kw(a:, b: 2)
  a + b
end
puts kw(a: 1)
puts kw(a: 1, b: 3)

def with_block(x, &blk)
  blk.call(x)
end
puts with_block(5) { |n| n * 3 }

# --- block_given? inside a fixed-arity-eligible method ---
# Calls without a block go through do_call → fast path (block_arg = None).
# Calls with a block go through do_call_block → slow path (block_arg set).
def asks
  block_given?
end
puts asks
puts asks { }
puts asks
puts asks { 1 }

# --- IncLocal on a non-Int local (slow path of the in-place mutation) ---
x = 1.5
i = 0
while i < 5
  x += 1
  i += 1
end
puts x

# --- IncLocal alternating Int/Float (must not corrupt the slot) ---
y = 0
i = 0
while i < 3
  y += 1
  i += 1
end
puts y
y = 0.5
i = 0
while i < 3
  y += 1
  i += 1
end
puts y

# --- toplevel def then call inside a block (self is still nil) ---
def inside_block(n)
  n + 100
end
[1, 2, 3].each { |n| puts inside_block(n) }

# --- builtin name shadowing must NOT happen for names in is_builtin_name ---
# `puts` is in is_builtin_name, so the user def is bypassed by the fast path
# and `builtin_call` wins on the slow path. Matches master's pre-PR behavior.
# (CRuby would call the user def — that is a documented, pre-existing
#  divergence tracked outside this PR.)
# We don't print here because the divergence would make this section fail
# under diff_cruby. The fact that the rest of the suite passes is the
# regression guard.
