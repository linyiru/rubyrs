# File.fnmatch `**` spans directories only as a bounded `**/` segment
# under FNM_PATHNAME. Regression: `**` collapsed to a single `*`, so
# `**/*.rb` never crossed a `/` and recursive globs matched nothing.
P = File::FNM_PATHNAME
[
  ["**/*.rb", "a/b/c.rb", P],   # spans dirs
  ["**/*.rb", "c.rb",     P],   # zero dirs
  ["**/z",    "a/b/c/z",  P],
  ["**/z",    "z",        P],
  ["a/**/z",  "a/b/c/z",  P],
  ["a/**/z",  "a/z",      P],   # **/ matches zero
  ["a/**/z",  "a/b/z",    P],
  ["**",      "a/b",      P],   # bare ** is not a segment -> like *
  ["**",      "abc",      P],
  ["**.rb",   "a/b.rb",   P],   # glued ** -> like *
  ["a**",     "a/b",      P],
  ["**/*.rb", "a/b/c.rb", 0],   # no PATHNAME: * crosses /
  ["*/*.rb",  "a/b/c.rb", P],   # single * needs exactly one level
].each do |pat, path, fl|
  puts "#{pat} ~ #{path} (#{fl}) => #{File.fnmatch(pat, path, fl)}"
end
