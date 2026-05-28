# Universal Object/BasicObject methods that previously dispatched
# only on primitive types (String#to_s, Integer#hash, etc.) now
# work on plain Object instances too — closing the
# "reflection-only" gap left by PR #264 (BasicObject reflection).
#
# Newly universal arms in this PR:
#   - `object_id` / `__id__` — stable session-unique integer
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
puts nil.object_id                                 # 8

# --- hash: content-based for value types ---
puts 42.hash == 42.hash                            # true
puts "abc".hash == "abc".hash                      # true (content)
puts :foo.hash == :foo.hash                        # true
puts 1.hash != 2.hash                              # true

# --- frozen? on plain Object ---
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

# --- __send__ now reaches universal arms ---
puts Object.new.__send__(:class).name              # "Object"
puts 42.__send__(:hash).is_a?(Integer)             # true
