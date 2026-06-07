# Exercises the small-argc stack-buffer path for primitive-receiver
# method calls (Array / Hash / String) across the inline-buffer
# boundary. The args of every `do_call` are drained into a fixed
# stack array for argc <= 3 (no heap Vec) and spill to a heap Vec for
# argc >= 4. Calls appear both inside tight loops (the hot path the
# optimization targets) and as one-shots, with argc 1/2/3/4 so the
# boundary at and above the inline capacity is locked. Only methods
# rubyrs actually supports are used (multi-arg Array#push and
# String#insert are documented subset gaps and intentionally avoided).

# --- 1-arg: Array#push / String#<< / Hash#[]/[]= inside a loop ---
a = []
s = +""
h = {}
i = 0
while i < 5
  a.push(i)        # Array#push, 1 arg
  s << "x"         # String#<<, 1 arg
  h[i] = i * 10    # Hash#[]=, 2 args
  i += 1
end
p a
p s
p h

# --- 1-arg Hash#[] read (the other half of the hot hash path) ---
sum = 0
k = 0
while k < 5
  sum += h[k]      # Hash#[], 1 arg
  k += 1
end
p sum

# --- 2-arg: Array#insert inside a loop ---
b = []
j = 0
while j < 4
  b.insert(0, j)   # Array#insert, 2 args
  j += 1
end
p b

# --- 3-arg: Array#[]= range-assign (argc == inline capacity) ---
c = [1, 2, 3, 4, 5]
c[1, 2] = [20, 30] # Array#[]=, 3 args (start, length, value)
p c

acc = [10, 20, 30]
acc[0, 2] = [7, 8, 9] # Array#[]=, 3 args, grows the array
p acc

# --- 3-arg: String#[]= (argc == inline capacity) ---
str = +"hello"
str[1, 3] = "ELL"  # String#[]=, 3 args
p str

# --- 4-arg call (argc just ABOVE inline capacity → heap-Vec spill) ---
# Routed through a plain user method so the spill path is exercised
# without relying on a 4-arg primitive (none are in the subset).
def take4(w, x, y, z)
  [w, x, y, z]
end
p take4(1, 2, 3, 4)

# 4-arg in a loop (repeated heap-spill).
out = []
n = 0
while n < 3
  out.push(take4(n, n + 1, n + 2, n + 3))
  n += 1
end
p out

# --- format/sprintf with 1/2/3/4 trailing args (varargs builtin) ---
p format("%d", 1)
p format("%d-%d", 1, 2)
p format("%d-%d-%d", 1, 2, 3)
p format("%d-%d-%d-%d", 1, 2, 3, 4)
