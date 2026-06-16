# Vendored Pathname / Kernel-Pathname surface bridgetown's Site read
# path exercises: Kernel#Pathname(), Pathname#expand_path,
# Pathname#basename(suffix), Pathname#fnmatch?.
require "pathname"

# Kernel#Pathname() conversion (bare, private — implicit self)
class C; def conv(s) = Pathname(s); end
puts C.new.conv("a/b").class
pn = Pathname.new("/p")
puts Pathname(pn).equal?(pn)

# expand_path (with + without base)
puts Pathname.new("foo").expand_path("/base")
puts Pathname.new("/abs/x").expand_path

# basename with suffix
puts Pathname.new("/d/post.md").basename(".*")
puts Pathname.new("/d/post.md").basename

# fnmatch?
puts Pathname.new("tpl.erb").fnmatch?("*.erb")
puts Pathname.new("tpl.md").fnmatch?("*.erb")
