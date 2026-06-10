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
