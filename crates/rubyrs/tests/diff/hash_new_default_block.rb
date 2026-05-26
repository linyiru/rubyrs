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
puts counter[:x]                                # computed-x
puts counter[:y]                                # computed-y
puts counter.inspect                            # {} (block didn't mutate)

# --- Hash.new arity check ---
# `Hash.new(default) { block }` is ArgumentError in CRuby
# ("wrong number of arguments"); rubyrs matches explicitly.
begin
  Hash.new(0) { |_, _| 0 }
rescue ArgumentError => e
  puts e.message
end

# `Hash.new(a, b)` (no block, 2+ positional args) — also
# ArgumentError. Without an explicit arity check the intercept
# would silently return an empty Hash, masking the caller bug.
begin
  Hash.new(1, 2)
rescue ArgumentError => e
  puts e.message
end

# --- merge with temporary RHS doesn't lose nested children ---
# GC-rooting: merge clones `other`'s pairs shallowly (ObjId-level)
# into a local Vec, then allocs the merged Hash. Without pinning
# `*other` across the alloc, maybe_gc could sweep `*other` plus
# its nested heap children (Arrays / Strings / Hashes) — the
# new Hash would hold dangling ObjIds. The literal-temporary
# shape `h.merge({a: [...]})` is the easy repro because the
# RHS is unreachable from the stack after the call.
1000.times do |i|
  m = {}.merge({a: [i, i+1]})
  raise "GC corruption iter #{i}: #{m[:a].inspect}" unless m[:a] == [i, i+1]
end
puts "merge-temporary-rhs ok"

# --- merge preserves receiver's default-block ---
# CRuby: derived hashes (merge/select/etc.) inherit the
# receiver's default_proc. Without this, `Hash.new {...}.merge(x)[:y]`
# loses auto-vivify. Pin the receiver's block survives the merge.
base = Hash.new { |h, k| h[k] = "base-#{k}" }
merged = base.merge({a: 1})
puts merged.inspect                             # {a: 1}
puts merged[:new_key]                           # base-new_key (block fired)
puts merged.inspect                             # {a: 1, new_key: "base-new_key"}

# --- Hash#dig consults default-block per step ---
# CRuby's `Hash#dig` walks via `[]` at each level, so a default-
# block fires on missing keys during the dig. Without this,
# nested auto-vivify patterns silently return nil at the dig
# site instead of materialising the intermediate level.
nested = Hash.new { |hh, k| hh[k] = {leaf: "from-block"} }
puts nested.dig(:foo, :leaf)                   # "from-block" (block fires for :foo)
puts nested.dig(:bar, :leaf)                   # "from-block" (block fires again, separate key)
# After the digs, the block-materialised entries persist in the
# hash because the block mutates `hh`.
puts nested.keys.sort.inspect                  # [:bar, :foo]

# --- non-local return from default-block ---
# `return` from inside the default-block exits the enclosing
# method with the return value, propagating through the `[]`
# call site. CRuby semantics: default-block is a Proc, so
# `return` works (non-local return through the enclosing method),
# while `break` would raise LocalJumpError (untested here —
# `break` from a Proc is not idiomatic and the diff_cruby
# fixture pins what's actually used).
def with_return_test
  h = Hash.new { |_, _| return :early_return }
  h[:any]
  :unreachable
end
puts with_return_test                           # early_return
