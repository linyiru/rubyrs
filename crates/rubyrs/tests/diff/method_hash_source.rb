# Method#hash + Method#source_location.
# hash: Integer derived from (recv identity, name_id). Two
# Methods that are == must have the same hash; distinct
# recv/name pairs collide rarely.
# source_location: [filename, lineno] for user-defined; nil
# for builtins where no Method record exists.

class C
  def m(x); x; end
  def n(x); x + 1; end
end

o = C.new

# hash: same recv + same name → equal hash.
puts o.method(:m).hash == o.method(:m).hash       # true

# different recv (same class) → different hash.
o2 = C.new
puts o.method(:m).hash == o2.method(:m).hash      # false

# different name (same recv) → different hash.
puts o.method(:m).hash != o.method(:n).hash       # true

# hash type.
puts o.method(:m).hash.class.name                 # Integer

# source_location: [filename, lineno] for user methods.
loc = o.method(:m).source_location
puts loc.class.name                               # Array
puts loc.length                                   # 2
puts loc[1] > 0                                   # true (some line)
puts loc[0].is_a?(String)                         # true

# UnboundMethod also has source_location.
u = o.method(:m).unbind
puts u.source_location.class.name                 # Array

# Builtin / primitive methods have no Method record;
# source_location returns nil. CRuby returns the same.
puts 5.method(:+).source_location.inspect         # nil
