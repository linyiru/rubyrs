# `block_given?` / `defined?(yield)` inside a block follow closure
# semantics: they refer to the LEXICALLY-enclosing method's block, and
# keep working when the block is stored as a Proc and `.call`ed AFTER
# that method has returned. Driver: RuboCop's Options#option does
# `opts.on(*args) { |arg| @opts[k] = arg; yield arg if block_given? }`
# — the block is stashed in OptionParser's specs and run later by
# `parse!`, so the enclosing `option` frame is long gone.

# (A) deferred block: stored, then called after the method returned
def store(&b); $stored = b; end
def option
  store { yield(99) if block_given? }
end
$stored = nil
option { |x| puts "A: yielded #{x}" }
$stored.call
option   # no block — deferred block_given? must be false, no yield
$stored.call
puts "A: done"

# (B) immediate block (control)
def option2
  [1].each { yield if block_given? }
end
option2 { puts "B: yielded" }
option2
puts "B: done"

# (C) bare block_given? value
def has?; r = nil; [1].each { r = block_given? }; r; end
p has? { }
p has?

# (D) defined?(yield) inside a deferred block
def store2(&b); $s2 = b; end
def opt3
  store2 { defined?(yield) ? "yield" : "nil" }
end
opt3 { }
p $s2.call
opt3
p $s2.call
