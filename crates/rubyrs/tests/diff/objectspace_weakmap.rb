# ObjectSpace::WeakMap — map API (the WEAK part is a documented Tier-1
# divergence: rubyrs holds strong refs, so entries never get collected).
# Keys compare by IDENTITY (not eql?/hash). connection_pool tracks live
# pools in `INSTANCES = ObjectSpace::WeakMap.new`.
w = ObjectSpace::WeakMap.new
p w.class                       # ObjectSpace::WeakMap
p w.size                        # 0

k1 = "key1"; k2 = [1, 2]; k3 = Object.new
p((w[k1] = "v1"))               # "v1"  (setter returns the value)
w[k2] = "v2"
w[k3] = :v3
p w[k1]                         # "v1"
p w[k2]                         # "v2"
p w.size                        # 3
p w.key?(k1)                    # true
p w.include?(k3)                # true

# identity, not equality: a different-but-equal String key misses
p w["key1"]                     # nil
p w.key?("key1")                # false

# overwrite under the same identity
w[k1] = "v1b"
p w[k1]                         # "v1b"
p w.size                        # 3

# iteration (all entries live — strong refs)
vals = []; w.each_value { |v| vals << v }
p vals.map(&:to_s).sort         # ["v1b", "v2", "v3"]
keys = []; w.each_key { |k| keys << k.class }
p keys.map(&:to_s).sort         # ["Array", "Object", "String"]
pairs = []; w.each { |k, v| pairs << v }
p pairs.map(&:to_s).sort        # ["v1b", "v2", "v3"]

# delete returns the value
p w.delete(k2)                  # "v2"
p w.size                        # 2
p w.delete("absent")            # nil
