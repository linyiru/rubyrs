# Two small gaps surfaced by the tilt-render diagnostic
# (Plan C, this-session try-run):
#
#   1. `Object` was missing from the preamble class list.
#      `Object.new` / `class Foo < Object` / `is_a?(Object)`
#      all hit nil → NoMethodError.
#   2. `String#hash` (script-visible #hash method) was missing
#      from `string_call`. tilt's heredoc tag at string.rb:17
#      (`"TILT#{@data.hash.abs}"`) was the canonical caller.
#
# Both ship in one PR because they're both required to reach
# the same next blocker (block-arg ICE in dispatch.rs:2755).
# Fixed in lib.rs preamble + vm/string.rs.

# --- Object stub ---
# Bare reference resolves to a Class (the stub).
puts Object.class

# Object.new returns an instance whose class is Object.
o = Object.new
puts o.class
puts o.is_a?(Object)

# Equality between two fresh instances — distinct objects.
a = Object.new
b = Object.new
puts(a == a)
puts(a == b)

# Inheritance: `class Foo < Object` makes Foo's parent the
# stub. `Foo.new.is_a?(Object)` reports true via class chain
# walk. (Primitive types like Integer don't inherit from
# Object in our model — documented divergence; covered below.)
class Foo < Object
end
puts Foo.new.is_a?(Object)

# --- String#hash ---
# Returns an Integer. Equal strings hash equal (CRuby's only
# guarantee).
puts "hello".hash.is_a?(Integer)
puts("hello".hash == "hello".hash)
puts("abc".hash != "xyz".hash)

# tilt's actual call shape — heredoc tag derived from data hash.
data = "Some template body"
tag = "TILT#{data.hash.abs}"
puts tag.start_with?("TILT")

# respond_to? agrees with dispatch — feature-detection guards
# that ask "does this String do .hash?" should now answer true.
puts "x".respond_to?(:hash)
