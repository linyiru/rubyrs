# File.join coerces a non-String/Array argument via to_path then to_str
# (CRuby's path-arg conversion). A Pathname answers to_path, so
# `File.join(BASE_DIR, pathname)` works — rouge's `load_lexer` relies on
# exactly this (`File.join(BASE_DIR, relpath)` where relpath is a
# Pathname from `relative_path_from`).
require "pathname"

p File.join("a", "b", "c")                          # "a/b/c"  (sanity)
p File.join("base", Pathname.new("x/y.rb"))         # "base/x/y.rb"
p File.join(Pathname.new("/root"), "sub", Pathname.new("f.rb"))  # "/root/sub/f.rb"
p File.join("a", ["b", Pathname.new("c")], "d")     # "a/b/c/d"  (nested array)

# A custom object that defines to_path is coerced too.
class HasToPath
  def to_path; "from_to_path"; end
end
p File.join("x", HasToPath.new)                     # "x/from_to_path"

# to_str is the fallback (a String-like object without to_path).
class HasToStr
  def to_str; "from_to_str"; end
end
p File.join("y", HasToStr.new)                       # "y/from_to_str"

# Neither conversion -> TypeError, same shape as CRuby.
begin
  File.join("a", 42)
rescue TypeError => e
  puts e.message                                     # no implicit conversion of Integer into String
end
class NoConv; end
begin
  File.join("a", NoConv.new)
rescue TypeError => e
  puts e.message                                     # no implicit conversion of NoConv into String
end
