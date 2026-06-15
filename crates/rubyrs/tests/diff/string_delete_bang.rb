# String#delete! — destructive `delete`: remove matching chars in place,
# return self if changed else nil, FrozenError-aware. Also confirms
# `delete` / `delete!` are now in the respond_to whitelist. Surfaced by
# stdlib uri/generic.rb's `query=`.
s = "hello world"
r = s.delete!("lo")
p s            # "he wrd"
p r.equal?(s)  # true — same object when changed

p "abc".delete!("xyz")   # nil — nothing matched
p "hello".delete("l")    # "heo" (non-mutating)

# tr-style sets: ranges and negation.
p "a1b2c3".delete!("0-9")   # "abc"
p "abcdef".delete!("^bd")   # "bd" (keep only b,d)

p "hello".respond_to?(:delete)
p "hello".respond_to?(:delete!)

begin
  "frozen".freeze.delete!("z")
  puts "NO RAISE (wrong)"
rescue FrozenError
  puts "FrozenError"
end
