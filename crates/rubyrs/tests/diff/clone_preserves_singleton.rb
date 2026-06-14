# `clone` copies an object's per-instance singleton class (and thus its
# singleton methods); `dup` drops it. Covers both plain objects AND Hash
# subclasses (rack's Rack::Headers#test_dup_and_clone defines `def h.foo`
# then checks `h.dup.foo` raises but `h.clone.foo` works).

# --- Hash (subclass) instance with a singleton method ---
class HSub < Hash; end
h = HSub.new
h["k"] = 1
def h.tag; "singleton-tag"; end

d = h.dup
c = h.clone

p c.tag                                  # "singleton-tag" (clone keeps it)
p d.respond_to?(:tag)                    # false (dup drops it)
begin
  d.tag
  puts "NO RAISE"
rescue NoMethodError
  puts "dup.tag => NoMethodError"
end

# data still copied on both
p c["k"]                                 # 1
p d["k"]                                 # 1

# clone's singleton class is independent: defining on the clone
# doesn't leak back to the original
def c.only_clone; "x"; end
p h.respond_to?(:only_clone)             # false
p c.respond_to?(:only_clone)             # true

# --- plain Object ---
o = Object.new
def o.greet; "hi"; end
p o.clone.greet                          # "hi"
p o.dup.respond_to?(:greet)              # false
