# File.join(*parts) — concatenate path components with "/", collapsing
# a doubled separator only at each join boundary. Nested Arrays flatten;
# non-String/Array leaves raise TypeError. Discovery: P3 Jekyll spike —
# Liquid's i18n.rb DEFAULT_LOCALE uses File.join.
p File.join("a", "b")
p File.join("a/", "b")
p File.join("a", "/b")
p File.join("a/", "/b")        # boundary collapse
p File.join("a//", "b")        # internal // preserved
p File.join("a", "b//")        # trailing preserved
p File.join("/", "a")
p File.join("a")
p File.join()
p File.join("a", "", "b")
p File.join(["a", "b"], "c")   # nested array flattens
p File.join("x", ["y", ["z", "w"]])
p File.join("usr", "local", "bin")
# TypeError for non-string leaves (message has parity; backtrace label
# differs across runtimes, so assert via rescue).
begin
  File.join("x", 1)
rescue TypeError => e
  puts "TypeError: #{e.message}"
end
