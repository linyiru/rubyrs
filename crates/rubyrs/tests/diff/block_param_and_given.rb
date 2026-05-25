# `&blk` named block param + `block_given?` builtin.
# Both rubyrs and CRuby must produce identical stdout.

# Named block param: callee captures the block as data, calls
# back via .call.
def each_double(&blk)
  [1, 2, 3].each { |x| blk.call(x * 2) }
end
each_double { |y| puts y }

# No-block case explicitly binds nil to the &blk slot. Verify
# the binding directly rather than relying on the
# "blk.call → NoMethodError" symptom.
def show_blk(&blk)
  puts blk.nil?
end
show_blk            # true
show_blk { }        # false

# block_given? alone (no &blk).
def maybe(x)
  if block_given?
    yield x
  else
    x
  end
end
puts maybe(5)
puts maybe(5) { |v| v * 10 }

# Both together: blk is non-nil iff block_given? is true.
def report(&blk)
  if block_given?
    blk.call("yes")
  else
    "no block"
  end
end
puts report
puts report { |s| "block ran: #{s}" }

# block_given? walks past block frames to the enclosing method.
# Inside an iterator block, it reports the outer method's block,
# not the block's own slot.
def outer
  [1].each do
    puts block_given?
  end
end
outer        # false — outer was called without a block
outer { }    # true  — outer received a block (the { } here)

# Storing &blk in a local; calling later.
def collect_into(arr, &blk)
  arr.each { |e| arr_push(blk.call(e)) }
end
def arr_push(_) end  # noop sink to keep the example tight

# Forwarding the block onward (the most common &blk use):
# rubyrs needs to accept blk as a positional block-arg in the
# downstream call. We don't yet support `&blk` in CALL position
# on a Proc-flavored value uniformly; the .call form above is
# the supported shape.

# `block_given?` at toplevel should be false (no method frame).
puts block_given?
