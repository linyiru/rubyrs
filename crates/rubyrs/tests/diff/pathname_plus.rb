# Pathname#+ (and its `/` alias) resolves the leading `.`/`..` of the
# right side against the trailing components of the left, leaving
# internal `..`/`.` intact (CRuby's plus, not a full cleanpath).
require "pathname"
[
  ["/usr/bin", ".."], ["a/b", "../c"], ["/usr", "."], ["a/b", "c"],
  ["a", "../.."], ["/a/b", "../../.."], ["a/b/c", ".."], ["", "x"],
  ["a", "/abs"], ["a/b", "."], [".", "a"], ["a/../b", "c"], ["/", "x"],
  ["a/b", "../../../x"], ["foo", "bar/.."], ["foo", "./bar"], ["a", "b/../c/."]
].each { |l, r| puts (Pathname.new(l) + r).to_s }
puts (Pathname.new("a") / "b").to_s
puts (Pathname.new("/var") / "../etc").to_s
