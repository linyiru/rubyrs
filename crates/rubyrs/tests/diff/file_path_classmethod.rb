# `File.path(obj)` — path-string representation of a path-like object
# (String as-is, `to_path` honoured, TypeError otherwise; no filesystem
# touch). Surfaced by the vendored fileutils' `fu_list` (`rm_f`) on
# bridgetown's `LoadersManager#initialize`.
puts File.path("/a/b/c")
class HasPath; def to_path = "/x/y"; end
puts File.path(HasPath.new)
begin
  File.path(42)
rescue TypeError => e
  puts "TypeError"
end
