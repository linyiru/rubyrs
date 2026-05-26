# `Hash.new` semantics — three forms in CRuby, two of them
# handled by rubyrs:
#
#   1. `Hash.new`                    → empty Hash, no default
#   2. `Hash.new(default_value)`     → scalar default — NOT MODELLED
#      (silently ignored; documented divergence)
#   3. `Hash.new { |h, k| block }`   → block called on missing-key
#      access with `(self_hash, key)`; auto-vivification idiom
#
# Tilt's `@lazy_map = Hash.new { |h, k| h[k] = [] }` (in
# `tilt/mapping.rb:131`) was the motivating case — without
# form (3), tilt-load stalled at the first `@lazy_map[ext]`.

# --- (1) Bare Hash.new ---
# Returns a real Hash (Value::Hash on the rubyrs side, not a
# bare Value::Object from the generic Class.new allocator).
h = Hash.new
puts h.class                                    # Hash
puts h.inspect                                  # {}
puts h[:missing].inspect                        # nil (no default)
h[:k] = 1
puts h.inspect                                  # {k: 1}

# --- (3) Hash.new with default-block ---
# Block invoked on missing-key access. Return value of the
# block becomes the [] result. The block can mutate the Hash
# (CRuby's classic auto-vivify idiom).
hb = Hash.new { |hh, k| hh[k] = [] }
puts hb.class                                   # Hash
puts hb.inspect                                 # {}
# Touch :a — block fires, assigns hh[:a] = [], returns it.
puts hb[:a].inspect                             # []
# Same key again — direct hit now, block does NOT fire.
hb[:a] << :first
puts hb[:a].inspect                             # [:first]
# Touch :b — block fires for a new key.
hb[:b] << :only
puts hb[:b].inspect                             # [:only]
puts hb.inspect                                 # {a: [:first], b: [:only]}

# --- Default-block return value, no mutation ---
# The block doesn't have to mutate; whatever it returns is
# what [] yields. Subsequent accesses re-invoke the block
# (no caching) so this returns a fresh value each time.
counter = Hash.new { |_, k| "computed-#{k}" }
puts counter[:x]                                # "computed-x"
puts counter[:y]                                # "computed-y"
puts counter.inspect                            # {} (block didn't mutate)
