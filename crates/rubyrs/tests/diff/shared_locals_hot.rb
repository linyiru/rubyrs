# Locals of a frame that keeps them in a shared cell (a method body that
# creates a block, or a block body) read and written by the hot
# LoadLocal / StoreLocal / IncLocalNoPush / BinOpLocalLocal /
# LoadLocalCall arms. A slot the frame owns goes through the cell
# directly. A captured outer slot must still reach the canonical binding.
def loop_with_block(n)
  i = 0; a = 0
  while i < n; a = a + i; i += 1; end
  [1].each { |x| a += x }
  [i, a]
end
p loop_with_block(1000)

# The block sees the method's updates, and the method sees the block's.
def shared_rw
  x = 1
  bump = -> { x += 10 }
  x = x + 1
  bump.call
  y = x * 2
  [x, y]
end
p shared_rw

# Non-Integer operands fall back to full dispatch.
def mixed(n)
  i = 0; f = 0.5; s = "a"; big = 2**62
  while i < n; f = f + f; big = big + big; i += 1; end
  i = 1.5
  i += 1
  [1].each { s += "b" }
  [f, big, i, s]
end
p mixed(3)
def divz; a = 1; b = 0; [1].each { }; a / b; rescue ZeroDivisionError => e; e.class; end
p divz

# Calls on a local (LoadLocalCall) in a block-creating method.
def local_call; s = "abc"; r = s.size; [1].map { |x| x + r } + [s.upcase]; end
p local_call

# Block bodies: their own locals, captured outer locals, and nested
# blocks rebinding the method's variable while it is live.
def in_blocks
  total = 0
  (1..5).each do |k|
    t = 0; j = 0
    while j < k; t = t + j; j += 1; end
    total += t
    [1, 2].each { |z| total += z }
  end
  total
end
p in_blocks

# Escaped closures keep their binding after the method returns, and
# each call gets fresh locals.
def counter; c = 0; [-> { c += 1 }, -> { c }]; end
inc, get = counter
3.times { inc.call }
inc2, get2 = counter
inc2.call
p [get.call, get2.call]

# A proc defined in a loop captures each iteration's variable.
def per_iter; out = []; i = 0; while i < 3; v = i; out << -> { v }; i += 1; end; out.map(&:call); end
p per_iter
