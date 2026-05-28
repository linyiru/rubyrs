# Universal Object/BasicObject methods that previously dispatched
# only on primitive types (String#to_s, Integer#hash, etc.) now
# work on plain Object instances too — closing the
# "reflection-only" gap left by PR #264 (BasicObject reflection).
#
# Newly universal arms in this PR:
#   - `object_id` / `__id__` — stable integer while the value
#     is alive (CRuby and rubyrs both reuse heap ids after
#     GC/deallocation, so we promise "stable while live", not
#     session-wide uniqueness)
#   - `hash` — DefaultHasher (content-based for value types,
#     identity-based for heap objects)
#   - `frozen?` — false on plain Object (we don't model freeze)
#   - `to_s` / `inspect` — `#<ClassName:0xHEXID>` default form

# --- object_id / __id__: contract is "same value → same id" ---
o = Object.new
puts o.object_id == o.object_id                    # true
puts o.__id__ == o.object_id                       # true
puts Object.new.object_id != o.object_id           # true (distinct)

# Value-type ids
puts 42.object_id == 42.object_id                  # true
puts :foo.object_id == :foo.object_id              # true
puts true.object_id                                # 20 (CRuby)
puts false.object_id                               # 0
puts nil.object_id                                 # 4 (CRuby 3.x — was 8 in 2.x)

# --- hash: content-based for value types ---
puts 42.hash == 42.hash                            # true
puts "abc".hash == "abc".hash                      # true (content)
puts :foo.hash == :foo.hash                        # true
puts 1.hash != 2.hash                              # true

# --- frozen? — true for immediates (CRuby semantics), false
#     for plain Object instances (we don't model freeze yet) ---
puts 42.frozen?                                    # true
puts :foo.frozen?                                  # true
puts nil.frozen?                                   # true
puts true.frozen?                                  # true
puts Object.new.frozen?                            # false

# --- to_s / inspect on plain Object ---
class MyClass; end
m = MyClass.new
puts m.to_s.start_with?("#<MyClass:0x")            # true
puts m.inspect.start_with?("#<MyClass:0x")         # true

# --- bind_call from reflection (the bridge that motivated
#     wiring inline dispatch) ---
oid_method = BasicObject.instance_method(:__id__)
result = oid_method.bind_call(Object.new)
puts result.is_a?(Integer)                         # true

hash_method = Kernel.instance_method(:hash)
puts hash_method.bind_call(42) == 42.hash          # true

# --- Collision regression guards (cycle-1 review findings) ---
# Each pair below was a real collision in the initial encoding;
# the high-bit type-discriminator scheme eliminates them.
class CCol; end
puts 1.object_id == CCol.new.object_id            # false (Int 1 → 3 was = heap Object ObjId 0)
puts true.object_id == :length.object_id          # false (true → 20 was = first interned Sym)
puts 1.0.object_id == (-1.0).object_id            # false (sign-bit mask collapsed them)
puts /a/.object_id != /b/.object_id               # true  (distinct regex allocations must have distinct ids — was a single constant id before identity-based encoding)

# Int overflow injectivity (cycle-3 review): naive
# `n.wrapping_mul(2).wrapping_add(1)` collapses i64::MAX to 0
# (= false.object_id). Out-of-range ints fall to a bit-59 tag
# scheme to stay injective.
big1 = 1 << 62
big2 = -big1
puts big1.object_id != big2.object_id             # true
puts big1.object_id != false.object_id            # true

# Cycle-5 review: `2n+1` only stays inside the safe Int domain
# while `n < (1<<58)`. For `n >= 1<<58`, naive `2n+1` sets
# bit 59 or higher and could collide with Float/Sym/Heap ids.
# rubyrs pushes such ints to the hash fallback so they stay
# distinct from those domains — this is one of the rare points
# where we intentionally diverge from CRuby's exact id values
# in exchange for cross-type injectivity. The cross-type guard
# below still holds under CRuby (CRuby uses LSB tagging so its
# fixnum/Sym/heap ids never collide either, by construction):
puts (1 << 60).object_id != :foo.object_id        # true

# Cycle-6 review: without per-variant type-tag salt,
# `nil.hash == false.hash` deterministically — Rust's
# `bool::hash` writes `u8(0)` and our Nil arm also wrote `u8(0)`,
# producing identical DefaultHasher state. Distinct salts
# (Nil=6, Bool=5, etc.) keep value-type domains injective.
puts nil.hash != false.hash                       # true

# respond_to? whitelist matches new universal arms
puts Object.new.respond_to?(:object_id)           # true
puts Object.new.respond_to?(:__id__)              # true
puts Object.new.respond_to?(:hash)                # true
puts Object.new.respond_to?(:frozen?)             # true
puts Object.new.respond_to?(:inspect)             # true

# Binary-safe Str hash (cycle-1 review): same content → same
# hash; different content → different hash. The bytes path
# (`s.content.borrow().hash`) avoids the lossy UTF-8 collapse
# that the prior `with_str_lossy` impl would have produced for
# non-UTF-8 binary inputs.
puts "abc".hash == "abc".hash                     # true
puts "ab".hash != "abc".hash                      # true

# --- __send__ now reaches universal arms ---
puts Object.new.__send__(:class).name              # "Object"
puts 42.__send__(:hash).is_a?(Integer)             # true
