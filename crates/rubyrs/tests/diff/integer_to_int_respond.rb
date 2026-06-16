# `Integer#to_int` (identity, implicit-int-conversion protocol) and its
# respond_to? whitelist entry; plus `Numeric#nonzero?` respond_to. tilt's
# `process_arg` does `arg.respond_to?(:to_int)` to treat an Integer arg
# as a line number — without to_int it fell through to a TypeError.
p 1.to_int
p(-5.to_int)
p 1.respond_to?(:to_int)
p 5.respond_to?(:nonzero?)
p 1.5.respond_to?(:nonzero?)
p 0.respond_to?(:to_int)
