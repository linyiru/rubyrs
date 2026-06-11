# File.basename two-arg form (suffix strip) — ground-truth table
# probed vs ruby 3.4. `".*"` strips the last extension unless the
# dot is the name's first character (dotfile rule); other suffixes
# strip on exact tail match only when shorter than the whole name.
# Discovery: Jekyll's Document#basename_without_ext calls
# File.basename(path, ".*") — previously NoMethodError.

cases = [
  ["/a/b/c.md", ".*"],
  ["/a/b/c.md", ".md"],
  ["/a/b/c.md", ".txt"],
  ["/a/b/c.tar.gz", ".*"],
  ["/a/b/c", ".*"],
  ["/a/b/.hidden", ".*"],
  ["/a/b/.hidden.rb", ".*"],
  ["c.md.", ".*"],
  ["/a/b/c.md/", ".md"],
  ["c.md", "md"],
  ["c.md", "d"],
  ["c.md", "c.md"],
  ["c.", ".*"],
  ["c.", "."],
  ["archive.tar.gz", ".tar.gz"],
  ["", ".*"],
  ["a.b/c", ".*"],
  ["2024-01-01-hello-world.md", ".*"],
]
cases.each do |path, sfx|
  puts "#{path.inspect} #{sfx.inspect} => #{File.basename(path, sfx).inspect}"
end

# Non-String suffix raises TypeError.
begin
  File.basename("a.md", 1)
rescue TypeError => e
  puts e.message
end

# One-arg base computation is a byte-level string op, not
# Path::file_name() — root / trailing-slash / dot-dir shapes.
["", "/", "//", "///", "a/", "a//", "..", ".", "/..", "/.",
 "///a///", "a/b/", "/a", "a/../", "/a/b/.."].each do |path|
  puts "#{path.inspect} => #{File.basename(path).inspect}"
end

# ".*" with all-dots prefixes: the stripped extension's dot must
# have a non-dot byte somewhere before it (".." / "..." keep,
# "a.." strips one trailing dot, "..a.b" strips ".b").
[["/", ".*"], ["..", ".*"], [".", ".*"], ["...", ".*"],
 ["a..", ".*"], ["..a.b", ".*"], ["a...", ".*"], ["..a", ".*"],
 [".a.", ".*"], ["a.b..", ".*"]].each do |path, sfx|
  puts "#{path.inspect} #{sfx.inspect} => #{File.basename(path, sfx).inspect}"
end

# File.dirname twin — same byte-level family (Path::parent() said
# "" for "a" and "." for "/"): trailing-slash strip, cut-adjacent
# separator runs removed, leading run collapses to one "/",
# interior runs away from the cut preserved.
["", "/", "//", "///", "a", "a/", "..", "/..", "/a", "//a",
 "///a", "a/b/", "/a/b/..", "a//b", "a/b//c", "//a/b", "//a//b",
 "////a/b", "//a/b/c", "/a//b/", "a//b/c", "/a//b/c",
 "a///b/c/d"].each do |path|
  puts "#{path.inspect} => dirname #{File.dirname(path).inspect}"
end
