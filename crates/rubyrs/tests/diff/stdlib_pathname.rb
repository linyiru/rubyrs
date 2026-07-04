# Pathname vendored as Tier 3 pure-Ruby stdlib (subset).
# Path-string manipulation is deterministic; the filesystem
# probes (empty?, exist?, ...) are exercised only against
# Dir.mktmpdir-controlled paths so the fixture is
# host-independent.
#
# This fixture only runs under the `stdlib` Cargo feature.
# Default builds bypass it (the `diff_cruby` registration is
# cfg-gated).

require 'pathname'
require 'tmpdir'

p = Pathname.new("/usr/local/lib")
puts p.class.name          # "Pathname"
puts p.to_s                # "/usr/local/lib"
puts p.to_path             # same
puts p.inspect             # "#<Pathname:/usr/local/lib>"
puts p.absolute?           # true
puts p.relative?           # false

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

# empty? — a FILESYSTEM probe (Dir.empty? for dirs, FileTest.empty?
# otherwise), NOT string emptiness. Controlled shapes only.
Dir.mktmpdir do |d|
  zero = File.join(d, "zero.txt"); File.write(zero, "")
  full = File.join(d, "full.txt"); File.write(full, "hi")
  edir = File.join(d, "edir"); Dir.mkdir(edir)
  ndir = File.join(d, "ndir"); Dir.mkdir(ndir); File.write(File.join(ndir, "x"), "1")
  missing = File.join(d, "missing")
  dangling = File.join(d, "dangling"); File.symlink(missing, dangling)
  link_edir = File.join(d, "link_edir"); File.symlink(edir, link_edir)
  link_zero = File.join(d, "link_zero"); File.symlink(zero, link_zero)

  puts Pathname.new(zero).empty?       # true  — zero-length file
  puts Pathname.new(full).empty?       # false — non-empty file
  puts Pathname.new(edir).empty?       # true  — empty directory
  puts Pathname.new(ndir).empty?       # false — non-empty directory
  puts Pathname.new(missing).empty?    # false — nonexistent path
  puts Pathname.new(dangling).empty?   # false — dangling symlink
  puts Pathname.new(link_edir).empty?  # true  — symlink to empty dir (followed)
  puts Pathname.new(link_zero).empty?  # true  — symlink to zero-length file
end
puts Pathname.new("").empty?           # false — degenerate empty path string
