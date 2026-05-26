# Pathname vendored as Tier 3 pure-Ruby stdlib (subset).
# Only the deterministic, fs-free methods are modelled; this
# fixture exercises path-string manipulation. Filesystem
# probes (exist?, read, children) are intentionally NOT
# modelled and NOT exercised here.
#
# This fixture only runs under the `stdlib` Cargo feature.
# Default builds bypass it (the `diff_cruby` registration is
# cfg-gated).

require 'pathname'

p = Pathname.new("/usr/local/lib")
puts p.class.name          # "Pathname"
puts p.to_s                # "/usr/local/lib"
puts p.to_path             # same
puts p.inspect             # "#<Pathname:/usr/local/lib>"
puts p.absolute?           # true
puts p.relative?           # false
puts p.empty?              # false

# Composition
puts (p + "bin").to_s             # "/usr/local/lib/bin"
puts (p + "/etc").to_s            # absolute right-hand side wins
puts (Pathname.new("/a/") + "b").to_s  # trailing-slash handling
puts (Pathname.new("") + "b").to_s     # empty-left handling

# basename / dirname / extname / parent
q = Pathname.new("/usr/local/lib/ruby.rb")
puts q.basename.to_s       # "ruby.rb"
puts q.dirname.to_s        # "/usr/local/lib"
puts q.parent.to_s         # same as dirname
puts q.extname             # ".rb"

# Equality + hash
a = Pathname.new("/x")
b = Pathname.new("/x")
c = Pathname.new("/y")
puts a == b                # true
puts a.eql?(b)             # true
puts a == c                # false
puts a == "/x"             # false — only Pathname equals Pathname
puts a.hash == b.hash      # true

# Wrapping a Pathname in Pathname.new should be a copy.
d = Pathname.new(a)
puts d.to_s                # "/x"
puts d == a                # true

# Argument type error.
begin
  Pathname.new(123)
rescue TypeError => e
  puts "TypeError:#{e.message}"
end
